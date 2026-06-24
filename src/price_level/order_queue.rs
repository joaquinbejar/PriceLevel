use crate::errors::PriceLevelError;
use crate::orders::{Id, OrderType};
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

/// A thread-safe queue of orders with specialized operations.
///
/// Time priority (price-time / FIFO within the level) is maintained by an
/// ordered index keyed by a monotonic insertion sequence rather than a plain
/// tail-only FIFO. This lets a partially-filled maker keep its place at the
/// front of the queue: the residual is re-inserted at its *original* sequence,
/// instead of being appended to the tail.
#[derive(Debug)]
pub struct OrderQueue {
    /// A map of order IDs to `(insertion sequence, order)` for O(1) lookups.
    /// The sequence travels with the value so it can be recovered on pop and
    /// reused when re-inserting a partial-fill residual.
    orders: DashMap<Id, (u64, Arc<OrderType<()>>)>,
    /// Ordered index `sequence -> Id`. The lowest sequence is the front
    /// (oldest) order, so iteration / pop honours strict time priority.
    index: SkipMap<u64, Id>,
    /// Monotonic source of insertion sequences.
    next_seq: AtomicU64,
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
    /// priority, so remove the old entry and re-queue the new order at the tail
    /// with a fresh insertion sequence.
    ReplaceAtTail(Arc<OrderType<()>>),
    /// The maker made no progress this sweep (a degenerate zero-progress shape).
    /// Leave it untouched in `orders`/`index`; the caller sets its sequence
    /// aside so the sweep advances to the maker behind it without re-popping it.
    SetAside,
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

