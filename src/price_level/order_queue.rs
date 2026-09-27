use crate::errors::{CapacityResource, ExhaustedCounter, PriceLevelError};
use crate::orders::{Id, OrderType};
use crate::utils::alloc::{try_push_vec, try_reserve_exact_vec, try_reserve_set, try_reserve_vec};
use crossbeam_skiplist::SkipMap;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use serde::de::{SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::fmt;
use std::fmt::Display;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// Test-only front-scan visit counter (issue #155). Counts every index entry
// the `match_front` front selection inspects, parked or not, on the calling
// thread. It documents the bound on repeated scans of parked makers; release
// and bench builds compile none of it.
#[cfg(test)]
thread_local! {
    static FRONT_SCAN_VISITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_front_scan_visit() {
    FRONT_SCAN_VISITS.with(|visits| {
        if let Some(next) = visits.get().checked_add(1) {
            visits.set(next);
        }
    });
}

/// Test-only: return and reset this thread's `match_front` index-entry visit
/// count (issue #155).
#[cfg(test)]
pub(crate) fn test_take_front_scan_visits() -> u64 {
    FRONT_SCAN_VISITS.with(|visits| visits.replace(0))
}

// Deterministic race seam for the cancel gap (issue #155). `remove` fires
// this hook on the calling thread after the map entry is gone (shard lock
// released) and before the index key is removed, so a test can run a
// readmission and match steps in exactly that window without sleeps. The hook
// is an `Rc` cloned out of the slot before it runs, so a hook may call
// `remove` again and nest overlapping cancels. Test builds only.
#[cfg(test)]
type RemoveGapHook = std::rc::Rc<dyn Fn(Id)>;

#[cfg(test)]
thread_local! {
    static REMOVE_GAP_HOOK: std::cell::RefCell<Option<RemoveGapHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the cancel-gap hook for this thread (test seam, issue #155).
/// Returns a guard that clears it on drop.
#[cfg(test)]
pub(crate) fn set_remove_gap_hook(hook: RemoveGapHook) -> RemoveGapHookGuard {
    REMOVE_GAP_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
    RemoveGapHookGuard
}

/// Clears the cancel-gap hook when dropped (test seam, issue #155).
#[cfg(test)]
pub(crate) struct RemoveGapHookGuard;

#[cfg(test)]
impl Drop for RemoveGapHookGuard {
    fn drop(&mut self) {
        REMOVE_GAP_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
fn fire_remove_gap_hook(order_id: Id) {
    let hook = REMOVE_GAP_HOOK.with(|slot| slot.borrow().clone());
    if let Some(hook) = hook {
        hook(order_id);
    }
}

/// Error for an update decision whose order does not carry the id it is stored
/// under (issue #163). Returned before any reservation or commit.
#[cold]
#[inline(never)]
fn update_id_mismatch(stored: Id, decided: Id) -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: format!(
            "update decision for order {stored} carries a different order id {decided}; rejected before commit"
        ),
    }
}

/// A thread-safe queue of orders with specialized operations.
///
/// Time priority (price-time / FIFO within the level) is maintained by an
/// ordered index keyed by a monotonic insertion sequence rather than a plain
/// tail-only FIFO. This lets a partially-filled maker keep its place at the
/// front of the queue: the residual is re-inserted at its *original* sequence,
/// instead of being appended to the tail.
///
/// # Concurrency
///
/// The ordered index is a lock-free `crossbeam-skiplist` `SkipMap`; the
/// id-keyed order storage is a `DashMap`, whose shards are reader-writer
/// locks. The queue as a whole is therefore **not** lock-free: admission,
/// update, cancel and each match step take the target entry's shard write
/// lock, and iteration takes shard read locks.
///
/// `Debug` materializes the orders first and only then writes to the caller's
/// formatter, so no shard lock is held while caller-supplied formatting
/// destination code runs (issue #172).
pub struct OrderQueue {
    /// A map of order IDs to `(insertion sequence, order)` for O(1) lookups.
    /// The sequence travels with the value so it can be recovered on pop and
    /// reused when re-inserting a partial-fill residual.
    orders: DashMap<Id, (u64, Arc<OrderType<()>>)>,
    /// Ordered index `sequence -> Id`. The lowest sequence is the front
    /// (oldest) order, so iteration / pop honours strict time priority.
    index: SkipMap<u64, Id>,
    /// Monotonic source of insertion sequences: the next value to hand out.
    ///
    /// Advanced only through [`OrderQueue::try_reserve_seq`], a checked CAS
    /// that never wraps (issue #165). Values `0 ..= u64::MAX - 1` can be
    /// minted; once this holds `u64::MAX` every further reservation fails with
    /// [`PriceLevelError::CounterExhausted`] and the counter stays put, so a
    /// sequence is never reused and an index entry is never overwritten.
    next_seq: AtomicU64,
}

/// A fresh FIFO insertion sequence reserved from [`OrderQueue::try_reserve_seq`]
/// (issue #165).
///
/// The field is private to this module, so the only way to obtain one is a
/// successful checked reservation: code that re-sequences a maker must reserve
/// first, before it commits anything, and can then no longer fail on
/// exhaustion. A reserved sequence that ends up unused (the caller refused the
/// operation for another reason) is simply skipped; sequences only need to be
/// unique and increasing, not dense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReservedSeq(u64);

impl ReservedSeq {
    /// The reserved sequence value.
    #[inline]
    #[must_use]
    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

/// The mutation a matcher decides to apply to the front maker it is currently
/// matching, while the maker's `orders` entry is held under the per-entry lock.
///
/// Returned by the decision closure passed to [`OrderQueue::match_front`]. The
/// queue applies the variant atomically (under the same per-entry lock that
/// guards a concurrent [`OrderQueue::remove`]), so a `cancel` of the same id
/// either runs entirely before the decision (the closure observes `Vacant` and
/// is never called) or entirely after the commit (it observes the residual /
/// emptiness the matcher left behind). A cancel can never be lost mid-decision.
#[derive(Debug)]
pub(crate) enum FrontAction {
    /// The maker was fully consumed: remove it from `orders` and drop its index
    /// entry. After this the id no longer rests at the level.
    Remove,
    /// Pure partial fill: keep the maker at its current insertion sequence
    /// (and therefore its price-time / FIFO position) by swapping the stored
    /// value to the residual in place under the per-entry lock.
    KeepInPlace(Arc<OrderType<()>>),
    /// Iceberg / reserve replenishment: the refreshed tranche loses time
    /// priority, so it is re-sequenced at the tail under the sequence the
    /// decision closure reserved BEFORE committing anything (issue #165).
    ReplaceAtTail(Arc<OrderType<()>>, ReservedSeq),
    /// The maker made no progress this sweep (a degenerate zero-progress shape).
    /// Leave it untouched in `orders`/`index`; the caller sets its sequence
    /// aside so the sweep advances to the maker behind it without re-popping it.
    SetAside,
}

/// The mutation an [`OrderQueue::update_entry_with`] decision closure asks the queue
/// to commit, after deriving it from the **live** stored order under the entry
/// lock. Mirrors the [`FrontAction`] precedent for the match sweep.
#[derive(Debug)]
pub(crate) enum UpdateDecision {
    /// Decrease / unchanged total: swap the stored value to the resized order at
    /// its existing insertion sequence, keeping its price-time position.
    KeepInPlace(Arc<OrderType<()>>),
    /// Increase in total: demote the resized order to a fresh tail sequence
    /// (losing time priority) by swapping the stored `(seq, order)` pair in
    /// place and re-keying the index — all under the entry lock the update
    /// already holds. The sequence is reserved by the decision closure before
    /// it reserves any level counter (issue #165), so the commit cannot fail.
    /// Same shape as the [`FrontAction::ReplaceAtTail`] the match sweep commits.
    ReplaceAtTail(Arc<OrderType<()>>, ReservedSeq),
}

/// Committed `(stored_seq, order)` pairs collected for a materialization.
type SeqPairs = Vec<(u64, Arc<OrderType<()>>)>;

/// A walk over the resting orders in ascending **insertion sequence** — the
/// order [`OrderQueue::match_front`] consumes them — for the fill-or-kill
/// dry run (issue #143). Created by [`OrderQueue::seq_walk`].
///
/// The walk has two phases, so that a caller that stops early does work
/// proportional to what it consumed, while one that walks everything pays
/// no more than a single materialize-and-sort:
///
/// * **Lazy prefix.** The first `lazy_budget` live orders come straight
///   from the `index`, with `match_front`'s liveness rule: a key is yielded
///   only when its id still rests in `orders` under that same sequence, so a
///   stale key (cancelled, or re-sequenced by a demotion / replenishment) is
///   skipped. Nothing is materialized, sorted or cloned beyond the `Arc` of
///   each yielded order. Each step holds one `DashMap` shard **read** lock
///   only while it clones that `Arc`, and no lock between steps.
/// * **Bulk continuation.** A walk that outlives the budget collects the
///   remaining committed pairs (sequence greater than the last one yielded)
///   from `orders` in one pass and sorts them, as
///   [`OrderQueue::snapshot_by_seq`] does. A per-entry index lookup costs
///   more than its share of one bulk pass, so this keeps a long walk (for
///   example a fill-or-kill that must visit every maker to prove a kill)
///   from paying the lookup per maker.
///
/// Under quiescence (the fill-or-kill exclusive guard with the one-matcher
/// contract) both phases see the same queue, and the walk yields exactly
/// [`OrderQueue::snapshot_by_seq`]'s sequence. Under concurrent mutation it
/// is not a point-in-time view.
pub(crate) struct SeqWalk<'a> {
    queue: &'a OrderQueue,
    index: crossbeam_skiplist::map::Iter<'a, u64, Id>,
    /// Live orders the lazy phase may still yield.
    lazy_left: u64,
    /// Sequence of the last order the lazy phase yielded.
    last_seq: Option<u64>,
    /// The bulk continuation, once started.
    bulk: Option<std::vec::IntoIter<(u64, Arc<OrderType<()>>)>>,
}

impl SeqWalk<'_> {
    /// The next resting order in insertion sequence, or `None` when the
    /// queue is exhausted.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the bulk continuation cannot
    /// be reserved. The queue is only read.
    #[inline]
    pub(crate) fn try_next(&mut self) -> Result<Option<Arc<OrderType<()>>>, PriceLevelError> {
        if let Some(bulk) = self.bulk.as_mut() {
            return Ok(bulk.next().map(|(_, order)| order));
        }
        let Some(lazy_left) = self.lazy_left.checked_sub(1) else {
            let mut bulk = self.queue.collect_pairs_after(self.last_seq)?.into_iter();
            let first = bulk.next().map(|(_, order)| order);
            self.bulk = Some(bulk);
            return Ok(first);
        };
        for entry in self.index.by_ref() {
            let seq = *entry.key();
            let Some(slot) = self.queue.orders.get(entry.value()) else {
                continue;
            };
            let (stored_seq, order) = slot.value();
            if *stored_seq != seq {
                continue;
            }
            self.lazy_left = lazy_left;
            self.last_seq = Some(seq);
            return Ok(Some(Arc::clone(order)));
        }
        Ok(None)
    }
}

// Test-only switch that disables the inline slot of `ParkedSeqs` (issue
// #164), so a test can drive the spill set's fallible reservation with a
// single park. Production builds compile none of this.
#[cfg(test)]
thread_local! {
    static PARK_INLINE_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Restores the inline slot when dropped (test seam, issue #164).
#[cfg(test)]
pub(crate) struct ParkInlineGuard(bool);

#[cfg(test)]
impl Drop for ParkInlineGuard {
    fn drop(&mut self) {
        PARK_INLINE_DISABLED.with(|cell| cell.set(self.0));
    }
}

/// Disables the inline slot of every `ParkedSeqs` on this thread until the
/// guard drops (test seam, issue #164).
#[cfg(test)]
pub(crate) fn disable_park_inline_slot() -> ParkInlineGuard {
    ParkInlineGuard(PARK_INLINE_DISABLED.with(|cell| cell.replace(true)))
}

#[cfg(test)]
fn park_inline_disabled() -> bool {
    PARK_INLINE_DISABLED.with(std::cell::Cell::get)
}

/// The insertion sequences of the makers one match sweep has parked (issue
/// #164; see [`OrderQueue::match_front`]).
///
/// The first live park is held in an inline slot, which never allocates; only
/// further live parks spill into a `HashSet`, growing fallibly. Orders are
/// id-keyed, so the only park that fires today (the self-trade skip of the
/// one order sharing the taker id) has at most one LIVE key at a time.
///
/// A parked key can go stale without the scan ever revisiting it: a cancel
/// removes its index key, and a readmission or a quantity-increase demotion
/// moves the id to a fresh, higher sequence. So when a new park arrives while
/// the inline slot is occupied, [`OrderQueue::match_front`] first checks the
/// inline key against the queue with the #155 rule (live iff the index still
/// maps it to an id whose map entry stores that sequence) and frees the slot
/// if it is dead. Sequences are never reused and a stored sequence only moves
/// forward, so a dead key can never become live again and dropping it is
/// safe. With that, a single live parked maker never allocates; the spill,
/// and with it the allocation-failure stop cause, is reached only by two
/// simultaneously live parks, which no current order shape produces.
#[derive(Debug, Default)]
pub(crate) struct ParkedSeqs {
    inline: Option<u64>,
    spill: HashSet<u64>,
}

impl ParkedSeqs {
    /// An empty set; allocates nothing.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `true` if `seq` is parked.
    #[inline]
    #[must_use]
    pub(crate) fn contains(&self, seq: u64) -> bool {
        self.inline == Some(seq) || (!self.spill.is_empty() && self.spill.contains(&seq))
    }

    /// Unparks `seq` (no-op if absent). Never allocates.
    #[inline]
    pub(crate) fn remove(&mut self, seq: u64) {
        if self.inline == Some(seq) {
            self.inline = None;
        } else if !self.spill.is_empty() {
            self.spill.remove(&seq);
        }
    }

    /// Number of parked sequences (test inspection).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.spill.len() + usize::from(self.inline.is_some())
    }

    /// `true` if nothing is parked (test inspection).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.inline.is_none() && self.spill.is_empty()
    }

    /// The sequence held in the inline slot, if any.
    #[inline]
    #[must_use]
    pub(crate) fn inline_seq(&self) -> Option<u64> {
        self.inline
    }

    #[inline]
    fn inline_available(&self) -> bool {
        #[cfg(test)]
        if park_inline_disabled() {
            return false;
        }
        self.inline.is_none()
    }

    /// Reserves room so the next `additional` parks of new sequences do not
    /// allocate (the fill-or-kill preflight).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::SweepScratch`]); the set is unchanged.
    pub(crate) fn try_reserve(&mut self, additional: usize) -> Result<(), PriceLevelError> {
        let spill = if self.inline_available() {
            additional.checked_sub(1)
        } else {
            Some(additional)
        };
        match spill {
            Some(n) if n > 0 => try_reserve_set(&mut self.spill, n, CapacityResource::SweepScratch),
            _ => Ok(()),
        }
    }

    /// Parks `seq`. Uses the inline slot when free; otherwise grows the spill
    /// set fallibly (only when it is full).
    ///
    /// # Errors
    ///
    /// The original [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::SweepScratch`]) of the refused reservation; `seq`
    /// is not parked and the set is unchanged.
    pub(crate) fn try_insert(&mut self, seq: u64) -> Result<(), PriceLevelError> {
        if self.contains(seq) {
            return Ok(());
        }
        if self.inline_available() {
            self.inline = Some(seq);
            return Ok(());
        }
        if self.spill.len() >= self.spill.capacity() {
            try_reserve_set(&mut self.spill, 1, CapacityResource::SweepScratch)?;
        }
        self.spill.insert(seq);
        Ok(())
    }
}

/// The outcome of [`OrderQueue::remove_if`] (issue #163).
#[derive(Debug)]
pub(crate) enum RemoveOutcome {
    /// The id is not resident; nothing was checked or changed.
    Absent,
    /// The id is resident but the check refused the removal; the entry, its
    /// sequence and the index are unchanged.
    Refused,
    /// The entry was removed from the map and the index.
    Removed(Arc<OrderType<()>>),
}

/// The outcome of a single [`OrderQueue::match_front`] step, reported back to
/// the sweep so it can drive the loop and apply counter deltas.
#[derive(Debug)]
pub(crate) enum FrontOutcome<R> {
    /// A front candidate existed and the decision closure ran. Carries the
    /// closure's result `R` (the trade bookkeeping data the sweep needs to apply
    /// counter deltas and emit the trade). The committed [`FrontAction`] is
    /// already encoded in that bookkeeping (full consume vs partial vs
    /// replenish), so it is not surfaced separately.
    Matched { result: R },
    /// The decision closure ran and chose [`FrontAction::SetAside`], but the
    /// caller's parked-sequence set could not grow to record it (issue #164).
    /// Nothing was committed (`SetAside` never mutates the queue) and the
    /// sequence was NOT parked, so re-running the sweep step would re-select
    /// the same maker: the caller must stop. Carries the closure's result so a
    /// terminal step (which stops anyway) keeps its own error, and the
    /// original typed reservation error.
    ParkRefused { result: R, error: PriceLevelError },
    /// The queue is empty (no front candidate that is not already set aside).
    /// The sweep is done.
    Empty,
}

impl OrderQueue {
    /// Create a new empty order queue
    #[must_use]
    pub fn new() -> Self {
        Self {
            orders: DashMap::new(),
            index: SkipMap::new(),
            next_seq: AtomicU64::new(0),
        }
    }

    /// Reserve the next FIFO insertion sequence with a checked CAS (issue
    /// #165).
    ///
    /// `Relaxed` is sufficient: only the uniqueness and monotonicity of the
    /// counter matter. The happens-before ordering between concurrent
    /// producers / consumers is provided by the `index` (`SkipMap`) / `orders`
    /// (`DashMap`) structures, not by this counter.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CounterExhausted`] (counter
    /// [`ExhaustedCounter::QueueSequence`]) once every sequence below
    /// `u64::MAX` has been handed out. The counter is left unchanged.
    #[inline]
    pub(crate) fn try_reserve_seq(&self) -> Result<ReservedSeq, PriceLevelError> {
        self.next_seq
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .map(ReservedSeq)
            .map_err(|_| PriceLevelError::counter_exhausted(ExhaustedCounter::QueueSequence))
    }

    /// Number of FIFO sequences that can still be reserved (issue #165).
    /// Exact while no concurrent admission or update reserves one (the
    /// fill-or-kill sweep holds the level's exclusive guard when it asks).
    #[inline]
    #[must_use]
    pub(crate) fn seq_headroom(&self) -> u64 {
        // `next_seq <= u64::MAX` always, so `abs_diff` is exactly
        // `u64::MAX - next_seq`; it is total, so no unchecked subtraction is
        // needed to express it (issue #163 arithmetic policy).
        u64::MAX.abs_diff(self.next_seq.load(Ordering::Relaxed))
    }

    /// Test-only seeding seam (issue #165): place the sequence counter at
    /// `next` so exhaustion can be exercised without `2^64` insertions.
    #[cfg(test)]
    pub(crate) fn test_seed_next_seq(&self, next: u64) {
        self.next_seq.store(next, Ordering::Relaxed);
    }

    /// Test-only read of the sequence counter (issue #165).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_next_seq(&self) -> u64 {
        self.next_seq.load(Ordering::Relaxed)
    }

    /// Test-only: the stored insertion sequence of `order_id`, if it rests
    /// here (issue #165).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_seq_of(&self, order_id: Id) -> Option<u64> {
        self.orders.get(&order_id).map(|slot| slot.value().0)
    }

    /// Add an order to the tail of the queue (newest time priority),
    /// **unconditionally overwriting** any existing entry for the same id.
    ///
    /// Test-only queue-building fixture. It has no production caller: admission
    /// uses [`OrderQueue::try_push`] / [`OrderQueue::try_push_with`]
    /// (insert-if-absent, issue #113) and every quantity update re-derives and
    /// re-sequences in place under the entry lock via
    /// [`OrderQueue::update_entry_with`] (issue #115). Its blind overwrite would
    /// leave the id-keyed map and the ordered index disagreeing, so it is
    /// deliberately not part of the public API — like [`OrderQueue::reinsert`],
    /// it is `#[cfg(test)]`.
    ///
    /// Sequences come from the checked [`OrderQueue::try_reserve_seq`] like
    /// every other site (issue #165); on exhaustion the fixture inserts
    /// nothing.
    #[cfg(test)]
    pub(crate) fn push(&self, order: Arc<OrderType<()>>) {
        let Ok(seq) = self.try_reserve_seq() else {
            return;
        };
        let seq = seq.get();
        let order_id = order.id();
        self.orders.insert(order_id, (seq, order));
        self.index.insert(seq, order_id);
    }

    /// Insert an order only if its id is not already present — the admission
    /// primitive.
    ///
    /// Thin wrapper over the crate-internal `try_push_with` with a no-op
    /// reservation: the id-uniqueness, held-lock publication, and
    /// no-sequence-gap guarantees documented there all apply. Use this when
    /// publication has no side effects to commit atomically with it; the
    /// reservation-hook form commits a caller-side reservation (e.g. the level's
    /// atomic counters) under the same shard lock that decides the id is free.
    /// The quantity-update path no longer vacates the id either: as of issues
    /// #119 / #115 it re-derives and re-sequences in place under the entry lock
    /// (`update_entry`), so there is no remove-then-push window for a same-id
    /// admission to slip into.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DuplicateOrderId`] if an order with the same
    /// id already rests in the queue.
    #[must_use = "a rejected duplicate must be handled, not ignored"]
    pub fn try_push(&self, order: Arc<OrderType<()>>) -> Result<(), PriceLevelError> {
        self.try_push_with(order, || Ok(()))
    }

    /// Insert an order only if its id is absent, committing a caller-supplied
    /// `reserve` step **atomically with the publication** — the admission
    /// primitive with a reservation hook.
    ///
    /// The `DashMap` entry API makes the whole operation atomic under the
    /// per-shard lock:
    ///
    /// 1. **Identity is decided first.** An `Occupied` entry means the id
    ///    already rests here: return [`PriceLevelError::DuplicateOrderId`]
    ///    with **nothing touched** — `reserve` is never run, no sequence is
    ///    minted, no counter moves. This is why a duplicate can never leave a
    ///    transient side effect (e.g. an inflated level counter) for another
    ///    thread to observe, and why a duplicate at counter capacity reports
    ///    `DuplicateOrderId` rather than a spurious overflow.
    /// 2. **Then the insertion sequence is reserved** with the checked
    ///    [`OrderQueue::try_reserve_seq`] (issue #165). An exhausted sequence
    ///    returns [`PriceLevelError::CounterExhausted`] with nothing touched:
    ///    `reserve` has not run, so the caller has no counter to roll back.
    /// 3. **Then `reserve` runs**, still under the shard lock, now that the id
    ///    is known free and a sequence is held. If it returns `Err`, propagate
    ///    it with nothing inserted — the caller is responsible for leaving its
    ///    own state unchanged on `Err` (e.g. rolling back a partial
    ///    multi-counter reservation before returning).
    /// 4. **Then publish, holding the shard lock across the index insert.** The
    ///    map value is inserted (its returned guard keeps the shard write lock),
    ///    the `seq -> id` index entry is added while that guard is still held,
    ///    and only then is the guard dropped. Holding the lock across both
    ///    publications closes the window where the lock released after the map
    ///    insert but before the index insert: a concurrent cancel + same-id
    ///    readmission could otherwise land its own entries and let this call's
    ///    stale index insert produce two index entries for one id. This is the
    ///    same held-lock shape [`OrderQueue::match_front`]'s `ReplaceAtTail`
    ///    uses. `self.index` is a separate structure (`SkipMap`), so inserting
    ///    into it under the `DashMap` shard lock cannot deadlock.
    ///
    /// The insertion sequence is reserved **inside** the `Vacant` arm, so a
    /// rejected duplicate consumes no sequence and a duplicate id is reported
    /// as [`PriceLevelError::DuplicateOrderId`] even when the sequence is
    /// exhausted (identity first). It is reserved **before** `reserve` so the
    /// last fallible step of admission is the caller's own reservation: a
    /// failed `reserve` skips the reserved value (a harmless gap: sequences
    /// only need to be unique and increasing), and the index entry is added
    /// only for the order that actually landed in the map.
    ///
    /// `reserve` runs while the shard lock is held, so it MUST NOT call back
    /// into this queue (that would deadlock on the same shard) and MUST NOT
    /// block; the level's counter reservations (plain atomic RMWs) satisfy both.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DuplicateOrderId`] if an order with the same
    /// id already rests in the queue,
    /// [`PriceLevelError::CounterExhausted`] if no insertion sequence is left,
    /// or whatever error `reserve` returns.
    #[must_use = "a rejected admission must be handled, not ignored"]
    pub(crate) fn try_push_with<F>(
        &self,
        order: Arc<OrderType<()>>,
        reserve: F,
    ) -> Result<(), PriceLevelError>
    where
        F: FnOnce() -> Result<(), PriceLevelError>,
    {
        let order_id = order.id();
        match self.orders.entry(order_id) {
            Entry::Occupied(_) => Err(PriceLevelError::DuplicateOrderId(order_id.to_string())),
            Entry::Vacant(slot) => {
                // Identity is already decided (this arm means the id is free).
                // Reserve the sequence first (checked, issue #165): on
                // exhaustion nothing has been touched, and the caller's
                // counter reservation below never has to be undone for it.
                let seq = self.try_reserve_seq()?.get();
                // Run the caller's reservation before publishing; on failure
                // nothing has been inserted, so the level stays byte-identical
                // once the caller unwinds its own partial reservation (the
                // reserved sequence is skipped).
                reserve()?;
                // Hold the shard lock across BOTH publications: the map insert
                // returns a guard that keeps the lock, the index entry is added
                // while it is held, and only then is the guard dropped.
                let guard = slot.insert((seq, order));
                self.index.insert(seq, order_id);
                drop(guard);
                Ok(())
            }
        }
    }

    /// Pop the front (oldest) order together with its insertion sequence,
    /// removing it from the queue.
    ///
    /// The sequence is returned alongside the order. As of #81 the match sweep
    /// no longer pops-then-reinserts a maker (it operates in place under the
    /// per-entry lock via [`OrderQueue::match_front`]); this remains the backing
    /// of the destructive [`OrderQueue::pop`] used by tests and queue draining.
    #[must_use]
    pub(crate) fn pop_entry(&self) -> Option<(u64, Arc<OrderType<()>>)> {
        loop {
            // `pop_front` atomically removes the lowest-sequence index entry.
            let entry = self.index.pop_front()?;
            let popped_seq = *entry.key();
            let order_id = *entry.value();
            // Validate the maker's STORED sequence against the key we popped,
            // under the map entry lock (issue #127). A concurrent
            // `resequence_to_tail` may have demoted this id to a fresh tail
            // sequence, making this a STALE old key; removing by id alone would
            // return the demoted maker ahead of older makers and strand its new
            // key. Mirror `match_front`'s stale-front guard: only take the maker
            // when the popped key IS its current key.
            match self.orders.entry(order_id) {
                Entry::Occupied(occupied) if occupied.get().0 == popped_seq => {
                    let (seq, order) = occupied.remove();
                    return Some((seq, order));
                }
                // Stale old key of a demoted maker (stored seq != popped), or the
                // id was cancelled (`Vacant`). Either way the popped key is
                // already gone from the index (`pop_front` removed it); the maker,
                // if it still rests, lives under its newer key and is popped in
                // order on a later iteration. Retry with the next front.
                _ => continue,
            }
        }
    }

    /// Attempt to pop an order from the queue (front / oldest first).
    #[must_use]
    pub fn pop(&self) -> Option<Arc<OrderType<()>>> {
        self.pop_entry().map(|(_, order)| order)
    }

    /// Select the front (oldest, not-yet-set-aside) maker and apply a match
    /// decision to it **atomically with respect to a concurrent
    /// [`OrderQueue::remove`] (cancel) of the same id**.
    ///
    /// This replaces the old `pop_entry` + later `reinsert` sequence used by the
    /// match sweep, which removed the maker from `orders` before deciding and so
    /// opened a "lost cancel" window: a `cancel` landing between the pop and the
    /// reinsert would `remove` an id that was no longer in `orders`, silently
    /// no-op (no counter decrement), and the matcher would then reinsert the
    /// residual — leaving the cancelled order resting.
    ///
    /// Here the maker is **kept resident in `orders`** while it is matched. The
    /// decision and the resulting mutation both run while the maker's `orders`
    /// entry is held under DashMap's per-entry (shard) lock — the same lock a
    /// concurrent `cancel`'s [`DashMap::remove`] must take. The two therefore
    /// serialize on that lock:
    ///
    /// - If `cancel` wins the lock first, this method observes the entry as
    ///   `Vacant` (cancel already removed it + decremented counters), drops the
    ///   stale index entry, and advances to the next candidate. The decision
    ///   closure is never run for the cancelled id.
    /// - If this method wins, it commits its [`FrontAction`] under the lock; a
    ///   `cancel` arriving afterwards observes whatever the matcher left — the
    ///   residual for a partial fill (and removes *that*, decrementing by the
    ///   residual), or nothing for a full consume (and correctly no-ops).
    ///
    /// In every interleaving a cancel either fully wins or fully loses; it is
    /// never lost. The counter delta for the matched transition is the caller's
    /// responsibility and is keyed off the returned [`FrontAction`], so it can
    /// never double-count with the cancel.
    ///
    /// `set_aside` carries the insertion sequences of makers the current sweep
    /// has parked (the no-progress guard and the self-trade skip); they are
    /// skipped when choosing the front so the sweep does not re-pick them. A
    /// `SetAside` action inserts the chosen seq into it. A parked key whose id
    /// no longer rests under that sequence (cancelled, readmitted or demoted
    /// since it was parked) is removed from the index and from `set_aside` the
    /// first time the scan meets it (issue #155). It is a `HashSet` so
    /// membership during the front scan is O(1); it is only ever inserted
    /// into, probed and pruned, never iterated for ordering.
    ///
    /// `decide` is the pure match decision (e.g. [`OrderType::match_against`]
    /// plus trade bookkeeping). It runs while the per-entry lock is held, so it
    /// MUST NOT call back into this queue's storage (that would deadlock on the
    /// same shard) and MUST NOT block. The one queue call it may make is
    /// [`OrderQueue::try_reserve_seq`], which touches only the sequence atomic:
    /// a [`FrontAction::ReplaceAtTail`] carries a sequence the closure reserved
    /// before it committed any level counter (issue #165). It receives the maker's insertion `seq` and an
    /// immutable borrow of the resident order; the borrow ends before any commit
    /// mutates the entry, so the decision must return OWNED action data and no
    /// reference may escape it.
    ///
    /// All three mutating actions keep the maker's value **resident in `orders`
    /// until it is genuinely gone** (a full consume) — even the
    /// [`FrontAction::ReplaceAtTail`] re-prioritisation swaps the value and
    /// re-sequences it in place rather than removing-then-re-pushing — so the
    /// lost-cancel window is closed for every action, not just the partial fill.
    pub(crate) fn match_front<F, R>(&self, set_aside: &mut ParkedSeqs, decide: F) -> FrontOutcome<R>
    where
        F: FnOnce(u64, &OrderType<()>) -> (FrontAction, R),
    {
        loop {
            // Find the lowest-sequence index entry not already set aside this
            // sweep. `index.iter()` yields entries in ascending sequence order
            // (front = oldest = highest time priority).
            //
            // A parked key can go stale while it stays in `set_aside`: a cancel
            // removes the map entry and only then the index key, and a
            // readmission or demotion of the same id moves it to a fresh
            // sequence (issue #155). Skipping parked keys on the hash probe
            // alone would re-visit such a stale key on every later step of the
            // sweep. So a parked key is checked against the map on each visit
            // and, if its id no longer rests there under this sequence, the key
            // is dropped for good. Sequences are never reused, so a key whose
            // map entry is gone or carries another sequence can never become
            // live again; removing it is the same self-heal as the `Vacant` and
            // stale-front (#119) arms below. The read lock is taken only for
            // parked keys, never on the common path with an empty `set_aside`,
            // and is released before the next entry is inspected.
            let mut front = None;
            for entry in self.index.iter() {
                #[cfg(test)]
                record_front_scan_visit();
                let seq = *entry.key();
                if !set_aside.contains(seq) {
                    front = Some((seq, *entry.value()));
                    break;
                }
                let live = self
                    .orders
                    .get(entry.value())
                    .is_some_and(|slot| slot.value().0 == seq);
                if !live {
                    self.index.remove(&seq);
                    set_aside.remove(seq);
                }
            }
            let Some((seq, order_id)) = front else {
                return FrontOutcome::Empty;
            };

            // Lock the maker's `orders` entry. `entry` takes the shard write
            // lock, which a concurrent `cancel`'s `remove` must also take, so the
            // decision + mutation below are atomic with respect to that cancel.
            match self.orders.entry(order_id) {
                Entry::Vacant(_) => {
                    // The maker was cancelled (removed from `orders`) but its
                    // index entry is stale. Drop the stale index entry and retry
                    // with the next front candidate. The cancel already
                    // decremented the counters, so there is nothing to account
                    // here.
                    self.index.remove(&seq);
                    continue;
                }
                Entry::Occupied(mut occupied) => {
                    // Stale front-selection guard (issue #119). The `(seq, id)`
                    // pair was read from the index BEFORE this entry lock was
                    // taken. A concurrent quantity-increase demotion (the
                    // `ReplaceAtTail` path of `update_entry`) may have moved this
                    // maker to a fresh tail sequence in that gap, so the entry
                    // now stores a DIFFERENT
                    // sequence and the maker is no longer the front. Acting on it
                    // via the stale front position would break FIFO. Drop the
                    // stale index key (the demoted maker already lives under its
                    // new key) and retry with a fresh front read — the same
                    // self-heal shape as the `Vacant` arm above. Sequences are
                    // monotonic and never reused, so `index[seq]` can only ever
                    // have pointed at this id, making the removal safe. The
                    // retry is unbounded; liveness relies on re-sequencings of
                    // the front maker being finite (the single-logical-writer
                    // update contract), as with the `Vacant` self-heal.
                    if occupied.get().0 != seq {
                        self.index.remove(&seq);
                        continue;
                    }

                    // `occupied.get()` is `(stored_seq, order)`. Decide against
                    // the live order while the entry lock is held. Borrow the
                    // resident order rather than cloning its `Arc` on the hot
                    // path: the immutable borrow lives only for the `decide`
                    // call, which returns OWNED action data, so it ends before
                    // any `get_mut()` / `remove()` commit below (no reference
                    // escapes into a `FrontAction`).
                    let (action, result) = decide(seq, occupied.get().1.as_ref());

                    // A `SetAside` records a sequence into the caller's scratch
                    // `HashSet`, whose first insert allocates. Defer that insert
                    // until AFTER the entry lock is released (issue #126) so no
                    // allocation ever runs under the shard lock — the set is
                    // per-sweep scratch owned by the caller, never shared, so it
                    // needs no lock protection. The other actions commit their
                    // queue mutations here, under the lock, as before.
                    let mut park_seq: Option<u64> = None;
                    // The order swapped OUT of the slot by a partial fill /
                    // replenish, captured with `mem::replace` and dropped only
                    // AFTER the entry lock is released (issue #128), so a
                    // last-reference deallocation never runs under the shard lock.
                    let mut evicted: Option<Arc<OrderType<()>>> = None;
                    // Every arm releases the entry lock by the time it finishes
                    // (either `occupied.remove()` consumes it, or an explicit
                    // `drop`), so the deferred `set_aside` insert and the evicted
                    // order's drop below never run under the shard lock.
                    //
                    // The action is consumed BY VALUE (issue #144): the
                    // residual / refreshed `Arc` the decision minted is MOVED
                    // into the slot, so the commit performs no reference-count
                    // increment / decrement pair. Only the evicted old `Arc`
                    // (a caller-visible order whose last-reference drop runs
                    // payload `Drop`) is carried out of the lock.
                    match action {
                        FrontAction::Remove => {
                            // Full consume: remove the entry under the lock, then
                            // drop its index entry. A cancel cannot also remove it
                            // (the entry is gone), so no double counter decrement.
                            // `remove` consumes the guard, releasing the lock
                            // before the removed value is dropped.
                            let _ = occupied.remove();
                            self.index.remove(&seq);
                        }
                        FrontAction::KeepInPlace(residual) => {
                            // Partial fill keeping priority: swap the stored value
                            // to the residual in place, keeping the same
                            // sequence/index entry. Still under the entry lock.
                            evicted = Some(std::mem::replace(&mut occupied.get_mut().1, residual));
                            drop(occupied);
                        }
                        FrontAction::ReplaceAtTail(refreshed, reserved) => {
                            // Replenished tranche loses time priority, but the
                            // maker keeps the SAME id and must stay resident in
                            // `orders` so a concurrent cancel cannot slip into a
                            // remove-then-push gap. So: take the fresh tail
                            // sequence the decision reserved (checked, issue
                            // #165) and swap BOTH the value and its stored
                            // sequence in place under the entry lock; only the
                            // index is re-keyed (old seq -> new seq) afterwards.
                            let new_seq = reserved.get();
                            {
                                let slot = occupied.get_mut();
                                slot.0 = new_seq;
                                evicted = Some(std::mem::replace(&mut slot.1, refreshed));
                            }
                            // `occupied` still holds the per-entry lock here, so
                            // re-keying the index — a different structure
                            // (`SkipMap`), no deadlock — happens while a concurrent
                            // cancel is still excluded from the entry. Insert the
                            // NEW key BEFORE removing the old (issue #127) so the
                            // id is never transiently absent from the index and a
                            // concurrent front scan can never miss it. Once the
                            // lock is released the value already carries `new_seq`,
                            // so a cancel removes `orders[id]` and `index[new_seq]`
                            // consistently. The only residue a race can leave is a
                            // stale `index[seq|new_seq] -> id` entry pointing at an
                            // already-removed id, which the next `match_front`
                            // self-heals on the `Vacant` branch. No order and no
                            // counter update is ever lost.
                            self.index.insert(new_seq, order_id);
                            self.index.remove(&seq);
                            drop(occupied);
                        }
                        FrontAction::SetAside => {
                            // No progress: leave the entry untouched. Release the
                            // lock and park its sequence below.
                            drop(occupied);
                            park_seq = Some(seq);
                        }
                    }

                    // The entry lock is released on every arm above; a
                    // possibly-allocating scratch-set insert and the evicted
                    // order's drop now run unlocked.
                    //
                    // The park never allocates for the first live key (inline
                    // slot) and grows fallibly beyond it (issue #164): a
                    // refused reservation reports `ParkRefused` with the
                    // original error and the step still a no-op. A set the
                    // caller pre-reserved (fill-or-kill) never reaches the
                    // reservation.
                    //
                    // Before a park that would spill, a dead inline key (its
                    // maker cancelled, readmitted or demoted since it was
                    // parked, so the scan may never revisit it) is dropped
                    // with the #155 liveness rule; see `ParkedSeqs`.
                    drop(evicted);
                    if let Some(seq) = park_seq {
                        if let Some(old) = set_aside.inline_seq()
                            && old != seq
                            && !self.seq_is_live(old)
                        {
                            set_aside.remove(old);
                        }
                        if let Err(error) = set_aside.try_insert(seq) {
                            return FrontOutcome::ParkRefused { result, error };
                        }
                    }

                    return FrontOutcome::Matched { result };
                }
            }
        }
    }

    /// `true` iff `seq` is still the live sequence of a resting order: the
    /// index maps it to an id whose map entry stores exactly `seq` (the #155
    /// stale-key rule). Takes a `DashMap` shard read lock briefly; call it
    /// with no queue lock held. A `false` is final: sequences are never
    /// reused, an index key is removed only after its map entry is gone or
    /// re-sequenced, and a stored sequence only moves forward.
    fn seq_is_live(&self, seq: u64) -> bool {
        self.index.get(&seq).is_some_and(|entry| {
            self.orders
                .get(entry.value())
                .is_some_and(|slot| slot.value().0 == seq)
        })
    }

    /// Single-closure [`OrderQueue::update_entry_with`] with no reservation
    /// phase. Production updates reserve level counters and call
    /// `update_entry_with` directly; this form is kept for queue-level tests.
    #[cfg(test)]
    #[must_use = "the caller must handle committed / rejected / absent outcomes"]
    pub(crate) fn update_entry<F>(
        &self,
        order_id: Id,
        decide: F,
    ) -> Option<Result<Arc<OrderType<()>>, PriceLevelError>>
    where
        F: FnOnce(&OrderType<()>) -> Result<UpdateDecision, PriceLevelError>,
    {
        self.update_entry_with(
            order_id,
            |live| decide(live).map(|decision| (decision, ())),
            |()| Ok(()),
        )
    }

    /// Atomically derive, decide, validate, reserve and commit an update
    /// against the **live** stored order for `order_id`, all inside the
    /// per-entry lock (issues #115, #163).
    ///
    /// `decide` runs against the order currently resident in the map — not a
    /// stale pre-read — and returns the [`UpdateDecision`] to commit, or a
    /// [`PriceLevelError`] to reject the update without touching the queue. The
    /// decision (the resized order and the in-place-vs-demote priority policy)
    /// therefore reflects any concurrent match / replenish that committed before
    /// the lock was taken, so an update can never resurrect executed or
    /// cancelled quantity, and the priority policy can never be chosen from a
    /// stale total. This mirrors the [`OrderQueue::match_front`] decision-closure
    /// pattern; the closure must not let a reference into the live order escape
    /// its return value.
    ///
    /// The protocol is two-phase (issue #163): a pure decision, queue-side
    /// validation, then the caller's side-effecting reservation, then the
    /// infallible commit.
    ///
    /// 1. `decide` runs against the live stored order and returns the
    ///    [`UpdateDecision`] plus a reservation plan `P`. It must be free of
    ///    side effects on anything a rejection would have to undo.
    /// 2. The queue validates the decision: the decided order must keep the id
    ///    it is stored under (an order re-keyed under a different id would
    ///    split the map from the index). A mismatch returns
    ///    [`PriceLevelError::InvalidOperation`] with `reserve` NEVER called, so
    ///    no level counter was reserved and the queue is untouched.
    /// 3. `reserve(plan)` takes the caller's reservations (level counters). On
    ///    `Err` it must have undone its own partial work; the queue is
    ///    untouched.
    /// 4. The commit (`KeepInPlace` swap / `ReplaceAtTail` re-sequence) has no
    ///    failure mode, so a reservation taken in step 3 is never left behind
    ///    by a rejection.
    ///
    /// Both commits happen under the single entry lock this method already
    /// holds: `KeepInPlace` swaps the stored value; `ReplaceAtTail` swaps the
    /// `(seq, order)` pair to the tail sequence the closure reserved and
    /// re-keys the index in place (delegating to a separate re-sequence method
    /// here would deadlock on the same shard lock). Like
    /// [`OrderQueue::match_front`]'s closure, `decide` may call
    /// [`OrderQueue::try_reserve_seq`] (sequence atomic only) but nothing else
    /// on this queue; it reserves the sequence in step 1, before the id
    /// validation and before `reserve` touches any level counter, so an
    /// exhausted sequence is rejected with nothing to roll back (issue #165).
    /// A sequence reserved for a decision the id validation then rejects is
    /// skipped, never reissued (FIFO order and uniqueness are preserved).
    ///
    /// Returns:
    /// - `None` if the id is not present (concurrently removed / never existed);
    ///   neither closure runs;
    /// - `Some(Err(_))` if `decide`, the id validation, or `reserve` rejected
    ///   the update (queue untouched);
    /// - `Some(Ok(new_order))` with the committed order on success.
    #[must_use = "the caller must handle committed / rejected / absent outcomes"]
    pub(crate) fn update_entry_with<F, R, P>(
        &self,
        order_id: Id,
        decide: F,
        reserve: R,
    ) -> Option<Result<Arc<OrderType<()>>, PriceLevelError>>
    where
        F: FnOnce(&OrderType<()>) -> Result<(UpdateDecision, P), PriceLevelError>,
        R: FnOnce(P) -> Result<(), PriceLevelError>,
    {
        match self.orders.entry(order_id) {
            Entry::Vacant(_) => None,
            Entry::Occupied(mut occupied) => {
                // Derive + decide against the LIVE stored order under the lock.
                // The borrow ends with the `decide` call (it returns owned data),
                // so `get_mut()` below is free to commit.
                let (decision, plan) = match decide(occupied.get().1.as_ref()) {
                    Ok(decided) => decided,
                    Err(err) => return Some(Err(err)),
                };
                // Typed validation BEFORE any reservation or commit (issue
                // #163; replaces a debug-only assertion that ran after the
                // caller had already reserved its counters).
                let decided_id = match &decision {
                    UpdateDecision::KeepInPlace(o) | UpdateDecision::ReplaceAtTail(o, _) => o.id(),
                };
                if decided_id != order_id {
                    // Release the entry lock before building the error.
                    drop(occupied);
                    return Some(Err(update_id_mismatch(order_id, decided_id)));
                }
                // Reserve (caller side effects); a failure leaves the queue
                // untouched and the caller has undone its own partial work.
                if let Err(err) = reserve(plan) {
                    return Some(Err(err));
                }
                // Each arm swaps the new order into the slot with `mem::replace`,
                // capturing the OLD `Arc` in `evicted` (issue #128). The old Arc
                // is dropped only AFTER the entry lock is released below, so if
                // the queue held the last reference, its deallocation never runs
                // inside the shard's critical section.
                let (committed, evicted) = match decision {
                    UpdateDecision::KeepInPlace(new_order) => {
                        let evicted =
                            std::mem::replace(&mut occupied.get_mut().1, new_order.clone());
                        (new_order, evicted)
                    }
                    UpdateDecision::ReplaceAtTail(new_order, reserved) => {
                        // Reserved by the decision before any level counter
                        // moved (checked, issue #165).
                        let new_seq = reserved.get();
                        let (old_seq, evicted) = {
                            let slot = occupied.get_mut();
                            let old_seq = slot.0;
                            slot.0 = new_seq;
                            let evicted = std::mem::replace(&mut slot.1, new_order.clone());
                            (old_seq, evicted)
                        };
                        // Re-key NEW-KEY-FIRST (issue #127): insert the new
                        // sequence before removing the old one, so the id is
                        // never transiently absent from the index and a
                        // concurrent front scan can never return `Empty` with
                        // liquidity resting. The transient two-key window is
                        // discarded on selection by the stale-front guard
                        // (the stored sequence is already `new_seq`).
                        self.index.insert(new_seq, order_id);
                        self.index.remove(&old_seq);
                        (new_order, evicted)
                    }
                };
                // Release the shard lock, THEN drop the evicted order.
                drop(occupied);
                drop(evicted);
                Some(Ok(committed))
            }
        }
    }

    /// Re-insert an order at a given (previously assigned) insertion sequence.
    ///
    /// Re-inserting at a maker's original sequence returns it to its place in
    /// the queue, keeping its time priority ahead of orders that arrived later.
    ///
    /// As of #81 the match sweep no longer removes-then-reinserts a partially
    /// filled maker; it swaps the residual in place under the per-entry lock via
    /// [`OrderQueue::match_front`], which keeps the maker resident in `orders`
    /// the whole time (closing the lost-cancel window). This helper survives
    /// only as a queue-priority test fixture and is therefore `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn reinsert(&self, seq: u64, order: Arc<OrderType<()>>) {
        let order_id = order.id();
        self.orders.insert(order_id, (seq, order));
        self.index.insert(seq, order_id);
    }

    /// Search for an order with the given ID. O(1) operation.
    #[must_use]
    #[inline]
    pub fn find(&self, order_id: Id) -> Option<Arc<OrderType<()>>> {
        self.orders.get(&order_id).map(|o| o.value().1.clone())
    }

    /// Remove an order with the given ID.
    /// Returns the removed order if found. Cleans both the map and the index.
    #[must_use]
    pub fn remove(&self, order_id: Id) -> Option<Arc<OrderType<()>>> {
        let (_, (seq, order)) = self.orders.remove(&order_id)?;
        // Race seam (issue #155): the gap between the map removal (which has
        // released the shard lock) and the index removal, where a concurrent
        // readmission and match step can run.
        #[cfg(test)]
        fire_remove_gap_hook(order_id);
        self.index.remove(&seq);
        Some(order)
    }

    /// Remove `order_id` only if `check` accepts it, with the check and the
    /// removal inside ONE per-entry critical section (issue #163).
    ///
    /// The occupied entry is selected and its shard write lock held while
    /// `check` runs against the resident order and, on acceptance, while the
    /// entry is removed. So `check` observes a state in which this order is
    /// resident and cannot be removed or replaced by anyone else until the
    /// decision commits — there is no gap between "the order is here" and
    /// "remove it" for a concurrent admission, cancellation or match step to
    /// fall into. The removal is still the single per-entry removal of issue
    /// #119: a concurrent cancel and match of the same id resolve to exactly
    /// one winner, and the other observes `Absent`.
    ///
    /// `check` runs under the shard write lock: it must be short, must not
    /// touch this queue, must not emit events and should not allocate. It
    /// returns a plain `bool` so the caller builds any error after the lock
    /// is released.
    ///
    /// The index entry is removed after the map entry (as in
    /// [`OrderQueue::remove`]); a front scan that meets the transient stale
    /// index key self-heals on its `Vacant` branch.
    pub(crate) fn remove_if<C>(&self, order_id: Id, check: C) -> RemoveOutcome
    where
        C: FnOnce(&OrderType<()>) -> bool,
    {
        let (seq, order) = match self.orders.entry(order_id) {
            Entry::Vacant(_) => return RemoveOutcome::Absent,
            Entry::Occupied(occupied) => {
                if !check(occupied.get().1.as_ref()) {
                    return RemoveOutcome::Refused;
                }
                // `remove_entry` consumes the guard: the entry lock is
                // released when this arm ends, so the index is cleaned
                // outside the shard critical section.
                let (_, slot) = occupied.remove_entry();
                slot
            }
        };
        // Same map-then-index gap as `remove` (issue #155 test seam).
        #[cfg(test)]
        fire_remove_gap_hook(order_id);
        self.index.remove(&seq);
        RemoveOutcome::Removed(order)
    }

    /// Test-only invariant check: the id-keyed map and the ordered index are
    /// **1:1**. Must be called only at quiescence (no concurrent mutation), when
    /// every in-flight operation has completed.
    ///
    /// Verifies there are exactly as many index entries as map entries and that
    /// every index entry `seq -> id` points to a map entry whose stored sequence
    /// is exactly `seq`. A split index entry (two sequences for one id — the
    /// publication-race bug closed by [`OrderQueue::try_push_with`] holding the
    /// shard lock across both publications) makes the index longer than the map,
    /// so this returns `false`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn debug_map_index_consistent(&self) -> bool {
        if self.index.len() != self.orders.len() {
            return false;
        }
        self.index.iter().all(|entry| {
            let seq = *entry.key();
            let id = *entry.value();
            self.orders
                .get(&id)
                .is_some_and(|slot| slot.value().0 == seq)
        })
    }

    /// Test-only: the resting ids grouped by `DashMap` shard, in the order the
    /// snapshot walk visits them (issue #162).
    ///
    /// Each inner vector is one non-empty shard; the outer order is the shard
    /// visiting order. Shard membership depends only on the id (the map's
    /// hasher is fixed per queue), so a test can pick ids in distinct shards and
    /// know which one the walk captures first. Two consecutive ids share a shard
    /// exactly when a write lock on the previous id is refused while the walk
    /// still holds the current shard's read lock.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn debug_shard_runs(&self) -> Vec<Vec<Id>> {
        let mut runs: Vec<Vec<Id>> = Vec::new();
        let mut previous: Option<Id> = None;
        for entry in self.orders.iter() {
            let id = *entry.key();
            let same_shard = previous.is_some_and(|prev| {
                matches!(
                    self.orders.try_get_mut(&prev),
                    dashmap::try_result::TryResult::Locked
                )
            });
            match runs.last_mut() {
                Some(run) if same_shard => run.push(id),
                _ => runs.push(vec![id]),
            }
            previous = Some(id);
        }
        runs
    }

    /// Iterate through current orders without materializing an intermediate vector.
    ///
    /// The iterator holds a `DashMap` shard read lock between `next()` calls,
    /// so the caller's loop body must not mutate this queue (or its owning
    /// level) from the same thread and should stay short; see
    /// `PriceLevel::iter_orders` for the full caller obligation (issue #172).
    pub fn iter_orders(&self) -> impl Iterator<Item = Arc<OrderType<()>>> + '_ {
        self.orders.iter().map(|entry| entry.value().1.clone())
    }

    /// Starts a [`SeqWalk`] over the resting orders in ascending insertion
    /// sequence (issue #143): lazily for the first `lazy_budget` live orders,
    /// then in one sorted bulk collection of the rest. See [`SeqWalk`].
    pub(crate) fn seq_walk(&self, lazy_budget: u64) -> SeqWalk<'_> {
        SeqWalk {
            queue: self,
            index: self.index.iter(),
            lazy_left: lazy_budget,
            last_seq: None,
            bulk: None,
        }
    }

    /// Collects the committed `(stored_seq, order)` pairs whose sequence is
    /// strictly greater than `after` (every pair when `after` is `None`),
    /// sorted by sequence: the continuation of a [`SeqWalk`] whose lazy
    /// prefix ended at `after`. Same committed-pair guarantees as
    /// [`OrderQueue::snapshot_by_seq`]; a visited order is filtered before
    /// its `Arc` is cloned.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the buffer cannot grow.
    fn collect_pairs_after(&self, after: Option<u64>) -> Result<SeqPairs, PriceLevelError> {
        // Pre-sized to the current length, as `collect_pairs`: the few
        // already-visited orders are an overshoot, not a regrowth.
        let mut pairs: SeqPairs = Vec::new();
        try_reserve_vec(
            &mut pairs,
            self.orders.len(),
            CapacityResource::OrderSnapshot,
        )?;
        for entry in self.orders.iter() {
            let (seq, order) = entry.value();
            if after.is_some_and(|last| *seq <= last) {
                continue;
            }
            #[cfg(test)]
            snapshot_hook::fire(snapshot_hook::SnapshotHookEvent::Collected(*entry.key()));
            try_push_vec(
                &mut pairs,
                (*seq, Arc::clone(order)),
                CapacityResource::OrderSnapshot,
            )?;
        }
        // Unique live sequences: the unstable in-place sort is deterministic
        // (see `snapshot_by_seq_into`).
        pairs.sort_unstable_by_key(|(seq, _)| *seq);
        Ok(pairs)
    }

    /// Materialize a stable snapshot vector sorted by `(timestamp, sequence)`.
    ///
    /// The insertion sequence is used as a deterministic tiebreak so orders
    /// sharing a millisecond timestamp are still ordered exactly as matching
    /// would consume them. This is the timestamp-sorted display / reporting
    /// view; it is not what a snapshot round-trip uses. Snapshot round-trips
    /// materialize via `snapshot_by_seq` (ascending insertion sequence), so the
    /// live queue order — including the "sizing up loses time priority"
    /// demotion — survives a restore.
    ///
    /// Every buffer grows fallibly (issue #164). The sort is the in-place,
    /// allocation-free `sort_unstable_by_key`: it is deterministic because
    /// the `(timestamp, sequence)` keys are unique (every live order carries
    /// its own sequence), so no stable-sort scratch buffer is needed.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if a buffer cannot be reserved.
    /// The queue is only read.
    pub fn snapshot_vec(&self) -> Result<Vec<Arc<OrderType<()>>>, PriceLevelError> {
        let mut pairs = self.collect_pairs()?;
        // Determinism invariant: every collected pair carries a distinct live
        // sequence (one map entry per id; tail sequences come from the checked
        // `try_reserve_seq` and are never reused; an in-place update keeps
        // the order's own sequence). The `(timestamp, seq)` keys are therefore
        // unique, so the unstable sort has no ties to order arbitrarily. A
        // change that could store two live orders under one sequence must
        // revisit this sort.
        pairs.sort_unstable_by_key(|(seq, o)| (o.timestamp(), *seq));
        let mut out = Vec::new();
        try_reserve_exact_vec(&mut out, pairs.len(), CapacityResource::OrderSnapshot)?;
        out.extend(pairs.into_iter().map(|(_, o)| o));
        Ok(out)
    }

    /// Convert the queue to a vector (for compatibility and snapshots).
    ///
    /// # Errors
    ///
    /// As [`OrderQueue::snapshot_vec`].
    pub fn to_vec(&self) -> Result<Vec<Arc<OrderType<()>>>, PriceLevelError> {
        self.snapshot_vec()
    }

    /// Collects the committed `(stored_seq, order)` pairs from the `orders`
    /// map (one entry per id), growing fallibly (issue #164).
    ///
    /// The buffer is pre-sized to the current length; a concurrent admission
    /// that lands during the walk grows it through the same fallible path.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]).
    fn collect_pairs(&self) -> Result<SeqPairs, PriceLevelError> {
        let mut pairs: SeqPairs = Vec::new();
        try_reserve_vec(
            &mut pairs,
            self.orders.len(),
            CapacityResource::OrderSnapshot,
        )?;
        for entry in self.orders.iter() {
            #[cfg(test)]
            snapshot_hook::fire(snapshot_hook::SnapshotHookEvent::Collected(*entry.key()));
            try_push_vec(
                &mut pairs,
                entry.value().clone(),
                CapacityResource::OrderSnapshot,
            )?;
        }
        Ok(pairs)
    }

    /// Materialize the resting orders in ascending **insertion-sequence** order —
    /// the exact order [`OrderQueue::match_front`] consumes them.
    ///
    /// Derived from the authoritative `orders` map (keyed by `Id`) rather than
    /// the `index`: we collect the `(stored_seq, order)` pairs and sort by the
    /// stored sequence. Because the map holds exactly one entry per order, a
    /// duplicate id is impossible by construction, and because each
    /// `(seq, order)` pair is swapped atomically under the `DashMap` per-entry
    /// lock (both the [`FrontAction::ReplaceAtTail`] sweep step and
    /// [`OrderQueue::update_entry_with`] mutate value and sequence together under
    /// it), every emitted pair is a real committed state — an order caught
    /// mid-re-sequencing appears at either its old or its new sequence, never
    /// both and never as a mixed `(old_seq, new_order)` pair. Walking the
    /// `index` instead could pin a stale `seq -> id` entry during a
    /// re-sequencing and emit the same order twice (or at the wrong priority),
    /// which corrupts a snapshot fold.
    ///
    /// Unlike [`OrderQueue::snapshot_vec`] (sorted by `(timestamp, sequence)`),
    /// this reflects pure insertion order, so it equals the sweep even when
    /// timestamps are not monotonic with insertion.
    ///
    /// This view also backs [`crate::price_level::PriceLevel::snapshot`]: the
    /// snapshot round-trip re-enqueues in this consumption order, so exact
    /// price-time priority — including the "sizing up loses time priority"
    /// demotion — is preserved across a restore.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if a buffer cannot be reserved.
    pub(crate) fn snapshot_by_seq(&self) -> Result<Vec<Arc<OrderType<()>>>, PriceLevelError> {
        let mut out = Vec::new();
        self.snapshot_by_seq_into(&mut out)?;
        Ok(out)
    }

    /// Fill `out` with the resting orders in ascending **insertion-sequence**
    /// order — the buffer-reuse variant of [`OrderQueue::snapshot_by_seq`].
    ///
    /// On success `out` holds exactly the materialized orders (its previous
    /// contents are cleared), so a caller can reuse one scratch buffer across
    /// calls and avoid the per-call allocation of the returned `Vec`. Note the
    /// internal `(seq, order)` pairs buffer plus its sort is still paid on
    /// every call — the reuse saves only the output `Vec` allocation, not the
    /// collect-and-sort. The duplicate-free, committed-pair guarantees are
    /// identical to [`OrderQueue::snapshot_by_seq`]; the only difference is
    /// where the result lands.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the internal buffer or `out`
    /// cannot grow. Every reservation happens before `out` is touched, so on
    /// `Err` `out` is left exactly as the caller passed it (issue #164).
    pub(crate) fn snapshot_by_seq_into(
        &self,
        out: &mut Vec<Arc<OrderType<()>>>,
    ) -> Result<(), PriceLevelError> {
        // Build from the `orders` map (one entry per id) so a concurrent
        // re-sequencing can never surface an order twice or at a mixed
        // priority; see `snapshot_by_seq` for the full rationale.
        let mut pairs = self.collect_pairs()?;
        // Determinism invariant: the unstable sort is deterministic only
        // because sequences are unique across live orders (one map entry per
        // id; the tail-appending paths take distinct seqs from the checked
        // `try_reserve_seq`, never reused; an in-place update keeps the
        // order's own seq), so there are no ties. A change that could store
        // two live orders under one sequence must revisit this sort. It sorts
        // in place: no scratch allocation.
        pairs.sort_unstable_by_key(|(seq, _)| *seq);
        // Reserve room for every pair WITHOUT clearing first: `try_reserve`
        // guarantees `capacity >= len + additional`, so reserving
        // `pairs.len() - out.len()` (nothing when `out` is already longer:
        // its capacity then already covers every pair)
        // makes the clear-and-extend below growth-free, and a refusal leaves
        // `out` untouched.
        if let Some(additional) = pairs.len().checked_sub(out.len()) {
            try_reserve_vec(out, additional, CapacityResource::OrderSnapshot)?;
        }
        out.clear();
        out.extend(pairs.into_iter().map(|(_, order)| order));
        Ok(())
    }

    /// Builds a queue holding `orders` in vector order (the first element is
    /// the front), rejecting instead of dropping anything that cannot be
    /// inserted (issue #165).
    ///
    /// Every order goes through [`OrderQueue::try_push`], and the first error
    /// is returned: a repeated id is
    /// [`PriceLevelError::DuplicateOrderId`], and an exhausted sequence is
    /// [`PriceLevelError::CounterExhausted`] (unreachable from a fresh queue,
    /// since a `Vec` holds fewer than `u64::MAX` elements, but reported rather
    /// than assumed). No order is ever silently discarded. This replaces the
    /// former keep-first `from_vec` / `From<Vec<_>>`, which ignored
    /// `try_push` errors.
    ///
    /// # Errors
    ///
    /// See above; on `Err` the partially built queue is dropped.
    pub(crate) fn try_from_vec(orders: Vec<Arc<OrderType<()>>>) -> Result<Self, PriceLevelError> {
        let queue = OrderQueue::new();
        for order in orders {
            queue.try_push(order)?;
        }
        Ok(queue)
    }

    /// Check if the queue is empty
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Returns the number of orders currently in the queue.
    ///
    /// # Returns
    ///
    /// * `usize` - The total count of orders in the queue.
    ///
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.orders.len()
    }
}

