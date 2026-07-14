//! Core price level implementation

use crate::UuidGenerator;
use crate::errors::PriceLevelError;
use crate::execution::{MatchResult, TakerKind, Trade};
use crate::orders::{Id, OrderType, OrderUpdate, Side, TimeInForce};
use crate::price_level::order_queue::{FrontAction, FrontOutcome, OrderQueue, UpdateDecision};
use crate::price_level::{PriceLevelSnapshot, PriceLevelSnapshotPackage, PriceLevelStatistics};
use crate::utils::{Price, Quantity, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::str::FromStr;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Bit layout of the [`PriceLevel::topology`] word (issue #126): the high two
/// bits carry the pinned-side tag, the low bits the resting-order count. Packing
/// both into one atomic makes the side pin and the count move together in a
/// single compare-exchange, so a drain's un-pin can never race an admission's
/// pin across two independent atomics.
mod topology {
    use crate::orders::Side;

    /// Bits reserved for the resting-order count (the rest hold the side tag).
    /// `u64::MAX >> 2` orders is astronomically beyond any level's capacity, so
    /// nothing is lost by borrowing the top two bits for the tag.
    pub(super) const COUNT_BITS: u32 = 62;
    pub(super) const COUNT_MASK: u64 = (1 << COUNT_BITS) - 1;
    pub(super) const TAG_UNPINNED: u64 = 0;
    pub(super) const TAG_BUY: u64 = 1;
    pub(super) const TAG_SELL: u64 = 2;

    #[inline]
    pub(super) fn tag_of(side: Side) -> u64 {
        match side {
            Side::Buy => TAG_BUY,
            Side::Sell => TAG_SELL,
        }
    }

    #[inline]
    pub(super) fn side_of_tag(tag: u64) -> Option<Side> {
        match tag {
            TAG_BUY => Some(Side::Buy),
            TAG_SELL => Some(Side::Sell),
            _ => None,
        }
    }

    #[inline]
    pub(super) fn pack(tag: u64, count: u64) -> u64 {
        (tag << COUNT_BITS) | count
    }

    #[inline]
    pub(super) fn tag(word: u64) -> u64 {
        word >> COUNT_BITS
    }

    #[inline]
    pub(super) fn count(word: u64) -> u64 {
        word & COUNT_MASK
    }
}

// Deterministic race seam for the post-only decision boundary (issue #130).
//
// `match_order` fires `fire_post_only_decision_hook` BETWEEN the post-only depth
// decision and its commit, so a test can install a hook that mutates the level
// (e.g. `add_order`) in that exact window and assert the outcome
// deterministically, without relying on scheduler stress. Production builds
// compile none of this — the hook call site is `#[cfg(test)]`.
#[cfg(test)]
thread_local! {
    static POST_ONLY_DECISION_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install a hook fired at the post-only decision boundary (test seam, issue
/// #130). Returns a guard that clears the hook on drop.
#[cfg(test)]
pub(crate) fn set_post_only_decision_hook(hook: Box<dyn FnMut()>) -> PostOnlyHookGuard {
    POST_ONLY_DECISION_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
    PostOnlyHookGuard
}

/// Clears the post-only decision hook when dropped (test seam, issue #130).
#[cfg(test)]
pub(crate) struct PostOnlyHookGuard;

#[cfg(test)]
impl Drop for PostOnlyHookGuard {
    fn drop(&mut self) {
        POST_ONLY_DECISION_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Fire the post-only decision hook if one is installed (test seam, issue #130).
#[cfg(test)]
fn fire_post_only_decision_hook() {
    // Take the hook OUT of the slot while firing so a re-entrant `match_order`
    // inside the hook does not double-borrow the `RefCell`.
    let hook = POST_ONLY_DECISION_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
        POST_ONLY_DECISION_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }
}

/// A price level in a limit order book, lock-free on the match path.
///
/// A `Gtc` / `Ioc` / `Day` match runs entirely on atomic counters and lock-free
/// (sharded / skiplist) structures — no lock. The mutators [`Self::add_order`]
/// and [`Self::update_order`] (cancel and resize included) are NOT lock-free:
/// each takes the **shared** side of a per-level reader-writer guard. That
/// acquisition is normally uncontended (it only coordinates with a fill-or-kill
/// match), but it can BLOCK behind a concurrent fill-or-kill: a `Fok` match
/// takes the guard's **exclusive** side across its feasibility check and sweep —
/// an `O(depth)` critical section — so it stays all-or-nothing against
/// concurrent mutation. See the `fok_guard` field and [`Self::match_order`] for
/// the full argument (issue #112).
///
/// # Topology
///
/// Every resting order sits at [`Self::price`] and shares a single side. The
/// side is **pinned atomically** rather than derived from the queue: a single
/// `topology` word packs `(pinned side, resting order count)` so that an
/// admission's side decision and the drain that un-pins an emptied level are
/// one compare-exchange, never two racing atomics (issue #126). The first
/// admitted maker pins the side; each later same-side admission bumps the
/// count under the same CAS; the removal that brings the count to zero un-pins
/// in the same CAS, so a fully drained level accepts either side again. An
/// opposite-side admission into a non-empty level is rejected.
///
/// Single-side coherence is a **correctness invariant**, not an
/// eventually-consistent one — unlike the advisory quantity / count counters
/// (issue #68), a level that admitted two makers of opposite sides would not
/// converge to a correct state later. Pinning side+count in one atomic upholds
/// it under **arbitrary concurrent admissions and removals**, closing both
/// races the earlier derive-from-queue scheme left open:
///
/// - Two **opposite-side admissions into a genuinely empty level** now
///   serialize on the pin CAS: exactly one establishes the side, the other
///   observes a non-empty opposite-side level and is rejected.
/// - An **opposite-side admission racing a same-side upsize** cannot slip
///   through a transient queue gap: the pin persists across a maker's
///   demotion because the count never reaches zero (and as of issue #119 the
///   quantity-increase demotion re-sequences in place without vacating the id
///   at all).
///
/// A concurrent [`Self::snapshot`] cannot capture a torn old-side/new-side view
/// across a drain-then-re-admit either: a `topology_epoch` is bumped on every
/// side pin / un-pin, and `snapshot` retries its materialization if the epoch
/// moves under it (see there).
#[derive(Debug)]
pub struct PriceLevel {
    /// The price of this level
    price: u128,

    /// Total visible quantity at this price level
    visible_quantity: AtomicU64,

    /// Total hidden quantity at this price level
    hidden_quantity: AtomicU64,

    /// Packed `(pinned side, resting order count)` — the atomic topology word
    /// (issue #126). The side tag lives in the high two bits, the count in the
    /// low [`topology::COUNT_BITS`]; see the [`topology`] module for the layout
    /// and [`Self::topology_admit`] / [`Self::topology_release_one`] for the CAS
    /// protocol. Replaces the former standalone `order_count` counter — the
    /// count is now read back out of this word.
    topology: AtomicU64,

    /// Monotonic counter bumped on every side pin / un-pin (issue #126). A
    /// [`Self::snapshot`] reads it before and after materializing the orders and
    /// retries if it moved, so a checksummed snapshot can never capture a torn
    /// old-side/new-side view across a drain-then-re-admit transition.
    topology_epoch: AtomicU64,

    /// Queue of orders at this price level
    orders: OrderQueue,

    /// Statistics for this price level
    stats: Arc<PriceLevelStatistics>,

    /// Fill-or-kill exclusion guard (issue #112).
    ///
    /// A fill-or-kill match must be **all-or-nothing** with respect to
    /// concurrent `add_order` / `update_order` (cancel / resize): between its
    /// depth dry-run and its sweep, a concurrent shrink could otherwise leave it
    /// partially filled. All-or-nothing across N makers is a cross-maker
    /// transactional property the per-entry atomics cannot express, so the FOK
    /// path takes this guard's **write** (exclusive) side across its dry-run and
    /// sweep, and the mutators ([`Self::add_order`] / [`Self::update_order`])
    /// take the **read** (shared) side. The common paths therefore pay only an
    /// uncontended shared acquisition; a FOK (a cold, specific TIF) excludes
    /// mutators for the duration of its feasibility check and sweep — an
    /// `O(depth)` exclusive section, not a constant-time one. The non-FOK sweep
    /// takes NO guard — it relies on the
    /// single-matcher-per-level model and the existing per-entry cancel
    /// atomicity (issue #81). The guarded value is `()`, so a poisoned lock is
    /// recovered with `into_inner` — but a poison means a holder PANICKED
    /// mid-operation, which may have left the level half-mutated, so the recovery
    /// also trips [`Self::level_poisoned`] and the level then fails fast rather
    /// than silently reopening (issue #130).
    fok_guard: RwLock<()>,

    /// Sticky fail-fast flag set when a poisoned [`Self::fok_guard`] is recovered
    /// (issue #130): a guard holder panicked mid-operation, so the level may be
    /// half-mutated and cannot be trusted. While set, `add_order` / `update_order`
    /// return [`PriceLevelError::InvalidOperation`] and `match_order` refuses to
    /// match (returns an empty result); `snapshot` stays allowed for diagnostics
    /// / reconstruction. The production match sweep has no unwind path (audited),
    /// so this is defense-in-depth; a poisoned level requires reconstruction from
    /// a snapshot. Never cleared once set.
    level_poisoned: AtomicBool,

    /// Monotonic counter bumped by every committing `add_order` / `update_order`
    /// (cancel / resize) so the non-atomic post-only depth scan can linearize
    /// (issue #130). [`Self::has_matchable_depth`] reads it, scans the queue,
    /// re-reads it, and retries on change: a stable epoch across the scan means
    /// no mutation committed during it, giving the post-only verdict a
    /// linearization point instead of a torn read.
    mutation_epoch: AtomicU64,
}

impl PriceLevel {
    /// Reconstructs a price level directly from a snapshot.
    ///
    /// The rebuilt level carries the per-level statistics persisted in the
    /// snapshot (orders added / removed / executed, quantity / value executed,
    /// waiting-time aggregates, and execution / arrival timestamps) rather than
    /// a fresh, zeroed set — so a restored level resumes with its recorded
    /// history.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any restored order's own
    /// visible + hidden total overflows `u64`, or if recomputing the snapshot's
    /// level aggregates overflows `u64` — the same per-order and per-level
    /// invariants [`Self::add_order`] enforces at admission — or
    /// [`PriceLevelError::DuplicateOrderId`] if the snapshot's orders vector
    /// repeats an order id.
    pub fn from_snapshot(mut snapshot: PriceLevelSnapshot) -> Result<Self, PriceLevelError> {
        snapshot.refresh_aggregates()?;

        // Reject a snapshot whose orders vector repeats an id. Building the
        // queue would drop the duplicate (keep-first), but the reconstructed
        // counters are taken from the snapshot's own aggregates / order count,
        // so a silently-dropped duplicate would leave the restored level's
        // counters disagreeing with its queue. Fail deterministically instead.
        {
            let orders = snapshot.orders();
            let mut seen = std::collections::HashSet::with_capacity(orders.len());
            for order in orders {
                if !seen.insert(order.id()) {
                    return Err(PriceLevelError::DuplicateOrderId(order.id().to_string()));
                }
            }
        }

        // Topology invariants (same rules `add_order` enforces): every order
        // must sit at the level's price and share a single side. A snapshot that
        // violates either would reconstruct a level that trades at the wrong
        // price or emits contradictory taker sides, so reject it here rather
        // than restore an incoherent level.
        let level_side: Option<Side> = {
            let level_price = snapshot.price().as_u128();
            let mut level_side = None;
            for order in snapshot.orders() {
                if order.price().as_u128() != level_price {
                    return Err(PriceLevelError::InvalidOperation {
                        message: format!(
                            "snapshot order price {} does not match level price {level_price}",
                            order.price().as_u128()
                        ),
                    });
                }
                match level_side {
                    None => level_side = Some(order.side()),
                    Some(side) if side != order.side() => {
                        return Err(PriceLevelError::InvalidOperation {
                            message: format!(
                                "snapshot order side {:?} is incompatible with the level side {side:?}",
                                order.side()
                            ),
                        });
                    }
                    Some(_) => {}
                }
            }
            level_side
        };

        let order_count = snapshot.orders().len();
        let visible_quantity = snapshot.visible_quantity().as_u64();
        let hidden_quantity = snapshot.hidden_quantity().as_u64();
        let price = snapshot.price().as_u128();
        // Clone the persisted statistics before consuming the snapshot's orders.
        let stats = (*snapshot.statistics()).clone();
        let queue = OrderQueue::from(snapshot.into_orders());

        // Pin the restored side alongside the restored count in the topology word
        // (issue #126). An empty snapshot restores Unpinned; a non-empty one pins
        // the single side the validation above proved coherent. `order_count` is
        // bounded by the snapshot's own vector length, which fits `COUNT_MASK`.
        let side_tag = level_side.map_or(topology::TAG_UNPINNED, topology::tag_of);
        let topology_word = topology::pack(side_tag, order_count as u64);

        Ok(Self {
            price,
            visible_quantity: AtomicU64::new(visible_quantity),
            hidden_quantity: AtomicU64::new(hidden_quantity),
            topology: AtomicU64::new(topology_word),
            topology_epoch: AtomicU64::new(0),
            orders: queue,
            stats: Arc::new(stats),
            fok_guard: RwLock::new(()),
            level_poisoned: AtomicBool::new(false),
            mutation_epoch: AtomicU64::new(0),
        })
    }

    /// Reconstructs a price level from a checksum-protected snapshot package.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::ChecksumMismatch`] if the package's embedded
    /// SHA-256 checksum does not match its payload (tampered or corrupted
    /// snapshot), [`PriceLevelError::SerializationError`] if re-encoding the
    /// payload to recompute that checksum fails,
    /// [`PriceLevelError::InvalidOperation`] if the package carries an
    /// unsupported snapshot format version, and propagates any
    /// [`PriceLevelError`] from rebuilding the level out of the validated
    /// snapshot.
    pub fn from_snapshot_package(
        package: PriceLevelSnapshotPackage,
    ) -> Result<Self, PriceLevelError> {
        let snapshot = package.into_snapshot()?;
        Self::from_snapshot(snapshot)
    }

    /// Restores a price level from its snapshot JSON representation.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DeserializationError`] if `data` is not a
    /// valid snapshot-package JSON document, [`PriceLevelError::ChecksumMismatch`]
    /// if the decoded package's SHA-256 checksum does not match its payload,
    /// [`PriceLevelError::SerializationError`] if re-encoding the payload to
    /// recompute that checksum fails, [`PriceLevelError::InvalidOperation`]
    /// on an unsupported snapshot format version, and
    /// [`PriceLevelError::DuplicateOrderId`] if the decoded snapshot's orders
    /// vector repeats an order id.
    pub fn from_snapshot_json(data: &str) -> Result<Self, PriceLevelError> {
        let package = PriceLevelSnapshotPackage::from_json(data)?;
        Self::from_snapshot_package(package)
    }
}

impl PriceLevel {
    /// Create a new price level
    #[must_use]
    pub fn new(price: u128) -> Self {
        Self {
            price,
            visible_quantity: AtomicU64::new(0),
            hidden_quantity: AtomicU64::new(0),
            // Unpinned side, zero resting orders.
            topology: AtomicU64::new(topology::pack(topology::TAG_UNPINNED, 0)),
            topology_epoch: AtomicU64::new(0),
            orders: OrderQueue::new(),
            stats: Arc::new(PriceLevelStatistics::new()),
            fok_guard: RwLock::new(()),
            level_poisoned: AtomicBool::new(false),
            mutation_epoch: AtomicU64::new(0),
        }
    }

    /// Get the price of this level
    #[must_use]
    pub fn price(&self) -> u128 {
        self.price
    }

    /// Get the visible quantity, in quantity units.
    ///
    /// This is an **advisory, eventually-consistent** read: it loads a single
    /// atomic counter, which under concurrent `add_order` / `match_order` /
    /// `update_order` can briefly lead or lag the queue contents (it may not yet
    /// include an order already in the queue, or still count one just removed).
    /// The relative order of the counter update and the queue mutation is not a
    /// guaranteed cross-method invariant — different paths order them
    /// differently (e.g. iceberg replenishment in `match_order` adjusts the
    /// counters before pushing the refreshed tranche). Treat any single counter
    /// read as approximate; for a reading where the counters and the order list
    /// are guaranteed mutually consistent, take a [`Self::snapshot`] and read
    /// from it.
    #[must_use]
    pub fn visible_quantity(&self) -> u64 {
        // `Relaxed`: this counter is advisory / eventually-consistent (see the
        // doc above and issue #68). It carries NO happens-before relationship —
        // the lock-free `SkipMap` / `DashMap` in `OrderQueue` carry the real
        // ordering between producers and consumers, and `snapshot()` is the
        // mutually-consistent view. Nothing is published or synchronized through
        // this load, so `Acquire` would buy nothing.
        self.visible_quantity.load(Ordering::Relaxed)
    }

    /// Get the hidden quantity, in quantity units.
    ///
    /// Advisory / eventually-consistent under concurrent mutation — see
    /// [`Self::visible_quantity`]; use [`Self::snapshot`] for a consistent view.
    #[must_use]
    pub fn hidden_quantity(&self) -> u64 {
        // `Relaxed`: advisory counter, no happens-before rides on it — see
        // `visible_quantity` for the full rationale.
        self.hidden_quantity.load(Ordering::Relaxed)
    }

    /// Get the total quantity (visible + hidden), in quantity units.
    ///
    /// Advisory / eventually-consistent under concurrent mutation (sums two
    /// independent atomic counters) — see [`Self::visible_quantity`]; use
    /// [`Self::snapshot`] for a consistent view.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `visible + hidden`
    /// overflows `u64`.
    pub fn total_quantity(&self) -> Result<u64, PriceLevelError> {
        self.visible_quantity()
            .checked_add(self.hidden_quantity())
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: "price level total quantity overflow".to_string(),
            })
    }

    /// Get the number of orders.
    ///
    /// Advisory / eventually-consistent under concurrent mutation — see
    /// [`Self::visible_quantity`]; use [`Self::snapshot`] for a consistent view.
    #[must_use]
    pub fn order_count(&self) -> usize {
        // `Relaxed`: advisory read of the count half of the topology word, no
        // happens-before rides on it — see `visible_quantity` for the rationale.
        topology::count(self.topology.load(Ordering::Relaxed)) as usize
    }

    /// The side currently pinned at this level, or `None` if the level is empty
    /// (Unpinned). Advisory: a concurrent admission / drain can change it right
    /// after the read.
    #[must_use]
    fn pinned_side(&self) -> Option<Side> {
        topology::side_of_tag(topology::tag(self.topology.load(Ordering::Relaxed)))
    }

    /// Reserve one admission slot for a `side` order: pin the side (or verify it
    /// matches the pinned side) and increment the resting-order count, in a
    /// single compare-exchange (issue #126).
    ///
    /// Returns `Ok(true)` iff this call pinned a previously-empty level (the
    /// caller then bumps [`Self::topology_epoch`]), `Ok(false)` if it joined an
    /// already-pinned same-side level.
    ///
    /// Because the side and the count move together, two opposite-side
    /// admissions into an empty level serialize here: exactly one wins the CAS
    /// that pins the side, and the loser then observes a non-empty opposite-side
    /// level and is rejected. There is no separate "is the level empty" atomic to
    /// fall out of step with the side.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] if `side` is incompatible with the
    /// pinned side of a non-empty level, or if the count would exceed
    /// [`topology::COUNT_MASK`].
    fn topology_admit(&self, side: Side) -> Result<bool, PriceLevelError> {
        let my_tag = topology::tag_of(side);
        loop {
            let cur = self.topology.load(Ordering::Acquire);
            let tag = topology::tag(cur);
            let count = topology::count(cur);
            if count == 0 {
                // Empty level: establish this side with count 1.
                let next = topology::pack(my_tag, 1);
                if self
                    .topology
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(true);
                }
            } else if tag == my_tag {
                // Same side: bump the count (checked — never wraps).
                let Some(new_count) = count.checked_add(1).filter(|c| *c <= topology::COUNT_MASK)
                else {
                    return Err(PriceLevelError::InvalidOperation {
                        message: "price level order count overflow on admission".to_string(),
                    });
                };
                let next = topology::pack(my_tag, new_count);
                if self
                    .topology
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(false);
                }
            } else {
                // Non-empty level pinned to the opposite side: reject.
                let resting = topology::side_of_tag(tag);
                return Err(PriceLevelError::InvalidOperation {
                    message: format!(
                        "order side {side:?} is incompatible with the level's resting side {resting:?}"
                    ),
                });
            }
            // Lost the CAS to a concurrent mutation; reload and retry.
        }
    }

    /// Release one admission slot after removing an order: decrement the count
    /// and un-pin the side when it reaches zero, in a single compare-exchange
    /// (issue #126).
    ///
    /// Returns `true` iff this call brought the count to zero and un-pinned the
    /// level (the caller then bumps [`Self::topology_epoch`]). Because the un-pin
    /// rides the same CAS as the decrement, a concurrent admission either sees
    /// the still-pinned non-empty level (and joins / is rejected) or the drained
    /// Unpinned level (and establishes) — never an inconsistent in-between.
    fn topology_release_one(&self) -> bool {
        loop {
            let cur = self.topology.load(Ordering::Acquire);
            let count = topology::count(cur);
            if count == 0 {
                // A removal only runs for an order this level held, so the count
                // is >= 1; never wrap (crate rule). Treat an impossible underflow
                // as a no-op rather than corrupt the word.
                debug_assert!(false, "topology count underflow on release");
                return false;
            }
            let new_count = count - 1;
            let next = if new_count == 0 {
                topology::pack(topology::TAG_UNPINNED, 0)
            } else {
                topology::pack(topology::tag(cur), new_count)
            };
            if self
                .topology
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return new_count == 0;
            }
        }
    }

    /// Bump the topology epoch on a side pin / un-pin so a racing
    /// [`Self::snapshot`] retries a materialization that spanned the transition.
    #[inline]
    fn bump_topology_epoch(&self) {
        self.topology_epoch.fetch_add(1, Ordering::Release);
    }

    /// Bump the mutation epoch on a committed add / cancel / resize so a racing
    /// post-only depth scan retries (issue #130). `Release` so the queue mutation
    /// that precedes it happens-before a scanner's `Acquire` read of the epoch.
    #[inline]
    fn bump_mutation_epoch(&self) {
        self.mutation_epoch.fetch_add(1, Ordering::Release);
    }

    /// Returns `true` if `orders` is empty or every order shares one side — the
    /// single-side coherence [`Self::from_snapshot`] requires. Used as the
    /// termination backstop for `snapshot`'s torn-topology retry (issue #126).
    fn is_single_side(orders: &[Arc<OrderType<()>>]) -> bool {
        let mut side = None;
        for order in orders {
            match side {
                None => side = Some(order.side()),
                Some(s) if s != order.side() => return false,
                Some(_) => {}
            }
        }
        true
    }

    /// Get the statistics for this price level
    #[must_use]
    pub fn stats(&self) -> Arc<PriceLevelStatistics> {
        self.stats.clone()
    }

    /// Acquire the fill-or-kill guard's **shared (read)** side — the mutator
    /// side. Multiple mutators proceed concurrently; a fill-or-kill match
    /// (holding the exclusive side) excludes them. A poisoned lock is recovered
    /// with `into_inner` (the guarded value is `()`), but the recovery trips the
    /// sticky [`Self::level_poisoned`] flag so the level then fails fast (issue
    /// #130): a poison means a holder panicked mid-operation.
    #[inline]
    fn fok_read(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.fok_guard.read().unwrap_or_else(|poison| {
            self.mark_poisoned();
            poison.into_inner()
        })
    }

    /// Acquire the fill-or-kill guard's **exclusive (write)** side — held across
    /// a fill-or-kill dry-run + sweep so no mutator can change the matchable
    /// depth mid-decision. A poisoned lock is recovered (see [`Self::fok_read`]).
    #[inline]
    fn fok_write(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.fok_guard.write().unwrap_or_else(|poison| {
            self.mark_poisoned();
            poison.into_inner()
        })
    }

    /// Trip the sticky poison flag when a [`Self::fok_guard`] poison is recovered
    /// (issue #130). Logs `ERROR` exactly once — on the `false -> true`
    /// transition decided by the `compare_exchange` — so a poisoned level is
    /// reported but not flooded.
    #[cold]
    fn mark_poisoned(&self) {
        if self
            .level_poisoned
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            tracing::error!(
                price = self.price,
                "price level poisoned by a panicked operation; matching and mutation are now refused — reconstruct the level from a snapshot"
            );
        }
    }

    /// Returns `true` if the level has been poisoned by a panicked guard holder
    /// (issue #130).
    #[inline]
    #[must_use]
    fn is_poisoned(&self) -> bool {
        self.level_poisoned.load(Ordering::Relaxed)
    }

    /// Fail-fast guard for the mutating public methods: `Err` once the level is
    /// poisoned (issue #130).
    #[inline]
    fn poison_check(&self) -> Result<(), PriceLevelError> {
        if self.is_poisoned() {
            Err(PriceLevelError::InvalidOperation {
                message: "price level poisoned by a panicked operation".to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Genuinely poison the fill-or-kill guard by panicking while holding its
    /// write side (issue #130 test seam). The panic is caught so the test
    /// process survives; the `RwLock` is left poisoned, so the NEXT guard
    /// acquisition recovers it and trips [`Self::level_poisoned`], exercising the
    /// real fail-fast path (not a directly-set flag).
    #[cfg(test)]
    pub(crate) fn test_poison_guard(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.fok_write();
            panic!("intentional guard poison for test");
        }));
    }

    /// Add an order to this price level.
    ///
    /// Decides the order's id IDENTITY first, then reserves its visible /
    /// hidden quantity and one count slot on the atomic counters **atomically
    /// with publishing** it to the queue, so an admission that reuses the id of
    /// an order already resting here — or that would overflow any counter — is
    /// rejected with nothing mutated: the queue, the counters, the statistics,
    /// and therefore any snapshot are left exactly as they were. On success the
    /// returned `Arc` is the admitted order.
    ///
    /// # Duplicate ids
    ///
    /// Publication goes through
    /// [`OrderQueue::try_push_with`](crate::price_level::OrderQueue) under the
    /// id-keyed map's per-shard lock, which decides the id is free **before**
    /// the counter reservation runs. Several concurrent submissions of the same
    /// id therefore resolve to exactly one admission; the rest return
    /// [`PriceLevelError::DuplicateOrderId`] **without touching any counter** —
    /// a rejected duplicate never overwrites the live order (which would leave
    /// the map and the ordered index disagreeing) and never transiently inflates
    /// a level counter.
    ///
    /// # Error precedence
    ///
    /// The order's own `visible + hidden` overflow is checked first (a pure
    /// property of the order). Then, under the shard lock, a **duplicate id is
    /// decided before any counter is touched**: an admission that both reuses a
    /// live id and would overflow a counter reports
    /// [`PriceLevelError::DuplicateOrderId`], never the overflow. Only for a
    /// free id is capacity reserved: the visible and hidden quantities with a
    /// checked [`AtomicU64::fetch_update`] (`checked_add`) — an atomic
    /// compare-exchange loop, free of the check-then-`fetch_add` TOCTOU race two
    /// admissions near `u64::MAX` would hit — and THEN the side pin and order
    /// count together in one compare-exchange (`topology_admit`), which also
    /// serializes concurrent opposite-side admissions. If the quantity
    /// reservations or the pin fail, the earlier ones are rolled back (by the
    /// exact delta this call added — a commutative, concurrency-safe undo on
    /// these advisory counters), so `try_push_with` publishes nothing
    /// and no counter is left drifted.
    ///
    /// # Topology invariants
    ///
    /// A level holds orders at exactly one price and one side. The order's price
    /// must equal the level's price, and its side must match the side of the
    /// orders already resting here (the first admitted maker pins the side; a
    /// fully drained level accepts either side again). Both are checked before
    /// any counter is touched, so a rejected order leaves the level unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the order's price does
    /// not match the level's, if its side is incompatible with the resting
    /// side, if the order's own visible + hidden total overflows `u64`, or if
    /// admitting it would overflow the level's visible-quantity,
    /// hidden-quantity, or order-count counter; or
    /// [`PriceLevelError::DuplicateOrderId`] if an order with the same id
    /// already rests at this level. A duplicate id takes precedence over a
    /// counter overflow. In every case the level is unchanged.
    pub fn add_order(&self, order: OrderType<()>) -> Result<Arc<OrderType<()>>, PriceLevelError> {
        // Hold the fill-or-kill guard's shared side for this admission so a
        // concurrent fill-or-kill match sees a stable depth (issue #112). This
        // is an uncontended shared acquisition in the common case (no FOK).
        let _fok = self.fok_read();
        // Fail fast if a prior panic poisoned the guard (or this very acquisition
        // just recovered one): the level may be half-mutated (issue #130).
        self.poison_check()?;

        // -------- Admission topology invariants (cheapest checks, no mutation) --------
        //
        // A level holds orders at exactly one price and one side. Reject a
        // mismatch BEFORE reserving any counter capacity, so the level is left
        // completely unchanged. Price is the cheapest check (two `u128`s), so it
        // goes first; the side is derived from whatever is already resting.
        if order.price().as_u128() != self.price {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "order price {} does not match level price {}",
                    order.price().as_u128(),
                    self.price
                ),
            });
        }
        // The level's side is pinned in the topology word (issue #126): the
        // first maker pins it, later same-side makers join, and the drain that
        // empties the level un-pins it so a drained level accepts either side
        // again. This is a cheap EARLY reject of an opposite-side order against a
        // non-empty level, so the common mismatch never reserves counter
        // capacity. It is only an optimization — the AUTHORITATIVE, race-free
        // side decision is the pin CAS ([`Self::topology_admit`]) run inside the
        // reservation closure below, which serializes concurrent admissions.
        let order_side = order.side();
        if let Some(resting_side) = self.pinned_side()
            && order_side != resting_side
        {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "order side {order_side:?} is incompatible with the level's resting side {resting_side:?}"
                ),
            });
        }

        // Calculate quantities.
        let visible_qty = order.visible_quantity().as_u64();
        let hidden_qty = order.hidden_quantity().as_u64();

        // Reject an order whose OWN visible + hidden total is not representable
        // in `u64`, before touching any counter. The level tracks visible and
        // hidden in two independent `u64` counters, so an order with, say,
        // visible `u64::MAX` and hidden `u64::MAX` would clear the per-counter
        // reservations below yet leave the level holding an order whose total
        // quantity overflows. Worse, the match sweep's reserve replenishment
        // computes `new_visible + drawn_hidden` (see `OrderType::match_against`),
        // where `drawn_hidden` is capped by the order's hidden tranche; that sum
        // is `<= visible + hidden`, so it can only overflow `u64` when the
        // order's own total already does. Enforcing the invariant here makes that
        // replenish add provably overflow-free for every admitted order.
        if visible_qty.checked_add(hidden_qty).is_none() {
            return Err(PriceLevelError::InvalidOperation {
                message: "order total quantity overflows u64".to_string(),
            });
        }

        // Publish through `try_push_with`, which decides id IDENTITY FIRST under
        // the DashMap shard lock and only then runs the counter reservation
        // below — atomically with the publication. Consequences:
        //
        // * A duplicate id is rejected with `DuplicateOrderId` and NOTHING
        //   touched: the reservation closure never runs, so a rejected duplicate
        //   cannot transiently inflate a counter, and a duplicate submitted at
        //   counter capacity reports `DuplicateOrderId` (identity) rather than a
        //   spurious overflow (the error precedence documented above).
        // * The reservation runs only once the id is known free; the queue then
        //   publishes the order into the map + index while holding the shard
        //   lock, so a concurrent cancel + readmission can never split this id
        //   across two index entries.
        //
        // Inside the closure, capacity is reserved visible → hidden with checked
        // `fetch_update` (an atomic CAS loop, free of the check-then-`fetch_add`
        // TOCTOU race two admissions near `u64::MAX` would hit), and THEN the
        // side is pinned and the count bumped in one CAS via `topology_admit`.
        // `Relaxed` on the advisory visible / hidden RMWs (issue #68); the pin
        // CAS uses `AcqRel` because side coherence is a hard invariant, not an
        // advisory counter. The pin goes LAST so it is only mutated on the
        // success path: an incompatible side or a count overflow returns `Err`
        // after rolling back the visible + hidden reservations this call made
        // (a commutative, concurrency-safe undo), leaving the topology word
        // untouched and `try_push_with` publishing nothing.
        let order_arc = Arc::new(order);
        self.orders.try_push_with(order_arc.clone(), || {
            if self
                .visible_quantity
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                    c.checked_add(visible_qty)
                })
                .is_err()
            {
                return Err(PriceLevelError::InvalidOperation {
                    message: "price level visible quantity overflow on admission".to_string(),
                });
            }

            if self
                .hidden_quantity
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                    c.checked_add(hidden_qty)
                })
                .is_err()
            {
                // Roll back the visible reservation this call made.
                self.visible_quantity
                    .fetch_sub(visible_qty, Ordering::Relaxed);
                return Err(PriceLevelError::InvalidOperation {
                    message: "price level hidden quantity overflow on admission".to_string(),
                });
            }

            // Pin the side and bump the count in one CAS. This is the
            // authoritative, race-free side decision: two opposite-side
            // admissions into an empty level serialize here, only one wins.
            match self.topology_admit(order_side) {
                Ok(established) => {
                    if established {
                        // Pinned a previously-empty level; bump the epoch BEFORE
                        // the publish that `try_push_with` does next, so a
                        // snapshot whose walk spans this transition sees the epoch
                        // move and retries.
                        self.bump_topology_epoch();
                    }
                }
                Err(err) => {
                    // Roll back the visible + hidden reservations this call made;
                    // the topology word was not mutated (pin goes last).
                    self.visible_quantity
                        .fetch_sub(visible_qty, Ordering::Relaxed);
                    self.hidden_quantity
                        .fetch_sub(hidden_qty, Ordering::Relaxed);
                    return Err(err);
                }
            }

            Ok(())
        })?;

        // Update statistics only after a committed admission.
        self.stats.record_order_added();

        // Signal the committed mutation so a racing post-only depth scan retries
        // (issue #130).
        self.bump_mutation_epoch();

        Ok(order_arc)
    }

    /// Creates a non-allocating iterator over current orders in this level.
    ///
    /// The iteration order is not guaranteed to be stable. Use [`Self::snapshot_orders`]
    /// when deterministic ordering is required.
    pub fn iter_orders(&self) -> impl Iterator<Item = Arc<OrderType<()>>> + '_ {
        self.orders.iter_orders()
    }

    /// Materializes a deterministic snapshot of orders sorted by timestamp.
    #[must_use]
    pub fn snapshot_orders(&self) -> Vec<Arc<OrderType<()>>> {
        self.orders.snapshot_vec()
    }

    /// Materializes the resting orders in the exact order [`Self::match_order`]
    /// consumes them: ascending **insertion sequence** (the oldest order first).
    ///
    /// Use this to predict the sweep — e.g. a self-trade-prevention pre-scan that
    /// must walk orders in consumption order to compute how much a taker may
    /// safely fill. It differs from the other two views:
    /// - [`Self::snapshot_orders`] sorts by `(timestamp, sequence)`, which equals
    ///   the sweep order *only* when timestamps are monotonic with insertion
    ///   (client-supplied or modify-restamped timestamps break that); and
    /// - [`Self::iter_orders`] has no stable order.
    ///
    /// Like `snapshot_orders`, this is a point-in-time view: a concurrent
    /// mutation after the call can change the queue.
    #[must_use]
    pub fn snapshot_by_insertion_seq(&self) -> Vec<Arc<OrderType<()>>> {
        self.orders.snapshot_by_seq()
    }

    /// Fill `out` with the resting orders in ascending **insertion sequence** —
    /// the buffer-reuse variant of [`Self::snapshot_by_insertion_seq`].
    ///
    /// `out` is cleared and then extended in place, yielding the exact same
    /// sequence [`Self::snapshot_by_insertion_seq`] returns — the order
    /// [`Self::match_order`] consumes resting orders. Reusing one scratch
    /// buffer across calls avoids the per-call allocation of the returned
    /// `Vec`, which matters for a downstream consumer that walks every level
    /// repeatedly (e.g. a self-trade-prevention pre-scan). Note that an
    /// internal `(sequence, order)` pairs buffer plus its sort is still paid
    /// per call, so the reuse saves only the output `Vec` allocation.
    ///
    /// Like `snapshot_by_insertion_seq`, this is a point-in-time view: a
    /// concurrent mutation after the call can change the queue.
    pub fn snapshot_by_seq_into(&self, out: &mut Vec<Arc<OrderType<()>>>) {
        self.orders.snapshot_by_seq_into(out);
    }

    /// Returns `true` if any resting order has matchable depth, i.e. a positive
    /// taker would cross at this level.
    ///
    /// Used by the post-only pre-check, which only needs to know whether *any*
    /// liquidity would be taken, not how much. Short-circuits on the first
    /// matchable order, so it is cheaper than `matchable_quantity`.
    ///
    /// Matchability is delegated to [`OrderType::is_matchable`] — the single
    /// source of truth shared with the fill-or-kill dry run — so the post-only
    /// verdict and the fill-or-kill prediction can never disagree about the same
    /// level. In particular a zero-visible iceberg (or auto-replenishing
    /// reserve) backed by hidden quantity counts as matchable depth, because the
    /// sweep will draw that hidden into visible and fill it.
    ///
    /// A resting maker sharing `taker_id` is ignored: the sweep skips it for
    /// self-trade prevention, so it is not liquidity this taker could take, and
    /// the post-only pre-check must agree.
    fn has_matchable_depth(&self, taker_id: Id) -> bool {
        // Linearize the non-atomic scan against concurrent mutation (issue #130):
        // read the mutation epoch, scan, re-read; retry if it moved. A stable
        // epoch across the scan means no add / cancel / resize committed during
        // it, so the verdict corresponds to a single queue state and has a
        // linearization point (at the scan). Unbounded retry, same liveness
        // argument as the statistics seqlock: mutators are finite and each commit
        // is a single `fetch_add`, so a scan converges as soon as mutation
        // quiesces (the post-only path is cold, so the retry cost is irrelevant).
        loop {
            let epoch_before = self.mutation_epoch.load(Ordering::Acquire);
            let verdict = self
                .iter_orders()
                .any(|order| order.id() != taker_id && order.is_matchable());
            std::sync::atomic::fence(Ordering::Acquire);
            let epoch_after = self.mutation_epoch.load(Ordering::Relaxed);
            if epoch_before == epoch_after {
                return verdict;
            }
        }
    }

    /// Computes how much of `incoming_quantity` this level could actually fill
    /// for a taker, in quantity units, **without mutating the queue**.
    ///
    /// This is a deterministic dry run of the FIFO sweep: it replays
    /// [`OrderType::match_against`] over a snapshot of the resting queue in the
    /// same price-time order the real sweep uses, including iceberg / reserve
    /// replenishment (a refreshed tranche is re-queued at the tail) and the
    /// removal of a non-replenishing reserve once its visible part is drained.
    /// It also models the sweep's **replenish-headroom abort** (issue
    /// #124/#130): it tracks the level's visible counter as the sweep would
    /// evolve it and stops at the exact maker where a replenish's net delta would
    /// overflow `u64`, because the real sweep sets that maker aside and
    /// terminates. The returned value is therefore exactly what
    /// [`Self::match_order`] would consume — never an over- or under-count —
    /// which is what fill-or-kill (all-or-nothing) correctly depends on: without
    /// modelling the abort, this could approve a fill-or-kill the sweep then
    /// aborts mid-fill, leaving a partial fill.
    ///
    /// `taker_id` must be the id of the taker this depth is being computed for:
    /// a resting maker sharing that id is skipped, exactly as the real sweep
    /// skips it for self-trade prevention, so the prediction and the sweep can
    /// never diverge.
    ///
    /// It allocates a working snapshot and is only used on the cold
    /// fill-or-kill path, not on the hot `Gtc` sweep.
    ///
    /// Public so an order book composing this level can reuse the single
    /// upstream source of truth for per-level fill-or-kill (all-or-nothing)
    /// feasibility instead of re-deriving the sweep, which would risk drifting
    /// from the real `match_order` behavior.
    #[must_use]
    pub fn matchable_quantity(&self, incoming_quantity: u64, taker_id: Id) -> u64 {
        if incoming_quantity == 0 {
            return 0;
        }

        // Snapshot the resting orders. `snapshot_orders()` is ordered by
        // `(timestamp, sequence)` whereas the real sweep pops by pure insertion
        // sequence; these coincide when timestamps are monotonic with insertion
        // (the normal case). The two can only differ in *visit order*, never in
        // the fillable *total*: every maker (including a fully-drained
        // replenishing iceberg/auto-reserve) contributes the same amount
        // regardless of when it is visited, so the sum this returns is exactly
        // what the sweep would consume — which is all fill-or-kill depends on.
        let mut pending: std::collections::VecDeque<Arc<OrderType<()>>> =
            self.snapshot_orders().into();
        let mut remaining = incoming_quantity;
        let mut filled: u64 = 0;

        // Track the level's visible counter as the sweep would evolve it, so the
        // dry run models the #124 replenish-headroom ABORT (issue #130). A
        // replenish step commits `visible -= consumed; visible += hidden_reduced`
        // with checked arithmetic under the entry lock, and if that net delta
        // would exceed `u64::MAX` the real sweep SETS THE MAKER ASIDE (no trade)
        // and TERMINATES the sweep. Starting from the live counter, this
        // simulation reproduces that stop point exactly. When the fill-or-kill
        // dry run holds the `fok_write` guard, mutators are frozen, so the live
        // value cannot drift and the projection is exact — a would-abort here
        // therefore correctly makes the fill-or-kill infeasible (killed) rather
        // than approving a taker the sweep would abort mid-fill (a partial fill).
        let mut projected_visible = self.visible_quantity();

        while remaining > 0 {
            let Some(order) = pending.pop_front() else {
                break;
            };
            // Self-trade prevention parity: the real sweep skips a maker sharing
            // the taker id (`SelfTradeSkipped`), so the dry run must skip it too,
            // or fill-or-kill would predict depth the sweep will not take.
            if order.id() == taker_id {
                continue;
            }
            let (consumed, updated_order, hidden_reduced, new_remaining) =
                order.match_against(remaining);

            // No-progress safety guard, identical in shape to the real sweep
            // (see `match_order`): a front maker that consumes nothing, draws no
            // hidden, and leaves `remaining` unchanged while handing itself back
            // is set aside (dropped from `pending`) rather than re-queued at the
            // front, which would spin forever. Dropping it here is the dry-run
            // analogue of the real sweep setting it aside: it contributes
            // nothing to `filled`, and the makers behind it are still visited.
            // Keeping this logic identical to the real sweep is what guarantees
            // `matchable_quantity` predicts exactly what `match_order` consumes,
            // which fill-or-kill depends on.
            if consumed == 0
                && hidden_reduced == 0
                && new_remaining == remaining
                && updated_order.is_some()
            {
                continue;
            }

            // Evolve the projected visible counter exactly as the sweep will, and
            // STOP on a would-abort so this maker (and everything behind it)
            // contributes nothing — the same terminal state the real sweep
            // reaches (issue #130).
            if hidden_reduced > 0 {
                // Replenish: checked net delta `- consumed + hidden_reduced`. A
                // failure is the #124 abort: the maker is set aside untouched and
                // the sweep ends, so do NOT count `consumed` and break.
                match projected_visible
                    .checked_sub(consumed)
                    .and_then(|v| v.checked_add(hidden_reduced))
                {
                    Some(next) => projected_visible = next,
                    None => break,
                }
            } else {
                // Pure consume: visible only decreases, so it cannot abort. Track
                // it (checked, never wraps: `consumed <= projected_visible`) so a
                // later replenish's headroom test is accurate; a defensive
                // underflow stops the dry run conservatively.
                let Some(next) = projected_visible.checked_sub(consumed) else {
                    break;
                };
                projected_visible = next;
            }

            // `consumed <= remaining <= incoming_quantity`, so this sum cannot
            // overflow `u64`; checked anyway per the no-saturate/no-wrap rule.
            filled = match filled.checked_add(consumed) {
                Some(total) => total,
                None => break,
            };
            remaining = new_remaining;

            if let Some(updated) = updated_order {
                if hidden_reduced > 0 {
                    // Replenished tranche loses time priority -> back of queue,
                    // exactly as the real sweep re-queues it.
                    pending.push_back(Arc::new(updated));
                } else {
                    // Pure partial fill keeps front position; the taker is now
                    // exhausted (`remaining == 0`) so the loop ends next check.
                    pending.push_front(Arc::new(updated));
                }
            }
        }

        filled
    }

    /// Matches an incoming taker order against existing orders at this price level.
    ///
    /// The sweep consumes resting makers in strict price-time (FIFO) order until
    /// the taker is filled or the matchable depth is exhausted. Trades are
    /// generated for each successful match, fully-consumed makers are removed,
    /// and the visible / hidden quantity counters and statistics are updated in
    /// lockstep with each execution.
    ///
    /// # Taker time-in-force / kind semantics
    ///
    /// Unlike earlier versions, this method **honors the taker's**
    /// [`TimeInForce`] and [`TakerKind`]. Let `available` be the quantity this
    /// level can actually fill for the taker (see `matchable_quantity`),
    /// capped at `incoming_quantity`:
    ///
    /// - [`TakerKind::PostOnly`]: must never take liquidity. If `available > 0`
    ///   the match is **rejected** — zero trades, the full `incoming_quantity`
    ///   reported as remaining, and the resting queue left untouched
    ///   ([`MatchResult::was_rejected`]).
    /// - [`TimeInForce::Fok`]: all-or-nothing. If `available < incoming_quantity`
    ///   the taker is **killed** — zero trades, full remaining, queue untouched
    ///   ([`MatchResult::was_killed`]). Otherwise it fills completely.
    /// - [`TimeInForce::Ioc`]: fills `available` and discards the remainder.
    ///   The taker is never enqueued here (this layer never rests a taker), so
    ///   the remainder is simply reported and dropped by the caller.
    /// - [`TimeInForce::Gtc`] / [`TimeInForce::Gtd`] / [`TimeInForce::Day`]:
    ///   fills `available`; the remainder is reported in
    ///   [`MatchResult::remaining_quantity`] for the order book to rest.
    /// - [`TakerKind::MarketToLimit`]: fills `available`; the remainder is
    ///   reported for the order book to convert into a resting limit. At this
    ///   single-level layer it fills like a standard taker.
    ///
    /// A post-only rejection and a fill-or-kill kill both leave zero trades and
    /// the full remainder; use [`MatchResult::outcome`] /
    /// [`MatchResult::was_rejected`] / [`MatchResult::was_killed`] to tell them
    /// apart from "the level had no liquidity".
    ///
    /// Time-in-force EXPIRY of resting **makers** is still NOT enforced here: a
    /// resting maker's `Gtd` / `Day` expiry is not consulted, so an expired
    /// maker still matches. Evicting or skipping expired makers is the
    /// caller's / order book's responsibility, keeping the match path a pure,
    /// deterministic sweep over the resting queue.
    ///
    /// # Arguments
    ///
    /// * `incoming_quantity`: The quantity of the incoming taker order to match.
    /// * `taker_order_id`: The ID of the incoming order (the "taker" order).
    /// * `taker_tif`: The taker's [`TimeInForce`], which governs how an unfilled
    ///   remainder is treated (kill / discard / rest).
    /// * `taker_kind`: The taker's [`TakerKind`] (standard / post-only /
    ///   market-to-limit).
    /// * `timestamp`: The taker timestamp (milliseconds since epoch) stamped
    ///   onto every emitted [`Trade`] and used as the execution time for
    ///   statistics. It is threaded in from the caller so the match path never
    ///   reads the wall clock — guaranteeing a deterministic, replayable trade
    ///   stream for a fixed input.
    /// * `trade_id_generator`: An atomic counter used to generate unique trade IDs.
    ///
    /// [`Trade`]: crate::execution::Trade
    /// [`TimeInForce`]: crate::orders::TimeInForce
    /// [`TakerKind`]: crate::execution::TakerKind
    /// [`MatchResult::was_rejected`]: crate::execution::MatchResult::was_rejected
    /// [`MatchResult::was_killed`]: crate::execution::MatchResult::was_killed
    /// [`MatchResult::outcome`]: crate::execution::MatchResult::outcome
    /// [`MatchResult::remaining_quantity`]: crate::execution::MatchResult::remaining_quantity
    ///
    /// # Returns
    ///
    /// A `MatchResult` carrying the generated trades, the remaining unmatched
    /// quantity, the completion flag, the fully-filled maker IDs, and the
    /// terminal [`MatchOutcome`](crate::execution::MatchOutcome).
    ///
    /// # Concurrency
    ///
    /// Resting orders are consumed in strict price-time (FIFO) order, and a
    /// partially-filled maker keeps its position at the front of the queue.
    ///
    /// **A concurrent `cancel` of the order currently being matched is safe and
    /// linearizable** (issue #81). The sweep keeps each maker resident in the
    /// queue and applies its decision (full consume / partial fill in place /
    /// replenish) while the maker's per-entry lock is held — the same lock a
    /// `cancel`'s removal takes (the internal `OrderQueue::match_front` step).
    /// The two therefore serialize: a cancel either fully wins (removes the
    /// maker and decrements the counters before the match observes it) or fully
    /// loses (the match commits first and the cancel then removes the residual
    /// it left). A cancel is never silently lost, and the counters never
    /// double-count. This closes the prior "lost cancel" window where a cancel
    /// landing between the matcher's pop and its reinsert would no-op while the
    /// matcher re-rested the residual.
    ///
    /// This method still assumes a **single logical matcher per level at a
    /// time**: two concurrent `match_order` calls on the *same* level are NOT
    /// made safe here and must be serialized by the caller (an order book
    /// typically matches a level from a single thread). Concurrent `add_order`
    /// from other threads is safe **for counter / queue integrity**. The
    /// single-side topology invariant carries an additional requirement beyond
    /// that integrity: it holds only when a given level's admissions arrive from
    /// one logical path (see the type-level note on [`PriceLevel`]), because the
    /// side is derived from the live queue rather than stored.
    ///
    /// ## PostOnly and fill-or-kill are atomic with the sweep (issue #112)
    ///
    /// Both the post-only and the fill-or-kill decisions are made
    /// all-or-nothing with respect to concurrent `add_order` / `update_order`,
    /// by two different mechanisms:
    ///
    /// * **PostOnly never enters the sweep.** A positive post-only taker either
    ///   crosses resting depth (rejected) or does not (rests) — in NEITHER case
    ///   is any resting order consumed, and in neither case does it sweep.
    ///   Because there is no sweep, no concurrently-added maker can be turned
    ///   into a trade: "post-only emits zero trades" is a *structural* property
    ///   that holds under every interleaving, needing no lock. The verdict is
    ///   linearized at the `has_matchable_depth` read — depth committed before
    ///   it is crossed (reject); depth committed after it is ordered behind this
    ///   taker (rest), the correct decision at the read instant. A non-crossing
    ///   post-only taker leaves the level completely untouched; the caller
    ///   re-admits the residual via `add_order`. The post-only decision is
    ///   evaluated *before* the [`TimeInForce`] branch, so a post-only taker
    ///   never takes liquidity even when its TIF is `Fok` or `Ioc` — the
    ///   never-cross kind dominates the fill-or-kill / immediate-or-cancel
    ///   semantics.
    /// * **Fill-or-kill holds an exclusive guard across its dry-run and sweep.**
    ///   A positive fill-or-kill taker takes the write side of the level's
    ///   fill-or-kill guard (the `fok_guard` note on [`PriceLevel`]) before its
    ///   feasibility dry-run and holds it through the sweep, while `add_order` /
    ///   `update_order` take the shared side. No mutator can then change the
    ///   matchable depth between the dry-run and the sweep, so with a stable
    ///   queue the sweep consumes exactly what the dry-run predicted: a
    ///   fill-or-kill taker either fills in full or is killed with the queue and
    ///   counters untouched — never a partial fill. The guard is acquired only
    ///   for fill-or-kill; the ordinary (`Gtc` / `Ioc` / `Day`) sweep is
    ///   unguarded and pays nothing.
    ///
    /// # Statistics
    ///
    /// Per-level execution statistics are recorded **all-or-nothing** and can
    /// never fail the match: the trade is already committed when
    /// `PriceLevelStatistics::record_execution` runs. If recording overflows a
    /// statistics counter, that execution's contribution is dropped atomically
    /// (it advances every aggregate or none), the drop is logged at `WARN` (a
    /// recoverable anomaly, not an aborted match), and the level's
    /// `PriceLevelStatistics::stats_degraded` flag is set (sticky,
    /// snapshot-persisted) so the under-count is observable. The emitted trades
    /// and the `MatchResult` are unaffected (issue #117).
    pub fn match_order(
        &self,
        incoming_quantity: u64,
        taker_order_id: Id,
        taker_tif: TimeInForce,
        taker_kind: TakerKind,
        timestamp: TimestampMs,
        trade_id_generator: &UuidGenerator,
    ) -> MatchResult {
        // -------- Fail-fast on a poisoned level (issue #130) --------
        //
        // If a guard holder panicked mid-operation the level may be half-mutated
        // and cannot be trusted to match. `match_order` returns [`MatchResult`],
        // not `Result`, so it cannot surface `InvalidOperation` the way
        // `add_order` / `update_order` do; instead it REFUSES to match — an empty
        // result (no trades, full remaining) — which is the safe outcome (the
        // taker takes no liquidity from a corrupt level). The one-time `ERROR`
        // log was already emitted when the poison was first recovered.
        if self.is_poisoned() {
            return MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
        }

        // -------- Self-match is terminal (issue #126, tightening #120) --------
        //
        // If the taker's own id already rests at this level, the taker cannot
        // take liquidity here: matching would either self-trade (forbidden) or,
        // via the in-sweep skip, walk PAST its own resting order to trade with
        // OTHER makers — but issue #120's acceptance is that a self-match attempt
        // emits NO trades and leaves the level byte-identical. So reject
        // terminally, before any sweep, for EVERY TIF and kind. This check
        // precedes and therefore dominates the post-only / fill-or-kill
        // pre-checks below (a self-match `Fok` is Rejected, not Killed); the
        // post-only behaviour for a taker that does NOT rest here is unchanged.
        // The lookup is an O(1) id probe. The in-sweep `SelfTradeSkipped` path is
        // retained as documented defense-in-depth for the narrow race where the
        // taker's resting order is admitted AFTER this probe but during the
        // sweep — even then it must never self-trade.
        if incoming_quantity > 0 && self.orders.find(taker_order_id).is_some() {
            tracing::debug!(
                taker_order_id = %taker_order_id,
                incoming_quantity,
                price = self.price,
                "taker rejected: own order id already rests at this level (self-match)"
            );
            let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
            result.mark_rejected(incoming_quantity);
            return result;
        }

        // -------- Taker TIF / kind pre-checks (before any queue mutation) --------
        //
        // PostOnly: must NEVER take liquidity (issue #112). A positive PostOnly
        // taker either crosses (reject) or does not (rest) — and in NEITHER case
        // does it enter the sweep. Not entering the sweep is what makes "PostOnly
        // emits zero trades" hold under *every* interleaving: no concurrent
        // `add_order` can turn a resting decision into a trade, because there is
        // no sweep to consume the newly-added depth. The verdict is linearized at
        // the `has_matchable_depth` check, which re-scans under the mutation epoch
        // so a concurrent add / cancel / resize racing the scan is DETECTED and
        // the scan retried (issue #130) — a stable epoch across the scan gives the
        // verdict a single-queue-state linearization point rather than a torn
        // read. Depth committed before the linearized scan is crossed (reject);
        // depth committed after it is ordered "later" (behind this taker), so
        // resting is correct at the scan instant. No guard is needed.
        if taker_kind.is_post_only() && incoming_quantity > 0 {
            let crossable = self.has_matchable_depth(taker_order_id);
            // Deterministic race seam (issue #130): fires BETWEEN the depth
            // decision and the commit below so a test can inject an `add_order`
            // in that exact window and confirm PostOnly still emits zero trades
            // (there is no sweep to consume the added depth). No-op in production.
            #[cfg(test)]
            fire_post_only_decision_hook();
            if crossable {
                tracing::debug!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    price = self.price,
                    "post-only taker rejected: would take liquidity"
                );
                let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                result.mark_rejected(incoming_quantity);
                return result;
            }
            // Not crossable: rest (report no trade) WITHOUT sweeping. The caller
            // re-admits the residual via `add_order`.
            return MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
        }

        // Fill-or-kill: all-or-nothing w.r.t. concurrent mutators (issue #112).
        // Take the fill-or-kill guard's EXCLUSIVE side and hold it across BOTH
        // the feasibility dry-run and the sweep below, so no `add_order` /
        // `update_order` can change the matchable depth between the two: with a
        // stable queue the sweep consumes exactly what the dry-run predicted, so
        // an FOK either fills in full or (insufficient depth) is killed with the
        // queue and counters untouched — never a partial fill. `_fok_guard` is
        // `Some` only for a positive FOK taker; it drops at the end of the
        // method (after the sweep). The non-FOK paths take no guard.
        let _fok_guard = if matches!(taker_tif, TimeInForce::Fok) && incoming_quantity > 0 {
            let guard = self.fok_write();
            // Acquiring the write guard may have just recovered a poison; refuse
            // to match a half-mutated level rather than sweep it (issue #130).
            if self.is_poisoned() {
                return MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
            }
            let available = self.matchable_quantity(incoming_quantity, taker_order_id);
            if available < incoming_quantity {
                tracing::debug!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    available,
                    price = self.price,
                    "fill-or-kill taker killed: insufficient depth"
                );
                let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                result.mark_killed(incoming_quantity);
                return result;
            }
            Some(guard)
        } else {
            None
        };

        // A single sweep emits at most one trade and at most one filled-order
        // id per resting order it actually consumes. Two independent upper
        // bounds hold: every emitted trade reduces `remaining` by at least one
        // unit (a `consumed == 0` maker is set aside without a trade), so the
        // sweep emits at most `incoming_quantity` trades; and it can touch at
        // most `order_count` resting orders. The tighter of the two pre-sizes
        // both vectors to cut per-fill reallocations on the hot path WITHOUT
        // reserving the whole level depth for a tiny taker (issue #106): a qty-1
        // taker against a deep level no longer reserves a multi-MB buffer it
        // immediately frees. The bound is advisory — `order_count` is read
        // `Relaxed` and both `Vec`s still grow if a concurrent `add_order` lands
        // mid-sweep — so it is a hint, not a cap.
        let capacity = (incoming_quantity as usize).min(self.order_count());
        let mut result =
            MatchResult::with_capacity(taker_order_id, Quantity::new(incoming_quantity), capacity);
        let mut remaining = incoming_quantity;

        // No-progress safety guard. A maker that yields no progress
        // (`consumed == 0`, re-queued unchanged, `remaining` not decreased)
        // must not be re-selected this sweep, or the loop would spin forever on
        // the same front order. Because such a dead order sits at the FRONT
        // (FIFO), simply breaking would starve any matchable makers behind it.
        // [`OrderQueue::match_front`] leaves a `SetAside` maker untouched in the
        // queue and parks its insertion sequence in `set_aside` so the sweep
        // advances to the maker behind it without re-selecting it. The maker is
        // never modified, never traded against, and its counters are never
        // touched, so the queue and the atomic counters stay exactly as if it
        // had been skipped — keeping counter <-> queue consistency intact and
        // preserving its price-time position for the next sweep.
        //
        // `match_against`'s own progress fix means this guard should never fire
        // for the iceberg/reserve states it now handles; it is defense-in-depth
        // against any future zero-progress shape (e.g. a degenerate residual
        // from `with_reduced_quantity(0)`).
        let mut set_aside: std::collections::HashSet<u64> = std::collections::HashSet::new();

        // Per-step bookkeeping carried out of the locked decision closure. The
        // trade / stats / counter work is done AFTER the closure returns so it
        // is not performed while the per-entry lock is held; correctness vs a
        // concurrent cancel rides on the `FrontAction` the queue committed under
        // the lock (see `OrderQueue::match_front`), not on when these counters
        // move (they are advisory — issue #68).
        struct StepData {
            consumed: u64,
            hidden_reduced: u64,
            fully_consumed: bool,
            maker_id: Id,
            maker_side: crate::orders::Side,
            maker_price: u128,
            maker_timestamp: u64,
            /// Hidden quantity stranded by a full consume with no replenishment
            /// (drained reserve / leftover iceberg hidden), to subtract from the
            /// hidden counter.
            hidden_stranded: u64,
            /// The taker's remaining quantity after this maker is matched.
            new_remaining: u64,
            /// `true` when this step's level-counter deltas were already applied
            /// INSIDE the locked decision closure (the replenish path, issue
            /// #128). The post-lock body then skips re-applying them so the
            /// counters move exactly once.
            counters_committed: bool,
        }

        // Either the maker progressed (carrying `StepData`), was parked
        // (`SetAside` for the no-progress guard, `SelfTradeSkipped` for a maker
        // whose id equals the taker's), or forced the sweep to abort because
        // committing its replenishment would overflow the level's visible
        // counter (`Abort`). The parked / abort variants thread the maker's id
        // (and, where relevant, its insertion seq) OUT of the locked decision
        // closure so the caller's `warn!` / `debug!` can name the maker without
        // logging inside the per-entry lock.
        enum StepResult {
            Progressed(StepData),
            SetAside {
                maker_id: Id,
                seq: u64,
            },
            SelfTradeSkipped {
                maker_id: Id,
                seq: u64,
            },
            /// The FIFO-front maker would replenish, but moving the drawn hidden
            /// tranche into the level's visible counter would take it past
            /// `u64::MAX` — a depth the level cannot represent. The maker is left
            /// byte-identical (the queue action is `SetAside`, a pure no-op that
            /// mutates nothing) and the sweep terminates immediately, so no
            /// younger maker trades past this front and no counter wraps.
            Abort {
                maker_id: Id,
            },
        }

        while remaining > 0 {
            let outcome = self.orders.match_front(&mut set_aside, |seq, order_arc| {
                // Self-trade prevention, DEFENSE-IN-DEPTH (issue #126). The
                // common case is already handled terminally before the sweep: if
                // the taker id rests here, `match_order` returns `Rejected` with
                // no trades. This in-sweep skip covers only the narrow race where
                // the taker's own order is admitted AFTER that pre-check but
                // before the sweep reaches its slot. Deterministic in every build
                // profile (not a debug-only assert): a resting maker must never
                // trade against a taker carrying the same id. Skip it — park its
                // sequence like a no-progress maker so the sweep advances to the
                // makers behind it — rather than emit a self-trade. The maker is
                // left untouched (no trade, counters and queue unchanged).
                //
                // Scope: this is ORDER-ID identity — an order can never match
                // *itself*. It is NOT account/owner-level self-trade prevention:
                // two distinct order ids owned by the same `user_id` will still
                // trade here. Account-level STP is the composing order book's
                // responsibility (it knows the owner relationships this level
                // does not).
                if order_arc.id() == taker_order_id {
                    return (
                        FrontAction::SetAside,
                        StepResult::SelfTradeSkipped {
                            maker_id: order_arc.id(),
                            seq,
                        },
                    );
                }

                let (consumed, updated_order, hidden_reduced, new_remaining) =
                    order_arc.match_against(remaining);

                // Detect a non-progressing maker: nothing consumed, no hidden
                // drawn, the taker's remaining unchanged, and the maker handed
                // back to us to re-queue. Park it and advance. Thread the maker
                // id + seq out so the caller can name it in the no-progress
                // `warn!` without logging under the per-entry lock.
                if consumed == 0
                    && hidden_reduced == 0
                    && new_remaining == remaining
                    && updated_order.is_some()
                {
                    return (
                        FrontAction::SetAside,
                        StepResult::SetAside {
                            maker_id: order_arc.id(),
                            seq,
                        },
                    );
                }

                let maker_id = order_arc.id();
                let maker_side = order_arc.side();
                let maker_price = order_arc.price().as_u128();
                let maker_timestamp = order_arc.timestamp().as_u64();

                // Hidden stranded by a full consume that does not replenish:
                // an iceberg / reserve whose visible was fully taken but whose
                // hidden is dropped (non-auto reserve, or a leftover the
                // `match_against` chose not to refresh). Identical condition to
                // the pre-#81 sweep's full-consume cleanup branch.
                let hidden_stranded = if updated_order.is_none() && hidden_reduced == 0 {
                    match order_arc {
                        OrderType::IcebergOrder {
                            hidden_quantity, ..
                        }
                        | OrderType::ReserveOrder {
                            hidden_quantity, ..
                        } if hidden_quantity.as_u64() > 0 => hidden_quantity.as_u64(),
                        _ => 0,
                    }
                } else {
                    0
                };

                let fully_consumed = updated_order.is_none();

                // Compute the action. For a replenishment, PUBLISH this step's
                // level-counter transition HERE — under the maker's entry lock,
                // before returning the action (issue #128) — so a concurrent
                // `UpdateQuantity` that next locks this same entry observes the
                // level counters already consistent with the replenished queue.
                // Applying it only after the entry released would leave the
                // counter transiently BELOW the queue's visible sum, and the
                // update's `old -> new` decrease could then underflow (`0 - 100`
                // wrap). `counters_committed` tells the post-lock body to skip
                // re-applying this step's deltas so the counters move exactly once.
                let mut counters_committed = false;
                let action = match updated_order {
                    None => FrontAction::Remove,
                    Some(updated) => {
                        if hidden_reduced > 0 {
                            // Replenishment: a fresh tranche moves hidden ->
                            // visible. Apply the visible NET delta
                            // (`- consumed + hidden_reduced`) as ONE checked RMW
                            // plus the hidden decrement, atomically visible before
                            // the entry lock releases. The checked `fetch_update`
                            // supersedes the old load-only `fits` pre-check: even
                            // when every resting order's own total fits `u64`, the
                            // level's visible SUM can exceed `u64::MAX` once hidden
                            // depth converts to visible, so a net delta that would
                            // overflow ABORTS the step (`SetAside` mutates nothing,
                            // emits no trade, ends the sweep) — no younger maker
                            // trades past this FIFO front and the counter never
                            // wraps. The stuck depth is unreachable until a cancel
                            // / downsize frees headroom.
                            let net_ok = self
                                .visible_quantity
                                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                                    c.checked_sub(consumed)
                                        .and_then(|v| v.checked_add(hidden_reduced))
                                })
                                .is_ok();
                            if !net_ok {
                                return (FrontAction::SetAside, StepResult::Abort { maker_id });
                            }
                            self.hidden_quantity
                                .fetch_sub(hidden_reduced, Ordering::Relaxed);
                            counters_committed = true;
                            // Refreshed tranche loses priority.
                            FrontAction::ReplaceAtTail(Arc::new(updated))
                        } else {
                            // Pure partial fill: keep priority in place.
                            FrontAction::KeepInPlace(Arc::new(updated))
                        }
                    }
                };

                let data = StepData {
                    consumed,
                    hidden_reduced,
                    fully_consumed,
                    maker_id,
                    maker_side,
                    maker_price,
                    maker_timestamp,
                    hidden_stranded,
                    new_remaining,
                    counters_committed,
                };

                (action, StepResult::Progressed(data))
            });

            match outcome {
                FrontOutcome::Empty => break,
                FrontOutcome::Matched { result: step } => {
                    let data = match step {
                        StepResult::SetAside { maker_id, seq } => {
                            // Parked by the queue; advance to the maker behind it.
                            // The id + seq were threaded out of the locked
                            // decision closure so we can name the parked maker
                            // here, outside the per-entry lock.
                            tracing::warn!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                seq,
                                "match sweep: front maker made no progress; set aside to avoid re-pop"
                            );
                            continue;
                        }
                        StepResult::Abort { maker_id } => {
                            // The FIFO-front maker's replenishment would overflow
                            // the level's visible counter. The queue committed a
                            // no-op (`SetAside`), so the maker rests unchanged and
                            // no counter moved. Terminate the sweep here WITHOUT
                            // decrementing `remaining`: no trade is emitted for
                            // this maker, and stopping (rather than advancing to a
                            // younger maker) preserves strict FIFO — liquidity
                            // behind this front stays unreachable until a cancel /
                            // downsize frees enough headroom to represent the
                            // replenished depth.
                            tracing::error!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                "match sweep aborted: replenishment would overflow the level visible counter; front maker left intact, sweep terminated"
                            );
                            break;
                        }
                        StepResult::SelfTradeSkipped { maker_id, seq } => {
                            // Self-trade prevention: the front maker shares the
                            // taker's id. Skip it (parked like a set-aside maker)
                            // and advance to the makers behind it; no trade is
                            // emitted and the maker is left untouched.
                            tracing::debug!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                seq,
                                "match sweep: front maker shares the taker id; skipped to prevent self-trade"
                            );
                            continue;
                        }
                        StepResult::Progressed(data) => data,
                    };
                    let new_remaining = data.new_remaining;

                    if data.consumed > 0 {
                        // Update visible quantity counter. `Relaxed`: advisory
                        // counter (issue #68); the queue mutation committed
                        // inside `match_front` carries the real happens-before,
                        // not this RMW. The delta is keyed off the committed
                        // action so it never double-counts with a concurrent
                        // cancel (which decrements only the residual it removes).
                        // Skipped for a replenish step: its visible net delta
                        // (already including `- consumed`) was applied under the
                        // entry lock in the decision closure (issue #128).
                        if !data.counters_committed {
                            self.visible_quantity
                                .fetch_sub(data.consumed, Ordering::Relaxed);
                        }

                        let trade_id = Id::from_uuid(trade_id_generator.next());

                        // A resting maker can never be the taker here: a maker
                        // sharing the taker id is skipped (`SelfTradeSkipped`)
                        // before it reaches this point, so no self-trade is ever
                        // emitted — deterministically, in every build profile.

                        let trade = Trade::with_timestamp(
                            trade_id,
                            taker_order_id,
                            data.maker_id,
                            Price::new(self.price),
                            Quantity::new(data.consumed),
                            data.maker_side.opposite(),
                            timestamp,
                        );

                        if result.add_trade(trade).is_err() {
                            remaining = new_remaining;
                            break;
                        }

                        if data.fully_consumed {
                            result.add_filled_order_id(data.maker_id);
                        }

                        // The trade is already committed (added to `result` and
                        // the queue mutated) — statistics recording cannot fail
                        // it retroactively. Recording is all-or-nothing (issue
                        // #117): on overflow the execution's contribution is
                        // dropped atomically and a sticky `stats_degraded` flag
                        // is set on the level's statistics (observable via
                        // `PriceLevelStatistics::stats_degraded`), leaving the
                        // trade stream unaffected. Log the drop rather than
                        // discard it silently.
                        // Read the degraded flag BEFORE recording: under the
                        // single-matcher-per-level model this is the faithful
                        // witness of the `false -> true` transition (issue #129).
                        // `record_execution` sets the flag atomically via
                        // `compare_exchange`; we log the WARN only on the FIRST
                        // drop that transitions it, so a burst of dropped
                        // executions logs once, not once per drop — the sticky
                        // flag remains the durable signal for the rest.
                        let was_degraded = self.stats.stats_degraded();
                        if let Err(err) = self.stats.record_execution(
                            data.consumed,
                            data.maker_price,
                            data.maker_timestamp,
                            timestamp.as_u64(),
                        ) && !was_degraded
                        {
                            // WARN, not ERROR: the match is not aborted — this is
                            // a recoverable observability anomaly flagged by the
                            // sticky degraded flag (the trade is committed).
                            tracing::warn!(
                                price = self.price,
                                taker_order_id = %taker_order_id,
                                maker_order_id = %data.maker_id,
                                consumed = data.consumed,
                                error = %err,
                                "execution statistics dropped (all-or-nothing); level stats marked degraded — trade unaffected"
                            );
                        }
                    }

                    remaining = new_remaining;

                    if data.fully_consumed {
                        // Maker fully consumed and removed inside `match_front`.
                        // Decrement the count and un-pin if this drained the level
                        // (issue #126); the removal already happened-before here.
                        if self.topology_release_one() {
                            self.bump_topology_epoch();
                        }
                        if data.hidden_stranded > 0 {
                            self.hidden_quantity
                                .fetch_sub(data.hidden_stranded, Ordering::Relaxed);
                        }
                    } else if data.hidden_reduced > 0 && !data.counters_committed {
                        // Replenishment: a fresh tranche moved from hidden into
                        // visible. The maker stayed resident (re-sequenced in
                        // place by `match_front`), so only the counters move.
                        // As of issue #128 the replenish path commits this
                        // transition under the entry lock (`counters_committed`),
                        // so this post-lock branch is now unreachable for it and
                        // kept only as a defensive no-op.
                        self.hidden_quantity
                            .fetch_sub(data.hidden_reduced, Ordering::Relaxed);
                        self.visible_quantity
                            .fetch_add(data.hidden_reduced, Ordering::Relaxed);
                    }
                    // Pure partial fill (KeepInPlace, hidden_reduced == 0):
                    // visible already decremented by `consumed` above; the maker
                    // stays resident with its residual. Nothing else to do.

                    if remaining == 0 {
                        break;
                    }
                }
            }
        }

        result.finalize(Quantity::new(remaining));

        result
    }

    /// Create a snapshot of the current price level state
    ///
    /// All aggregates are derived from a single materialized order vector so the
    /// snapshot is internally consistent under concurrent mutation: the counter
    /// fields can never disagree with a sum over the snapshot's own `orders`. We
    /// fold the vector instead of reading the live atomic counters separately,
    /// which would be a torn read (the atomics could advance between the counter
    /// load and the order materialization).
    ///
    /// The `orders` vector is materialized in **queue-consumption order**
    /// (ascending insertion sequence — the exact order [`Self::match_order`]
    /// consumes resting orders), not the `(timestamp, sequence)` display order
    /// of [`Self::snapshot_orders`]. Because [`Self::from_snapshot`] re-enqueues
    /// in vector order, a restore reproduces the live queue's priority exactly —
    /// including the "sizing an order up loses time priority" demotion, where an
    /// order that was moved to the back of the queue keeps its original
    /// admission timestamp. Using the timestamp view here would let such an
    /// order sort back to its old position and wrongly regain front priority on
    /// restore.
    #[must_use]
    pub fn snapshot(&self) -> PriceLevelSnapshot {
        // Hold the fill-or-kill guard's SHARED side across the materialization
        // (issue #130) so a snapshot can never capture a multi-maker fill-or-kill
        // mid-transaction: the FOK holds the EXCLUSIVE side across its dry-run and
        // sweep, so this read waits for it to fully commit or is excluded before
        // it starts — the snapshot sees the pre- or post-FOK state, never a
        // partial sweep. Ordinary mutators (`add_order` / `update_order`) also
        // take the shared side, so they run concurrently with this read (read vs
        // read) and are handled by the topology-epoch retry below for side
        // transitions. `snapshot` intentionally does NOT poison-check: it stays
        // available on a poisoned level for diagnostics / reconstruction.
        let _fok = self.fok_read();

        // Materialize the orders exactly once, in queue-consumption (insertion
        // sequence) order so a snapshot round-trip re-enqueues them in identical
        // priority order; every aggregate is derived from this same snapshot so
        // they are mutually consistent by construction.
        //
        // Guard against a TORN topology (issue #126): a walk that spans a
        // drain-then-re-admit to the opposite side could capture old-side and
        // new-side orders together, producing a checksummed snapshot that
        // `from_snapshot` would reject for mixed sides. `topology_epoch` is
        // bumped on every side pin / un-pin, so if it moves across the walk we
        // retry. The `is_single_side` fallback GUARANTEES termination: a stable
        // epoch already implies a coherent walk (a tear requires a transition,
        // which bumps the epoch), so we only ever loop while a walk actually came
        // back mixed-side, which needs an in-progress opposite-side flip — once
        // flipping stops (finite writers) the next walk is coherent and returns.
        let orders = loop {
            let epoch_before = self.topology_epoch.load(Ordering::Acquire);
            let orders = self.snapshot_by_insertion_seq();
            let epoch_after = self.topology_epoch.load(Ordering::Acquire);
            if epoch_before == epoch_after || Self::is_single_side(&orders) {
                break orders;
            }
            // A side transition raced the walk AND left a mixed-side view; retry.
        };

        let order_count = orders.len();

        let mut visible_quantity: u64 = 0;
        let mut hidden_quantity: u64 = 0;

        for order in &orders {
            // Checked arithmetic per the crate's no-saturate/no-wrap rule.
            // `snapshot()` is infallible (changing it would ripple to
            // `snapshot_package` / `snapshot_to_json` and every caller), so the
            // overflow branch needs a value, not a `Result`. That branch is
            // unreachable for any state the level can represent: the level tracks
            // the same running total in a `u64` atomic counter, so a sum that
            // overflows `u64` here is one the level itself could never have held.
            // On that impossible branch we fall back to the live atomic counter —
            // the engine's own authoritative `u64` total (best-effort, since the
            // branch cannot occur for representable state).
            match visible_quantity.checked_add(order.visible_quantity().as_u64()) {
                Some(total) => visible_quantity = total,
                None => {
                    debug_assert!(false, "snapshot visible quantity overflow is unreachable");
                    visible_quantity = self.visible_quantity();
                }
            }

            match hidden_quantity.checked_add(order.hidden_quantity().as_u64()) {
                Some(total) => hidden_quantity = total,
                None => {
                    debug_assert!(false, "snapshot hidden quantity overflow is unreachable");
                    hidden_quantity = self.hidden_quantity();
                }
            }
        }

        // Persist the per-level statistics alongside the aggregates so the
        // snapshot round-trip reproduces the recorded execution history. The
        // clone snapshots the eight atomic counters (best-effort, like every
        // other read path); statistics are independent counters, not part of
        // the queue / counter consistency invariant.
        PriceLevelSnapshot::from_raw_parts_with_stats(
            Price::new(self.price),
            Quantity::new(visible_quantity),
            Quantity::new(hidden_quantity),
            order_count,
            orders,
            (*self.stats).clone(),
        )
    }

    /// Serialize the current price level state into a checksum-protected snapshot package.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if computing the snapshot's
    /// aggregate quantities overflows while building the package's checksummed
    /// payload, or [`PriceLevelError::SerializationError`] if encoding the
    /// snapshot payload to compute its SHA-256 checksum fails.
    pub fn snapshot_package(&self) -> Result<PriceLevelSnapshotPackage, PriceLevelError> {
        PriceLevelSnapshotPackage::new(self.snapshot())
    }

    /// Serialize the current price level state to JSON, including checksum metadata.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if building the snapshot
    /// package overflows an aggregate quantity, or
    /// [`PriceLevelError::SerializationError`] if the package cannot be encoded
    /// to JSON.
    pub fn snapshot_to_json(&self) -> Result<String, PriceLevelError> {
        self.snapshot_package()?.to_json()
    }
}