    /// Add an order to the tail of the queue (newest time priority).
    pub fn push(&self, order: Arc<OrderType<()>>) {
        // `Relaxed` is sufficient: only the uniqueness and monotonicity of the
        // counter matter. The happens-before ordering between concurrent
        // producers/consumers is provided by the lock-free `index`/`orders`
        // structures, not by this counter, so no synchronization rides on it.
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let order_id = order.id();
        self.orders.insert(order_id, (seq, order));
        self.index.insert(seq, order_id);
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
            let order_id = *entry.value();
            // The id may have been concurrently cancelled via `remove`; in that
            // case the map no longer holds it, so skip it and try the next one.
            if let Some((_, (seq, order))) = self.orders.remove(&order_id) {
                return Some((seq, order));
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
    /// has parked (the no-progress guard); they are skipped when choosing the
    /// front so the sweep does not re-pick them. A `SetAside` action inserts the
    /// chosen seq into it. It is a `HashSet` so membership during the front scan
    /// is O(1) rather than the O(n) `Vec::contains` it replaced; it is only ever
    /// inserted into and probed, never iterated for ordering.
    ///
    /// `decide` is the pure match decision (e.g. [`OrderType::match_against`]
    /// plus trade bookkeeping). It runs while the per-entry lock is held, so it
    /// MUST NOT call back into this queue (that would deadlock on the same
    /// shard) and MUST NOT block. It receives the maker's insertion `seq` and an
    /// immutable borrow of the resident order; the borrow ends before any commit
    /// mutates the entry, so the decision must return OWNED action data and no
    /// reference may escape it.
    ///
    /// All three mutating actions keep the maker's value **resident in `orders`
    /// until it is genuinely gone** (a full consume) — even the
    /// [`FrontAction::ReplaceAtTail`] re-prioritisation swaps the value and
    /// re-sequences it in place rather than removing-then-re-pushing — so the
    /// lost-cancel window is closed for every action, not just the partial fill.
    pub(crate) fn match_front<F, R>(
        &self,
        set_aside: &mut HashSet<u64>,
        decide: F,
    ) -> FrontOutcome<R>
    where
        F: FnOnce(u64, &OrderType<()>) -> (FrontAction, R),
    {
        loop {
            // Find the lowest-sequence index entry not already set aside this
            // sweep. `index.iter()` yields entries in ascending sequence order
            // (front = oldest = highest time priority).
            let Some((seq, order_id)) = self
                .index
                .iter()
                .find(|e| !set_aside.contains(e.key()))
                .map(|e| (*e.key(), *e.value()))
            else {
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
                    // `occupied.get()` is `(stored_seq, order)`. Decide against
                    // the live order while the entry lock is held. Borrow the
                    // resident order rather than cloning its `Arc` on the hot
                    // path: the immutable borrow lives only for the `decide`
                    // call, which returns OWNED action data, so it ends before
                    // any `get_mut()` / `remove()` commit below (no reference
                    // escapes into a `FrontAction`).
                    let (action, result) = decide(seq, occupied.get().1.as_ref());

                    match &action {
                        FrontAction::Remove => {
                            // Full consume: remove the entry under the lock, then
                            // drop its index entry. A cancel cannot also remove it
                            // (the entry is gone), so no double counter decrement.
                            let _ = occupied.remove();
                            self.index.remove(&seq);
                        }
                        FrontAction::KeepInPlace(residual) => {
                            // Partial fill keeping priority: swap the stored value
                            // to the residual in place, keeping the same
                            // sequence/index entry. Still under the entry lock.
                            let slot = occupied.get_mut();
                            slot.1 = residual.clone();
                        }
                        FrontAction::ReplaceAtTail(refreshed) => {
                            // Replenished tranche loses time priority, but the
                            // maker keeps the SAME id and must stay resident in
                            // `orders` so a concurrent cancel cannot slip into a
                            // remove-then-push gap. So: mint a fresh tail sequence
                            // and swap BOTH the value and its stored sequence in
                            // place under the entry lock; only the index is
                            // re-keyed (old seq -> new seq) afterwards.
                            let new_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                            {
                                let slot = occupied.get_mut();
                                slot.0 = new_seq;
                                slot.1 = refreshed.clone();
                            }
                            // `occupied` still holds the per-entry lock here (it
                            // is dropped at the end of this arm), so re-keying the
                            // index — a different structure (`SkipMap`), no
                            // deadlock — happens while a concurrent cancel is
                            // still excluded from the entry. Once the lock is
                            // released the value already carries `new_seq`, so a
                            // cancel removes `orders[id]` and `index[new_seq]`
                            // consistently. The only residue a race can leave is a
                            // stale `index[seq|new_seq] -> id` entry pointing at an
                            // already-removed id, which the next `match_front`
                            // self-heals on the `Vacant` branch. No order and no
                            // counter update is ever lost.
                            self.index.remove(&seq);
                            self.index.insert(new_seq, order_id);
                        }
                        FrontAction::SetAside => {
                            // No progress: leave the entry untouched and park its
                            // sequence so the sweep advances past it.
                            set_aside.insert(seq);
                        }
                    }

                    return FrontOutcome::Matched { result };
                }
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

    /// Replace the stored order for `order_id` in place, keeping its existing
    /// insertion sequence (and therefore its price-time / FIFO position).
    ///
    /// Returns the previous order if `order_id` was present, or `None` if it
    /// was not (e.g. concurrently removed). The `index` entry `seq -> id`
    /// stays valid because the sequence is unchanged, so only the `DashMap`
    /// value is swapped.
    ///
    /// The whole swap happens under the `DashMap` per-entry lock, so a
    /// concurrent [`OrderQueue::remove`] of the same id either observes the
    /// old value and removes it, or observes the new value and removes that —
    /// it never sees the entry mid-update. This closes the
    /// absent-from-`orders` window that a remove-then-push sequence would open.
    #[must_use]
    pub(crate) fn update_in_place(
        &self,
        order_id: Id,
        new_order: Arc<OrderType<()>>,
    ) -> Option<Arc<OrderType<()>>> {
        debug_assert_eq!(
            new_order.id(),
            order_id,
            "update_in_place: new_order id must match the key it is stored under"
        );
        let mut entry = self.orders.get_mut(&order_id)?;
        let (_seq, slot) = entry.value_mut();
        Some(std::mem::replace(slot, new_order))
    }

    /// Remove an order with the given ID.
    /// Returns the removed order if found. Cleans both the map and the index.
    #[must_use]
    pub fn remove(&self, order_id: Id) -> Option<Arc<OrderType<()>>> {
        let (_, (seq, order)) = self.orders.remove(&order_id)?;
        self.index.remove(&seq);
        Some(order)
    }

    /// Iterate through current orders without materializing an intermediate vector.
    pub fn iter_orders(&self) -> impl Iterator<Item = Arc<OrderType<()>>> + '_ {
        self.orders.iter().map(|entry| entry.value().1.clone())
    }

    /// Materialize a stable snapshot vector sorted by `(timestamp, sequence)`.
    ///
    /// The insertion sequence is used as a deterministic tiebreak so orders
    /// sharing a millisecond timestamp are still ordered exactly as matching
    /// would consume them. Note the sequence itself is not serialized: a
    /// snapshot round-trip reconstructs queue order from `(timestamp,
    /// sequence)`, so exact price-time priority survives a round-trip only when
    /// timestamps are monotonic with insertion order (the normal case).
    #[must_use]
    pub fn snapshot_vec(&self) -> Vec<Arc<OrderType<()>>> {
        let mut orders: Vec<(u64, Arc<OrderType<()>>)> =
            self.orders.iter().map(|o| o.value().clone()).collect();
        orders.sort_by_key(|(seq, o)| (o.timestamp(), *seq));
        orders.into_iter().map(|(_, o)| o).collect()
    }

    /// Convert the queue to a vector (for compatibility and snapshots).
    #[must_use]
    pub fn to_vec(&self) -> Vec<Arc<OrderType<()>>> {
        self.snapshot_vec()
    }

    /// Materialize the resting orders in ascending **insertion-sequence** order —
    /// the exact order [`OrderQueue::match_front`] consumes them.
    ///
    /// Walks the `index` (`SkipMap<seq, Id>`, which iterates ascending) and
    /// resolves each id in `orders`, skipping any transient index entry whose
    /// order was already removed. Unlike [`OrderQueue::snapshot_vec`] (sorted by
    /// `(timestamp, sequence)`), this reflects pure insertion order, so it equals
    /// the sweep even when timestamps are not monotonic with insertion.
    #[must_use]
    pub(crate) fn snapshot_by_seq(&self) -> Vec<Arc<OrderType<()>>> {
        let mut out = Vec::new();
        self.snapshot_by_seq_into(&mut out);
        out
    }

    /// Fill `out` with the resting orders in ascending **insertion-sequence**
    /// order — the buffer-reuse variant of [`OrderQueue::snapshot_by_seq`].
    ///
    /// `out` is cleared first, then extended in place, so a caller can reuse one
    /// scratch buffer across many calls and avoid a per-call allocation (e.g. a
    /// pooled buffer in a per-level pre-scan). The walk and skip-removed-entry
    /// semantics are identical to [`OrderQueue::snapshot_by_seq`]; the only
    /// difference is where the result lands.
    pub(crate) fn snapshot_by_seq_into(&self, out: &mut Vec<Arc<OrderType<()>>>) {
        out.clear();
        out.extend(self.index.iter().filter_map(|index_entry| {
            self.orders
                .get(index_entry.value())
                .map(|order_entry| order_entry.value().1.clone())
        }));
    }

    /// Creates a new `OrderQueue` instance and populates it with orders from the provided vector.
    ///
    /// This function takes ownership of a vector of order references (wrapped in `Arc`) and constructs
    /// a new `OrderQueue` by iteratively pushing each order into the queue. The resulting queue
    /// maintains the insertion order of the original vector.
    ///
    /// # Parameters
    ///
    /// * `orders` - A vector of atomic reference counted (`Arc`) order instances representing
    ///   the orders to be added to the new queue.
    ///
    /// # Returns
    ///
    /// A new `OrderQueue` instance containing all the orders from the input vector.
    ///
    #[allow(dead_code)]
    #[must_use]
    pub fn from_vec(orders: Vec<Arc<OrderType<()>>>) -> Self {
        let queue = OrderQueue::new();
        for order in orders {
            queue.push(order);
        }
        queue
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
        // the number of elements emitted. Iterating `index` while hinting
        // `self.len()` (from `orders`) could disagree in a transient state
        // where an order is in one structure but not yet the other.
        // Insertion-sequence order keeps the round-trip price-time priority
        // (the DashMap alone has no deterministic iteration order).
        let ordered = self.snapshot_by_seq();
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
        if !s.starts_with("OrderQueue:orders=[") || !s.ends_with(']') {
            return Err(PriceLevelError::ParseError {
                message: "Invalid format".to_string(),
            });
        }

        let content = &s["OrderQueue:orders=[".len()..s.len() - 1];
        let queue = OrderQueue::new();

        if !content.is_empty() {
            for order_str in content.split(',') {
                let order =
                    OrderType::from_str(order_str).map_err(|e| PriceLevelError::ParseError {
                        message: format!("Order parse error: {e}"),
                    })?;
                queue.push(Arc::new(order));
            }
        }

        Ok(queue)
    }
}

impl Display for OrderQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OrderQueue:orders=[")?;
        let mut first = true;
        for order in self.snapshot_vec() {
            if !first {
                write!(f, ",")?;
            }
            write!(f, "{order}")?;
            first = false;
        }
        write!(f, "]")
    }
}

impl From<Vec<Arc<OrderType<()>>>> for OrderQueue {
    fn from(orders: Vec<Arc<OrderType<()>>>) -> Self {
        let queue = OrderQueue::new();
        for order in orders {
            queue.push(order);
        }
        queue
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

        // Deserialize each order and add it to the queue
        while let Some(order) = seq.next_element::<OrderType<()>>()? {
            queue.push(Arc::new(order));
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