impl fmt::Debug for OrderQueue {
    /// Materializes the resting orders (in insertion-sequence order) BEFORE
    /// writing anything, so the formatter's destination — caller-supplied
    /// `fmt::Write` code — never runs while a `DashMap` shard read lock is held
    /// (issue #172). A derived impl would format while iterating the shards,
    /// blocking writers to those shards for as long as the destination takes
    /// and deadlocking a destination that re-enters this queue.
    ///
    /// If the materialization cannot be reserved (issue #164) the `orders`
    /// field shows the allocation-free capacity error instead: `Debug` never
    /// reports a `fmt::Error` of its own, which would make `format!` panic.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let orders = self.snapshot_by_seq();
        let next_seq = self.next_seq.load(Ordering::Relaxed);
        let mut out = f.debug_struct("OrderQueue");
        match &orders {
            Ok(orders) => out.field("orders", orders),
            Err(err) => out.field("orders", &format_args!("<unavailable: {err}>")),
        };
        out.field("next_seq", &next_seq).finish_non_exhaustive()
    }
}

impl Default for OrderQueue {
    fn default() -> Self {
        Self::new()
    }
}
// Implement serialization for OrderQueue
impl Serialize for OrderQueue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Materialize the ordered view first so the length hint always matches
        // the number of elements emitted. `snapshot_by_seq` derives its pairs
        // from the `orders` map (one entry per id) and sorts by insertion
        // sequence, so it can neither duplicate an order nor disagree with its
        // own length during a concurrent re-sequencing. Insertion-sequence
        // order keeps the round-trip price-time priority (the DashMap alone has
        // no deterministic iteration order).
        //
        // The materialization is fallible (issue #164); a refused reservation
        // is reported through the serializer's own error type.
        let ordered = self.snapshot_by_seq().map_err(serde::ser::Error::custom)?;
        let mut seq = serializer.serialize_seq(Some(ordered.len()))?;
        for order in &ordered {
            seq.serialize_element(order.as_ref())?;
        }
        seq.end()
    }
}