impl PriceLevel {
    /// Apply an update to an existing order at this price level.
    ///
    /// # Quantity-update priority policy
    ///
    /// For [`OrderUpdate::UpdateQuantity`] (and the same-price branch of
    /// [`OrderUpdate::UpdatePriceAndQuantity`]) this method follows the
    /// conventional exchange price-time-priority rules:
    ///
    /// - **Decrease or unchanged total quantity** keeps the maker's queue
    ///   position. The stored order is updated *in place* at its existing
    ///   insertion sequence, so it is consumed at the same point in FIFO order
    ///   as before. Reducing size never forfeits time priority.
    /// - **Increase in total quantity** demotes the order to the *back* of the
    ///   queue (it is assigned a fresh insertion sequence). Sizing an order up
    ///   loses time priority, matching standard exchange behaviour.
    ///
    /// Total quantity is `visible + hidden`; the branch is chosen by comparing
    /// the order's total before and after the update. Every order variant is
    /// resized by [`OrderType::with_reduced_quantity`] (single-quantity
    /// variants rewrite their `quantity`; two-tranche variants rewrite the
    /// visible tranche and keep hidden), so the branch reflects the real size
    /// change rather than a silent no-op.
    ///
    /// # Applied to the live maker (issue #115)
    ///
    /// The resize, the priority decision, and the level-counter update are all
    /// derived from the order **currently resident in the queue**, computed and
    /// committed together under a single per-entry lock — never from a pre-read.
    /// A concurrent match or replenishment that commits first is therefore fully
    /// reflected: an update applies `new_quantity` to the *live* visible tranche
    /// and preserves the *live* hidden depth, so it can never resurrect executed
    /// or cancelled quantity, and the increase-vs-decrease policy is chosen from
    /// the live total (never stale). The level counters are validated with
    /// checked math before the queue mutates, so an update that would overflow a
    /// level counter is rejected with the level left unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if an
    /// [`OrderUpdate::UpdatePrice`] / [`OrderUpdate::Replace`] would not move
    /// the order to a different price level, if computing an order's total
    /// quantity overflows `u64`, or if an [`OrderUpdate::UpdateQuantity`] would
    /// overflow the level's visible- or hidden-quantity counter (the maker and
    /// its queue position are left unchanged in that case).
    #[must_use = "the updated order (or None when the order is absent) must be handled"]
    pub fn update_order(
        &self,
        update: OrderUpdate,
    ) -> Result<Option<Arc<OrderType<()>>>, PriceLevelError> {
        // Hold the fill-or-kill guard's shared side for the whole update so a
        // concurrent fill-or-kill match cannot observe the depth shrink (cancel
        // / down-size) or grow mid-decision (issue #112). Uncontended in the
        // common case (no FOK running). Acquired exactly ONCE here: the
        // same-price branches of `UpdatePriceAndQuantity` / `Replace` delegate
        // to the guard-free `update_order_inner` rather than recursing into this
        // method, because std `RwLock` reads are NOT reentrant. A recursive
        // `read()` with a writer queued between the two acquisitions (the lock
        // is writer-preferring, so the queued writer blocks the second reader)
        // would deadlock against a `fok_write` waiting on the first reader.
        let _fok = self.fok_read();
        // Fail fast on a poisoned level (issue #130).
        self.poison_check()?;
        let result = self.update_order_inner(update);
        // A committed mutation (`Ok(Some(_))` — the order was found and
        // cancelled / resized / moved) bumps the mutation epoch so a racing
        // post-only depth scan retries (issue #130). `Ok(None)` (not found) and
        // `Err` change nothing, so they do not bump.
        if matches!(result, Ok(Some(_))) {
            self.bump_mutation_epoch();
        }
        result
    }

