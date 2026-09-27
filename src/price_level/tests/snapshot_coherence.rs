//! Issue #162: `PriceLevel::snapshot` returns a coherent snapshot or a typed
//! failure, never a panic or a substituted live counter.
//!
//! The concurrency regressions are deterministic: a thread-local collection
//! hook (`order_queue::snapshot_hook`) runs at fixed points of the shard walk,
//! and the concurrent mutation runs on a helper thread that the hook joins
//! before the walk continues. No sleeps, no scheduler stress.

#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::level::{PriceLevel, SNAPSHOT_MAX_ATTEMPTS};
    use crate::price_level::order_queue::snapshot_hook::{self, SnapshotHookEvent};
    use crate::price_level::{PriceLevelSnapshot, PriceLevelSnapshotPackage};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::thread;

    const PRICE: u128 = 10_000;

    fn standard(id: Id, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id,
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_716_000_000_000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn iceberg(id: Id, visible: u64, hidden: u64) -> OrderType<()> {
        OrderType::IcebergOrder {
            id,
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_716_000_000_000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn admit(level: &PriceLevel, order: OrderType<()>) {
        level.add_order(order).expect("admission must succeed");
    }

    fn cancel(level: &PriceLevel, id: Id) {
        level
            .update_order(OrderUpdate::Cancel { order_id: id })
            .expect("cancel must succeed")
            .expect("order must be resting");
    }

    fn resize(level: &PriceLevel, id: Id, quantity: u64) {
        level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: id,
                new_quantity: Quantity::new(quantity),
            })
            .expect("resize must succeed")
            .expect("order must be resting");
    }

    /// Runs `mutation` on a helper thread and waits for it. Called from a
    /// `Collected` hook, which runs while the walk holds the captured order's
    /// shard read lock, so the mutation must not run on the walking thread and
    /// must only touch ids in other shards.
    fn run_concurrently(
        level: &Arc<PriceLevel>,
        mutation: impl FnOnce(&PriceLevel) + Send + 'static,
    ) {
        let level = Arc::clone(level);
        thread::spawn(move || mutation(&level))
            .join()
            .expect("concurrent mutation thread panicked");
    }

    /// Picks three ids that live in three distinct order-storage shards, in
    /// the order the snapshot walk visits them: `(first, middle, last)`.
    ///
    /// The map's hasher is random per level, so the ids are discovered on the
    /// level under test: placeholders are admitted until three shards are
    /// occupied, then every placeholder is cancelled. Shard membership depends
    /// only on the id, so re-admitting the chosen ids later reproduces the
    /// same walk order.
    fn three_shard_ids(level: &PriceLevel) -> (Id, Id, Id) {
        let mut admitted = Vec::new();
        for raw in 1..=4_096u64 {
            let id = Id::from_u64(raw);
            admit(level, standard(id, 1));
            admitted.push(id);
            let runs = level.test_shard_runs();
            if runs.len() >= 3 {
                let first = runs[0][0];
                let middle = runs[1][0];
                let last = runs[2][0];
                for id in admitted {
                    cancel(level, id);
                }
                assert_eq!(level.order_count(), 0, "placeholders drained");
                return (first, middle, last);
            }
        }
        panic!("could not occupy three distinct shards");
    }

    fn expect_exhausted(result: Result<PriceLevelSnapshot, PriceLevelError>, reason: &str) {
        match result {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(
                    message.contains(&format!("after {SNAPSHOT_MAX_ATTEMPTS} attempts")),
                    "message must state the bound: {message}"
                );
                assert!(
                    message.contains(reason),
                    "message must carry the last rejection ({reason}): {message}"
                );
            }
            other => panic!("expected a typed InvalidOperation, got {other:?}"),
        }
    }

    fn assert_coherent(snapshot: &PriceLevelSnapshot) {
        let visible: u128 = snapshot
            .orders()
            .iter()
            .map(|o| u128::from(o.visible_quantity().as_u64()))
            .sum();
        let hidden: u128 = snapshot
            .orders()
            .iter()
            .map(|o| u128::from(o.hidden_quantity().as_u64()))
            .sum();
        assert_eq!(u128::from(snapshot.visible_quantity().as_u64()), visible);
        assert_eq!(u128::from(snapshot.hidden_quantity().as_u64()), hidden);
        assert_eq!(snapshot.order_count(), snapshot.orders().len());
    }

    fn visible_of(snapshot: &PriceLevelSnapshot, id: Id) -> u64 {
        snapshot
            .orders()
            .iter()
            .find(|o| o.id() == id)
            .map(|o| o.visible_quantity().as_u64())
            .expect("order present in snapshot")
    }

    /// Level with makers A (visible `u64::MAX - 1`), C (hidden-only iceberg,
    /// visible 0) and B (visible 1) in three distinct shards visited A, C, B.
    fn visible_transfer_level() -> (Arc<PriceLevel>, Id, Id, Id) {
        let level = Arc::new(PriceLevel::new(PRICE));
        let (a, c, b) = three_shard_ids(&level);
        admit(&level, standard(a, u64::MAX - 1));
        admit(&level, iceberg(c, 0, 1));
        admit(&level, standard(b, 1));
        assert_eq!(level.visible_quantity(), u64::MAX, "committed state fits");
        (level, a, c, b)
    }

    #[test]
    fn quiescent_snapshot_succeeds_on_first_attempt() {
        let (level, _a, _c, _b) = visible_transfer_level();
        let attempts = Rc::new(Cell::new(0u32));
        let guard = snapshot_hook::install({
            let attempts = Rc::clone(&attempts);
            move |event| {
                if event == SnapshotHookEvent::AttemptStart {
                    attempts.set(attempts.get() + 1);
                }
            }
        });
        let snapshot = level.snapshot().expect("quiescent snapshot succeeds");
        drop(guard);

        assert_eq!(attempts.get(), 1);
        assert_eq!(snapshot.visible_quantity(), Quantity::new(u64::MAX));
        assert_eq!(snapshot.hidden_quantity(), Quantity::new(1));
        assert_coherent(&snapshot);
    }

    #[test]
    fn two_shard_visible_transfer_is_recollected_not_substituted() {
        // Issue #162 schedule: the walk captures A = u64::MAX - 1, then (while
        // it sits on C's shard, after A's shard is released) A is resized to 1
        // and B to u64::MAX - 1, then the walk captures B. Every committed
        // total fits u64 and the side never changes, but the collected visible
        // sum is 2 * (u64::MAX - 1). The old code asserted (debug) or stored
        // the live counter (release); now the walk is rejected and recollected.
        let (level, a, c, b) = visible_transfer_level();
        let attempts = Rc::new(Cell::new(0u32));
        let transfers = Rc::new(Cell::new(0u32));
        let guard = snapshot_hook::install({
            let (attempts, transfers, level) = (
                Rc::clone(&attempts),
                Rc::clone(&transfers),
                Arc::clone(&level),
            );
            move |event| match event {
                SnapshotHookEvent::AttemptStart => attempts.set(attempts.get() + 1),
                SnapshotHookEvent::Collected(id) if id == c && transfers.get() == 0 => {
                    run_concurrently(&level, move |l| {
                        resize(l, a, 1);
                        resize(l, b, u64::MAX - 1);
                    });
                    transfers.set(1);
                }
                SnapshotHookEvent::Collected(_) | SnapshotHookEvent::LazyYield(_) => {}
            }
        });
        let snapshot = level
            .snapshot()
            .expect("the recollection sees the post-transfer state");
        drop(guard);

        assert_eq!(transfers.get(), 1, "the transfer ran inside the first walk");
        assert_eq!(attempts.get(), 2, "the overflowing first walk was rejected");
        assert_coherent(&snapshot);
        assert_eq!(snapshot.visible_quantity(), Quantity::new(u64::MAX));
        assert_eq!(visible_of(&snapshot, a), 1);
        assert_eq!(visible_of(&snapshot, b), u64::MAX - 1);

        // The coherent snapshot packages and restores with the same aggregates.
        let package = PriceLevelSnapshotPackage::new(snapshot).expect("package");
        let restored = PriceLevel::from_snapshot_package(package).expect("restore");
        assert_eq!(restored.visible_quantity(), u64::MAX);
        assert_eq!(restored.hidden_quantity(), 1);
        assert_eq!(restored.order_count(), 3);
    }

    #[test]
    fn sustained_visible_transfer_returns_typed_error_after_bounded_attempts() {
        // The transfer is undone before every attempt and redone inside every
        // walk, so no attempt can be coherent. The call must stop after
        // SNAPSHOT_MAX_ATTEMPTS with a typed error, not loop or substitute.
        let (level, a, c, b) = visible_transfer_level();
        let attempts = Rc::new(Cell::new(0u32));
        let transfers = Rc::new(Cell::new(0u32));
        let guard = snapshot_hook::install({
            let (attempts, transfers, level) = (
                Rc::clone(&attempts),
                Rc::clone(&transfers),
                Arc::clone(&level),
            );
            move |event| match event {
                SnapshotHookEvent::AttemptStart => {
                    attempts.set(attempts.get() + 1);
                    if transfers.get() > 0 {
                        // Undo: shrink B first so the level never overflows.
                        run_concurrently(&level, move |l| {
                            resize(l, b, 1);
                            resize(l, a, u64::MAX - 1);
                        });
                    }
                }
                SnapshotHookEvent::Collected(id) if id == c => {
                    run_concurrently(&level, move |l| {
                        resize(l, a, 1);
                        resize(l, b, u64::MAX - 1);
                    });
                    transfers.set(transfers.get() + 1);
                }
                SnapshotHookEvent::Collected(_) | SnapshotHookEvent::LazyYield(_) => {}
            }
        });
        let result = level.snapshot();
        drop(guard);

        assert_eq!(
            attempts.get(),
            SNAPSHOT_MAX_ATTEMPTS,
            "bounded, no extra walk"
        );
        assert_eq!(transfers.get(), SNAPSHOT_MAX_ATTEMPTS);
        expect_exhausted(result, "snapshot visible quantity overflow");

        // The failed snapshot mutated nothing: the level holds exactly the
        // state the (finished) concurrent writer left, counters agree with the
        // queue, and once mutation stops the next snapshot succeeds.
        assert_eq!(level.visible_quantity(), u64::MAX);
        assert_eq!(level.hidden_quantity(), 1);
        assert_eq!(level.order_count(), 3);
        let snapshot = level.snapshot().expect("progress once mutation pauses");
        assert_coherent(&snapshot);
        assert_eq!(visible_of(&snapshot, a), 1);
        assert_eq!(visible_of(&snapshot, b), u64::MAX - 1);
        assert_eq!(
            snapshot.statistics().orders_added(),
            level.stats().orders_added()
        );
    }

    /// Level with icebergs A (hidden `u64::MAX - 1`) and B (hidden 1) and a
    /// standard C, in three distinct shards visited A, C, B.
    fn hidden_transfer_level() -> (Arc<PriceLevel>, Id, Id, Id) {
        let level = Arc::new(PriceLevel::new(PRICE));
        let (a, c, b) = three_shard_ids(&level);
        admit(&level, iceberg(a, 1, u64::MAX - 1));
        admit(&level, standard(c, 1));
        admit(&level, iceberg(b, 1, 1));
        assert_eq!(level.hidden_quantity(), u64::MAX, "committed state fits");
        (level, a, c, b)
    }

    /// Moves the hidden depth from A to B: cancel A, then replace B (same id,
    /// so same shard) by an iceberg with hidden `u64::MAX - 1`.
    fn transfer_hidden(level: &PriceLevel, a: Id, b: Id) {
        cancel(level, a);
        cancel(level, b);
        admit(level, iceberg(b, 1, u64::MAX - 1));
    }

    #[test]
    fn two_shard_hidden_transfer_is_recollected_not_substituted() {
        let (level, a, c, b) = hidden_transfer_level();
        let attempts = Rc::new(Cell::new(0u32));
        let transfers = Rc::new(Cell::new(0u32));
        let guard = snapshot_hook::install({
            let (attempts, transfers, level) = (
                Rc::clone(&attempts),
                Rc::clone(&transfers),
                Arc::clone(&level),
            );
            move |event| match event {
                SnapshotHookEvent::AttemptStart => attempts.set(attempts.get() + 1),
                SnapshotHookEvent::Collected(id) if id == c && transfers.get() == 0 => {
                    run_concurrently(&level, move |l| transfer_hidden(l, a, b));
                    transfers.set(1);
                }
                SnapshotHookEvent::Collected(_) | SnapshotHookEvent::LazyYield(_) => {}
            }
        });
        let snapshot = level
            .snapshot()
            .expect("the recollection sees the post-transfer state");
        drop(guard);

        assert_eq!(attempts.get(), 2, "the overflowing first walk was rejected");
        assert_coherent(&snapshot);
        assert_eq!(snapshot.order_count(), 2, "A is gone after the transfer");
        assert_eq!(snapshot.hidden_quantity(), Quantity::new(u64::MAX - 1));
        assert_eq!(snapshot.visible_quantity(), Quantity::new(2));
    }

    #[test]
    fn sustained_hidden_transfer_returns_typed_error_after_bounded_attempts() {
        let (level, a, c, b) = hidden_transfer_level();
        let attempts = Rc::new(Cell::new(0u32));
        let transfers = Rc::new(Cell::new(0u32));
        let guard = snapshot_hook::install({
            let (attempts, transfers, level) = (
                Rc::clone(&attempts),
                Rc::clone(&transfers),
                Arc::clone(&level),
            );
            move |event| match event {
                SnapshotHookEvent::AttemptStart => {
                    attempts.set(attempts.get() + 1);
                    if transfers.get() > 0 {
                        // Undo: shrink B first, then re-admit A.
                        run_concurrently(&level, move |l| {
                            cancel(l, b);
                            admit(l, iceberg(b, 1, 1));
                            admit(l, iceberg(a, 1, u64::MAX - 1));
                        });
                    }
                }
                SnapshotHookEvent::Collected(id) if id == c => {
                    run_concurrently(&level, move |l| transfer_hidden(l, a, b));
                    transfers.set(transfers.get() + 1);
                }
                SnapshotHookEvent::Collected(_) | SnapshotHookEvent::LazyYield(_) => {}
            }
        });
        let result = level.snapshot();
        drop(guard);

        assert_eq!(attempts.get(), SNAPSHOT_MAX_ATTEMPTS);
        expect_exhausted(result, "snapshot hidden quantity overflow");
        assert_eq!(level.hidden_quantity(), u64::MAX - 1);
        assert_eq!(level.order_count(), 2);
        assert_coherent(&level.snapshot().expect("progress once mutation pauses"));
    }

    #[test]
    fn snapshot_package_and_json_propagate_the_typed_error() {
        let (level, a, c, b) = visible_transfer_level();
        let transfers = Rc::new(Cell::new(0u32));
        let guard = snapshot_hook::install({
            let (transfers, level) = (Rc::clone(&transfers), Arc::clone(&level));
            move |event| match event {
                SnapshotHookEvent::AttemptStart if transfers.get() % 2 == 1 => {
                    run_concurrently(&level, move |l| {
                        resize(l, b, 1);
                        resize(l, a, u64::MAX - 1);
                    });
                    transfers.set(transfers.get() + 1);
                }
                SnapshotHookEvent::Collected(id) if id == c => {
                    run_concurrently(&level, move |l| {
                        resize(l, a, 1);
                        resize(l, b, u64::MAX - 1);
                    });
                    transfers.set(transfers.get() + 1);
                }
                _ => {}
            }
        });
        expect_exhausted(
            level.snapshot_package().map(|p| p.snapshot().clone()),
            "snapshot visible quantity overflow",
        );
        let json = level.snapshot_to_json();
        drop(guard);
        assert!(
            matches!(json, Err(PriceLevelError::InvalidOperation { .. })),
            "snapshot_to_json propagates the typed error: {json:?}"
        );
    }

    // ------------------------------------------------------------------
    // refresh_aggregates: compute every replacement first, commit together.
    // ------------------------------------------------------------------

    fn stale_snapshot(orders: Vec<Arc<OrderType<()>>>) -> PriceLevelSnapshot {
        PriceLevelSnapshot::from_raw_parts(
            Price::new(PRICE),
            Quantity::new(7),
            Quantity::new(9),
            42,
            orders,
        )
    }

    fn assert_unchanged(snapshot: &PriceLevelSnapshot, len: usize) {
        assert_eq!(snapshot.visible_quantity(), Quantity::new(7));
        assert_eq!(snapshot.hidden_quantity(), Quantity::new(9));
        assert_eq!(
            snapshot.order_count(),
            42,
            "order_count not committed early"
        );
        assert_eq!(snapshot.orders().len(), len);
    }

    fn expect_invalid(result: Result<(), PriceLevelError>, reason: &str) {
        match result {
            Err(PriceLevelError::InvalidOperation { message }) => assert_eq!(message, reason),
            other => panic!("expected InvalidOperation({reason}), got {other:?}"),
        }
    }

    #[test]
    fn refresh_aggregates_hidden_overflow_leaves_snapshot_unchanged() {
        let mut snapshot = stale_snapshot(vec![
            Arc::new(iceberg(Id::from_u64(1), 5, u64::MAX - 10)),
            Arc::new(iceberg(Id::from_u64(2), 5, 20)),
        ]);
        expect_invalid(
            snapshot.refresh_aggregates(),
            "snapshot hidden quantity overflow",
        );
        assert_unchanged(&snapshot, 2);
    }

    #[test]
    fn refresh_aggregates_visible_overflow_leaves_snapshot_unchanged() {
        let mut snapshot = stale_snapshot(vec![
            Arc::new(standard(Id::from_u64(1), u64::MAX)),
            Arc::new(standard(Id::from_u64(2), 1)),
        ]);
        expect_invalid(
            snapshot.refresh_aggregates(),
            "snapshot visible quantity overflow",
        );
        assert_unchanged(&snapshot, 2);
    }

    #[test]
    fn refresh_aggregates_order_total_overflow_leaves_snapshot_unchanged() {
        let mut snapshot = stale_snapshot(vec![
            Arc::new(standard(Id::from_u64(1), 3)),
            Arc::new(iceberg(Id::from_u64(2), u64::MAX, 1)),
        ]);
        expect_invalid(
            snapshot.refresh_aggregates(),
            "order total quantity overflows u64",
        );
        assert_unchanged(&snapshot, 2);
    }

    #[test]
    fn refresh_aggregates_success_commits_all_three_fields() {
        let mut snapshot = stale_snapshot(vec![
            Arc::new(standard(Id::from_u64(1), 3)),
            Arc::new(iceberg(Id::from_u64(2), 4, 11)),
        ]);
        snapshot.refresh_aggregates().expect("fits u64");
        assert_eq!(snapshot.visible_quantity(), Quantity::new(7));
        assert_eq!(snapshot.hidden_quantity(), Quantity::new(11));
        assert_eq!(snapshot.order_count(), 2);
    }

    #[test]
    fn hidden_total_overflow_is_rejected_by_constructors_and_package() {
        let orders = vec![
            Arc::new(iceberg(Id::from_u64(1), 1, u64::MAX - 1)),
            Arc::new(iceberg(Id::from_u64(2), 1, 2)),
        ];
        match PriceLevelSnapshot::with_orders(Price::new(PRICE), orders.clone()) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert_eq!(message, "snapshot hidden quantity overflow");
            }
            other => panic!("expected hidden overflow, got {other:?}"),
        }
        let package = PriceLevelSnapshotPackage::new(stale_snapshot(orders));
        assert!(
            matches!(package, Err(PriceLevelError::InvalidOperation { .. })),
            "package construction rejects a hidden-overflowing snapshot"
        );
    }
}