impl FromStr for OrderQueue {
    type Err = PriceLevelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let content = s
            .strip_prefix("OrderQueue:orders=[")
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or_else(|| PriceLevelError::ParseError {
                message: "Invalid format".to_string(),
            })?;
        let queue = OrderQueue::new();

        if !content.is_empty() {
            for order_str in content.split(',') {
                let order =
                    OrderType::from_str(order_str).map_err(|e| PriceLevelError::ParseError {
                        message: format!("Order parse error: {e}"),
                    })?;
                // Reject a repeated id rather than overwriting it.
                queue.try_push(Arc::new(order))?;
            }
        }

        Ok(queue)
    }
}

/// Writes `OrderQueue:orders=[<order>,...]` (timestamp order).
///
/// If the order materialization cannot be reserved (issue #164) this writes
/// `OrderQueue:orders=!<error>` instead: `Display` must not report a
/// `fmt::Error` of its own (`to_string` would panic), and [`FromStr`] rejects
/// the marker, so a failed rendering is never parsed back as an empty queue.
impl Display for OrderQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let orders = match self.snapshot_vec() {
            Ok(orders) => orders,
            Err(err) => return write!(f, "OrderQueue:orders=!{err}"),
        };
        write!(f, "OrderQueue:orders=[")?;
        let mut first = true;
        for order in orders {
            if !first {
                write!(f, ",")?;
            }
            write!(f, "{order}")?;
            first = false;
        }
        write!(f, "]")
    }
}