    /// Guard-free body of [`Self::update_order`].
    ///
    /// The caller MUST already hold the fill-or-kill shared guard
    /// ([`Self::fok_read`]); this method never acquires it. That is what lets
    /// the same-price `UpdatePriceAndQuantity` / `Replace` branches re-enter it
    /// without taking a second, non-reentrant [`std::sync::RwLock`] read
    /// (issue #112).
    fn update_order_inner(
        &self,
        update: OrderUpdate,
    ) -> Result<Option<Arc<OrderType<()>>>, PriceLevelError> {
        match update {
            OrderUpdate::UpdatePrice {
                order_id,
                new_price,
            } => {
                // If price changes, this order needs to be moved to a different price level
                // So we remove it from this level and return it for re-insertion elsewhere
                if new_price != Price::new(self.price) {
                    let order = self.orders.remove(order_id);

                    if let Some(ref order_arc) = order {
                        // Update atomic counters from the order actually removed
                        // from the queue above. `Relaxed` on all three: advisory
                        // counters (issue #68); the `OrderQueue::remove` carries
                        // the happens-before, not these counters.
                        let visible_qty = order_arc.visible_quantity().as_u64();
                        let hidden_qty = order_arc.hidden_quantity().as_u64();

                        self.visible_quantity
                            .fetch_sub(visible_qty, Ordering::Relaxed);
                        self.hidden_quantity
                            .fetch_sub(hidden_qty, Ordering::Relaxed);
                        // Decrement the count and un-pin if this drained the
                        // level (issue #126); the `remove` above happened-before.
                        if self.topology_release_one() {
                            self.bump_topology_epoch();
                        }

                        // Update statistics
                        self.stats.record_order_removed();
                    }

                    Ok(order)
                } else {
                    // If price is the same, this is a no-op at the price level
                    // (Should be handled at the order book level)
                    Err(PriceLevelError::InvalidOperation {
                        message: "Cannot update price to the same value".to_string(),
                    })
                }
            }

            OrderUpdate::UpdateQuantity {
                order_id,
                new_quantity,
            } => {
                // Overflow-checked forward reservation of a level counter for a
                // component moving `old -> new`. BOTH directions use a checked
                // `fetch_update` and reject before any queue mutation: an
                // increase must not overflow `u64`, and a decrease must not
                // underflow it (issue #128 defense). `Relaxed`: advisory counters
                // (issue #68).
                //
                // The underflow guard is a belt-and-suspenders backstop. It is
                // unreachable in practice because issue #128's structural fix
                // publishes the match sweep's replenish counter transition UNDER
                // the maker's entry lock: by the time this `UpdateQuantity` holds
                // that same entry lock and reads `old` from the live maker, the
                // level counter already includes this maker's full `old`
                // contribution, so `counter >= old >= old - new` and the subtract
                // cannot go negative. The check simply refuses to wrap if that
                // invariant were ever violated, leaving the level untouched.
                fn reserve(
                    counter: &std::sync::atomic::AtomicU64,
                    old: u64,
                    new: u64,
                ) -> Result<(), PriceLevelError> {
                    let result = if new >= old {
                        let delta = new - old;
                        counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                            c.checked_add(delta)
                        })
                    } else {
                        let delta = old - new;
                        counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                            c.checked_sub(delta)
                        })
                    };
                    result
                        .map(|_| ())
                        .map_err(|_| PriceLevelError::InvalidOperation {
                            message: "price level quantity counter overflow on update".to_string(),
                        })
                }
                // Undo this call's own `old -> new` reservation (commutative with
                // concurrent deltas — it reverses exactly what it added).
                fn unreserve(counter: &std::sync::atomic::AtomicU64, old: u64, new: u64) {
                    if new >= old {
                        counter.fetch_sub(new - old, Ordering::Relaxed);
                    } else {
                        counter.fetch_add(old - new, Ordering::Relaxed);
                    }
                }

                let visible_counter = &self.visible_quantity;
                let hidden_counter = &self.hidden_quantity;

                // Derive the resized order, choose the priority policy, and
                // reserve the level counters ALL against the LIVE stored order,
                // under the entry lock (issue #115). Nothing is read before the
                // lock, so a concurrent match / replenish that committed first is
                // fully reflected: the update can never resurrect executed or
                // cancelled visible / hidden quantity, and the policy is chosen
                // from the live total, not a stale pre-read. The counters are
                // reserved with checked math BEFORE the queue commits, so an
                // update that would overflow a level counter is rejected with the
                // level (and queue) untouched.
                let outcome = self.orders.update_entry(order_id, |live| {
                    let old_visible = live.visible_quantity().as_u64();
                    let old_hidden = live.hidden_quantity().as_u64();
                    let live_total = old_visible.checked_add(old_hidden).ok_or_else(|| {
                        PriceLevelError::InvalidOperation {
                            message: "order total quantity overflow".to_string(),
                        }
                    })?;

                    // `with_reduced_quantity` sets the visible/main tranche to
                    // exactly `new_quantity` for every variant and preserves the
                    // LIVE hidden depth (never restored from a pre-read).
                    let new_order = live.with_reduced_quantity(new_quantity.as_u64());
                    let new_visible = new_order.visible_quantity().as_u64();
                    let new_hidden = new_order.hidden_quantity().as_u64();
                    let new_total = new_visible.checked_add(new_hidden).ok_or_else(|| {
                        PriceLevelError::InvalidOperation {
                            message: "order total quantity overflow".to_string(),
                        }
                    })?;

                    // Validate + reserve the level counters before mutating the
                    // queue. On a hidden overflow, roll the visible reservation
                    // back so a rejected update leaves the counters unchanged.
                    reserve(visible_counter, old_visible, new_visible)?;
                    if let Err(err) = reserve(hidden_counter, old_hidden, new_hidden) {
                        unreserve(visible_counter, old_visible, new_visible);
                        return Err(err);
                    }

                    // Priority policy from the LIVE total (cannot be stale).
                    let arc = Arc::new(new_order);
                    if new_total > live_total {
                        Ok(UpdateDecision::ReplaceAtTail(arc))
                    } else {
                        Ok(UpdateDecision::KeepInPlace(arc))
                    }
                });

                match outcome {
                    None => Ok(None), // Order not found / concurrently removed.
                    Some(result) => result.map(Some),
                }
            }

            OrderUpdate::UpdatePriceAndQuantity {
                order_id,
                new_price,
                new_quantity,
            } => {
                // If price changes, remove the order and let the order book handle re-insertion
                if new_price != Price::new(self.price) {
                    let order = self.orders.remove(order_id);

                    if let Some(ref order_arc) = order {
                        // Update atomic counters from the order actually removed
                        // from the queue above. `Relaxed` on all three: advisory
                        // counters (issue #68); the `OrderQueue::remove` carries
                        // the happens-before, not these counters.
                        let visible_qty = order_arc.visible_quantity().as_u64();
                        let hidden_qty = order_arc.hidden_quantity().as_u64();

                        self.visible_quantity
                            .fetch_sub(visible_qty, Ordering::Relaxed);
                        self.hidden_quantity
                            .fetch_sub(hidden_qty, Ordering::Relaxed);
                        // Decrement the count and un-pin if this drained the
                        // level (issue #126); the `remove` above happened-before.
                        if self.topology_release_one() {
                            self.bump_topology_epoch();
                        }

                        // Update statistics
                        self.stats.record_order_removed();
                    }
                    Ok(order)
                } else {
                    // If price is the same, just update the quantity (reuse
                    // logic). Call the guard-free inner body — we already hold
                    // the fill-or-kill shared guard, and a second `fok_read`
                    // here would be a non-reentrant recursive read (issue #112).
                    self.update_order_inner(OrderUpdate::UpdateQuantity {
                        order_id,
                        new_quantity,
                    })
                }
            }

            OrderUpdate::Cancel { order_id } => {
                // Remove the order
                let order = self.orders.remove(order_id);

                if let Some(ref order_arc) = order {
                    // Update atomic counters from the order actually removed from
                    // the queue above. `Relaxed` on all three: advisory counters
                    // (issue #68); the `OrderQueue::remove` carries the
                    // happens-before, not these counters.
                    let visible_qty = order_arc.visible_quantity().as_u64();
                    let hidden_qty = order_arc.hidden_quantity().as_u64();

                    self.visible_quantity
                        .fetch_sub(visible_qty, Ordering::Relaxed);
                    self.hidden_quantity
                        .fetch_sub(hidden_qty, Ordering::Relaxed);
                    // Decrement the count and un-pin if this drained the level
                    // (issue #126); the `remove` above happened-before.
                    if self.topology_release_one() {
                        self.bump_topology_epoch();
                    }

                    // Update statistics
                    self.stats.record_order_removed();
                }

                Ok(order)
            }

            OrderUpdate::Replace {
                order_id,
                price,
                quantity,
                side: _,
            } => {
                // For replacement, check if the price is changing
                if price != Price::new(self.price) {
                    // If price is different, remove the order and let order book handle re-insertion
                    let order = self.orders.remove(order_id);

                    if let Some(ref order_arc) = order {
                        // Update atomic counters from the order actually removed
                        // from the queue above. `Relaxed` on all three: advisory
                        // counters (issue #68); the `OrderQueue::remove` carries
                        // the happens-before, not these counters.
                        let visible_qty = order_arc.visible_quantity().as_u64();
                        let hidden_qty = order_arc.hidden_quantity().as_u64();

                        self.visible_quantity
                            .fetch_sub(visible_qty, Ordering::Relaxed);
                        self.hidden_quantity
                            .fetch_sub(hidden_qty, Ordering::Relaxed);
                        // Decrement the count and un-pin if this drained the
                        // level (issue #126); the `remove` above happened-before.
                        if self.topology_release_one() {
                            self.bump_topology_epoch();
                        }

                        // Update statistics
                        self.stats.record_order_removed();
                    }

                    Ok(order)
                } else {
                    // If price is the same, just update the quantity. Call the
                    // guard-free inner body — we already hold the fill-or-kill
                    // shared guard, and a second `fok_read` here would be a
                    // non-reentrant recursive read (issue #112).
                    self.update_order_inner(OrderUpdate::UpdateQuantity {
                        order_id,
                        new_quantity: quantity,
                    })
                }
            }
        }
    }
}

