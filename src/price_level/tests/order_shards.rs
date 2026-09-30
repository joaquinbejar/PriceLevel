//! Issue #224: `PriceLevel::with_order_shards`. The shard count is validated
//! before it reaches `DashMap` (a power of two in `2 ..= 1024`, a typed
//! error otherwise), and a level with any accepted count behaves exactly like
//! one from `PriceLevel::new`.

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): raw arithmetic is
// permitted inside `mod tests` per the Testing section of
// `rules/global_rules.md`. Production code outside this module keeps the
// full deny list.
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::PriceLevelError;
    use crate::execution::{MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const TAKER: u64 = 999;

    fn standard(id: u64, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn take(level: &PriceLevel, quantity: u64) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(TAKER),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &UuidGenerator::new(Uuid::nil()),
        )
    }

    fn assert_rejected(shards: usize) {
        match PriceLevel::with_order_shards(PRICE, shards) {
            Err(PriceLevelError::InvalidFieldValue { field, value }) => {
                assert_eq!(field, "order_shards");
                assert!(
                    value.starts_with(&format!("{shards} ")),
                    "value should echo the rejected count: {value}"
                );
            }
            Err(other) => panic!("shards={shards}: unexpected error {other:?}"),
            Ok(_) => panic!("shards={shards}: expected InvalidFieldValue"),
        }
    }

    #[test]
    fn test_with_order_shards_zero_invalid_field_value() {
        assert_rejected(0);
    }

    #[test]
    fn test_with_order_shards_one_invalid_field_value() {
        assert_rejected(1);
    }

    #[test]
    fn test_with_order_shards_not_power_of_two_invalid_field_value() {
        assert_rejected(3);
    }

    #[test]
    fn test_with_order_shards_above_maximum_invalid_field_value() {
        assert_rejected(2048);
    }

    /// Adds three makers, fills the first and half the second, cancels the
    /// third, and returns the match result and the level's snapshot, both as
    /// JSON.
    fn exercise(level: &PriceLevel) -> (MatchResult, String, String) {
        for id in 1..=3 {
            level
                .add_order(standard(id, 10))
                .expect("add_order should succeed");
        }
        let result = take(level, 15);
        let removed = level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(3),
            })
            .expect("cancel should succeed");
        assert_eq!(removed.map(|order| order.id()), Some(Id::from_u64(3)));
        let result_json = serde_json::to_string(&result).expect("serialize MatchResult");
        let json = level
            .snapshot_to_json()
            .expect("snapshot_to_json should succeed");
        (result, result_json, json)
    }

    #[test]
    fn test_with_order_shards_bounds_behave_like_new() {
        let (expected_result, expected_result_json, expected_json) =
            exercise(&PriceLevel::new(PRICE));
        assert_eq!(expected_result.trades().len(), 2);
        assert!(expected_result.is_complete());

        for shards in [2, 1024] {
            let level = PriceLevel::with_order_shards(PRICE, shards)
                .expect("a power of two in range is accepted");
            assert_eq!(level.price(), PRICE);
            assert_eq!(level.order_count(), 0);

            let (_, result_json, json) = exercise(&level);
            assert_eq!(result_json, expected_result_json, "shards={shards}");
            assert_eq!(json, expected_json, "shards={shards}");
            assert_eq!(level.order_count(), 1, "shards={shards}");
            assert_eq!(level.visible_quantity(), 5, "shards={shards}");

            let restored = PriceLevel::from_snapshot_json(&json).expect("restore should succeed");
            assert_eq!(
                restored.snapshot_to_json().expect("snapshot_to_json"),
                json,
                "shards={shards}: a restored level (default shards) is equivalent"
            );
        }
    }

    #[test]
    fn test_with_order_shards_two_concurrent_add_cancel_consistent() {
        const THREADS: u64 = 4;
        const ORDERS_PER_THREAD: u64 = 200;
        const QTY: u64 = 7;

        let level =
            Arc::new(PriceLevel::with_order_shards(PRICE, 2).expect("2 shards is a valid count"));
        let barrier = Arc::new(Barrier::new(
            usize::try_from(THREADS).expect("thread count fits usize"),
        ));
        let handles: Vec<_> = (0..THREADS)
            .map(|thread_id| {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let base = thread_id * ORDERS_PER_THREAD;
                    for i in 0..ORDERS_PER_THREAD {
                        level
                            .add_order(standard(base + i + 1, QTY))
                            .expect("add_order should succeed");
                        // After every second add, cancel the order added
                        // just before it (an odd id), interleaving adds and
                        // cancels; the even ids survive.
                        if i % 2 == 1 {
                            let removed = level
                                .update_order(OrderUpdate::Cancel {
                                    order_id: Id::from_u64(base + i),
                                })
                                .expect("cancel should succeed");
                            assert!(removed.is_some(), "own order must still rest");
                        }
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("worker thread panicked");
        }

        let survivors = THREADS * ORDERS_PER_THREAD / 2;
        assert_eq!(level.order_count() as u64, survivors);
        assert_eq!(level.visible_quantity(), survivors * QTY);
        let orders = level.snapshot_orders().expect("snapshot_orders");
        assert_eq!(orders.len() as u64, survivors);
        let resting: HashSet<Id> = orders.iter().map(|order| order.id()).collect();
        let expected: HashSet<Id> = (1..=THREADS * ORDERS_PER_THREAD)
            .filter(|id| id % 2 == 0)
            .map(Id::from_u64)
            .collect();
        assert_eq!(resting, expected);
    }
}