impl TryFrom<Vec<Arc<OrderType<()>>>> for OrderQueue {
    type Error = PriceLevelError;

    /// Builds a queue holding `orders` in vector order (the first element is
    /// the front). Fallible (issue #165): the first order that cannot be
    /// inserted ends the conversion with its error, a repeated id as
    /// [`PriceLevelError::DuplicateOrderId`] or an exhausted sequence as
    /// [`PriceLevelError::CounterExhausted`]. No order is silently dropped.
    fn try_from(orders: Vec<Arc<OrderType<()>>>) -> Result<Self, Self::Error> {
        Self::try_from_vec(orders)
    }
}

/// Materializes the queue in `(timestamp, sequence)` order, like
/// [`OrderQueue::to_vec`].
///
/// Fallible since v0.10 (issue #164): this replaces the infallible
/// `From<OrderQueue> for Vec<Arc<OrderType<()>>>`, whose buffers grew
/// infallibly (it also lived in `orders/`, which must not depend on
/// `price_level/`).
impl TryFrom<OrderQueue> for Vec<Arc<OrderType<()>>> {
    type Error = PriceLevelError;

    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the vector cannot be reserved.
    fn try_from(queue: OrderQueue) -> Result<Self, Self::Error> {
        queue.to_vec()
    }
}