/// Serializable representation of a price level for easier data transfer and storage.
///
/// The `orders` vector is materialized in **queue-consumption order**
/// (ascending insertion sequence — exactly as `match_order` sweeps), and
/// [`TryFrom<PriceLevelData>`](PriceLevel#impl-TryFrom<PriceLevelData>-for-PriceLevel)
/// re-admits in vector order, so a `PriceLevelData` round-trip preserves
/// price-time (FIFO) priority just like the checksum-protected snapshot
/// package. Unlike the package, this plain representation carries no checksum
/// and no statistics — prefer [`PriceLevel::snapshot_package`] for
/// persistence.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceLevelData {
    /// The price of this level
    pub price: u128,
    /// Total visible quantity at this price level
    pub visible_quantity: u64,
    /// Total hidden quantity at this price level
    pub hidden_quantity: u64,
    /// Number of orders at this price level
    pub order_count: usize,
    /// Orders at this price level
    pub orders: Vec<OrderType<()>>,
}

impl From<&PriceLevel> for PriceLevelData {
    fn from(price_level: &PriceLevel) -> Self {
        Self {
            price: price_level.price(),
            visible_quantity: price_level.visible_quantity(),
            hidden_quantity: price_level.hidden_quantity(),
            order_count: price_level.order_count(),
            // Consumption (insertion-sequence) order, NOT the unordered DashMap
            // iteration: `TryFrom<PriceLevelData>` re-admits in vector order,
            // so this is what makes the round-trip preserve price-time / FIFO
            // priority (issue #131) — the same contract the snapshot package
            // has kept since issue #109.
            orders: price_level
                .snapshot_by_insertion_seq()
                .into_iter()
                .map(|order_arc| *order_arc)
                .collect(),
        }
    }
}

