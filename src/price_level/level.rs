//! Core price level implementation

use crate::UuidGenerator;
use crate::errors::PriceLevelError;
use crate::execution::{MatchResult, Trade};
use crate::orders::{Id, OrderType, OrderUpdate};
use crate::price_level::order_queue::OrderQueue;
use crate::price_level::{PriceLevelSnapshot, PriceLevelSnapshotPackage, PriceLevelStatistics};
use crate::utils::{Price, Quantity, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::str::FromStr;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// A lock-free implementation of a price level in a limit order book
#[derive(Debug)]
pub struct PriceLevel {
    /// The price of this level
    price: u128,

    /// Total visible quantity at this price level
    visible_quantity: AtomicU64,

    /// Total hidden quantity at this price level
    hidden_quantity: AtomicU64,

    /// Number of orders at this price level
    order_count: AtomicUsize,

    /// Queue of orders at this price level
    orders: OrderQueue,

    /// Statistics for this price level
    stats: Arc<PriceLevelStatistics>,
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
    /// Returns [`PriceLevelError::InvalidOperation`] if recomputing the snapshot
    /// aggregates overflows `u64`.
    pub fn from_snapshot(mut snapshot: PriceLevelSnapshot) -> Result<Self, PriceLevelError> {
        snapshot.refresh_aggregates()?;

        let order_count = snapshot.orders().len();
        let visible_quantity = snapshot.visible_quantity();
        let hidden_quantity = snapshot.hidden_quantity();
        let price = snapshot.price();
        // Clone the persisted statistics before consuming the snapshot's orders.
        let stats = (*snapshot.statistics()).clone();
        let queue = OrderQueue::from(snapshot.into_orders());

        Ok(Self {
            price,
            visible_quantity: AtomicU64::new(visible_quantity),
            hidden_quantity: AtomicU64::new(hidden_quantity),
            order_count: AtomicUsize::new(order_count),
            orders: queue,
            stats: Arc::new(stats),
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
    /// recompute that checksum fails, and [`PriceLevelError::InvalidOperation`]
    /// on an unsupported snapshot format version.
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
            order_count: AtomicUsize::new(0),
            orders: OrderQueue::new(),
            stats: Arc::new(PriceLevelStatistics::new()),
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
        // `Relaxed`: advisory counter, no happens-before rides on it — see
        // `visible_quantity` for the full rationale.
        self.order_count.load(Ordering::Relaxed)
    }

    /// Get the statistics for this price level
    #[must_use]
    pub fn stats(&self) -> Arc<PriceLevelStatistics> {
        self.stats.clone()
    }

    /// Add an order to this price level
    pub fn add_order(&self, order: OrderType<()>) -> Arc<OrderType<()>> {
        // Calculate quantities
        let visible_qty = order.visible_quantity();
        let hidden_qty = order.hidden_quantity();

        // On this path, publish to the queue FIRST, then bump the counters, so a
        // concurrent reader of this add tends to see the order resting before it
        // is counted rather than the reverse. This is a local ordering choice,
        // not a cross-method guarantee (other paths, e.g. iceberg replenishment
        // in `match_order`, adjust counters before pushing). The bare counters
        // are advisory under concurrency; `snapshot()` is the mutually-consistent
        // view (see `visible_quantity`).
        let order_arc = Arc::new(order);
        self.orders.push(order_arc.clone());

        // Update atomic counters. `Relaxed` on all three: these are the
        // advisory counters (issue #68). The queue publication above (the
        // `push` into `OrderQueue`'s `SkipMap` / `DashMap`) carries the actual
        // happens-before for a concurrent reader; the counters are not a
        // synchronization channel, so a release here would publish nothing a
        // reader relies on. The RMWs only need to be atomic, not ordered.
        self.visible_quantity
            .fetch_add(visible_qty, Ordering::Relaxed);
        self.hidden_quantity
            .fetch_add(hidden_qty, Ordering::Relaxed);
        self.order_count.fetch_add(1, Ordering::Relaxed);

        // Update statistics
        self.stats.record_order_added();

        order_arc
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

    /// Matches an incoming order against existing orders at this price level.
    ///
    /// This function attempts to match the incoming order quantity against the orders present in the
    /// `OrderQueue`. It iterates through the queue, matching orders until the incoming quantity is
    /// fully filled or the queue is exhausted. Trades are generated for each successful match,
    /// and filled orders are removed from the queue.  The function also updates the visible and hidden
    /// quantity counters and records statistics for each execution.
    ///
    /// Time-in-force EXPIRY is NOT enforced here: a resting maker's
    /// [`TimeInForce`](crate::orders::TimeInForce) (e.g. `Gtd` / `Day`) is not
    /// consulted, so an expired maker still matches. Evicting or skipping
    /// expired orders is the caller's / order book's responsibility, keeping
    /// the match path a pure, deterministic sweep over the resting queue.
    ///
    /// # Arguments
    ///
    /// * `incoming_quantity`: The quantity of the incoming order to be matched.
    /// * `taker_order_id`: The ID of the incoming order (the "taker" order).
    /// * `timestamp`: The taker timestamp (milliseconds since epoch) stamped
    ///   onto every emitted [`Trade`] and used as the execution time for
    ///   statistics. It is threaded in from the caller so the match path never
    ///   reads the wall clock — guaranteeing a deterministic, replayable trade
    ///   stream for a fixed input.
    /// * `trade_id_generator`: An atomic counter used to generate unique trade IDs.
    ///
    /// [`Trade`]: crate::execution::Trade
    ///
    /// # Returns
    ///
    /// A `MatchResult` object containing the results of the matching operation, including a list of
    /// generated trades, the remaining unmatched quantity, a flag indicating whether the
    /// incoming order was completely filled, and a list of IDs of orders that were completely filled
    /// during the matching process.
    ///
    /// # Concurrency
    ///
    /// Resting orders are consumed in strict price-time (FIFO) order, and a
    /// partially-filled maker keeps its position at the front of the queue.
    /// This method assumes a **single logical matcher per level at a time**:
    /// concurrent `add_order` / `cancel` from other threads are safe, but two
    /// concurrent `match_order` calls on the *same* level — or a `match_order`
    /// racing a `cancel` of the resting order it is currently matching — must
    /// be serialized by the caller (an order book typically matches a level
    /// from a single thread).
    pub fn match_order(
        &self,
        incoming_quantity: u64,
        taker_order_id: Id,
        timestamp: TimestampMs,
        trade_id_generator: &UuidGenerator,
    ) -> MatchResult {
        // A single sweep emits at most one trade and at most one filled-order
        // id per resting order, so the live order count is an upper bound for
        // both vectors. Pre-size them to cut per-fill reallocations on the hot
        // path; the count is advisory (`Relaxed`) so the `Vec` still grows if a
        // concurrent `add_order` lands mid-sweep — capacity is a hint, not a
        // bound.
        let capacity = self.order_count();
        let mut result = MatchResult::with_capacity(taker_order_id, incoming_quantity, capacity);
        let mut remaining = incoming_quantity;

        while remaining > 0 {
            if let Some((order_seq, order_arc)) = self.orders.pop_entry() {
                let (consumed, updated_order, hidden_reduced, new_remaining) =
                    order_arc.match_against(remaining);

                if consumed > 0 {
                    // Update visible quantity counter. `Relaxed`: advisory
                    // counter (issue #68); the queue mutation (`pop_entry`
                    // above) carries the real happens-before, not this RMW.
                    self.visible_quantity.fetch_sub(consumed, Ordering::Relaxed);

                    // Use UUID generator directly
                    let trade_id = Id::from_uuid(trade_id_generator.next());

                    // A resting maker must never be the taker that is matching
                    // against it. Debug-only guard: the type system cannot
                    // encode this, and it is a logic invariant of the caller
                    // (the order book never routes an order against itself).
                    debug_assert!(
                        order_arc.id() != taker_order_id,
                        "self-fill: maker == taker"
                    );

                    let trade = Trade::with_timestamp(
                        trade_id,
                        taker_order_id,
                        order_arc.id(),
                        Price::new(self.price),
                        Quantity::new(consumed),
                        order_arc.side().opposite(),
                        timestamp,
                    );

                    if result.add_trade(trade).is_err() {
                        remaining = new_remaining;
                        break;
                    }

                    // If the order was completely executed, add it to filled_order_ids
                    if updated_order.is_none() {
                        result.add_filled_order_id(order_arc.id());
                    }

                    // Update statistics only for real executions. A popped maker
                    // can yield `consumed == 0` (e.g. a zero-quantity resting
                    // order from `update_order`); recording it would inflate
                    // `orders_executed` / `last_execution_time` without a trade.
                    let _ = self.stats.record_execution(
                        consumed,
                        order_arc.price().as_u128(),
                        order_arc.timestamp(),
                        timestamp.as_u64(),
                    );
                }

                remaining = new_remaining;

                if let Some(updated) = updated_order {
                    if hidden_reduced > 0 {
                        // Iceberg/Reserve replenishment: a fresh tranche is
                        // drawn from hidden into visible. By convention the
                        // refreshed tranche loses time priority, so it is
                        // appended to the tail via `push` (new sequence).
                        // `Relaxed` on both counter RMWs: advisory counters
                        // (issue #68); the `push` below carries the real
                        // happens-before for the refreshed tranche.
                        self.hidden_quantity
                            .fetch_sub(hidden_reduced, Ordering::Relaxed);
                        self.visible_quantity
                            .fetch_add(hidden_reduced, Ordering::Relaxed);
                        self.orders.push(Arc::new(updated));
                    } else {
                        // Pure partial fill: the maker keeps its place in the
                        // queue. Re-insert the residual at its original
                        // sequence so it stays ahead of later arrivals,
                        // preserving strict price-time priority.
                        self.orders.reinsert(order_seq, Arc::new(updated));
                    }
                } else {
                    // Maker fully consumed and removed from the queue (the
                    // `pop_entry` above already removed it). `Relaxed` on both
                    // counter RMWs: advisory counters (issue #68); the queue
                    // removal carries the happens-before, not these counters.
                    self.order_count.fetch_sub(1, Ordering::Relaxed);
                    match &*order_arc {
                        OrderType::IcebergOrder {
                            hidden_quantity, ..
                        }
                        | OrderType::ReserveOrder {
                            hidden_quantity, ..
                        } if hidden_quantity.as_u64() > 0 && hidden_reduced == 0 => {
                            self.hidden_quantity
                                .fetch_sub(hidden_quantity.as_u64(), Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }

                if remaining == 0 {
                    break;
                }
            } else {
                break;
            }
        }

        result.finalize(remaining);

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
    #[must_use]
    pub fn snapshot(&self) -> PriceLevelSnapshot {
        // Materialize the orders exactly once; every aggregate is derived from
        // this snapshot so they are mutually consistent by construction.
        let orders = self.snapshot_orders();

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
            match visible_quantity.checked_add(order.visible_quantity()) {
                Some(total) => visible_quantity = total,
                None => {
                    debug_assert!(false, "snapshot visible quantity overflow is unreachable");
                    visible_quantity = self.visible_quantity();
                }
            }

            match hidden_quantity.checked_add(order.hidden_quantity()) {
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
            self.price,
            visible_quantity,
            hidden_quantity,
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
    /// the order's total before and after the update. Order types that cannot
    /// be resized fall back to an unchanged order and therefore take the
    /// in-place (position-preserving) branch.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if an
    /// [`OrderUpdate::UpdatePrice`] / [`OrderUpdate::Replace`] would not move
    /// the order to a different price level, or if computing an order's total
    /// quantity overflows `u64`.
    #[must_use = "the updated order (or None when the order is absent) must be handled"]
    pub fn update_order(
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
                        let visible_qty = order_arc.visible_quantity();
                        let hidden_qty = order_arc.hidden_quantity();

                        self.visible_quantity
                            .fetch_sub(visible_qty, Ordering::Relaxed);
                        self.hidden_quantity
                            .fetch_sub(hidden_qty, Ordering::Relaxed);
                        self.order_count.fetch_sub(1, Ordering::Relaxed);

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
                // Read the current order to build the resized order and pick the
                // priority policy. The counter deltas below are taken from the
                // order actually removed/replaced under the queue's per-entry
                // lock — not from this pre-read — so a concurrent update cannot
                // drift `visible_quantity` / `hidden_quantity` from the queue.
                let Some(order) = self.orders.find(order_id) else {
                    return Ok(None); // Order not found
                };

                let prev_total = order
                    .visible_quantity()
                    .checked_add(order.hidden_quantity())
                    .ok_or_else(|| PriceLevelError::InvalidOperation {
                        message: "order total quantity overflow".to_string(),
                    })?;

                // Build the updated order. `with_reduced_quantity` sets the
                // (visible/main) quantity to exactly `new_quantity` for order
                // types that support resizing; types it cannot resize fall back
                // to an unchanged clone, so `new_total` equals the old total for
                // them and they take the position-preserving in-place branch.
                let new_order = order.with_reduced_quantity(new_quantity.as_u64());
                let new_visible = new_order.visible_quantity();
                let new_hidden = new_order.hidden_quantity();
                let new_total = new_visible.checked_add(new_hidden).ok_or_else(|| {
                    PriceLevelError::InvalidOperation {
                        message: "order total quantity overflow".to_string(),
                    }
                })?;

                let new_order_arc = Arc::new(new_order);

                // Perform the queue mutation and capture the order it actually
                // removed/replaced, so the counter deltas reflect the real
                // transition rather than the possibly-stale pre-read above.
                let old = if new_total > prev_total {
                    // Quantity INCREASE: demote to the back of the queue (mint a
                    // new sequence), losing time priority. Retains the
                    // remove+push shape; its concurrency window is the broader
                    // #81 work and is intentionally not addressed here.
                    let Some(removed) = self.orders.remove(order_id) else {
                        return Ok(None); // Removed by another thread.
                    };
                    self.orders.push(new_order_arc.clone());
                    removed
                } else {
                    // Quantity DECREASE or unchanged total: keep the maker's
                    // queue position by swapping the stored order in place at its
                    // existing insertion sequence, under the DashMap per-entry
                    // lock.
                    let Some(replaced) =
                        self.orders.update_in_place(order_id, new_order_arc.clone())
                    else {
                        return Ok(None); // Removed by another thread.
                    };
                    replaced
                };

                // Apply the counter deltas from the actual replaced order. A
                // single component (visible or hidden) can move in EITHER
                // direction even when the total shrinks or is unchanged, because
                // quantity can shift between the visible and hidden portions, so
                // handle both signs.
                let old_visible = old.visible_quantity();
                let old_hidden = old.hidden_quantity();
                // `Relaxed` on both branches: advisory counters (issue #68); the
                // queue mutation above (`update_in_place` / `remove` + `push`)
                // carries the happens-before, not these counter RMWs.
                let apply = |counter: &std::sync::atomic::AtomicU64, old: u64, new: u64| {
                    if new >= old {
                        counter.fetch_add(new - old, Ordering::Relaxed);
                    } else {
                        counter.fetch_sub(old - new, Ordering::Relaxed);
                    }
                };
                apply(&self.visible_quantity, old_visible, new_visible);
                apply(&self.hidden_quantity, old_hidden, new_hidden);

                Ok(Some(new_order_arc))
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
                        let visible_qty = order_arc.visible_quantity();
                        let hidden_qty = order_arc.hidden_quantity();

                        self.visible_quantity
                            .fetch_sub(visible_qty, Ordering::Relaxed);
                        self.hidden_quantity
                            .fetch_sub(hidden_qty, Ordering::Relaxed);
                        self.order_count.fetch_sub(1, Ordering::Relaxed);

                        // Update statistics
                        self.stats.record_order_removed();
                    }
                    Ok(order)
                } else {
                    // If price is the same, just update the quantity (reuse logic)
                    self.update_order(OrderUpdate::UpdateQuantity {
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
                    let visible_qty = order_arc.visible_quantity();
                    let hidden_qty = order_arc.hidden_quantity();

                    self.visible_quantity
                        .fetch_sub(visible_qty, Ordering::Relaxed);
                    self.hidden_quantity
                        .fetch_sub(hidden_qty, Ordering::Relaxed);
                    self.order_count.fetch_sub(1, Ordering::Relaxed);

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
                        let visible_qty = order_arc.visible_quantity();
                        let hidden_qty = order_arc.hidden_quantity();

                        self.visible_quantity
                            .fetch_sub(visible_qty, Ordering::Relaxed);
                        self.hidden_quantity
                            .fetch_sub(hidden_qty, Ordering::Relaxed);
                        self.order_count.fetch_sub(1, Ordering::Relaxed);

                        // Update statistics
                        self.stats.record_order_removed();
                    }

                    Ok(order)
                } else {
                    // If price is the same, just update the quantity
                    self.update_order(OrderUpdate::UpdateQuantity {
                        order_id,
                        new_quantity: quantity,
                    })
                }
            }
        }
    }
}

/// Serializable representation of a price level for easier data transfer and storage
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
            orders: price_level
                .iter_orders()
                .map(|order_arc| *order_arc)
                .collect(),
        }
    }
}

impl From<&PriceLevelSnapshot> for PriceLevel {
    fn from(value: &PriceLevelSnapshot) -> Self {
        let mut snapshot = value.clone();
        let _ = snapshot.refresh_aggregates();

        let order_count = snapshot.orders().len();
        let visible_quantity = snapshot.visible_quantity();
        let hidden_quantity = snapshot.hidden_quantity();
        let price = snapshot.price();
        // Preserve the persisted statistics instead of resetting them.
        let stats = (*snapshot.statistics()).clone();
        let queue = OrderQueue::from(snapshot.into_orders());

        Self {
            price,
            visible_quantity: AtomicU64::new(visible_quantity),
            hidden_quantity: AtomicU64::new(hidden_quantity),
            order_count: AtomicUsize::new(order_count),
            orders: queue,
            stats: Arc::new(stats),
        }
    }
}

impl TryFrom<PriceLevelData> for PriceLevel {
    type Error = PriceLevelError;

    fn try_from(data: PriceLevelData) -> Result<Self, Self::Error> {
        let price_level = PriceLevel::new(data.price);

        // Add orders to the price level
        for order in data.orders {
            price_level.add_order(order);
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
                        price_level.add_order(order);
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
                price_level.add_order(order);
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