// Custom visitor for deserializing OrderQueue
struct OrderQueueVisitor {
    marker: PhantomData<fn() -> OrderQueue>,
}

impl OrderQueueVisitor {
    fn new() -> Self {
        OrderQueueVisitor {
            marker: PhantomData,
        }
    }
}

impl<'de> Visitor<'de> for OrderQueueVisitor {
    type Value = OrderQueue;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a sequence of orders")
    }

    fn visit_seq<V>(self, mut seq: V) -> Result<OrderQueue, V::Error>
    where
        V: SeqAccess<'de>,
    {
        let queue = OrderQueue::new();

        // Deserialize each order and add it to the queue, rejecting a repeated
        // id rather than silently overwriting it.
        while let Some(order) = seq.next_element::<OrderType<()>>()? {
            queue
                .try_push(Arc::new(order))
                .map_err(serde::de::Error::custom)?;
        }

        Ok(queue)
    }
}

// Implement deserialization for OrderQueue
impl<'de> Deserialize<'de> for OrderQueue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize as a sequence of orders
        deserializer.deserialize_seq(OrderQueueVisitor::new())

        // Alternative approach: Deserialize as OrderQueueData first, then convert
        // let data = OrderQueueData::deserialize(deserializer)?;
        // let queue = OrderQueue::new();
        // for order in data.orders {
        //     queue.push(Arc::new(order));
        // }
        // Ok(queue)
    }
}