impl TryFrom<&PriceLevelSnapshot> for PriceLevel {
    type Error = PriceLevelError;

    /// Rebuilds a price level from a borrowed snapshot.
    ///
    /// Fallible on purpose (this replaced an infallible `From` in v0.9): the old
    /// `From` swallowed [`PriceLevelSnapshot::refresh_aggregates`] errors and
    /// built the queue with keep-first duplicate handling, so a snapshot whose
    /// orders repeated an id would restore counters computed over every copy
    /// while the queue kept only one — a level whose counters silently disagreed
    /// with its contents. This clones the snapshot and delegates to
    /// [`PriceLevel::from_snapshot`], which rejects a repeated id and propagates
    /// the aggregate-overflow / per-order-total errors instead of hiding them.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DuplicateOrderId`] if the snapshot's orders
    /// vector repeats an id, or [`PriceLevelError::InvalidOperation`] if a
    /// per-order or level aggregate overflows `u64` — see
    /// [`PriceLevel::from_snapshot`].
    fn try_from(value: &PriceLevelSnapshot) -> Result<Self, Self::Error> {
        PriceLevel::from_snapshot(value.clone())
    }
}

impl TryFrom<PriceLevelData> for PriceLevel {
    type Error = PriceLevelError;

    fn try_from(data: PriceLevelData) -> Result<Self, Self::Error> {
        let price_level = PriceLevel::new(data.price);

        // Add orders to the price level. Propagate an admission overflow rather
        // than panicking while reconstructing from external data.
        for order in data.orders {
            price_level.add_order(order)?;
        }

        Ok(price_level)
    }
}

