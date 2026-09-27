//! Issue #147: an order handle held outside the queue is never mutated by a
//! later match step.
//!
//! `add_order`, `iter_orders`, `snapshot_orders` and `snapshot` hand out
//! `Arc<OrderType<()>>` values that share the resting allocation. A partial
//! fill or a replenishment commits a new value for the maker under its entry
//! lock; every handle obtained before that commit must keep showing the value
//! it was handed. #147 evaluated reusing the resting allocation when the
//! stored `Arc` is uniquely owned (see `BENCH.md`, "Residual allocation reuse")
//! and kept the current commit; these tests pin the ownership contract any
//! such change must keep:
//! - a retained admission handle, `snapshot_orders` view, `snapshot` and
//!   checksum package keep their values across partial fills;
//! - replenishment keeps the tail demotion and leaves a retained handle
//!   unchanged;
//! - concurrent readers never observe a change through a handle they hold,
//!   and a racing cancel still either fully wins or fully loses.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::execution::{MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::price_level::snapshot::PriceLevelSnapshotPackage;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const EXECUTION_MS: u64 = 1_700_000_000_000;

    fn standard(id: u64, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn iceberg(id: u64, visible: u64, hidden: u64) -> OrderType<()> {
        OrderType::IcebergOrder {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn take(
        level: &PriceLevel,
        quantity: u64,
        taker: u64,
        generator: &UuidGenerator,
    ) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(taker),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_MS),
            generator,
        )
    }

    fn generator() -> UuidGenerator {
        UuidGenerator::new(Uuid::nil())
    }

    fn ids_in_fifo(level: &PriceLevel) -> Vec<Id> {
        level
            .snapshot_by_insertion_seq()
            .expect("materialize")
            .iter()
            .map(|o| o.id())
            .collect()
    }

    fn assert_counters_match_queue(level: &PriceLevel) {
        let orders = level.snapshot_orders().expect("materialize");
        let visible: u64 = orders.iter().map(|o| o.visible_quantity().as_u64()).sum();
        let hidden: u64 = orders.iter().map(|o| o.hidden_quantity().as_u64()).sum();
        assert_eq!(
            level.visible_quantity(),
            visible,
            "visible counter vs queue"
        );
        assert_eq!(level.hidden_quantity(), hidden, "hidden counter vs queue");
        assert_eq!(level.order_count(), orders.len(), "order count vs queue");
    }

    fn assert_snapshot_round_trips(level: &PriceLevel) {
        let json = level.snapshot_to_json().expect("snapshot serializes");
        let restored = PriceLevel::from_snapshot_json(&json).expect("snapshot restores");
        assert_eq!(restored.visible_quantity(), level.visible_quantity());
        assert_eq!(restored.hidden_quantity(), level.hidden_quantity());
        let original: Vec<OrderType<()>> = level
            .snapshot_by_insertion_seq()
            .expect("materialize")
            .iter()
            .map(|o| **o)
            .collect();
        let back: Vec<OrderType<()>> = restored
            .snapshot_by_insertion_seq()
            .expect("materialize")
            .iter()
            .map(|o| **o)
            .collect();
        assert_eq!(
            original, back,
            "queue contents and priority survive the round trip"
        );
    }

    #[test]
    fn test_partial_fill_retained_admission_handle_is_never_mutated() {
        let level = PriceLevel::new(PRICE);
        let admission = level.add_order(standard(1, 1_000)).expect("admit");
        level.add_order(standard(2, 5)).expect("admit");
        let generator = generator();

        for i in 0..6 {
            let result = take(&level, 10, 100 + i, &generator);
            assert!(result.is_complete());
            assert_eq!(result.trades().len(), 1);
            assert!(result.filled_order_ids().is_empty());
        }

        assert_eq!(admission.visible_quantity().as_u64(), 1_000);
        // The partially filled maker kept its time priority.
        assert_eq!(ids_in_fifo(&level), vec![Id::from_u64(1), Id::from_u64(2)]);
        assert_eq!(level.visible_quantity(), 940 + 5);
        assert_counters_match_queue(&level);
        assert_snapshot_round_trips(&level);
    }

    #[test]
    fn test_partial_fill_retained_views_keep_their_values() {
        let level = PriceLevel::new(PRICE);
        drop(level.add_order(standard(1, 1_000)).expect("admit"));
        let generator = generator();

        let snapshot = level.snapshot().expect("snapshot");
        let package = PriceLevelSnapshotPackage::new(snapshot.clone()).expect("package");
        let view = level.snapshot_orders().expect("materialize");
        take(&level, 10, 100, &generator);
        assert_eq!(snapshot.orders()[0].visible_quantity().as_u64(), 1_000);
        assert_eq!(view[0].visible_quantity().as_u64(), 1_000);
        package
            .validate()
            .expect("the retained package still validates");
        assert_eq!(
            package.snapshot().orders()[0].visible_quantity().as_u64(),
            1_000
        );

        let held = level.iter_orders().next().expect("rests");
        take(&level, 10, 101, &generator);
        assert_eq!(held.visible_quantity().as_u64(), 990);
        assert_eq!(level.visible_quantity(), 980);
        assert_counters_match_queue(&level);
        assert_snapshot_round_trips(&level);
    }

    #[test]
    fn test_replenish_retained_handle_is_never_mutated_and_demotes_to_tail() {
        let level = PriceLevel::new(PRICE);
        let admission = level.add_order(iceberg(1, 10, 100)).expect("admit");
        drop(level.add_order(standard(2, 5)).expect("admit"));
        let generator = generator();

        let result = take(&level, 10, 100, &generator);
        assert!(result.is_complete());
        assert_eq!(result.trades().len(), 1);
        assert_eq!(admission.visible_quantity().as_u64(), 10);
        assert_eq!(admission.hidden_quantity().as_u64(), 100);
        // The refreshed tranche lost time priority.
        assert_eq!(ids_in_fifo(&level), vec![Id::from_u64(2), Id::from_u64(1)]);
        assert_eq!(level.visible_quantity(), 15);
        assert_eq!(level.hidden_quantity(), 90);
        assert_counters_match_queue(&level);
        assert_snapshot_round_trips(&level);
    }

    #[test]
    fn test_concurrent_views_never_observe_a_mutation() {
        const FILLS: u64 = 2_000;
        const READERS: usize = 2;
        let level = Arc::new(PriceLevel::new(PRICE));
        drop(level.add_order(standard(1, 1_000_000)).expect("admit"));
        let done = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(READERS + 1));

        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let level = Arc::clone(&level);
                let done = Arc::clone(&done);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let mut last = u64::MAX;
                    // `Acquire` pairs with the matcher's `Release` store so the
                    // loop ends after the last fill.
                    while !done.load(Ordering::Acquire) {
                        let Some(view) = level.iter_orders().next() else {
                            continue;
                        };
                        let seen = view.visible_quantity().as_u64();
                        for _ in 0..16 {
                            assert_eq!(view.visible_quantity().as_u64(), seen);
                            std::hint::spin_loop();
                        }
                        assert!(seen <= last, "a view went backwards");
                        last = seen;
                    }
                })
            })
            .collect();

        let generator = generator();
        barrier.wait();
        for i in 0..FILLS {
            let result = take(&level, 1, 100 + i, &generator);
            assert_eq!(result.trades().len(), 1);
        }
        done.store(true, Ordering::Release);
        for reader in readers {
            reader.join().expect("reader");
        }

        assert_eq!(level.visible_quantity(), 1_000_000 - FILLS);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn test_concurrent_cancel_fully_wins_or_loses_against_partial_fills() {
        const FILLS: u64 = 500;
        const ORIGINAL: u64 = 1_000_000;
        for round in 0..20u64 {
            let level = Arc::new(PriceLevel::new(PRICE));
            drop(level.add_order(standard(1, ORIGINAL)).expect("admit"));
            let barrier = Arc::new(Barrier::new(2));

            let canceller = {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..round * 10 {
                        std::hint::spin_loop();
                    }
                    level
                        .update_order(OrderUpdate::Cancel {
                            order_id: Id::from_u64(1),
                        })
                        .expect("cancel")
                })
            };

            let generator = generator();
            barrier.wait();
            let mut traded = 0u64;
            for i in 0..FILLS {
                let result = take(&level, 1, 1_000 + i, &generator);
                traded += result.executed_quantity().expect("sum").as_u64();
            }
            let cancelled = canceller.join().expect("canceller");
            let cancelled = cancelled.map_or(0, |o| o.visible_quantity().as_u64());

            assert_eq!(level.order_count(), 0, "the cancel is never lost");
            assert_eq!(level.visible_quantity(), 0);
            assert_eq!(traded + cancelled, ORIGINAL, "quantity is conserved");
        }
    }
}