/// Test-only collection hook for the snapshot walk (issue #162).
///
/// Lets a test run code at deterministic points of
/// [`crate::price_level::PriceLevel::snapshot`]: at the start of each attempt
/// and as each order is captured by the shard walk. The hook is thread-local,
/// so it only fires on the thread that installed it (other tests and helper
/// threads are unaffected), and the whole module is `#[cfg(test)]`: it does
/// not exist in production builds and adds no knob to the public surface.
///
/// A `Collected` event fires while the walk holds the captured order's
/// `DashMap` shard read lock. A hook that mutates the level must do so from a
/// different thread (and wait for it) and must only touch ids in other shards;
/// see `OrderQueue::debug_shard_runs`.
#[cfg(test)]
pub(crate) mod snapshot_hook {
    use crate::orders::Id;
    use std::cell::RefCell;

    /// A point in the snapshot walk at which the installed hook runs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum SnapshotHookEvent {
        /// A snapshot attempt is about to walk the orders.
        AttemptStart,
        /// The walk has just captured the order with this id.
        Collected(Id),
    }

    type Hook = Box<dyn FnMut(SnapshotHookEvent)>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Clears the current thread's hook when dropped, so a failing test cannot
    /// leak its hook into a later test on the same thread.
    pub(crate) struct SnapshotHookGuard;

    impl Drop for SnapshotHookGuard {
        fn drop(&mut self) {
            HOOK.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    *slot = None;
                }
            });
        }
    }

    /// Installs `hook` for the current thread, replacing any previous one. The
    /// hook stays installed until the returned guard is dropped.
    #[must_use = "the hook is removed when the guard is dropped"]
    pub(crate) fn install(hook: impl FnMut(SnapshotHookEvent) + 'static) -> SnapshotHookGuard {
        HOOK.with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut() {
                *slot = Some(Box::new(hook));
            }
        });
        SnapshotHookGuard
    }

    /// Runs the current thread's hook for `event`. A re-entrant fire (the hook
    /// itself triggering a walk on this thread) is skipped.
    pub(crate) fn fire(event: SnapshotHookEvent) {
        HOOK.with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut()
                && let Some(hook) = slot.as_mut()
            {
                hook(event);
            }
        });
    }
}