// Implement custom serialization for the atomic types
impl Serialize for PriceLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Convert to a serializable representation
        let data: PriceLevelData = self.into();
        data.serialize(serializer)
    }
}

impl FromStr for PriceLevel {
    type Err = PriceLevelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use std::borrow::Cow;

        if !s.starts_with("PriceLevel:") {
            return Err(PriceLevelError::ParseError {
                message: "Invalid format: missing 'PriceLevel:' prefix".to_string(),
            });
        }

        let content = &s["PriceLevel:".len()..];

        let mut parts = std::collections::HashMap::new();
        let remaining_content: Cow<str>;

        if let Some(orders_start) = content.find("orders=[") {
            let orders_end =
                content[orders_start..]
                    .find(']')
                    .ok_or_else(|| PriceLevelError::ParseError {
                        message: "Invalid format: unclosed orders bracket".to_string(),
                    })?
                    + orders_start;

            let orders_str = &content[orders_start + "orders=[".len()..orders_end];
            parts.insert("orders", orders_str);

            let before_orders = &content[..orders_start];
            let after_orders = &content[orders_end + 1..];
            remaining_content = Cow::Owned([before_orders, after_orders].join(""));
        } else {
            remaining_content = Cow::Borrowed(content);
        }

        for part in remaining_content.split(';').filter(|s| !s.is_empty()) {
            let mut iter = part.splitn(2, '=');
            if let (Some(key), Some(value)) = (iter.next(), iter.next()) {
                parts.insert(key, value);
            }
        }

        let price = parts
            .get("price")
            .and_then(|v| v.parse::<u128>().ok())
            .ok_or_else(|| PriceLevelError::ParseError {
                message: "Missing or invalid price".to_string(),
            })?;

        let price_level = PriceLevel::new(price);

        if let Some(orders_part) = parts.get("orders")
            && !orders_part.is_empty()
        {
            let mut bracket_level = 0;
            let mut last_split = 0;

            for (i, c) in orders_part.char_indices() {
                match c {
                    '(' | '[' => bracket_level += 1,
                    ')' | ']' => bracket_level -= 1,
                    ',' if bracket_level == 0 => {
                        let order_str = &orders_part[last_split..i];
                        let order = OrderType::<()>::from_str(order_str).map_err(|e| {
                            PriceLevelError::ParseError {
                                message: format!("Order parse error: {e}"),
                            }
                        })?;
                        price_level.add_order(order)?;
                        last_split = i + 1;
                    }
                    _ => {}
                }
            }

            let order_str = &orders_part[last_split..];
            if !order_str.is_empty() {
                let order = OrderType::<()>::from_str(order_str).map_err(|e| {
                    PriceLevelError::ParseError {
                        message: format!("Order parse error: {e}"),
                    }
                })?;
                price_level.add_order(order)?;
            }
        }

        Ok(price_level)
    }
}

impl<'de> Deserialize<'de> for PriceLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize into the data representation
        let data = PriceLevelData::deserialize(deserializer)?;

        // Convert to PriceLevel
        PriceLevel::try_from(data).map_err(serde::de::Error::custom)
    }
}

impl PartialEq for PriceLevel {
    fn eq(&self, other: &Self) -> bool {
        self.price == other.price
    }
}

impl Eq for PriceLevel {}

impl PartialOrd for PriceLevel {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriceLevel {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.price.cmp(&other.price)
    }
}

impl Display for PriceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PriceLevel:price={};visible_quantity={};hidden_quantity={};order_count={};orders=[",
            self.price(),
            self.visible_quantity(),
            self.hidden_quantity(),
            self.order_count()
        )?;

        let mut first = true;
        for order in self.snapshot_orders() {
            if !first {
                write!(f, ",")?;
            }
            write!(f, "{order}")?;
            first = false;
        }

        write!(f, "]")
    }
}
