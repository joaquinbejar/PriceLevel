#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::execution::{MatchOutcome, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, PegReferenceType, Side, TimeInForce};
    use crate::price_level::PriceLevelSnapshotPackage;
    use crate::price_level::level::{PriceLevel, PriceLevelData};
    use crate::price_level::snapshot::SNAPSHOT_FORMAT_VERSION;
    use crate::utils::{Price, Quantity, TimestampMs};
    use crate::{DEFAULT_RESERVE_REPLENISH_AMOUNT, UuidGenerator};
    use std::num::NonZeroU64;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tracing::error;
    use uuid::Uuid;

    // Shared timestamp counter for all order creation functions to ensure proper ordering
    static TIMESTAMP_COUNTER: AtomicU64 = AtomicU64::new(1616823000000);

    // Helper functions to create different order types for testing
    pub fn create_standard_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let order_id = Id::from_u64(id);
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::Standard {
            id: order_id,
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    #[test]
    fn test_price_level_snapshot_roundtrip() {
        let price_level = PriceLevel::new(10000);
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(2, 10000, 50, 200))
            .expect("add_order should succeed");

        let package = price_level
            .snapshot_package()
            .expect("Failed to create snapshot package");

        assert_eq!(package.version(), SNAPSHOT_FORMAT_VERSION);
        package.validate().expect("Snapshot validation failed");

        let json = package
            .to_json()
            .expect("Failed to serialize snapshot package");
        let restored = PriceLevel::from_snapshot_json(&json)
            .expect("Failed to restore price level from snapshot JSON");

        assert_eq!(restored.price(), price_level.price());
        assert_eq!(restored.visible_quantity(), price_level.visible_quantity());
        assert_eq!(restored.hidden_quantity(), price_level.hidden_quantity());
        assert_eq!(restored.order_count(), price_level.order_count());

        let original_ids: Vec<Id> = price_level
            .snapshot_orders()
            .iter()
            .map(|order| order.id())
            .collect();
        let restored_ids: Vec<Id> = restored
            .snapshot_orders()
            .iter()
            .map(|order| order.id())
            .collect();
        assert_eq!(restored_ids, original_ids);
    }

    #[test]
    fn test_price_level_snapshot_checksum_failure() {
        let price_level = PriceLevel::new(20000);
        price_level
            .add_order(create_standard_order(1, 20000, 100))
            .expect("add_order should succeed");

        let package = price_level
            .snapshot_package()
            .expect("Failed to create snapshot package");

        package.validate().expect("Snapshot validation should pass");

        // Corrupt the checksum via JSON manipulation and ensure validation fails
        let json = package.to_json().expect("Failed to serialize package");
        let mut value: serde_json::Value =
            serde_json::from_str(&json).expect("JSON parsing failed");
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "checksum".to_string(),
                serde_json::Value::String("deadbeef".to_string()),
            );
        }
        let tampered_json = serde_json::to_string(&value).expect("JSON serialization failed");
        let tampered_package = PriceLevelSnapshotPackage::from_json(&tampered_json)
            .expect("Deserialization should still succeed");

        let err = PriceLevel::from_snapshot_package(tampered_package)
            .expect_err("Restoration should fail due to checksum mismatch");

        assert!(matches!(err, PriceLevelError::ChecksumMismatch { .. }));
    }

    #[test]
    fn test_price_level_snapshot_roundtrip_preserves_statistics() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        // Rest several makers so we accumulate non-trivial waiting-time and
        // arrival aggregates, then partially match them to drive every counter.
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(3, 10000, 100))
            .expect("add_order should succeed");

        // Execution timestamp is comfortably after the order arrival timestamps
        // (TIMESTAMP_COUNTER starts at 1_616_823_000_000), so waiting times are
        // positive and deterministic.
        let execution_ts = TimestampMs::new(1_716_000_000_000);

        // First match: fully consumes maker 1 and partially maker 2.
        let _ = price_level.match_order(
            150,
            Id::from_u64(900),
            TimeInForce::Gtc,
            TakerKind::Standard,
            execution_ts,
            &trade_id_generator,
        );
        // Second match: consumes the rest of maker 2 and part of maker 3.
        let _ = price_level.match_order(
            60,
            Id::from_u64(901),
            TimeInForce::Gtc,
            TakerKind::Standard,
            execution_ts,
            &trade_id_generator,
        );

        let stats = price_level.stats();
        // Sanity: stats are genuinely non-zero before we snapshot.
        assert!(stats.orders_added() >= 3);
        assert!(stats.orders_executed() > 0);
        assert!(stats.quantity_executed() > 0);
        assert!(stats.value_executed() > 0);
        assert!(stats.average_waiting_time().is_some());

        let json = price_level
            .snapshot_to_json()
            .expect("Failed to serialize snapshot to JSON");

        let restored = PriceLevel::from_snapshot_json(&json)
            .expect("Failed to restore price level from snapshot JSON");

        let restored_stats = restored.stats();

        // Every persisted statistic must survive the round-trip identically.
        assert_eq!(restored_stats.orders_added(), stats.orders_added());
        assert_eq!(restored_stats.orders_removed(), stats.orders_removed());
        assert_eq!(restored_stats.orders_executed(), stats.orders_executed());
        assert_eq!(
            restored_stats.quantity_executed(),
            stats.quantity_executed()
        );
        assert_eq!(restored_stats.value_executed(), stats.value_executed());
        assert_eq!(
            restored_stats.average_waiting_time(),
            stats.average_waiting_time()
        );
        assert_eq!(
            restored_stats.average_execution_price(),
            stats.average_execution_price()
        );
        // The raw timestamp / waiting-time aggregates round-trip exactly too.
        assert_eq!(
            restored_stats.last_execution_time(),
            stats.last_execution_time()
        );
        assert_eq!(
            restored_stats.first_arrival_time(),
            stats.first_arrival_time()
        );
        assert_eq!(restored_stats.sum_waiting_time(), stats.sum_waiting_time());
    }

    #[test]
    fn test_price_level_from_snapshot_json_v1_package_rejected() {
        // Build a current (v2) package, then downgrade its `version` to 1 to
        // emulate a snapshot written by a pre-#63 release. Restoration must fail
        // with a clear version mismatch (InvalidOperation), not a checksum error
        // and not a panic.
        let price_level = PriceLevel::new(10000);
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let json = price_level
            .snapshot_to_json()
            .expect("Failed to serialize snapshot to JSON");

        let mut value: serde_json::Value =
            serde_json::from_str(&json).expect("JSON parsing failed");
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "version".to_string(),
                serde_json::Value::Number(serde_json::Number::from(1u32)),
            );
        }
        let downgraded_json = serde_json::to_string(&value).expect("JSON serialization failed");

        let err = PriceLevel::from_snapshot_json(&downgraded_json)
            .expect_err("Restoration should reject a v1 package");

        match err {
            PriceLevelError::InvalidOperation { message } => {
                assert!(
                    message.contains("Unsupported snapshot version"),
                    "unexpected message: {message}"
                );
                assert!(message.contains('1'));
                assert!(message.contains('2'));
            }
            other => panic!("expected InvalidOperation version mismatch, got {other:?}"),
        }
    }

    #[test]
    fn test_price_level_from_snapshot_preserves_order_positions() {
        let price_level = PriceLevel::new(15000);
        price_level
            .add_order(create_standard_order(1, 15000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(2, 15000, 40, 120))
            .expect("add_order should succeed");
        price_level
            .add_order(create_post_only_order(3, 15000, 60))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_reserve_order(
                4,
                15000,
                30,
                90,
                15,
                true,
                Some(20),
            ))
            .expect("add_order should succeed");

        let snapshot = price_level.snapshot();
        let restored = PriceLevel::try_from(&snapshot).expect("valid snapshot restores");

        let original_orders = price_level.snapshot_orders();
        let restored_orders = restored.snapshot_orders();

        assert_eq!(restored_orders.len(), original_orders.len());
        assert_eq!(restored.order_count(), price_level.order_count());
        assert_eq!(restored.visible_quantity(), price_level.visible_quantity());
        assert_eq!(restored.hidden_quantity(), price_level.hidden_quantity());

        for (index, (expected, actual)) in original_orders
            .iter()
            .zip(restored_orders.iter())
            .enumerate()
        {
            assert_eq!(
                actual.id(),
                expected.id(),
                "Order mismatch at position {index}"
            );
            assert_eq!(actual.timestamp(), expected.timestamp());
        }
    }

    #[test]
    fn test_price_level_from_snapshot_package_preserves_order_positions() {
        let price_level = PriceLevel::new(17500);
        price_level
            .add_order(create_standard_order(10, 17500, 80))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_trailing_stop_order(11, 17500, 50))
            .expect("add_order should succeed");
        price_level
            .add_order(create_pegged_order(12, 17500, 40))
            .expect("add_order should succeed");
        price_level
            .add_order(create_market_to_limit_order(13, 17500, 70))
            .expect("add_order should succeed");

        let package = price_level
            .snapshot_package()
            .expect("Failed to create snapshot package");
        let restored = PriceLevel::from_snapshot_package(package)
            .expect("Failed to restore price level from snapshot package");

        let original_orders = price_level.snapshot_orders();
        let restored_orders = restored.snapshot_orders();

        assert_eq!(restored_orders.len(), original_orders.len());
        assert_eq!(restored.order_count(), price_level.order_count());

        for (index, (expected, actual)) in original_orders
            .iter()
            .zip(restored_orders.iter())
            .enumerate()
        {
            assert_eq!(
                actual.id(),
                expected.id(),
                "Order mismatch at position {index}"
            );
            assert_eq!(actual.timestamp(), expected.timestamp());
        }
    }

    fn create_iceberg_order(id: u64, price: u128, visible: u64, hidden: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::IcebergOrder {
            id: Id::from_u64(id),
            price: Price::new(price),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn create_post_only_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::PostOnly {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn create_trailing_stop_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::TrailingStop {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            trail_amount: Quantity::new(100),
            last_reference_price: Price::new(price + 100u128),
            extra_fields: (),
        }
    }

    fn create_pegged_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::PeggedOrder {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            reference_price_offset: -50,
            reference_price_type: PegReferenceType::BestAsk,
            extra_fields: (),
        }
    }

    fn create_market_to_limit_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::MarketToLimit {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn create_reserve_order(
        id: u64,
        price: u128,
        visible: u64,
        hidden: u64,
        threshold: u64,
        auto_replenish: bool,
        replenish_amount: Option<u64>,
    ) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::ReserveOrder {
            id: Id::from_u64(id),
            price: Price::new(price),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(threshold),
            replenish_amount: replenish_amount
                .map(|amount| NonZeroU64::new(amount).expect("test replenish amount must be > 0")),
            auto_replenish,
            extra_fields: (),
        }
    }

    // Buy-side variants of the Sell-defaulting helpers, for tests that mix
    // several order types at one level (issue #120: a level holds a single
    // side, so every maker in these tests must share it).
    fn create_buy_iceberg_order(id: u64, price: u128, visible: u64, hidden: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::IcebergOrder {
            id: Id::from_u64(id),
            price: Price::new(price),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn create_buy_trailing_stop_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::TrailingStop {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            trail_amount: Quantity::new(100),
            last_reference_price: Price::new(price + 100u128),
            extra_fields: (),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_buy_reserve_order(
        id: u64,
        price: u128,
        visible: u64,
        hidden: u64,
        threshold: u64,
        auto_replenish: bool,
        replenish_amount: Option<u64>,
    ) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::ReserveOrder {
            id: Id::from_u64(id),
            price: Price::new(price),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(threshold),
            replenish_amount: replenish_amount
                .map(|amount| NonZeroU64::new(amount).expect("test replenish amount must be > 0")),
            auto_replenish,
            extra_fields: (),
        }
    }

    fn create_fill_or_kill_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Fok,
            extra_fields: (),
        }
    }

    fn create_immediate_or_cancel_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Ioc,
            extra_fields: (),
        }
    }

    fn create_good_till_date_order(
        id: u64,
        price: u128,
        quantity: u64,
        expiry: u64,
    ) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtd(expiry),
            extra_fields: (),
        }
    }

    #[test]
    fn test_price_level_creation() {
        let price_level = PriceLevel::new(10000);

        assert_eq!(price_level.price(), 10000);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert!(matches!(price_level.total_quantity(), Ok(0)));

        // Test the statistics are properly initialized
        let stats = price_level.stats();
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
    }

    #[test]
    fn test_add_standard_order() {
        let price_level = PriceLevel::new(10000);
        let order = create_standard_order(1, 10000, 100);

        let order_arc = price_level
            .add_order(order)
            .expect("add_order should succeed");

        assert_eq!(price_level.visible_quantity(), 100);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 1);
        assert!(matches!(price_level.total_quantity(), Ok(100)));

        // Verify the returned Arc contains the expected order
        assert_eq!(order_arc.id(), Id::from_u64(1));
        assert_eq!(order_arc.price(), Price::new(10000));
        assert_eq!(order_arc.visible_quantity().as_u64(), 100);

        // Verify stats
        assert_eq!(price_level.stats().orders_added(), 1);
    }

    #[test]
    fn test_add_iceberg_order() {
        let price_level = PriceLevel::new(10000);
        let order = create_iceberg_order(2, 10000, 50, 200);

        price_level
            .add_order(order)
            .expect("add_order should succeed");

        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.hidden_quantity(), 200);
        assert_eq!(price_level.order_count(), 1);
        assert!(matches!(price_level.total_quantity(), Ok(250)));
    }

    #[test]
    fn test_add_multiple_orders() {
        let price_level = PriceLevel::new(10000);

        // Add different order types
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(2, 10000, 50, 200))
            .expect("add_order should succeed");
        price_level
            .add_order(create_post_only_order(3, 10000, 75))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_reserve_order(4, 10000, 25, 100, 100, true, None))
            .expect("add_order should succeed");

        assert_eq!(price_level.visible_quantity(), 250); // 100 + 50 + 75 + 25
        assert_eq!(price_level.hidden_quantity(), 300); // 0 + 200 + 0 + 100
        assert_eq!(price_level.order_count(), 4);
        assert!(matches!(price_level.total_quantity(), Ok(550)));

        // Verify stats
        assert_eq!(price_level.stats().orders_added(), 4);
    }

    #[test]
    fn test_update_order_cancel() {
        let price_level = PriceLevel::new(10000);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(2, 10000, 50, 200))
            .expect("add_order should succeed");

        // Cancel the standard order using OrderUpdate
        let result = price_level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(1),
        });

        assert!(result.is_ok());
        let removed = result.unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id(), Id::from_u64(1));
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.hidden_quantity(), 200);
        assert_eq!(price_level.order_count(), 1);

        // Cancel the iceberg order
        let result = price_level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(2),
        });

        assert!(result.is_ok());
        let removed = result.unwrap();
        assert!(removed.is_some());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);

        // Try to cancel a non-existent order
        let result = price_level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(3),
        });

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // Verify stats
        assert_eq!(price_level.stats().orders_added(), 2);
        assert_eq!(price_level.stats().orders_removed(), 2);
    }

    #[test]
    fn test_iter_orders() {
        let price_level = PriceLevel::new(10000);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(2, 10000, 50, 200))
            .expect("add_order should succeed");

        let orders = price_level.snapshot_orders();

        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].id(), Id::from_u64(1));
        assert_eq!(orders[1].id(), Id::from_u64(2));

        // Verify the orders are still in the queue after iteration
        assert_eq!(price_level.order_count(), 2);
    }

    #[test]
    fn test_match_standard_order_full() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Match the entire order
        let taker_id = Id::from_u64(999); // market order ID
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);

        assert_eq!(match_result.trades().len(), 1);
        let transaction = &match_result.trades().as_vec()[0];
        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(100));
        assert_eq!(transaction.taker_side(), Side::Sell); // Taker is a market order, so it's a sell side opposite of maker

        assert_eq!(match_result.filled_order_ids().len(), 1);
        assert_eq!(match_result.filled_order_ids()[0], Id::from_u64(1));

        // Verify stats
        assert_eq!(price_level.stats().orders_executed(), 1);
        assert_eq!(price_level.stats().quantity_executed(), 100);
        assert_eq!(price_level.stats().value_executed(), 1000000); // 100 * 10000
    }

    #[test]
    fn test_match_order_multi_maker_deterministic_timestamps() {
        // Matching the same input twice with the same threaded timestamp must
        // yield byte-identical trade streams — including each trade's timestamp,
        // trade_id, and quantity. This guarantees a replayable trade stream and
        // proves the match path never reads the wall clock.
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let taker_id = Id::from_u64(999);
        let timestamp = TimestampMs::new(1_716_000_000_000);

        // Build makers with FIXED timestamps so both runs use truly identical
        // input. `create_standard_order` draws from a global counter that
        // advances on every call, which would differ between the two runs.
        let mk = |id: u64, qty: u64| OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(10000),
            quantity: Quantity::new(qty),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_700_000_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };

        // Run a scenario that crosses several resting makers (partial fill of
        // the last maker) so the trade stream has multiple entries.
        let run = || {
            let price_level = PriceLevel::new(10000);
            price_level
                .add_order(mk(1, 40))
                .expect("add_order should succeed");
            price_level
                .add_order(mk(2, 30))
                .expect("add_order should succeed");
            price_level
                .add_order(mk(3, 50))
                .expect("add_order should succeed");

            let trade_id_generator = UuidGenerator::new(namespace);
            price_level.match_order(
                90,
                taker_id,
                TimeInForce::Gtc,
                TakerKind::Standard,
                timestamp,
                &trade_id_generator,
            )
        };

        let first = run();
        let second = run();

        let first_trades = first.trades().as_vec();
        let second_trades = second.trades().as_vec();

        // Crossed two full makers (40 + 30) and partially filled the third (20).
        assert_eq!(first_trades.len(), 3);
        assert_eq!(first.executed_quantity().unwrap_or_default().as_u64(), 90);
        assert_eq!(second.executed_quantity().unwrap_or_default().as_u64(), 90);

        // Byte-identical trade streams (Trade derives PartialEq over every
        // field, including the timestamp).
        assert_eq!(first_trades, second_trades);

        // Explicitly assert each trade's timestamp is exactly the threaded one.
        for trade in first_trades {
            assert_eq!(trade.timestamp(), timestamp);
        }
    }

    #[test]
    fn test_match_standard_order_partial() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Match part of the order
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            60,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Verificar el resultado de matching
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.order_count(), 1);

        // Verificar las transacciones generadas
        assert_eq!(match_result.trades().len(), 1);
        let transaction = &match_result.trades().as_vec()[0];
        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(60));
        assert_eq!(transaction.taker_side(), Side::Sell);

        // Verificar que no hay órdenes completadas
        assert_eq!(match_result.filled_order_ids().len(), 0);

        // Verify stats
        assert_eq!(price_level.stats().orders_executed(), 1);
        assert_eq!(price_level.stats().quantity_executed(), 60);
    }

    #[test]
    fn test_match_standard_order_excess() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Match with quantity exceeding available
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            150,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity().as_u64(), 50); // 150 - 100 = 50 remaining
        assert!(!match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);

        assert_eq!(match_result.trades().len(), 1);
        let transaction = &match_result.trades().as_vec()[0];
        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(100));

        assert_eq!(match_result.filled_order_ids().len(), 1);
        assert_eq!(match_result.filled_order_ids()[0], Id::from_u64(1));
    }

    // ------------------------------------------- ICEBERG ORDERS -------------------------------------------

    #[test]
    /// This test verifies the matching behavior of iceberg orders within a `PriceLevel`.
    /// It focuses on how the visible and hidden quantities are updated during matching,
    /// and how transactions are generated.  It also checks the state of the `PriceLevel`
    /// after each match, including visible/hidden quantities and the number of orders.
    fn test_match_iceberg_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Add a new iceberg order with a visible quantity of 50 and a hidden quantity of 100.
        price_level
            .add_order(create_iceberg_order(1, 10000, 50, 100))
            .expect("add_order should succeed");

        // Match the visible portion of the iceberg order.
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Assertions to validate the match result.
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.hidden_quantity(), 50); // Hidden quantity reduced
        assert_eq!(price_level.order_count(), 1);
        assert_eq!(match_result.trades().len(), 1);

        // Assertions about the generated transaction
        let transaction = &match_result.trades().as_vec()[0];
        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(50));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(match_result.filled_order_ids().len(), 0);

        // Match another 50 units, which should deplete the visible portion and reveal more.
        let taker_id = Id::from_u64(1000);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50); // Visible quantity replenished
        assert_eq!(price_level.hidden_quantity(), 0); // Hidden quantity reduced
        assert_eq!(price_level.order_count(), 1);
        let transaction = &match_result.trades().as_vec()[0];

        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(50));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(match_result.filled_order_ids().len(), 0);

        // Match the remaining 50 units (50 visible + 0 hidden).
        let taker_id = Id::from_u64(1001);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(match_result.filled_order_ids().len(), 1);
        assert_eq!(match_result.filled_order_ids()[0], Id::from_u64(1));
    }

    #[test]
    fn test_match_iceberg_order_overlapping() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Add a new iceberg order with a visible quantity of 50 and a hidden quantity of 100.
        price_level
            .add_order(create_iceberg_order(1, 10000, 100, 100))
            .expect("add_order should succeed");

        // Match the visible portion of the iceberg order.
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Assertions to validate the match result.
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.hidden_quantity(), 100); // Hidden quantity reduced
        assert_eq!(price_level.order_count(), 1);
        assert_eq!(match_result.trades().len(), 1);

        // Assertions about the generated transaction
        let transaction = &match_result.trades().as_vec()[0];
        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(50));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(match_result.filled_order_ids().len(), 0);

        // Match another 50 units, which should deplete the visible portion and reveal more.
        let taker_id = Id::from_u64(1000);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50); // Visible quantity replenished
        assert_eq!(price_level.hidden_quantity(), 50); // Hidden quantity reduced
        assert_eq!(price_level.order_count(), 1);
        let transaction = &match_result.trades().as_vec()[0];

        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(50));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(match_result.filled_order_ids().len(), 0);

        // Match the remaining 50 units (50 visible + 0 hidden).
        let taker_id = Id::from_u64(1001);

        // This should match the remaining visible quantity and deplete the hidden quantity.
        let match_result = price_level.match_order(
            150,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity().as_u64(), 50);
        assert!(!match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(match_result.filled_order_ids().len(), 1);
        assert_eq!(match_result.filled_order_ids()[0], Id::from_u64(1));
    }

    #[test]
    fn test_match_iceberg_order_partial_visible() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_iceberg_order(1, 10000, 50, 150))
            .expect("add_order should succeed");

        // Match part of the visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            30,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 20);
        assert_eq!(price_level.hidden_quantity(), 150); // Hidden unchanged
        assert_eq!(price_level.order_count(), 1);
    }

    // ------------------------------------------- RESERVE ORDERS -------------------------------------------

    #[test]
    /// Tests the behavior of a Reserve Order with auto-replenish disabled.
    /// When the visible quantity is consumed completely, the order should be removed
    /// from the price level even if there is remaining hidden quantity.
    fn test_match_reserve_order_no_auto_replenish() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with auto-replenish disabled
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 150, 20, false, None))
            .expect("add_order should succeed");

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // The order should be removed since the visible quantity reached 0 and auto_replenish is false
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    /// Tests the behavior of a Reserve Order with auto-replenish enabled.
    /// When the visible quantity is fully consumed, the order should automatically
    /// replenish from the hidden quantity.
    fn test_match_reserve_order_with_auto_replenish() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with auto-replenish enabled
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 150, 20, true, None))
            .expect("add_order should succeed");

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // The order should be replenished with the default amount
        assert_eq!(
            price_level.visible_quantity(),
            DEFAULT_RESERVE_REPLENISH_AMOUNT.get()
        );
        assert_eq!(
            price_level.hidden_quantity(),
            150 - DEFAULT_RESERVE_REPLENISH_AMOUNT.get()
        );
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    /// Tests partial matching of a Reserve Order with auto-replenish disabled.
    /// Verifies that the visible quantity decreases correctly and there is no automatic
    /// replenishment even when falling below the threshold.
    fn test_match_reserve_order_partial_no_replenish() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with auto-replenish disabled
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 150, 20, false, None))
            .expect("add_order should succeed");

        // Match partially, but still above threshold
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            25,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 25); // 50 - 25 = 25
        assert_eq!(price_level.hidden_quantity(), 150); // No change to hidden quantity

        // Match more to go below threshold
        let taker_id = Id::from_u64(1000);
        let match_result = price_level.match_order(
            10,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // No automatic replenishment because auto_replenish is false
        assert_eq!(price_level.visible_quantity(), 15); // 25 - 10 = 15, no replenishment
        assert_eq!(price_level.hidden_quantity(), 150); // No change to hidden quantity
    }

    #[test]
    /// Tests a Reserve Order with a custom replenishment amount.
    /// When the visible quantity is fully consumed, the order should replenish
    /// using the specified custom amount rather than the default.
    fn test_match_reserve_order_with_custom_replenish_amount() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with auto-replenish enabled and a custom replenishment amount
        let custom_amount = 50;
        price_level
            .add_order(create_reserve_order(
                1,
                10000,
                50,
                150,
                20,
                true,
                Some(custom_amount),
            ))
            .expect("add_order should succeed");

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // The order should be replenished with the custom amount
        assert_eq!(price_level.visible_quantity(), custom_amount);
        assert_eq!(price_level.hidden_quantity(), 150 - custom_amount);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    /// Tests a Reserve Order with threshold 0 and auto-replenish enabled.
    /// A threshold of 0 is treated as 1, but no replenishment should occur
    /// when visible quantity equals the threshold.
    fn test_match_reserve_order_with_zero_threshold() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with threshold 0 and auto-replenish enabled
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 150, 0, true, None))
            .expect("add_order should succeed");

        // Match partially
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            49,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // 1 visible unit will remain, which equals the safe threshold (1), so no replenishment occurs
        assert_eq!(price_level.visible_quantity(), 1);
        assert_eq!(price_level.hidden_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    /// Tests a Reserve Order with threshold 0 and auto-replenish disabled.
    /// The order should be removed from the book when visible quantity reaches 0.
    fn test_match_reserve_order_threshold_zero() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with threshold 0 and auto-replenish disabled
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 150, 0, false, None))
            .expect("add_order should succeed");

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // The order should be removed from the price level
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    /// Tests a Reserve Order with threshold 1 and auto-replenish disabled.
    /// The order should be removed from the book when visible quantity reaches 0.
    fn test_match_reserve_order_threshold_one() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with threshold 1 and auto-replenish disabled
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 150, 1, false, None))
            .expect("add_order should succeed");

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // The order should be removed from the price level
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    /// Tests a Reserve Order with a specific threshold and auto-replenish disabled.
    /// Verifies behavior when matching above and below the threshold.
    fn test_match_reserve_order_with_threshold() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with threshold 20 and auto-replenish disabled
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 150, 20, false, None))
            .expect("add_order should succeed");

        // Match part of the visible portion, but still above threshold
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            25,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 25); // 50 - 25 = 25
        assert_eq!(price_level.hidden_quantity(), 150); // No replenishment yet

        // Match more to go below threshold
        let taker_id = Id::from_u64(1000);
        let match_result = price_level.match_order(
            10,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        // No automatic replenishment because auto_replenish is false
        assert_eq!(price_level.visible_quantity(), 15); // 25 - 10 = 15
        assert_eq!(price_level.hidden_quantity(), 150); // No change to hidden quantity
    }

    #[test]
    /// Tests a comprehensive scenario with a Reserve Order including:
    /// 1. Matching above the threshold
    /// 2. Matching below the threshold with automatic replenishment
    /// 3. Matching with an amount larger than available
    ///    This test verifies correct transaction generation and order state throughout.
    fn test_match_reserve_order_overlapping() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Create a reserve order with threshold 20, auto-replenish enabled
        // and default replenish amount (80)
        price_level
            .add_order(create_reserve_order(1, 10000, 100, 100, 20, true, None))
            .expect("add_order should succeed");

        // Match 80 units, which is above the replenish threshold
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            80,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Validate the match result
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 20); // 100 - 80 = 20
        assert_eq!(price_level.hidden_quantity(), 100); // Hidden quantity unchanged (still above threshold)
        assert_eq!(price_level.order_count(), 1);
        assert_eq!(match_result.trades().len(), 1);

        // Validate the transaction details
        let transaction = &match_result.trades().as_vec()[0];
        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(80));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(match_result.filled_order_ids().len(), 0);

        // Match 10 more units, which will take us below the replenish threshold
        let taker_id = Id::from_u64(1000);
        let match_result = price_level.match_order(
            10,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 90); // 20 - 10 = 10, then replenished to 90 (10 + 80)
        assert_eq!(price_level.hidden_quantity(), 20); // 100 - 80 (replenish amount) = 20
        assert_eq!(price_level.order_count(), 1);

        let transaction = &match_result.trades().as_vec()[0];
        assert_eq!(transaction.taker_order_id(), taker_id);
        assert_eq!(transaction.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(10));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(match_result.filled_order_ids().len(), 0);

        // Match with a larger amount than what's available
        let taker_id = Id::from_u64(1001);
        let match_result = price_level.match_order(
            150,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 40); // 150 - 90 - 20 = 40
        assert!(!match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(match_result.filled_order_ids().len(), 1);
        assert_eq!(match_result.filled_order_ids()[0], Id::from_u64(1));

        // Verify the correct number and sizes of transactions
        assert_eq!(match_result.trades().len(), 2); // One for visible, one for hidden

        let transaction1 = &match_result.trades().as_vec()[0];
        assert_eq!(transaction1.taker_order_id(), taker_id);
        assert_eq!(transaction1.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction1.price(), Price::new(10000));
        assert_eq!(transaction1.quantity(), Quantity::new(90)); // First consumes all visible
        assert_eq!(transaction1.taker_side(), Side::Buy);

        let transaction2 = &match_result.trades().as_vec()[1];
        assert_eq!(transaction2.taker_order_id(), taker_id);
        assert_eq!(transaction2.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction2.price(), Price::new(10000));
        assert_eq!(transaction2.quantity(), Quantity::new(20)); // Then consumes all hidden
        assert_eq!(transaction2.taker_side(), Side::Buy);
    }

    // ------------------------------------------- POST-ONLY, TRAILING STOP, PEGGED, MARKET TO LIMIT, FOK, IOC, GTD ORDERS -------------------------------------------

    #[test]
    fn test_match_post_only_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_post_only_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Post-only orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            60,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_match_trailing_stop_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_trailing_stop_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Trailing stop orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_pegged_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_pegged_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Pegged orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_match_market_to_limit_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_market_to_limit_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Market-to-limit orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    // --------------------------------- ORDER-TYPE MATRIX (#78) ---------------------------------
    //
    // The matching rules require every order type to ship unit tests covering:
    // empty level, partial fill, full fill, and the type-specific branch.
    // Issue #77 added the {standard, iceberg, reserve} matrix and the
    // `assert_match_result_consistent` helper. This block fills the remaining
    // gaps for PostOnly / TrailingStop / PeggedOrder / MarketToLimit, plus the
    // `incoming_quantity == 0` boundary.
    //
    // IMPORTANT — these test RESTING MAKERS of each order type. Post-only,
    // market-to-limit, pegged, and trailing-stop are taker-side / order-book
    // policies; as *resting makers* these order types are plain liquidity and
    // are consumed FIFO exactly like a `Standard` order, at the level price. The
    // genuine taker-side semantics (post-only rejection, market-to-limit
    // conversion, fill-or-kill / IOC) live in `match_order` and are covered by
    // the "TAKER TIF / KIND SEMANTICS (#65)" block further below — those tests
    // vary the *taker's* intent, while these vary the *resting maker's* type.
    //
    // Maker sides (taken from each helper, which differ): PostOnly = Buy,
    // TrailingStop = Sell, PeggedOrder = Buy, MarketToLimit = Buy. The correct
    // side is threaded into `assert_match_result_consistent` so the trade
    // `taker_side` cross-check is the opposite of the KNOWN resting side.

    // ----- PostOnly (resting maker, side = Buy) -----

    #[test]
    fn test_match_post_only_partial_fill_taker_complete() {
        // Taker (60) smaller than the resting PostOnly maker (100): taker is
        // fully filled, maker is partially consumed and keeps resting.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_post_only_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            60,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.order_count(), 1);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_post_only_full_fill_maker_consumed() {
        // Taker exactly equals the resting PostOnly maker (100): the maker is
        // fully consumed and removed; the taker is complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_post_only_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(result.filled_order_ids()[0], Id::from_u64(1));
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_post_only_resting_maker_consumed_like_standard() {
        // Post-only is a TAKER-side policy: a PostOnly order resting as a
        // *maker* is just ordinary liquidity and is consumed exactly like a
        // `Standard` maker. (The real post-only rejection — a post-only TAKER
        // refusing to cross — is covered by the taker-side tests below.) An
        // over-large `Gtc` taker drains the PostOnly maker and leaves a
        // positive remainder.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_post_only_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            150,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        // Pass-through: maker fully consumed, 50 left over on the taker.
        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 50);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.trades().as_vec()[0].quantity(), Quantity::new(100));
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_post_only_empty_level_no_trades() {
        // Empty level: matching against a PostOnly-free, empty `PriceLevel`
        // yields no trades, the full incoming quantity remains, and the result
        // is not complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        let result = price_level.match_order(
            75,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 75);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
    }

    // ----- TrailingStop (resting maker, side = Sell) -----

    #[test]
    fn test_match_trailing_stop_partial_fill_taker_complete() {
        // Taker (40) smaller than the resting TrailingStop maker (100): taker
        // fully filled, maker partially consumed and still resting.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_trailing_stop_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            40,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(price_level.visible_quantity(), 60);
        assert_eq!(price_level.order_count(), 1);
        // TrailingStop helper rests on Side::Sell.
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_match_trailing_stop_full_fill_maker_consumed() {
        // Taker exactly equals the resting TrailingStop maker (100): maker
        // fully consumed and removed; taker complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_trailing_stop_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(result.filled_order_ids()[0], Id::from_u64(1));
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_match_trailing_stop_resting_maker_consumed_like_standard() {
        // A resting TrailingStop maker is matched as ordinary liquidity: trail
        // repricing is the order book's job, not the single-level match. An
        // over-large `Gtc` taker drains the maker at the level price and leaves
        // a positive remainder.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_trailing_stop_order(1, 10000, 80))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            120,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 40);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.trades().as_vec()[0].quantity(), Quantity::new(80));
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_match_trailing_stop_empty_level_no_trades() {
        // Empty level: no resting orders -> no trades, full remainder, not
        // complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        let result = price_level.match_order(
            55,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 55);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
    }

    // ----- PeggedOrder (resting maker, side = Buy) -----

    #[test]
    fn test_match_pegged_partial_fill_taker_complete() {
        // Taker (50) smaller than the resting PeggedOrder maker (100): taker
        // fully filled, maker partially consumed and still resting.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_pegged_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.order_count(), 1);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_pegged_full_fill_maker_consumed() {
        // Taker exactly equals the resting PeggedOrder maker (100): maker fully
        // consumed and removed; taker complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_pegged_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(result.filled_order_ids()[0], Id::from_u64(1));
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_pegged_resting_maker_consumed_like_standard() {
        // A resting PeggedOrder maker is matched as ordinary liquidity at the
        // level price; pegging to a reference price is the order book's job, not
        // the single-level match. An over-large `Gtc` taker drains the maker.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_pegged_order(1, 10000, 90))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            130,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 40);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.trades().as_vec()[0].quantity(), Quantity::new(90));
        // Pass-through fills at the level price, NOT a pegged reference price.
        assert_eq!(result.trades().as_vec()[0].price(), Price::new(10000));
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_pegged_empty_level_no_trades() {
        // Empty level: no resting orders -> no trades, full remainder, not
        // complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        let result = price_level.match_order(
            33,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 33);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
    }

    // ----- MarketToLimit (resting maker, side = Buy) -----

    #[test]
    fn test_match_market_to_limit_partial_fill_taker_complete() {
        // Taker (70) smaller than the resting MarketToLimit maker (100): taker
        // fully filled, maker partially consumed and still resting.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_market_to_limit_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            70,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(price_level.visible_quantity(), 30);
        assert_eq!(price_level.order_count(), 1);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_market_to_limit_full_fill_maker_consumed() {
        // Taker exactly equals the resting MarketToLimit maker (100): maker
        // fully consumed and removed; taker complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_market_to_limit_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(result.filled_order_ids()[0], Id::from_u64(1));
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_market_to_limit_resting_maker_consumed_like_standard() {
        // A resting MarketToLimit maker is matched as ordinary liquidity.
        // Market-to-limit is a TAKER-side policy (converting the taker's unfilled
        // remainder into a resting limit); as a maker it is consumed like a
        // `Standard` order. An over-large `Gtc` taker drains it.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_market_to_limit_order(1, 10000, 60))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 40);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.trades().as_vec()[0].quantity(), Quantity::new(60));
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_market_to_limit_empty_level_no_trades() {
        // Empty level: no resting orders -> no trades, full remainder, not
        // complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        let result = price_level.match_order(
            90,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 90);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
    }

    // --------------------------------- RESIDUAL CONSERVATION (#118) ---------------------------------
    //
    // `OrderType::with_reduced_quantity` used to no-op on TrailingStop /
    // PeggedOrder / MarketToLimit, so a partially-filled maker of one of those
    // types kept its ORIGINAL quantity: a later taker could execute the same
    // depth again, and repeated fills could drive the advisory visible counter
    // below zero. These drivers rest a single maker, partially fill it, then
    // prove the residual is the ONLY thing left — in the advisory counter, the
    // live queue, and a snapshot round-trip — and that a second, over-large
    // taker can take only that residual. The `empty / partial / full` matrix and
    // FIFO-position checks for these types live in the ORDER-TYPE MATRIX block
    // above; here we specifically pin quantity conservation across two takers.

    /// Rest `maker` (original size `original_qty`, known resting `side`),
    /// partially fill it with a taker of `first_take` (`< original_qty`), then
    /// assert the residual is exposed identically by the advisory
    /// `visible_quantity()` counter, the live `snapshot_by_insertion_seq()`
    /// queue, and a `snapshot_to_json` -> `from_snapshot_json` round-trip. A
    /// second, over-large taker must then execute ONLY the residual, so the
    /// total executed across both takers equals `original_qty` exactly — never
    /// more (quantity conservation, issue #118).
    fn assert_two_takers_conserve_quantity(
        maker: OrderType<()>,
        original_qty: u64,
        first_take: u64,
        side: Side,
    ) {
        let level_price = maker.price().as_u128();
        let maker_id = maker.id();
        let price_level = PriceLevel::new(level_price);
        price_level
            .add_order(maker)
            .expect("add_order should succeed");

        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        // First taker: strictly partial fill of the maker.
        let first = price_level.match_order(
            first_take,
            Id::from_u64(901),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );
        assert_eq!(
            first.executed_quantity().unwrap_or_default().as_u64(),
            first_take
        );
        assert_match_result_consistent(&first, level_price, side);

        let residual = original_qty - first_take;

        // Advisory counter, live queue contents, and a snapshot round-trip must
        // all expose the SAME residual on the stored maker.
        assert_eq!(price_level.visible_quantity(), residual);

        let resting = price_level.snapshot_by_insertion_seq();
        assert_eq!(resting.len(), 1);
        assert_eq!(resting[0].id(), maker_id);
        assert_eq!(
            resting[0].visible_quantity().as_u64(),
            residual,
            "the resting maker must carry exactly the residual, not its original size"
        );

        let json = price_level
            .snapshot_to_json()
            .expect("snapshot must serialize");
        let restored = PriceLevel::from_snapshot_json(&json).expect("snapshot must restore");
        let restored_orders = restored.snapshot_by_insertion_seq();
        assert_eq!(restored_orders.len(), 1);
        assert_eq!(
            restored_orders[0].visible_quantity().as_u64(),
            residual,
            "the residual must survive a snapshot round-trip"
        );
        assert_eq!(restored.visible_quantity(), residual);

        // Second, over-large taker: it can take only the residual, never the
        // maker's original size.
        let second = price_level.match_order(
            original_qty + 1000,
            Id::from_u64(902),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_001),
            &trade_id_generator,
        );
        assert_eq!(
            second.executed_quantity().unwrap_or_default().as_u64(),
            residual,
            "the second taker can only execute the residual"
        );
        assert_match_result_consistent(&second, level_price, side);

        let total = first.executed_quantity().unwrap_or_default().as_u64()
            + second.executed_quantity().unwrap_or_default().as_u64();
        assert_eq!(
            total, original_qty,
            "total executed across both takers must equal the maker's original quantity, never more"
        );

        // Maker fully consumed; level empty.
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
        assert!(price_level.snapshot_by_insertion_seq().is_empty());
    }

    #[test]
    fn test_match_trailing_stop_two_takers_second_only_takes_residual() {
        // TrailingStop rests on Side::Sell.
        assert_two_takers_conserve_quantity(
            create_trailing_stop_order(1, 10000, 100),
            100,
            40,
            Side::Sell,
        );
    }

    #[test]
    fn test_match_pegged_two_takers_second_only_takes_residual() {
        // PeggedOrder rests on Side::Buy.
        assert_two_takers_conserve_quantity(create_pegged_order(1, 10000, 100), 100, 55, Side::Buy);
    }

    #[test]
    fn test_match_market_to_limit_two_takers_second_only_takes_residual() {
        // MarketToLimit rests on Side::Buy.
        assert_two_takers_conserve_quantity(
            create_market_to_limit_order(1, 10000, 100),
            100,
            70,
            Side::Buy,
        );
    }

    /// `UpdateQuantity` DECREASE on a resizable maker: the maker is resized to
    /// exactly `decrease_to` and keeps its front queue position (issue #118 made
    /// TrailingStop / PeggedOrder / MarketToLimit resizable; the decrease branch
    /// preserves time priority).
    fn assert_update_quantity_decrease_keeps_position(front: OrderType<()>, decrease_to: u64) {
        let level_price = front.price().as_u128();
        let front_id = front.id();
        let level = PriceLevel::new(level_price);
        level.add_order(front).expect("add_order should succeed");
        // A plain maker queued behind the resized one.
        let behind_id = Id::from_u64(778);
        level
            .add_order(create_standard_order(778, level_price, 100))
            .expect("add_order should succeed");

        let updated = level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: front_id,
                new_quantity: Quantity::new(decrease_to),
            })
            .expect("decrease update should succeed")
            .expect("order must be present");
        assert_eq!(
            updated.visible_quantity().as_u64(),
            decrease_to,
            "decrease must resize the maker to exactly the new quantity"
        );

        // Decrease keeps priority: the resized maker is still first by insertion
        // sequence, ahead of the maker queued behind it.
        let by_seq: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|order| order.id())
            .collect();
        assert_eq!(
            by_seq,
            vec![front_id, behind_id],
            "a decreased maker keeps its front position"
        );
    }

    /// `UpdateQuantity` INCREASE on a resizable maker: the maker is resized to
    /// exactly `increase_to` (`> its original total`) and demoted to the back of
    /// the queue, behind a later maker (issue #118; increase forfeits time
    /// priority, matching the existing policy for Standard orders).
    fn assert_update_quantity_increase_demotes(front: OrderType<()>, increase_to: u64) {
        let level_price = front.price().as_u128();
        let front_id = front.id();
        let level = PriceLevel::new(level_price);
        level.add_order(front).expect("add_order should succeed");
        let behind_id = Id::from_u64(778);
        level
            .add_order(create_standard_order(778, level_price, 100))
            .expect("add_order should succeed");

        let updated = level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: front_id,
                new_quantity: Quantity::new(increase_to),
            })
            .expect("increase update should succeed")
            .expect("order must be present");
        assert_eq!(
            updated.visible_quantity().as_u64(),
            increase_to,
            "increase must resize the maker to exactly the new quantity"
        );

        // Increase demotes: the resized maker moves behind the later maker.
        let by_seq: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|order| order.id())
            .collect();
        assert_eq!(
            by_seq,
            vec![behind_id, front_id],
            "an increased maker is demoted to the back of the queue"
        );
    }

    #[test]
    fn test_update_quantity_trailing_stop_decrease_keeps_position() {
        // Buy side to match the plain maker the helper queues behind it (a level
        // holds a single side, issue #120).
        assert_update_quantity_decrease_keeps_position(
            create_buy_trailing_stop_order(1, 10000, 100),
            40,
        );
    }

    #[test]
    fn test_update_quantity_trailing_stop_increase_demotes() {
        assert_update_quantity_increase_demotes(create_buy_trailing_stop_order(1, 10000, 100), 150);
    }

    #[test]
    fn test_update_quantity_pegged_decrease_keeps_position() {
        assert_update_quantity_decrease_keeps_position(create_pegged_order(1, 10000, 100), 40);
    }

    #[test]
    fn test_update_quantity_pegged_increase_demotes() {
        assert_update_quantity_increase_demotes(create_pegged_order(1, 10000, 100), 150);
    }

    #[test]
    fn test_update_quantity_market_to_limit_decrease_keeps_position() {
        assert_update_quantity_decrease_keeps_position(
            create_market_to_limit_order(1, 10000, 100),
            40,
        );
    }

    #[test]
    fn test_update_quantity_market_to_limit_increase_demotes() {
        assert_update_quantity_increase_demotes(create_market_to_limit_order(1, 10000, 100), 150);
    }

    // ----- incoming_quantity == 0 boundary -----

    #[test]
    fn test_match_order_zero_incoming_quantity_no_trades_complete() {
        // Boundary: matching an incoming quantity of 0 against a populated
        // level. Observed engine behavior (level.rs `while remaining > 0`): the
        // sweep loop never runs, so no maker is touched, no trade is emitted,
        // `remaining_quantity()` stays 0 and `finalize(0)` therefore reports
        // `is_complete() == true` (a vacuous full fill: nothing to fill, so the
        // taker is trivially "complete"). The resting depth is left intact.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_post_only_order(2, 10000, 50))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            0,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        // No trades produced.
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
        // remaining == 0 and is_complete agree (vacuously complete).
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert!(result.is_complete());
        // executed_quantity is 0 and matches the (empty) trade sum.
        assert!(matches!(result.executed_quantity(), Ok(q) if q.as_u64() == 0));
        // Resting depth untouched: both makers still rest at full size.
        assert_eq!(price_level.order_count(), 2);
        assert_eq!(price_level.visible_quantity(), 150);
    }

    #[test]
    fn test_match_fill_or_kill_taker_fully_filled() {
        // FOK TAKER, sufficient depth: the level can fill the taker in full
        // (available == incoming == 100), so it fills completely like any other.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(match_result.outcome(), MatchOutcome::Filled);
        assert!(!match_result.was_killed());
        assert_eq!(match_result.trades().len(), 1);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_immediate_or_cancel_taker_fills_available_and_discards() {
        // IOC TAKER smaller than resting depth: fills fully, nothing discarded.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimeInForce::Ioc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(match_result.outcome(), MatchOutcome::Filled);
        // The maker keeps the unmatched 50 resting; the IOC taker is never
        // enqueued by this layer.
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.order_count(), 1);
    }

    // --------------------------------- TAKER TIF / KIND SEMANTICS (#65) --------
    //
    // `match_order` honors the taker's TimeInForce and TakerKind. These tests
    // pin the single-level semantics: FOK fills-completely-or-kills, IOC
    // fills-available-and-discards, PostOnly rejects on cross, MarketToLimit
    // fills-available. Resting makers are plain `Standard` Buy orders so the
    // only variable is the taker's intent.

    fn fok_namespace_gen() -> UuidGenerator {
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        UuidGenerator::new(namespace)
    }

    // ----- FOK boundary: fills completely or kills (both sides) -----

    #[test]
    fn test_match_fok_taker_exactly_fillable_fills_completely() {
        // available (100) == incoming (100): on the fill side of the boundary.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_standard_order(1, 10000, 60))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert!(!result.was_killed());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.executed_quantity().expect("ok").as_u64(), 100);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
    }

    #[test]
    fn test_match_fok_taker_one_short_is_killed() {
        // available (100) < incoming (101): on the kill side of the boundary by
        // exactly one unit. Zero trades, full remainder, queue untouched.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_standard_order(1, 10000, 60))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            101,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(!result.is_complete());
        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_eq!(result.remaining_quantity().as_u64(), 101);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(result.executed_quantity().expect("ok").as_u64(), 0);
        // No partial state: the resting depth is fully intact.
        assert_eq!(price_level.order_count(), 2);
        assert_eq!(price_level.visible_quantity(), 100);
    }

    #[test]
    fn test_match_fok_taker_killed_against_empty_level() {
        // Empty level cannot fill any positive FOK taker -> killed.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();

        let result = price_level.match_order(
            10,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_eq!(result.remaining_quantity().as_u64(), 10);
        assert_eq!(result.trades().len(), 0);
    }

    #[test]
    fn test_match_fok_taker_drains_iceberg_hidden_then_fills() {
        // `available` must count replenishable hidden depth the single sweep
        // would draw: an iceberg with visible 10 + hidden 40 can fill a FOK
        // taker of 50, so it fills rather than (wrongly) being killed.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_iceberg_order(1, 10000, 10, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(result.executed_quantity().expect("ok").as_u64(), 50);
        assert_eq!(price_level.order_count(), 0);
    }

    // ----- IOC: fills available and discards remainder -----

    #[test]
    fn test_match_ioc_taker_fills_available_and_discards_remainder() {
        // available (100) < incoming (150): fill 100, discard 50. The taker is
        // never enqueued; the level is emptied of makers.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_standard_order(1, 10000, 60))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            150,
            Id::from_u64(999),
            TimeInForce::Ioc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(!result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);
        assert!(!result.was_killed());
        assert!(!result.was_rejected());
        assert_eq!(result.executed_quantity().expect("ok").as_u64(), 100);
        assert_eq!(result.remaining_quantity().as_u64(), 50);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
    }

    // ----- PostOnly: rejects on cross -----

    #[test]
    fn test_match_post_only_taker_rejected_on_cross() {
        // The level has matchable depth, so a post-only taker would take
        // liquidity -> rejected: zero trades, full remainder, queue untouched.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            60,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.was_rejected());
        assert_eq!(result.outcome(), MatchOutcome::Rejected);
        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 60);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(result.executed_quantity().expect("ok").as_u64(), 0);
        // Resting maker untouched.
        assert_eq!(price_level.order_count(), 1);
        assert_eq!(price_level.visible_quantity(), 100);
    }

    #[test]
    fn test_match_post_only_taker_accepted_on_empty_level() {
        // No matchable depth -> the post-only taker does not cross and is NOT
        // rejected. It simply finds nothing to fill (NotFilled).
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();

        let result = price_level.match_order(
            60,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(!result.was_rejected());
        assert_eq!(result.outcome(), MatchOutcome::NotFilled);
        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 60);
        assert_eq!(result.trades().len(), 0);
    }

    #[test]
    fn test_match_post_only_taker_zero_quantity_not_rejected() {
        // A zero-quantity post-only taker has nothing to cross -> not rejected;
        // it falls through to the vacuous-complete sweep.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            0,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(!result.was_rejected());
        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(price_level.order_count(), 1);
        assert_eq!(price_level.visible_quantity(), 100);
    }

    // ----- MarketToLimit: fills available, reports remainder -----

    #[test]
    fn test_match_market_to_limit_taker_fills_available_reports_remainder() {
        // available (100) < incoming (130): fill 100, report 40 for the order
        // book to convert/rest. At this layer it behaves like a standard taker.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            140,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::MarketToLimit,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(!result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);
        assert!(!result.was_killed());
        assert!(!result.was_rejected());
        assert_eq!(result.executed_quantity().expect("ok").as_u64(), 100);
        assert_eq!(result.remaining_quantity().as_u64(), 40);
        assert_eq!(result.filled_order_ids().len(), 1);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_market_to_limit_taker_full_fill() {
        // available (100) == incoming (100): fully filled, no remainder.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::MarketToLimit,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.executed_quantity().expect("ok").as_u64(), 100);
    }

    // ----- resting FOK / IOC makers are consumed like any other liquidity -----

    #[test]
    fn test_match_resting_fok_maker_consumed_by_standard_taker() {
        // A resting maker tagged FOK is just liquidity here; a Gtc taker
        // consumes it normally. (FOK is a taker-side policy.)
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_fill_or_kill_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_resting_ioc_maker_partially_consumed_by_standard_taker() {
        // A resting maker tagged IOC is just liquidity; a smaller Gtc taker
        // partially consumes it and the remainder keeps resting.
        let price_level = PriceLevel::new(10000);
        let trade_gen = fok_namespace_gen();
        price_level
            .add_order(create_immediate_or_cancel_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_match_good_till_date_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_good_till_date_order(1, 10000, 100, 1617000000000))
            .expect("add_order should succeed");

        // GTD orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    /// A `Gtc` taker larger than the available resting depth fills everything it
    /// can and reports the unfilled remainder.
    ///
    /// `match_order` NEVER enqueues the taker: it fills every unit it can
    /// against the resting queue and reports the unfilled remainder via
    /// `remaining_quantity()`. With a taker (150) that exceeds total depth
    /// (100), the resting depth is fully consumed, `remaining_quantity()` stays
    /// positive, `is_complete()` is false, and nothing of the taker is left
    /// resting at the level (the level only holds makers, and `match_order` adds
    /// no new order). For a `Gtc` taker the order book rests the 50 remainder;
    /// distinguishing that from an `Ioc` discard or a `Fok` kill is the job of
    /// the taker-TIF tests above.
    #[test]
    fn test_match_order_taker_exceeds_depth_fills_available_and_reports_remainder() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        // Total resting depth = 100 (two Buy makers).
        price_level
            .add_order(create_standard_order(1, 10000, 60))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 40))
            .expect("add_order should succeed");
        assert_eq!(price_level.visible_quantity(), 100);
        assert_eq!(price_level.order_count(), 2);

        // Taker of 150 exceeds the resting depth of 100.
        let taker_id = Id::from_u64(999);
        let result = price_level.match_order(
            150,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        // All available depth filled (100 of 150); 50 reported as remainder.
        assert_eq!(
            result
                .executed_quantity()
                .expect("real output is Ok")
                .as_u64(),
            100
        );
        assert_eq!(result.remaining_quantity().as_u64(), 50);
        assert!(
            result.remaining_quantity().as_u64() > 0,
            "taker remainder must be strictly positive"
        );
        assert!(
            !result.is_complete(),
            "an under-filled taker must not be reported complete"
        );

        // Both resting makers were fully consumed and removed.
        assert_eq!(result.filled_order_ids().len(), 2);

        // The taker is NOT left resting: `match_order` never enqueues it. The
        // level is now empty — only the (consumed) makers ever lived here.
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);

        // Makers were Buy, so the taker is Sell.
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    /// Pin that `match_order` does NOT enforce maker time-in-force expiry.
    ///
    /// A `TimeInForce::Gtd` maker whose expiry timestamp is in the *past*
    /// relative to the match timestamp still matches normally — the engine does
    /// not consult `TimeInForce::is_expired` inside the match path. Enforcing
    /// expiry (skipping or evicting expired makers) is intentionally the
    /// caller's / order book's responsibility, not the price level's:
    /// `TimeInForce::is_expired(current_ts, market_close_ts)` exists and is unit
    /// tested in isolation (`src/orders/tests/time_in_force.rs`), but it is
    /// deliberately not invoked here, so the match path stays a pure,
    /// timestamp-driven, deterministic sweep over the resting queue.
    #[test]
    fn test_match_order_does_not_enforce_gtd_maker_expiry() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        // Maker expiry is in the PAST relative to the match timestamp below.
        let past_expiry: u64 = 1_000_000_000_000;
        let match_ts: u64 = 1_716_000_000_000;
        assert!(
            match_ts > past_expiry,
            "fixture: match time is after expiry"
        );

        // Sanity-check the isolated helper to make explicit WHAT the level is
        // choosing not to consult: this maker IS expired by `is_expired`.
        assert!(
            TimeInForce::Gtd(past_expiry).is_expired(match_ts, None),
            "fixture: the GTD maker is expired per TimeInForce::is_expired"
        );

        price_level
            .add_order(create_good_till_date_order(1, 10000, 100, past_expiry))
            .expect("add_order should succeed");

        // Despite the expired maker, the match fills it like a standard order.
        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(match_ts),
            &trade_id_generator,
        );

        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert!(result.is_complete());
        assert_eq!(result.trades().len(), 1);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);

        // Maker was Buy, so the taker is Sell.
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_multiple_orders() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 50))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 75))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(3, 10000, 25))
            .expect("add_order should succeed");

        // Match first two orders completely and third partially
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            140,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Verificar el resultado de matching
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 10); // 25 - (140 - 50 - 75) = 10
        assert_eq!(price_level.order_count(), 1);

        assert_eq!(match_result.trades().len(), 3);

        let transaction1 = &match_result.trades().as_vec()[0];
        assert_eq!(transaction1.taker_order_id(), taker_id);
        assert_eq!(transaction1.maker_order_id(), Id::from_u64(1));
        assert_eq!(transaction1.quantity(), Quantity::new(50));

        let transaction2 = &match_result.trades().as_vec()[1];
        assert_eq!(transaction2.taker_order_id(), taker_id);
        assert_eq!(transaction2.maker_order_id(), Id::from_u64(2));
        assert_eq!(transaction2.quantity(), Quantity::new(75));

        let transaction3 = &match_result.trades().as_vec()[2];
        assert_eq!(transaction3.taker_order_id(), taker_id);
        assert_eq!(transaction3.maker_order_id(), Id::from_u64(3));
        assert_eq!(transaction3.quantity(), Quantity::new(15));

        assert_eq!(match_result.filled_order_ids().len(), 2);
        assert!(match_result.filled_order_ids().contains(&Id::from_u64(1)));
        assert!(match_result.filled_order_ids().contains(&Id::from_u64(2)));

        let orders = price_level.snapshot_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id(), Id::from_u64(3));
        assert_eq!(orders[0].visible_quantity().as_u64(), 10);
        assert_eq!(orders[0].hidden_quantity().as_u64(), 0);
    }

    #[test]
    fn test_snapshot() {
        let price_level = PriceLevel::new(10000);

        // Add some orders
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 50))
            .expect("add_order should succeed");

        // Create a snapshot
        let snapshot = price_level.snapshot();

        // Verify snapshot data
        assert_eq!(snapshot.price().as_u128(), 10000);
        assert_eq!(snapshot.visible_quantity().as_u64(), 150); // 100 + 50
        assert_eq!(snapshot.hidden_quantity().as_u64(), 0);
        assert_eq!(snapshot.order_count(), 2);
        assert_eq!(snapshot.orders().len(), 2);

        // Verify that orders in the snapshot match those in the price level
        let orders_from_level = price_level.snapshot_orders();
        assert_eq!(snapshot.orders().len(), orders_from_level.len());

        // Check that all orders from the price level are in the snapshot
        for order in orders_from_level {
            let found = snapshot.orders().iter().any(|o| o.id() == order.id());
            assert!(found, "Order with ID {} not found in snapshot", order.id());
        }
    }

    #[test]
    fn test_update_order_update_price() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = create_standard_order(1, 10000, 100);
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Update the price to a different value
        let update = OrderUpdate::UpdatePrice {
            order_id: Id::from_u64(1),
            new_price: Price::new(11000),
        };

        let result = price_level.update_order(update);

        // The order should be removed from this price level (to be inserted in another price level)
        assert!(result.is_ok());
        let removed_order = result.unwrap();
        assert!(removed_order.is_some());
        assert_eq!(removed_order.unwrap().id(), Id::from_u64(1));

        // The price level should now be empty
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);

        // Test updating price to same value (should return error)
        let order = create_standard_order(2, 10000, 100);
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        let same_price_update = OrderUpdate::UpdatePrice {
            order_id: Id::from_u64(2),
            new_price: Price::new(10000),
        };

        let result = price_level.update_order(same_price_update);
        assert!(result.is_err());
        match result {
            Err(PriceLevelError::InvalidOperation { .. }) => (),
            _ => panic!("Expected InvalidOperation error"),
        }
    }

    #[test]
    fn test_update_order_update_quantity() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = create_standard_order(1, 10000, 100);
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Update to increase quantity
        let update = OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(150),
        };

        let result = price_level.update_order(update);

        // The order should be updated with the new quantity
        assert!(result.is_ok());
        let updated_order = result.unwrap();
        assert!(updated_order.is_some());
        assert_eq!(updated_order.unwrap().visible_quantity().as_u64(), 150);

        // The price level should reflect the new quantity
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);

        // Update to decrease quantity
        let update = OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(50),
        };

        let result = price_level.update_order(update);

        // The order should be updated with the new quantity
        assert!(result.is_ok());
        let updated_order = result.unwrap();
        assert!(updated_order.is_some());
        assert_eq!(updated_order.unwrap().visible_quantity().as_u64(), 50);

        // The price level should reflect the new quantity
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.order_count(), 1);

        // Test updating non-existent order
        let update = OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(999),
            new_quantity: Quantity::new(50),
        };

        let result = price_level.update_order(update);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_update_order_reduce_quantity_keeps_queue_position() {
        let price_level = PriceLevel::new(10000);

        // Add makers A (id 1) then B (id 2) at the same price. A is ahead in
        // price-time priority.
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 100))
            .expect("add_order should succeed");

        // Reduce A's quantity (decrease). A must keep its front position.
        let result = price_level.update_order(OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(40),
        });
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().visible_quantity().as_u64(), 40);

        // Match a quantity that only consumes the first resting order. A (id 1)
        // must be hit before B (id 2).
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);
        let execution_ts = TimestampMs::new(1_716_000_000_000);
        let match_result = price_level.match_order(
            40,
            Id::from_u64(900),
            TimeInForce::Gtc,
            TakerKind::Standard,
            execution_ts,
            &trade_id_generator,
        );

        let trades = match_result.trades().as_vec();
        assert_eq!(trades.len(), 1);
        // The first (and only) trade must name A as the maker: A kept its
        // position despite the reduction.
        assert_eq!(trades[0].maker_order_id(), Id::from_u64(1));
        assert_eq!(trades[0].quantity(), Quantity::new(40));
    }

    #[test]
    fn test_update_order_increase_quantity_demotes_to_back() {
        let price_level = PriceLevel::new(10000);

        // Add makers A (id 1) then B (id 2) at the same price.
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 100))
            .expect("add_order should succeed");

        // Increase A's quantity (Standard orders support resizing). This must
        // demote A to the back of the queue, behind B.
        let result = price_level.update_order(OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(150),
        });
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().visible_quantity().as_u64(), 150);

        // A subsequent match that only consumes the first resting order must
        // now hit B (id 2) before the resized A (id 1).
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);
        let execution_ts = TimestampMs::new(1_716_000_000_000);
        let match_result = price_level.match_order(
            100,
            Id::from_u64(900),
            TimeInForce::Gtc,
            TakerKind::Standard,
            execution_ts,
            &trade_id_generator,
        );

        let trades = match_result.trades().as_vec();
        assert_eq!(trades.len(), 1);
        // B is now at the front: it is matched before the resized A.
        assert_eq!(trades[0].maker_order_id(), Id::from_u64(2));
        assert_eq!(trades[0].quantity(), Quantity::new(100));
    }

    #[test]
    fn test_update_order_quantity_counters_consistent() {
        let price_level = PriceLevel::new(10000);

        // Two standard makers plus an iceberg (so hidden_quantity is exercised).
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(3, 10000, 50, 200))
            .expect("add_order should succeed");

        // Decrease (in place) on a standard order.
        let _ = price_level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(1),
                new_quantity: Quantity::new(30),
            })
            .expect("decrease update should succeed");

        // Increase (demote) on a standard order.
        let _ = price_level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(2),
                new_quantity: Quantity::new(180),
            })
            .expect("increase update should succeed");

        // Reduce the iceberg's visible part in place (hidden unchanged).
        let _ = price_level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(3),
                new_quantity: Quantity::new(20),
            })
            .expect("iceberg decrease update should succeed");

        // Atomic counters must equal the sum over the live queue contents.
        let snapshot = price_level.snapshot_orders();
        let expected_visible: u64 = snapshot.iter().map(|o| o.visible_quantity().as_u64()).sum();
        let expected_hidden: u64 = snapshot.iter().map(|o| o.hidden_quantity().as_u64()).sum();

        assert_eq!(price_level.order_count(), snapshot.len());
        assert_eq!(price_level.visible_quantity(), expected_visible);
        assert_eq!(price_level.hidden_quantity(), expected_hidden);

        // Spot-check the expected values: A=30, B=180, iceberg visible=20.
        assert_eq!(expected_visible, 30 + 180 + 20);
        // Iceberg hidden remains 200.
        assert_eq!(expected_hidden, 200);
        assert_eq!(price_level.order_count(), 3);
    }

    #[test]
    fn test_update_order_update_price_and_quantity() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = create_standard_order(1, 10000, 100);
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Update both price and quantity with different price
        let update = OrderUpdate::UpdatePriceAndQuantity {
            order_id: Id::from_u64(1),
            new_price: Price::new(11000),
            new_quantity: Quantity::new(150),
        };

        let result = price_level.update_order(update);

        // The order should be removed from this price level (to be inserted in another price level)
        assert!(result.is_ok());
        let removed_order = result.unwrap();
        assert!(removed_order.is_some());
        assert_eq!(removed_order.unwrap().id(), Id::from_u64(1));

        // The price level should now be empty
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);

        // Test with same price but different quantity
        let order = create_standard_order(2, 10000, 100);
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        let update = OrderUpdate::UpdatePriceAndQuantity {
            order_id: Id::from_u64(2),
            new_price: Price::new(10000),
            new_quantity: Quantity::new(150),
        };

        let result = price_level.update_order(update);

        // The order should be updated with the new quantity
        assert!(result.is_ok());
        let updated_order = result.unwrap();
        assert!(updated_order.is_some());
        assert_eq!(updated_order.unwrap().visible_quantity().as_u64(), 150);

        // The price level should reflect the new quantity
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_update_order_replace() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = create_standard_order(1, 10000, 100);
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Replace with different price
        let update = OrderUpdate::Replace {
            order_id: Id::from_u64(1),
            price: Price::new(11000),
            quantity: Quantity::new(150),
            side: Side::Buy,
        };

        let result = price_level.update_order(update);

        // The order should be removed from this price level (to be inserted in another price level)
        assert!(result.is_ok());
        let removed_order = result.unwrap();
        assert!(removed_order.is_some());
        assert_eq!(removed_order.unwrap().id(), Id::from_u64(1));

        // The price level should now be empty
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);

        // Test with same price but different quantity
        let order = create_standard_order(2, 10000, 100);
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        let update = OrderUpdate::Replace {
            order_id: Id::from_u64(2),
            price: Price::new(10000),
            quantity: Quantity::new(150),
            side: Side::Buy,
        };

        let result = price_level.update_order(update);

        // The order should be updated with the new quantity
        assert!(result.is_ok());
        let updated_order = result.unwrap();
        assert!(updated_order.is_some());
        assert_eq!(updated_order.unwrap().visible_quantity().as_u64(), 150);

        // The price level should reflect the new quantity
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);
    }

    // Test the From<&PriceLevel> implementation for PriceLevelData
    #[test]
    fn test_price_level_data_from_price_level() {
        let price_level = PriceLevel::new(10000);

        // Add some orders
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 50))
            .expect("add_order should succeed");

        // Convert to PriceLevelData
        let data: PriceLevelData = (&price_level).into();

        // Verify data fields
        assert_eq!(data.price, 10000);
        assert_eq!(data.visible_quantity, 150); // 100 + 50
        assert_eq!(data.hidden_quantity, 0);
        assert_eq!(data.order_count, 2);
        assert_eq!(data.orders.len(), 2);

        // Verify order IDs
        let order_ids: Vec<Id> = data.orders.iter().map(|o| o.id()).collect();
        assert!(order_ids.contains(&Id::from_u64(1)));
        assert!(order_ids.contains(&Id::from_u64(2)));
    }

    // Test the TryFrom<PriceLevelData> implementation for PriceLevel
    #[test]
    fn test_price_level_try_from_price_level_data() {
        // Create PriceLevelData directly
        let data = PriceLevelData {
            price: 10000,
            visible_quantity: 150,
            hidden_quantity: 0,
            order_count: 2,
            orders: vec![
                create_standard_order(1, 10000, 100),
                create_standard_order(2, 10000, 50),
            ],
        };

        // Convert to PriceLevel
        let result = PriceLevel::try_from(data);
        assert!(result.is_ok());

        let price_level = result.unwrap();

        // Verify price level properties
        assert_eq!(price_level.price(), 10000);
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 2);

        // Verify orders
        let orders = price_level.snapshot_orders();
        assert_eq!(orders.len(), 2);

        let order_ids: Vec<Id> = orders.iter().map(|o| o.id()).collect();
        assert!(order_ids.contains(&Id::from_u64(1)));
        assert!(order_ids.contains(&Id::from_u64(2)));
    }

    // Test Display implementation for PriceLevel
    #[test]
    fn test_price_level_display() {
        let price_level = PriceLevel::new(10000);
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let display_str = format!("{price_level}");

        // Verify the format
        assert!(display_str.starts_with("PriceLevel:price=10000;"));
        assert!(display_str.contains("visible_quantity=100"));
        assert!(display_str.contains("hidden_quantity=0"));
        assert!(display_str.contains("order_count=1"));
        assert!(display_str.contains("orders=["));
        assert!(display_str.contains("Standard:id=00000000-0000-0001-0000-000000000000"));
    }

    // Test FromStr implementation for PriceLevel
    #[test]
    fn test_price_level_from_str() {
        let price_level = PriceLevel::new(10000);
        price_level
            .add_order(create_standard_order(1, 10000, 50))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 75))
            .expect("add_order should succeed");
        price_level
            .add_order(create_good_till_date_order(3, 10000, 100, 1617000000000))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_reserve_order(4, 10000, 100, 100, 20, true, None))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(5, 10000, 50, 100))
            .expect("add_order should succeed");

        let input = "PriceLevel:price=10000;visible_quantity=375;hidden_quantity=200;order_count=5;orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=10000;quantity=50;side=BUY;timestamp=1616823000000;time_in_force=GTC,Standard:id=00000000-0000-0002-0000-000000000000;price=10000;quantity=75;side=BUY;timestamp=1616823000001;time_in_force=GTC,Standard:id=00000000-0000-0003-0000-000000000000;price=10000;quantity=100;side=BUY;timestamp=1616823000002;time_in_force=GTD-1617000000000,ReserveOrder:id=00000000-0000-0004-0000-000000000000;price=10000;visible_quantity=100;hidden_quantity=100;side=BUY;timestamp=1616823000003;time_in_force=GTC;replenish_threshold=20;replenish_amount=None;auto_replenish=true,IcebergOrder:id=00000000-0000-0005-0000-000000000000;price=10000;visible_quantity=50;hidden_quantity=100;side=BUY;timestamp=1616823000004;time_in_force=GTC]";
        let result = PriceLevel::from_str(input);

        if let Err(ref err) = result {
            error!("Error parsing PriceLevel: {:?}", err);
        }

        assert!(result.is_ok());

        let price_level = result.unwrap();

        // Verify price level properties
        assert_eq!(price_level.price(), 10000);
        assert_eq!(price_level.visible_quantity(), 375);
        assert_eq!(price_level.hidden_quantity(), 200);
        assert_eq!(price_level.order_count(), 5);

        // Verify the order
        let orders = price_level.snapshot_orders();
        assert_eq!(orders.len(), 5);
        assert_eq!(orders[0].id(), Id::from_u64(1));
        assert_eq!(orders[0].price(), Price::new(10000));
        assert_eq!(orders[0].visible_quantity().as_u64(), 50);
    }

    // Test serialization and deserialization for PriceLevel
    #[test]
    fn test_price_level_serde() {
        let price_level = PriceLevel::new(10000);
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Serialize to JSON
        let serialized = serde_json::to_string(&price_level).unwrap();

        // Verify the JSON structure
        assert!(serialized.contains("\"price\":10000"));
        assert!(serialized.contains("\"visible_quantity\":100"));
        assert!(serialized.contains("\"hidden_quantity\":0"));
        assert!(serialized.contains("\"order_count\":1"));
        assert!(serialized.contains("\"orders\":"));

        // Deserialize back
        let deserialized: PriceLevel = serde_json::from_str(&serialized).unwrap();

        // Verify deserialized price level
        assert_eq!(deserialized.price(), 10000);
        assert_eq!(deserialized.visible_quantity(), 100);
        assert_eq!(deserialized.hidden_quantity(), 0);
        assert_eq!(deserialized.order_count(), 1);

        // Verify the order in the deserialized price level
        let orders = deserialized.snapshot_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id(), Id::from_u64(1));
        assert_eq!(orders[0].price(), Price::new(10000));
        assert_eq!(orders[0].visible_quantity().as_u64(), 100);
    }

    // `PriceLevelData` is a plain input/transfer DTO: with `deny_unknown_fields`
    // an unexpected key must be rejected rather than silently ignored.
    #[test]
    fn test_price_level_data_unknown_field_rejected() {
        let json = r#"{
            "price": 10000,
            "visible_quantity": 100,
            "hidden_quantity": 0,
            "order_count": 0,
            "orders": [],
            "unexpected_field": 42
        }"#;

        let result = serde_json::from_str::<PriceLevelData>(json);
        assert!(
            result.is_err(),
            "deny_unknown_fields should reject the unexpected key"
        );

        // The same payload without the unknown field still deserializes,
        // proving the wire format itself is unchanged.
        let valid_json = r#"{
            "price": 10000,
            "visible_quantity": 100,
            "hidden_quantity": 0,
            "order_count": 0,
            "orders": []
        }"#;
        let data = serde_json::from_str::<PriceLevelData>(valid_json)
            .expect("valid PriceLevelData must deserialize");
        assert_eq!(data.price, 10000);
        assert_eq!(data.visible_quantity, 100);
    }

    // Deserializing a `PriceLevel` (which routes through `PriceLevelData`) from a
    // payload carrying an unknown field is likewise rejected.
    #[test]
    fn test_price_level_deserialize_unknown_field_rejected() {
        let json = r#"{
            "price": 10000,
            "visible_quantity": 0,
            "hidden_quantity": 0,
            "order_count": 0,
            "orders": [],
            "bogus": "value"
        }"#;

        let result = serde_json::from_str::<PriceLevel>(json);
        assert!(
            result.is_err(),
            "PriceLevel deserialize must reject unknown fields via PriceLevelData"
        );
    }

    // In price_level/level.rs test module or in a separate test file

    #[test]
    fn test_level_partial_match_remaining() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        // Add orders with more quantity than we'll match
        price_level
            .add_order(create_standard_order(1, 10000, 200))
            .expect("add_order should succeed");

        // Match only part of what's available
        let match_result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity().as_u64(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 100); // 200 - 100 = 100
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_level_update_price_different_price() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Update to a different price (should remove from this level)
        let result = price_level.update_order(OrderUpdate::UpdatePrice {
            order_id: Id::from_u64(1),
            new_price: Price::new(10100), // Different price
        });

        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_level_update_price_and_quantity_same_price() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        // Update the quantity but keep the same price
        let result = price_level.update_order(OrderUpdate::UpdatePriceAndQuantity {
            order_id: Id::from_u64(1),
            new_price: Price::new(10000), // Same price
            new_quantity: Quantity::new(150),
        });

        assert!(result.is_ok());
        let updated_order = result.unwrap().unwrap();
        assert_eq!(updated_order.visible_quantity().as_u64(), 150);
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_serialize_deserialize_with_orders() {
        let price_level = PriceLevel::new(10000);

        // Add some orders
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_buy_iceberg_order(2, 10000, 50, 150))
            .expect("add_order should succeed");

        // Serialize to JSON
        let serialized = serde_json::to_string(&price_level).unwrap();

        // Deserialize back
        let deserialized: PriceLevel = serde_json::from_str(&serialized).unwrap();

        // Verify deserialized state matches original
        assert_eq!(deserialized.price(), price_level.price());
        assert_eq!(
            deserialized.visible_quantity(),
            price_level.visible_quantity()
        );
        assert_eq!(
            deserialized.hidden_quantity(),
            price_level.hidden_quantity()
        );
        assert_eq!(deserialized.order_count(), price_level.order_count());
    }

    #[test]
    fn test_price_level_update_price_same_value() {
        // Test lines 187-188
        let price_level = PriceLevel::new(10000);
        let order = OrderType::<()>::Standard {
            id: Id::from_u64(1),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Try to update price to the same value
        let update = OrderUpdate::UpdatePrice {
            order_id: Id::from_u64(1),
            new_price: Price::new(10000),
        };

        // This should return an error
        let result = price_level.update_order(update);
        assert!(result.is_err());
        match result {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(message.contains("Cannot update price to the same value"));
            }
            _ => panic!("Expected InvalidOperation error"),
        }
    }

    #[test]
    fn test_price_level_update_quantity_order_not_found() {
        // Test line 282
        let price_level = PriceLevel::new(10000);
        // No orders added

        // Try to update quantity of a non-existent order
        let update = OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(123),
            new_quantity: Quantity::new(20),
        };

        let result = price_level.update_order(update);
        // Should return Ok(None) when order not found
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_price_level_update_quantity_by_another_thread() {
        // Test lines 304-306, 308-309
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = OrderType::<()>::Standard {
            id: Id::from_u64(1),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Set up a test that simulates order removal by another thread
        // This can be done by modifying the OrderQueue's internal state directly
        // or by simply testing the behavior of the update_quantity method when it returns None

        // For now, we'll just mock this behavior by ensuring the method handles
        // cases where an order is not found after initial check (order was found but removed)

        // First find the order to make sure it exists
        assert!(
            price_level
                .update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(1)
                })
                .unwrap()
                .is_some()
        );

        // Now try to update it after it's been removed
        let update = OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(20),
        };

        let result = price_level.update_order(update);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_price_level_update_quantity_increase() {
        // Test line 473
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = OrderType::<()>::Standard {
            id: Id::from_u64(1),
            price: Price::new(10000),
            quantity: Quantity::new(50),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Update to increase quantity (old visible < new visible)
        let update = OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(100),
        };

        let result = price_level.update_order(update);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());

        // Verify quantity increased
        assert_eq!(price_level.visible_quantity(), 100);
    }

    #[test]
    fn test_price_level_update_hidden_quantity() {
        // Test lines 488, 498
        let price_level = PriceLevel::new(10000);

        // Add an iceberg order with visible and hidden quantities
        let order = OrderType::IcebergOrder {
            id: Id::from_u64(1),
            price: Price::new(10000),
            visible_quantity: Quantity::new(50),
            hidden_quantity: Quantity::new(150),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Verify initial quantities
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.hidden_quantity(), 150);

        // Create a new iceberg order with different quantities
        let new_order = OrderType::IcebergOrder {
            id: Id::from_u64(1),
            price: Price::new(10000),
            visible_quantity: Quantity::new(40),
            hidden_quantity: Quantity::new(200),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };

        // Test increasing hidden quantity
        let result = price_level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(1),
        });
        assert!(result.is_ok());
        price_level
            .add_order(new_order)
            .expect("add_order should succeed");

        // Verify both visible and hidden quantities were updated
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.hidden_quantity(), 200);
    }

    #[test]
    fn test_price_level_update_price_and_quantity_same_price() {
        // Test line 510
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = OrderType::<()>::Standard {
            id: Id::from_u64(1),
            price: Price::new(10000),
            quantity: Quantity::new(50),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");

        // Update both price and quantity with same price
        let update = OrderUpdate::UpdatePriceAndQuantity {
            order_id: Id::from_u64(1),
            new_price: Price::new(10000), // Same price
            new_quantity: Quantity::new(100),
        };

        let result = price_level.update_order(update);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());

        // Verify quantity was updated but price remained the same
        assert_eq!(price_level.visible_quantity(), 100);
        assert_eq!(price_level.price(), 10000);
    }

    #[test]
    fn test_price_level_from_price_level_data_conversion() {
        // Test lines 521-523, 527, 537, 558-560, 562-564, 566-568, 607

        // Create a price level
        let price_level = PriceLevel::new(10000);

        // Add some orders
        let order1 = OrderType::<()>::Standard {
            id: Id::from_u64(1),
            price: Price::new(10000),
            quantity: Quantity::new(50),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order1)
            .expect("add_order should succeed");

        let order2 = OrderType::<()>::IcebergOrder {
            id: Id::from_u64(2),
            price: Price::new(10000),
            visible_quantity: Quantity::new(30),
            hidden_quantity: Quantity::new(70),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000001),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order2)
            .expect("add_order should succeed");

        // Convert to PriceLevelData
        let data: PriceLevelData = (&price_level).into();

        // Verify data
        assert_eq!(data.price, 10000);
        assert_eq!(data.visible_quantity, 80); // 50 + 30
        assert_eq!(data.hidden_quantity, 70);
        assert_eq!(data.order_count, 2);
        assert_eq!(data.orders.len(), 2);

        // Convert back to PriceLevel
        let result = PriceLevel::try_from(data);
        assert!(result.is_ok());

        // Verify converted price level
        let converted_level = result.unwrap();
        assert_eq!(converted_level.price(), 10000);
        assert_eq!(converted_level.visible_quantity(), 80);
        assert_eq!(converted_level.hidden_quantity(), 70);
        assert_eq!(converted_level.order_count(), 2);

        // Test display implementation
        let display_string = price_level.to_string();
        assert!(display_string.starts_with("PriceLevel:price=10000;"));
        assert!(display_string.contains("visible_quantity=80"));
        assert!(display_string.contains("hidden_quantity=70"));
        assert!(display_string.contains("order_count=2"));

        // Test serialization
        let serialized = serde_json::to_string(&price_level).unwrap();
        assert!(serialized.contains("\"price\":10000"));
        assert!(serialized.contains("\"visible_quantity\":80"));
        assert!(serialized.contains("\"hidden_quantity\":70"));
        assert!(serialized.contains("\"order_count\":2"));

        // Test deserialization
        let deserialized: PriceLevel = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.price(), 10000);
        assert_eq!(deserialized.visible_quantity(), 80);
        assert_eq!(deserialized.hidden_quantity(), 70);
        assert_eq!(deserialized.order_count(), 2);
    }

    // ------------------------- PRICE-TIME PRIORITY (issue #39) -------------------------

    #[test]
    /// Regression for issue #39: a partial fill must keep the resting maker at
    /// the FRONT of the queue. Rest A then B at the same price, partially fill
    /// A, then send a second aggressor — it must consume A's remainder before
    /// touching the later-arriving B.
    fn test_match_partial_fill_keeps_maker_price_time_priority() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_ids = UuidGenerator::new(namespace);

        // A (id=1) arrives before B (id=2), both 100 @ 10000.
        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 100))
            .expect("add_order should succeed");

        // First aggressor partially fills A (60 of 100). A's residual = 40.
        let first = price_level.match_order(
            60,
            Id::from_u64(901),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_ids,
        );
        assert_eq!(first.trades().len(), 1);
        assert_eq!(
            first.trades().as_vec()[0].maker_order_id(),
            Id::from_u64(1),
            "first aggressor must hit A"
        );
        assert_eq!(first.trades().as_vec()[0].quantity(), Quantity::new(60));
        // A(40) + B(100) still resting.
        assert_eq!(price_level.visible_quantity(), 140);
        assert_eq!(price_level.order_count(), 2);

        // Second aggressor (50) must hit A's remainder (40) FIRST, then B (10).
        let second = price_level.match_order(
            50,
            Id::from_u64(902),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_ids,
        );
        assert_eq!(second.trades().len(), 2);

        let t0 = &second.trades().as_vec()[0];
        assert_eq!(
            t0.maker_order_id(),
            Id::from_u64(1),
            "price-time priority: A's residual must be consumed before B"
        );
        assert_eq!(t0.quantity(), Quantity::new(40));

        let t1 = &second.trades().as_vec()[1];
        assert_eq!(t1.maker_order_id(), Id::from_u64(2));
        assert_eq!(t1.quantity(), Quantity::new(10));

        // A fully consumed; B has 90 left. Conservation holds.
        assert_eq!(second.filled_order_ids(), &[Id::from_u64(1)]);
        assert_eq!(price_level.visible_quantity(), 90);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    /// Conservation: a partial fill never changes the total resting quantity at
    /// the level beyond what was consumed.
    fn test_match_partial_fill_conserves_quantity() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_ids = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 100))
            .expect("add_order should succeed");
        let total_before = match price_level.total_quantity() {
            Ok(q) => q,
            Err(e) => panic!("total_quantity failed: {e}"),
        };
        assert_eq!(total_before, 200);

        let _ = price_level.match_order(
            60,
            Id::from_u64(901),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_ids,
        );
        let total_after = match price_level.total_quantity() {
            Ok(q) => q,
            Err(e) => panic!("total_quantity failed: {e}"),
        };
        assert_eq!(total_after, 140, "exactly the consumed 60 left the level");
    }

    #[test]
    /// Iceberg/Reserve replenishment keeps its existing semantics: a refreshed
    /// tranche LOSES time priority (goes to the tail), unlike a pure partial
    /// fill. Confirms the `hidden_reduced` discriminator in `match_order`.
    fn test_match_iceberg_replenish_loses_priority() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_ids = UuidGenerator::new(namespace);

        // Iceberg I (id=1) arrives first: visible 50, hidden 100.
        price_level
            .add_order(create_iceberg_order(1, 10000, 50, 100))
            .expect("add_order should succeed");
        // O (id=2) arrives later: a plain 50 (iceberg with no hidden).
        price_level
            .add_order(create_iceberg_order(2, 10000, 50, 0))
            .expect("add_order should succeed");

        // Aggressor consumes I's visible tip (50) → I refreshes from hidden and
        // moves to the tail. remaining hits 0, so this call stops there.
        let first = price_level.match_order(
            50,
            Id::from_u64(901),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_ids,
        );
        assert_eq!(first.trades().len(), 1);
        assert_eq!(first.trades().as_vec()[0].maker_order_id(), Id::from_u64(1));

        // Next aggressor must now hit O (id=2) FIRST, because the refreshed
        // iceberg tranche lost its priority to the tail.
        let second = price_level.match_order(
            50,
            Id::from_u64(902),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_ids,
        );
        assert_eq!(
            second.trades().as_vec()[0].maker_order_id(),
            Id::from_u64(2),
            "refreshed iceberg tranche must lose time priority"
        );
    }

    #[test]
    /// A partial-fill residual at the front must survive a snapshot round-trip
    /// with its priority intact.
    fn test_snapshot_roundtrip_preserves_partial_fill_priority() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_ids = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 100))
            .expect("add_order should succeed");
        let _ = price_level.match_order(
            60,
            Id::from_u64(901),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_ids,
        );

        let json = match price_level.snapshot_to_json() {
            Ok(j) => j,
            Err(e) => panic!("snapshot_to_json failed: {e}"),
        };
        let restored = match PriceLevel::from_snapshot_json(&json) {
            Ok(r) => r,
            Err(e) => panic!("from_snapshot_json failed: {e}"),
        };

        // Match against the restored level: A's residual (40) must still come
        // first, proving the snapshot preserved price-time priority.
        let restored_trade_ids = UuidGenerator::new(namespace);
        let result = restored.match_order(
            50,
            Id::from_u64(903),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &restored_trade_ids,
        );
        assert_eq!(
            result.trades().as_vec()[0].maker_order_id(),
            Id::from_u64(1),
            "restored level must keep A's residual ahead of B"
        );
        assert_eq!(result.trades().as_vec()[0].quantity(), Quantity::new(40));
    }

    fn assert_snapshot_internally_consistent(snapshot: &crate::price_level::PriceLevelSnapshot) {
        let orders = snapshot.orders();

        let visible_sum: u64 = orders
            .iter()
            .map(|order| order.visible_quantity().as_u64())
            .sum();
        let hidden_sum: u64 = orders
            .iter()
            .map(|order| order.hidden_quantity().as_u64())
            .sum();

        assert_eq!(
            snapshot.visible_quantity().as_u64(),
            visible_sum,
            "snapshot visible_quantity must equal the sum over its own orders"
        );
        assert_eq!(
            snapshot.hidden_quantity().as_u64(),
            hidden_sum,
            "snapshot hidden_quantity must equal the sum over its own orders"
        );
        assert_eq!(
            snapshot.order_count(),
            orders.len(),
            "snapshot order_count must equal the length of its own orders vector"
        );
    }

    #[test]
    fn test_snapshot_concurrent_mutation_internally_consistent() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        // Workers: order adders + matchers + one reader (the snapshot taker).
        const ADDER_THREADS: usize = 4;
        // `match_order` requires a single logical matcher per level (see its
        // rustdoc); concurrent matchers are an unsupported, racy contract. The
        // supported concurrency under test is many adders + snapshot reads
        // racing exactly one matcher.
        const MATCHER_THREADS: usize = 1;
        const TOTAL_THREADS: usize = ADDER_THREADS + MATCHER_THREADS + 1;
        const OPS_PER_THREAD: usize = 500;
        const ORDERS_PER_THREAD: usize = OPS_PER_THREAD;
        const PRICE: u128 = 10_000;

        let price_level = Arc::new(PriceLevel::new(PRICE));
        let barrier = Arc::new(Barrier::new(TOTAL_THREADS));
        // Deterministically seeded so the trade-id stream is reproducible.
        let trade_id_generator = Arc::new(UuidGenerator::new(Uuid::from_u128(0x1234_5678)));

        let mut handles = Vec::with_capacity(TOTAL_THREADS);

        // Adder threads: each pushes a deterministic stream of standard +
        // iceberg orders (iceberg exercises both visible and hidden counters).
        for t in 0..ADDER_THREADS {
            let level = Arc::clone(&price_level);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                for i in 0..ORDERS_PER_THREAD {
                    // Ids / quantities derived purely from indices (deterministic).
                    let base = (t * ORDERS_PER_THREAD + i) as u64;
                    let id = base * 2 + 1_000;
                    if i % 2 == 0 {
                        level
                            .add_order(create_standard_order(id, PRICE, 1 + (base % 7)))
                            .expect("add_order should succeed");
                    } else {
                        level
                            .add_order(create_buy_iceberg_order(
                                id,
                                PRICE,
                                1 + (base % 5),
                                1 + (base % 11),
                            ))
                            .expect("add_order should succeed");
                    }
                }
            }));
        }

        // Matcher threads: drain liquidity concurrently with the adders.
        for t in 0..MATCHER_THREADS {
            let level = Arc::clone(&price_level);
            let barrier = Arc::clone(&barrier);
            let generator = Arc::clone(&trade_id_generator);
            handles.push(thread::spawn(move || {
                barrier.wait();
                for i in 0..OPS_PER_THREAD {
                    let taker_id = Id::from_u64((t * OPS_PER_THREAD + i) as u64 + 5_000_000);
                    let _ = level.match_order(
                        3,
                        taker_id,
                        TimeInForce::Gtc,
                        TakerKind::Standard,
                        TimestampMs::new(1_716_000_000_000),
                        &generator,
                    );
                }
            }));
        }

        // Reader thread: repeatedly snapshot and assert internal consistency
        // while the level is being mutated concurrently.
        let reader = {
            let level = Arc::clone(&price_level);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..OPS_PER_THREAD {
                    assert_snapshot_internally_consistent(&level.snapshot());
                }
            })
        };

        for handle in handles {
            handle.join().expect("worker thread panicked");
        }
        reader.join().expect("reader thread panicked");

        // The final quiescent snapshot must also be self-consistent.
        assert_snapshot_internally_consistent(&price_level.snapshot());
    }

    // ------------------------------------------------------------------
    // Issue #77: assert MatchResult field-agreement and Trade structural
    // invariants on output produced by `PriceLevel::match_order` itself,
    // not on hand-built results.
    // ------------------------------------------------------------------

    /// Assert that the `MatchResult` returned by `match_order` is internally
    /// consistent across all its derived views.
    ///
    /// Checks the documented field-agreement invariants:
    /// - `is_complete()` is true iff `remaining_quantity() == 0`;
    /// - `executed_quantity()` equals the sum of trade quantities;
    /// - `executed_value()` equals the sum of each trade's `price * quantity`;
    /// - `filled_order_ids()` contains no duplicates (a maker is consumed at
    ///   most once per sweep). The exact filled count per scenario is asserted by
    ///   each test, not here.
    ///
    /// `maker_side` is the side every resting maker was added on; it is used to
    /// check each trade's `taker_side` against the *known* resting side rather
    /// than against a value derived from `taker_side` itself.
    fn assert_match_result_consistent(
        result: &crate::execution::MatchResult,
        level_price: u128,
        maker_side: Side,
    ) {
        // is_complete <=> remaining_quantity == 0
        assert_eq!(
            result.is_complete(),
            result.remaining_quantity().as_u64() == 0,
            "is_complete must agree with remaining_quantity == 0"
        );

        let trades = result.trades().as_vec();

        // executed_quantity == sum of trade quantities. Use checked addition to
        // mirror `executed_quantity()`'s own checked arithmetic (and avoid a
        // debug overflow panic in the test on pathological inputs).
        let expected_qty = trades
            .iter()
            .try_fold(0u64, |acc, t| acc.checked_add(t.quantity().as_u64()))
            .expect("summing trade quantities must not overflow u64");
        let executed_qty = match result.executed_quantity() {
            Ok(q) => q.as_u64(),
            Err(e) => panic!("executed_quantity must not error on real output: {e}"),
        };
        assert_eq!(
            executed_qty, expected_qty,
            "executed_quantity must equal the sum of trade quantities"
        );

        // executed_value == sum of each trade's price * quantity, checked the
        // same way as `executed_value()`.
        let expected_value = trades
            .iter()
            .try_fold(0u128, |acc, t| {
                let v = t
                    .price()
                    .as_u128()
                    .checked_mul(u128::from(t.quantity().as_u64()))?;
                acc.checked_add(v)
            })
            .expect("summing trade values must not overflow u128");
        let executed_value = match result.executed_value() {
            Ok(v) => v,
            Err(e) => panic!("executed_value must not error on real output: {e}"),
        };
        assert_eq!(
            executed_value, expected_value,
            "executed_value must equal the sum of price * quantity over trades"
        );

        // Each filled id is unique (a maker is consumed at most once per
        // sweep). `Id` is `Hash + Eq` but not `Ord`, so dedup via a set.
        let filled = result.filled_order_ids();
        let unique: std::collections::HashSet<_> = filled.iter().collect();
        assert_eq!(
            unique.len(),
            filled.len(),
            "filled_order_ids must not contain duplicates"
        );

        assert_match_result_trades_valid(result, level_price, maker_side);
    }

    /// Assert the structural invariants on every `Trade` emitted by a real
    /// `match_order` call: maker != taker, price == level price, quantity > 0,
    /// and `taker_side` is the opposite of the *known* resting `maker_side`.
    ///
    /// `maker_side` is passed in (not read back from the trade) so the check is
    /// not tautological: `Trade::maker_side()` is derived as
    /// `taker_side().opposite()`, so comparing the two would always hold even if
    /// the engine stamped the wrong `taker_side`.
    fn assert_match_result_trades_valid(
        result: &crate::execution::MatchResult,
        level_price: u128,
        maker_side: Side,
    ) {
        let taker_id = result.order_id();
        for trade in result.trades().as_vec() {
            assert_ne!(
                trade.maker_order_id(),
                trade.taker_order_id(),
                "maker and taker must differ (no self-fill)"
            );
            assert_eq!(
                trade.taker_order_id(),
                taker_id,
                "every trade's taker must be the incoming order"
            );
            assert_eq!(
                trade.price(),
                Price::new(level_price),
                "trade price must equal the level price"
            );
            assert!(
                trade.quantity().as_u64() > 0,
                "trade quantity must be strictly positive"
            );
            // Cross-check taker_side against the KNOWN resting maker side.
            assert_eq!(
                trade.taker_side(),
                maker_side.opposite(),
                "taker side must be the opposite of the resting maker side"
            );
        }
    }

    #[test]
    fn test_match_order_partial_fill_result_invariants_hold() {
        // Taker smaller than a single resting maker: the taker is fully filled
        // (complete), the maker is only partially consumed and keeps resting, so
        // no maker appears in filled_order_ids.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 100))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            40,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        // The taker (40) is exhausted against the maker (100): complete.
        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 1);
        // The maker is only partially filled and remains resting.
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(price_level.order_count(), 1);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_order_exact_full_fill_result_invariants_hold() {
        // Taker exactly equals total resting depth across two makers: every
        // maker is fully consumed and the taker is complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 60))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 2);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_order_taker_larger_than_depth_result_invariants_hold() {
        // Taker exceeds resting depth: queue drained, all makers filled, and a
        // positive remainder is left so the result is NOT complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 30))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 40);
        assert_eq!(result.trades().len(), 2);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_order_multi_maker_sweep_result_invariants_hold() {
        // Sweep three makers, partially filling the last: two fully-consumed
        // makers, three trades, taker complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level
            .add_order(create_standard_order(1, 10000, 40))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(2, 10000, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_standard_order(3, 10000, 50))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            90,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(result.trades().len(), 3);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_order_empty_level_result_invariants_hold() {
        // No resting orders: no trades, nothing filled, remainder == taker qty,
        // not complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 50);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_match_result_consistent(&result, 10000, Side::Buy);
    }

    #[test]
    fn test_match_order_iceberg_maker_result_invariants_hold() {
        // Iceberg maker: matching beyond the visible tranche triggers a
        // replenishment from hidden. The emitted trades must still satisfy
        // every field-agreement and structural invariant.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        // visible 50, hidden 200.
        price_level
            .add_order(create_iceberg_order(1, 10000, 50, 200))
            .expect("add_order should succeed");

        // Consume the full visible tranche; the maker replenishes and keeps
        // resting, so it is not in filled_order_ids.
        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert!(!result.trades().as_vec().is_empty());
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_match_order_reserve_maker_result_invariants_hold() {
        // Reserve maker with auto-replenish: same shape as iceberg — exercise
        // the hidden->visible replenishment branch and assert invariants on the
        // real output.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        // visible 50, hidden 200, replenish threshold 10, auto-replenish on.
        price_level
            .add_order(create_reserve_order(1, 10000, 50, 200, 10, true, Some(50)))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert!(!result.trades().as_vec().is_empty());
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_match_order_iceberg_maker_deterministic_trade_stream() {
        // Determinism with a replenishing iceberg maker: matching the same
        // input twice with the same threaded timestamp must yield byte-identical
        // trade streams. Complements the standard-maker determinism test (#61)
        // by covering the hidden->visible replenishment branch.
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let taker_id = Id::from_u64(999);
        let timestamp = TimestampMs::new(1_716_000_000_000);

        // Fixed-timestamp iceberg maker so both runs use identical input.
        let mk = || OrderType::IcebergOrder {
            id: Id::from_u64(1),
            price: Price::new(10000),
            visible_quantity: Quantity::new(50),
            hidden_quantity: Quantity::new(200),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_700_000_000_001),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };

        let run = || {
            let price_level = PriceLevel::new(10000);
            price_level
                .add_order(mk())
                .expect("add_order should succeed");
            let trade_id_generator = UuidGenerator::new(namespace);
            // Cross more than the visible tranche to force replenishment and a
            // multi-trade stream.
            price_level.match_order(
                120,
                taker_id,
                TimeInForce::Gtc,
                TakerKind::Standard,
                timestamp,
                &trade_id_generator,
            )
        };

        let first = run();
        let second = run();

        assert_eq!(first.trades().as_vec(), second.trades().as_vec());
        assert_match_result_consistent(&first, 10000, Side::Sell);
        assert_match_result_consistent(&second, 10000, Side::Sell);
    }

    // ============================================================
    // Regression tests for issue #65: zero-visible iceberg / reserve
    // at the FRONT of the queue must not cause an infinite match loop.
    //
    // Each of these tests would HANG before the fix: a zero-visible
    // iceberg/reserve with hidden depth returned no-progress from
    // `match_against`, so the sweep (and the FOK dry run) re-popped the
    // same front order forever. A normal matchable maker is parked
    // BEHIND the dead order to prove the sweep still reaches makers
    // behind a non-progressing front order (FIFO, no starvation).
    // All makers rest on Side::Sell so `assert_match_result_consistent`
    // sees a single, known maker side.
    // ============================================================

    /// Sell-side standard maker (the queue-behind liquidity). The shared
    /// `create_standard_order` rests on Side::Buy; these regression tests need
    /// the behind maker on the same side as the zero-visible iceberg/reserve.
    fn create_sell_standard_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        let timestamp = TIMESTAMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn new_trade_id_generator() -> UuidGenerator {
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        UuidGenerator::new(namespace)
    }

    #[test]
    fn test_zero_visible_iceberg_front_gtc_taker_terminates_and_fills_behind() {
        // Front: zero-visible iceberg with 30 hidden (id 1).
        // Behind: standard sell maker of 40 (id 2).
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_iceberg_order(1, 10000, 0, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        // Counters reflect both orders: visible 0+40, hidden 30+0.
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.hidden_quantity(), 30);
        assert_eq!(price_level.order_count(), 2);

        // A GTC taker of 70 must drain both: 40 from the standard maker and
        // 30 replenished from the iceberg's hidden. This call MUST terminate.
        let result = price_level.match_order(
            70,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity().as_u64(), 0);
        assert_eq!(
            result
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            70
        );
        // Both makers are fully consumed and removed.
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_zero_visible_iceberg_front_fok_prediction_matches_sweep() {
        // FOK taker of exactly 70 fits (40 + 30 hidden) -> fills fully.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_iceberg_order(1, 10000, 0, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            70,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        // FOK must fill, not kill: matchable_quantity(70) == 70 == sweep.
        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert!(!result.was_killed());
        assert_eq!(
            result
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            70
        );
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_zero_visible_iceberg_front_fok_killed_when_too_large() {
        // FOK taker of 71 exceeds available depth (70) -> killed, queue intact.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_iceberg_order(1, 10000, 0, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            71,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_eq!(result.remaining_quantity().as_u64(), 71);
        assert_eq!(result.trades().len(), 0);
        // Queue untouched.
        assert_eq!(price_level.order_count(), 2);
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.hidden_quantity(), 30);
    }

    #[test]
    fn test_zero_visible_iceberg_front_ioc_taker_fills_available() {
        // IOC taker of 200 fills the available 70 and discards the rest.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_iceberg_order(1, 10000, 0, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            200,
            Id::from_u64(999),
            TimeInForce::Ioc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert_eq!(
            result
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            70
        );
        assert_eq!(result.remaining_quantity().as_u64(), 130);
        assert!(!result.is_complete());
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_zero_visible_iceberg_front_post_only_rejected_consistent_depth() {
        // PostOnly taker must be rejected because the level has matchable depth:
        // both the zero-visible iceberg (hidden 30) and the standard maker count.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_iceberg_order(1, 10000, 0, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            10,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.was_rejected());
        assert_eq!(result.outcome(), MatchOutcome::Rejected);
        assert_eq!(result.remaining_quantity().as_u64(), 10);
        assert_eq!(result.trades().len(), 0);
        // Queue untouched by the rejection.
        assert_eq!(price_level.order_count(), 2);
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.hidden_quantity(), 30);
    }

    #[test]
    fn test_zero_visible_iceberg_alone_post_only_rejected_hidden_only() {
        // A level whose ONLY resting order is a zero-visible iceberg with hidden
        // depth still has matchable depth: PostOnly must be rejected, and FOK of
        // the hidden size must fill. Proves `has_matchable_depth` and
        // `matchable_quantity` agree on the degenerate hidden-only state.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_iceberg_order(1, 10000, 0, 25))
            .expect("add_order should succeed");

        let rejected = price_level.match_order(
            5,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );
        assert!(rejected.was_rejected());
        assert_eq!(price_level.order_count(), 1);

        // FOK of exactly the hidden size must fill (depth == 25).
        let filled = price_level.match_order(
            25,
            Id::from_u64(998),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_001),
            &trade_gen,
        );
        assert!(filled.is_complete());
        assert_eq!(
            filled
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            25
        );
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_match_result_consistent(&filled, 10000, Side::Sell);
    }

    #[test]
    fn test_zero_visible_reserve_auto_front_gtc_terminates_and_fills_behind() {
        // Front: zero-visible reserve, auto_replenish=true, hidden 50,
        // replenish_amount=20 (id 1). Behind: standard sell maker of 40 (id 2).
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_reserve_order(1, 10000, 0, 50, 10, true, Some(20)))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.hidden_quantity(), 50);
        assert_eq!(price_level.order_count(), 2);

        // GTC taker large enough to drain everything (40 + 50). MUST terminate.
        let result = price_level.match_order(
            200,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        // 40 from the standard maker + 50 from the reserve (drained tranche by
        // tranche). Total available depth is 90.
        assert_eq!(
            result
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            90
        );
        assert_eq!(result.remaining_quantity().as_u64(), 110);
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_zero_visible_reserve_auto_front_fok_prediction_matches_sweep() {
        // FOK of exactly the available depth (40 + 50 = 90) must fill.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_reserve_order(1, 10000, 0, 50, 10, true, Some(20)))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        let result = price_level.match_order(
            90,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert!(!result.was_killed());
        assert_eq!(
            result
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            90
        );
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);

        // And FOK of 91 (one over) must be killed with the queue intact.
        let price_level2 = PriceLevel::new(10000);
        let trade_gen2 = new_trade_id_generator();
        price_level2
            .add_order(create_reserve_order(1, 10000, 0, 50, 10, true, Some(20)))
            .expect("add_order should succeed");
        price_level2
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");
        let killed = price_level2.match_order(
            91,
            Id::from_u64(999),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen2,
        );
        assert!(killed.was_killed());
        assert_eq!(price_level2.order_count(), 2);
        assert_eq!(price_level2.visible_quantity(), 40);
        assert_eq!(price_level2.hidden_quantity(), 50);
    }

    #[test]
    fn test_zero_visible_reserve_no_auto_front_dropped_behind_fills() {
        // Front: zero-visible reserve, auto_replenish=FALSE, hidden 50 (id 1).
        // This reserve cannot replenish, so the sweep DROPS it (returns None)
        // without filling. Its hidden quantity is removed from the level. The
        // standard maker behind it (id 2, qty 40) must still match.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        price_level
            .add_order(create_reserve_order(1, 10000, 0, 50, 10, false, Some(20)))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.hidden_quantity(), 50);
        assert_eq!(price_level.order_count(), 2);

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        // Only the standard maker (40) is matchable; the non-replenishing,
        // zero-visible reserve is dropped without a trade.
        assert_eq!(
            result
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            40
        );
        assert_eq!(result.remaining_quantity().as_u64(), 60);
        // Both orders are gone from the queue (one filled, one dropped) and the
        // counters are consistent: hidden of the dropped reserve was removed.
        assert_eq!(price_level.order_count(), 0);
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);
    }

    #[test]
    fn test_zero_visible_reserve_no_auto_alone_depth_definitions_agree() {
        // A non-replenishing zero-visible reserve is NOT matchable depth: the
        // sweep would drop it (returns None) without ever filling. The two depth
        // views must AGREE on this: FOK's `matchable_quantity` sees 0 (kills,
        // leaving the queue intact since FOK is a pure pre-check), and PostOnly's
        // `has_matchable_depth` is false (so PostOnly is NOT rejected).

        // FOK pre-check: 0 matchable depth -> killed, queue untouched.
        let fok_level = PriceLevel::new(10000);
        let fok_gen = new_trade_id_generator();
        fok_level
            .add_order(create_reserve_order(1, 10000, 0, 50, 10, false, Some(20)))
            .expect("add_order should succeed");

        let fok = fok_level.match_order(
            5,
            Id::from_u64(998),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_001),
            &fok_gen,
        );
        assert!(fok.was_killed());
        assert_eq!(fok.remaining_quantity().as_u64(), 5);
        // FOK is a pre-check: the dead reserve is left resting, queue intact.
        assert_eq!(fok_level.order_count(), 1);
        assert_eq!(fok_level.hidden_quantity(), 50);

        // PostOnly on a fresh level: no matchable depth -> not rejected. It then
        // falls through to the sweep as an ordinary (zero-taking) taker, which
        // garbage-collects the unmatchable reserve with no trade and keeps the
        // counters consistent with the queue.
        let po_level = PriceLevel::new(10000);
        let po_gen = new_trade_id_generator();
        po_level
            .add_order(create_reserve_order(1, 10000, 0, 50, 10, false, Some(20)))
            .expect("add_order should succeed");

        let post_only = po_level.match_order(
            5,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(1_716_000_000_000),
            &po_gen,
        );
        assert!(!post_only.was_rejected());
        assert_eq!(post_only.trades().len(), 0);
        assert_eq!(
            post_only
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64(),
            0
        );
        // The non-matchable reserve is dropped by the fall-through sweep; the
        // hidden counter is decremented in lockstep with the queue removal.
        assert_eq!(po_level.order_count(), 0);
        assert_eq!(po_level.visible_quantity(), 0);
        assert_eq!(po_level.hidden_quantity(), 0);
    }

    #[test]
    fn test_update_quantity_zero_on_iceberg_then_match_terminates() {
        // Drive the iceberg into the degenerate zero-visible state via
        // update_order(UpdateQuantity { new_quantity: 0 }), then match. The
        // matcher must terminate and the maker behind must still fill.
        let price_level = PriceLevel::new(10000);
        let trade_gen = new_trade_id_generator();

        // Iceberg with 20 visible / 30 hidden, then a standard maker behind it.
        price_level
            .add_order(create_iceberg_order(1, 10000, 20, 30))
            .expect("add_order should succeed");
        price_level
            .add_order(create_sell_standard_order(2, 10000, 40))
            .expect("add_order should succeed");

        // Reduce the iceberg's quantity to 0 (degenerate zero-visible state).
        price_level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(1),
                new_quantity: Quantity::new(0),
            })
            .expect("update to zero quantity must succeed");

        // The matcher MUST terminate. A GTC taker drains whatever is matchable.
        let result = price_level.match_order(
            500,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &trade_gen,
        );

        // The standard maker (40) is matched regardless of how the zeroed
        // iceberg is resolved; the call terminates and counters stay consistent.
        assert!(
            result
                .executed_quantity()
                .expect("executed_quantity")
                .as_u64()
                >= 40
        );
        assert_eq!(price_level.visible_quantity(), 0);
        assert_match_result_consistent(&result, 10000, Side::Sell);

        // Snapshot round-trip must still hold after the degenerate match.
        let json = price_level
            .snapshot_to_json()
            .expect("snapshot_to_json after degenerate match");
        let restored =
            PriceLevel::from_snapshot_json(&json).expect("from_snapshot_json round-trip");
        assert_eq!(restored.visible_quantity(), price_level.visible_quantity());
        assert_eq!(restored.hidden_quantity(), price_level.hidden_quantity());
        assert_eq!(restored.order_count(), price_level.order_count());
    }

    // ------------------------------------------------------------------
    // Issue #81: real-implementation stress tests for `match_order` racing
    // `cancel` on the SAME price level. These exercise the per-entry-lock
    // protocol of `OrderQueue::match_front` (cancel either fully wins or fully
    // loses) under genuine `std::thread` concurrency with a `Barrier` start and
    // no `sleep`. loom proves the protocol exhaustively in `tests/loom/`; these
    // exercise the real DashMap / SkipMap structures it cannot instrument.
    // ------------------------------------------------------------------

    /// Assert the level's advisory counters agree with the queue contents read
    /// from a single consistent `snapshot()`, AND that no cancelled id is left
    /// silently resting.
    fn assert_counters_match_queue(level: &PriceLevel) {
        // `snapshot()` derives every aggregate from one materialized order
        // vector, so its counter fields are mutually consistent with its own
        // order list by construction (issue #62). Asserting on it (rather than on
        // the live atomics + a separate iteration) avoids a benign torn read of
        // two independent reads.
        let snapshot = level.snapshot();
        let orders = snapshot.orders();

        let visible_sum: u64 = orders.iter().map(|o| o.visible_quantity().as_u64()).sum();
        let hidden_sum: u64 = orders.iter().map(|o| o.hidden_quantity().as_u64()).sum();

        assert_eq!(
            snapshot.visible_quantity().as_u64(),
            visible_sum,
            "visible counter must equal the sum over the snapshot's own orders"
        );
        assert_eq!(
            snapshot.hidden_quantity().as_u64(),
            hidden_sum,
            "hidden counter must equal the sum over the snapshot's own orders"
        );
        assert_eq!(
            snapshot.order_count(),
            orders.len(),
            "order_count must equal the snapshot's own order-list length"
        );
    }

    #[test]
    fn test_match_order_concurrent_cancel_same_id_never_lost() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};
        use std::thread;

        const ITERATIONS: usize = 2_000;
        const PRICE: u128 = 10_000;
        // The single maker rests with quantity 10; the taker would consume 4,
        // leaving a residual of 6 on a clean (uncancelled) match.
        const MAKER_QTY: u64 = 10;
        const TAKER_QTY: u64 = 4;

        for iter in 0..ITERATIONS {
            let level = Arc::new(PriceLevel::new(PRICE));
            // Deterministic id derived from the iteration index.
            let maker_id_u64 = (iter as u64) * 4 + 1;
            let maker_id = Id::from_u64(maker_id_u64);
            // A sell maker so a buy taker crosses it.
            level
                .add_order(OrderType::Standard {
                    id: maker_id,
                    price: Price::new(PRICE),
                    quantity: Quantity::new(MAKER_QTY),
                    side: Side::Sell,
                    user_id: Hash32::zero(),
                    timestamp: TimestampMs::new(1_600_000_000_000 + iter as u64),
                    time_in_force: TimeInForce::Gtc,
                    extra_fields: (),
                })
                .expect("add_order should succeed");

            let barrier = Arc::new(Barrier::new(2));
            // Deterministic, per-iteration trade-id stream.
            let generator = Arc::new(UuidGenerator::new(Uuid::from_u128(
                0xA11C_E000_0000_0000u128 + iter as u128,
            )));

            let matcher = {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                let generator = Arc::clone(&generator);
                thread::spawn(move || {
                    barrier.wait();
                    level.match_order(
                        TAKER_QTY,
                        Id::from_u64(maker_id_u64 + 1),
                        TimeInForce::Gtc,
                        TakerKind::Standard,
                        TimestampMs::new(1_700_000_000_000),
                        &generator,
                    )
                })
            };

            let canceller = {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    level
                        .update_order(OrderUpdate::Cancel { order_id: maker_id })
                        .expect("cancel must not error")
                })
            };

            let result = matcher.join().expect("matcher thread panicked");
            let cancelled = canceller.join().expect("canceller thread panicked");

            // The cancel is never lost: either it removed the maker (Some), or
            // the match fully consumed it first (None). It is NEVER the case
            // that the maker is left silently resting with the cancel no-op'd.
            let cancel_won = cancelled.is_some();

            // Whatever the interleaving, the level's counters must agree with the
            // queue, and the maker must NOT be silently resting at full quantity.
            assert_counters_match_queue(&level);

            // The traded quantity plus what cancel removed plus what still rests
            // must conserve the original maker quantity. Read the residual from a
            // consistent snapshot.
            let snapshot = level.snapshot();
            let resting_ids: HashSet<Id> = snapshot.orders().iter().map(|o| o.id()).collect();
            let resting_qty: u64 = snapshot
                .orders()
                .iter()
                .filter(|o| o.id() == maker_id)
                .map(|o| o.visible_quantity().as_u64())
                .sum();

            let traded = result
                .executed_quantity()
                .expect("executed_quantity must not error")
                .as_u64();
            let cancelled_qty = cancelled
                .as_ref()
                .map_or(0, |o| o.visible_quantity().as_u64());

            assert_eq!(
                traded + cancelled_qty + resting_qty,
                MAKER_QTY,
                "iter {iter}: quantity not conserved (traded={traded} \
                 cancelled={cancelled_qty} resting={resting_qty})"
            );

            // The lost-cancel invariant: for the cancelled id, either a trade
            // consumed it fully (gone, cancel returned None) or the cancel
            // removed it (gone). If the cancel won, the maker must be absent.
            if cancel_won {
                assert!(
                    !resting_ids.contains(&maker_id),
                    "iter {iter}: cancel won but maker {maker_id} is still resting"
                );
            }
            // If the cancel lost (returned None), the match must have fully
            // consumed the maker (a partial fill would have left a residual that
            // the losing cancel would then have removed — so a None cancel here
            // means the maker is gone via trade).
            if !cancel_won {
                assert!(
                    !resting_ids.contains(&maker_id),
                    "iter {iter}: cancel returned None yet maker {maker_id} \
                     is still resting (lost cancel!)"
                );
                assert_eq!(
                    traded, MAKER_QTY,
                    "iter {iter}: cancel lost so the match must have fully \
                     consumed the maker"
                );
            }
        }
    }

    #[test]
    fn test_match_order_concurrent_cancel_different_ids_consistent() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};
        use std::thread;

        const ITERATIONS: usize = 400;
        const PRICE: u128 = 10_000;
        const MAKERS: u64 = 8;
        const MAKER_QTY: u64 = 5;

        for iter in 0..ITERATIONS {
            let level = Arc::new(PriceLevel::new(PRICE));

            // Add MAKERS sell makers with deterministic ids.
            let base = (iter as u64) * 1_000 + 1;
            for k in 0..MAKERS {
                level
                    .add_order(OrderType::Standard {
                        id: Id::from_u64(base + k),
                        price: Price::new(PRICE),
                        quantity: Quantity::new(MAKER_QTY),
                        side: Side::Sell,
                        user_id: Hash32::zero(),
                        timestamp: TimestampMs::new(1_600_000_000_000 + iter as u64 * 16 + k),
                        time_in_force: TimeInForce::Gtc,
                        extra_fields: (),
                    })
                    .expect("add_order should succeed");
            }

            // The matcher will consume the first ~2.5 makers; the canceller
            // cancels a DIFFERENT id near the back (id base+6), which the matcher
            // is unlikely to reach, exercising match || cancel(different id).
            let cancel_id = Id::from_u64(base + 6);
            let total_qty = MAKERS * MAKER_QTY;
            let taker_qty = MAKER_QTY * 2 + 2; // 12: two full makers + partial.

            let barrier = Arc::new(Barrier::new(2));
            let generator = Arc::new(UuidGenerator::new(Uuid::from_u128(
                0xB0B0_0000_0000_0000u128 + iter as u128,
            )));

            let matcher = {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                let generator = Arc::clone(&generator);
                thread::spawn(move || {
                    barrier.wait();
                    level.match_order(
                        taker_qty,
                        Id::from_u64(9_000_000 + iter as u64),
                        TimeInForce::Gtc,
                        TakerKind::Standard,
                        TimestampMs::new(1_700_000_000_000),
                        &generator,
                    )
                })
            };

            let canceller = {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    level
                        .update_order(OrderUpdate::Cancel {
                            order_id: cancel_id,
                        })
                        .expect("cancel must not error")
                })
            };

            let result = matcher.join().expect("matcher thread panicked");
            let cancelled = canceller.join().expect("canceller thread panicked");

            assert_counters_match_queue(&level);

            let traded = result
                .executed_quantity()
                .expect("executed_quantity must not error")
                .as_u64();
            let cancelled_qty = cancelled
                .as_ref()
                .map_or(0, |o| o.visible_quantity().as_u64());

            let snapshot = level.snapshot();
            let resting_qty: u64 = snapshot
                .orders()
                .iter()
                .map(|o| o.visible_quantity().as_u64())
                .sum();
            let resting_ids: HashSet<Id> = snapshot.orders().iter().map(|o| o.id()).collect();

            // Global conservation across all makers.
            assert_eq!(
                traded + cancelled_qty + resting_qty,
                total_qty,
                "iter {iter}: quantity not conserved (traded={traded} \
                 cancelled={cancelled_qty} resting={resting_qty})"
            );

            // The cancelled id must be gone whether the cancel won or the matcher
            // reached and consumed it. Either way it must not silently rest.
            assert!(
                !resting_ids.contains(&cancel_id),
                "iter {iter}: cancelled id {cancel_id} still resting"
            );
        }
    }

    // ------------------------------------------------------------------
    // Issue #102 — snapshot_by_insertion_seq (predicts match_order order)
    // ------------------------------------------------------------------

    #[test]
    fn test_snapshot_by_insertion_seq_matches_match_order_consumption() {
        // Build a Buy maker with an explicit (non-counter) timestamp so we can
        // make timestamps NON-monotonic with insertion order.
        let mk_buy = |id: u64, ts: u64, qty: u64| OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(10_000),
            quantity: Quantity::new(qty),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(ts),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };

        let level = PriceLevel::new(10_000);
        // Insert id 1 FIRST but with a LATER timestamp than id 2 (added second).
        level
            .add_order(mk_buy(1, 2_000, 50))
            .expect("add_order should succeed");
        level
            .add_order(mk_buy(2, 1_000, 50))
            .expect("add_order should succeed");

        let by_seq: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        let by_ts: Vec<Id> = level.snapshot_orders().iter().map(|o| o.id()).collect();

        // Insertion-sequence order is the order they were added: 1, 2.
        assert_eq!(by_seq, vec![Id::from_u64(1), Id::from_u64(2)]);
        // Timestamp order is 2, 1 (id 2 has the earlier timestamp) — different.
        assert_eq!(by_ts, vec![Id::from_u64(2), Id::from_u64(1)]);
        assert_ne!(
            by_seq, by_ts,
            "the two views must differ under non-monotonic timestamps"
        );

        // The sweep consumes in insertion-sequence order: id 1 then id 2.
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        let result = level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(3_000),
            &generator,
        );
        let consumed: Vec<Id> = result
            .trades()
            .as_vec()
            .iter()
            .map(|t| t.maker_order_id())
            .collect();
        assert_eq!(
            consumed, by_seq,
            "match_order consumes makers in snapshot_by_insertion_seq order"
        );
    }

    #[test]
    fn test_snapshot_restore_preserves_upsize_demotion() {
        // Issue #109: sizing an order up demotes it to the back of the queue
        // (remove+push mints a fresh insertion sequence) while keeping its
        // original admission timestamp. A snapshot round-trip must reproduce
        // that demotion, not let the order sort back to its timestamp position
        // and wrongly regain front priority.
        let level = PriceLevel::new(10_000);
        // Three standard makers with monotonic timestamps (TIMESTAMP_COUNTER),
        // so insertion sequence and timestamp order initially agree.
        level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("add_order should succeed");
        level
            .add_order(create_standard_order(2, 10_000, 100))
            .expect("add_order should succeed");
        level
            .add_order(create_standard_order(3, 10_000, 100))
            .expect("add_order should succeed");

        // Upsize maker 1 (Standard orders resize): total increases 100 -> 150,
        // so update_order takes the quantity-increase branch and demotes it to
        // the back, behind 2 and 3.
        let updated = level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(1),
                new_quantity: Quantity::new(150),
            })
            .expect("upsize update should succeed")
            .expect("maker 1 must still be present");
        assert_eq!(updated.visible_quantity().as_u64(), 150);

        // Pre-snapshot consumption order reflects the demotion: 2, 3, then 1.
        let pre_ids: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(
            pre_ids,
            vec![Id::from_u64(2), Id::from_u64(3), Id::from_u64(1)],
            "upsized maker 1 must sit at the back of the live queue"
        );
        let original_visible = level.visible_quantity();

        // Full JSON round-trip through the checksum-protected package.
        let json = level
            .snapshot_to_json()
            .expect("snapshot_to_json should succeed");
        let restored =
            PriceLevel::from_snapshot_json(&json).expect("from_snapshot_json should succeed");

        // The restored level reproduces the demoted consumption order exactly.
        let restored_ids: Vec<Id> = restored
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(
            restored_ids,
            vec![Id::from_u64(2), Id::from_u64(3), Id::from_u64(1)],
            "restore must preserve the upsize demotion, not regain front priority"
        );

        // Counters survive the round-trip: 100 + 100 + 150 = 350 visible.
        assert_eq!(
            restored.visible_quantity(),
            original_visible,
            "restored visible quantity must equal the original"
        );
        assert_eq!(restored.visible_quantity(), 350);

        // Draining the restored level to completion must emit trades in the
        // demoted maker order: 2, 3, then the upsized 1.
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        let result = restored.match_order(
            1_000,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(9_000),
            &generator,
        );
        let makers: Vec<Id> = result
            .trades()
            .as_vec()
            .iter()
            .map(|t| t.maker_order_id())
            .collect();
        assert_eq!(
            makers,
            vec![Id::from_u64(2), Id::from_u64(3), Id::from_u64(1)],
            "restored match_order must consume makers in the demoted order"
        );
    }

    #[test]
    fn test_snapshot_restore_preserves_iceberg_replenish_demotion() {
        // Issue #109 (same bug class as the upsize): an iceberg/reserve
        // replenishment re-queues the refreshed tranche at the TAIL
        // (ReplaceAtTail, fresh sequence) while keeping its ORIGINAL timestamp.
        // A snapshot round-trip must reproduce that demotion, not let the
        // refreshed tranche sort back to its timestamp position and regain
        // front priority.
        let level = PriceLevel::new(10_000);
        // Iceberg maker (id 1, Sell) added first: visible 50 over hidden 100.
        level
            .add_order(create_iceberg_order(1, 10_000, 50, 100))
            .expect("add_order should succeed");
        // A plain Sell maker (id 2) rests behind it.
        level
            .add_order(create_sell_standard_order(2, 10_000, 100))
            .expect("add_order should succeed");

        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        // Fully consume the iceberg's visible tranche (50): it replenishes from
        // hidden and is re-queued at the TAIL, behind maker 2.
        let _ = level.match_order(
            50,
            Id::from_u64(901),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        // Live consumption order reflects the replenish demotion: 2, then 1.
        let pre_ids: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(
            pre_ids,
            vec![Id::from_u64(2), Id::from_u64(1)],
            "replenished iceberg must sit at the back of the live queue"
        );
        let original_visible = level.visible_quantity();

        // Full JSON round-trip through the checksum-protected package.
        let json = level
            .snapshot_to_json()
            .expect("snapshot_to_json should succeed");
        let restored =
            PriceLevel::from_snapshot_json(&json).expect("from_snapshot_json should succeed");

        // The restored level reproduces the demoted consumption order exactly.
        let restored_ids: Vec<Id> = restored
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(
            restored_ids,
            vec![Id::from_u64(2), Id::from_u64(1)],
            "restore must preserve the replenish demotion, not regain front priority"
        );
        assert_eq!(
            restored.visible_quantity(),
            original_visible,
            "restored visible quantity must equal the original"
        );

        // Draining the restored level to completion must emit trades with
        // maker 2 before maker 1. The iceberg may emit several trades as it
        // replenishes, so compare the order-preserving de-duplicated makers.
        let drain = restored.match_order(
            1_000,
            Id::from_u64(902),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_001),
            &generator,
        );
        let mut deduped: Vec<Id> = Vec::new();
        for trade in drain.trades().as_vec() {
            let maker = trade.maker_order_id();
            if deduped.last() != Some(&maker) {
                deduped.push(maker);
            }
        }
        assert_eq!(
            deduped,
            vec![Id::from_u64(2), Id::from_u64(1)],
            "restored match_order must consume maker 2 fully before the demoted iceberg 1"
        );
    }

    #[test]
    fn test_snapshot_concurrent_resequencing_no_duplicates() {
        // Issue #110 (PR review): while a maker is re-sequenced (an upsize's
        // remove+push demotion), a concurrent reader must never observe the
        // same order twice — or a torn (order_count, orders) pair — in a
        // snapshot. The old `snapshot_by_seq` walked the SkipMap `index` and
        // could pin a stale `seq -> id` entry mid-re-sequencing, emit the
        // refreshed order at BOTH its old and new sequence, and hand
        // `snapshot()` a vector with a duplicate id whose fold corrupts the
        // restore. Deriving the view from the id-keyed `orders` map makes a
        // duplicate impossible by construction; this test guards that.
        use std::collections::HashSet;
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Barrier};
        use std::thread;

        const N: usize = 8;
        const WRITER_ITERS: usize = 300;
        const READERS: usize = 2;

        let level = Arc::new(PriceLevel::new(10_000));
        // N resting standard makers with known ids 1..=N and initial quantity
        // 100 (monotonic timestamps via the shared counter).
        for id in 1..=N as u64 {
            level
                .add_order(create_standard_order(id, 10_000, 100))
                .expect("add_order should succeed");
        }

        // Barrier aligns the single writer with the readers so the resequencing
        // churn and the snapshots genuinely overlap (global_rules requires a
        // Barrier start for concurrency tests).
        let barrier = Arc::new(Barrier::new(READERS + 1));
        let writer_done = Arc::new(AtomicBool::new(false));

        let writer = {
            let level = Arc::clone(&level);
            let barrier = Arc::clone(&barrier);
            let writer_done = Arc::clone(&writer_done);
            thread::spawn(move || {
                barrier.wait();
                for k in 0..WRITER_ITERS {
                    // Rotate through the ids; the target quantity strictly
                    // increases every time (1000 + k > any prior value assigned
                    // to this id, and > the initial 100), so each update takes
                    // the quantity-increase branch and demotes via remove+push.
                    let id = Id::from_u64((k % N) as u64 + 1);
                    let new_quantity = Quantity::new(1_000 + k as u64);
                    let _ = level
                        .update_order(OrderUpdate::UpdateQuantity {
                            order_id: id,
                            new_quantity,
                        })
                        .expect("upsize update must not error");
                }
                writer_done.store(true, Ordering::Release);
            })
        };

        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                let writer_done = Arc::clone(&writer_done);
                thread::spawn(move || {
                    barrier.wait();
                    loop {
                        // Observe the flag BEFORE taking the snapshot, then run
                        // one more check after it flips, so a final post-writer
                        // snapshot is always validated too.
                        let finished = writer_done.load(Ordering::Acquire);

                        let snap = level.snapshot();
                        let ids: HashSet<Id> = snap.orders().iter().map(|o| o.id()).collect();
                        assert_eq!(
                            ids.len(),
                            snap.orders().len(),
                            "snapshot contains a duplicate order id under concurrent resequencing"
                        );
                        assert_eq!(
                            snap.orders().len(),
                            snap.order_count(),
                            "snapshot order_count disagrees with its own orders vector"
                        );

                        // The snapshot must also round-trip into a level whose
                        // rebuilt queue length matches its order_count.
                        let restored =
                            PriceLevel::from_snapshot(snap).expect("from_snapshot must succeed");
                        assert_eq!(
                            restored.order_count(),
                            restored.snapshot_by_insertion_seq().len(),
                            "restored order_count disagrees with rebuilt queue length"
                        );

                        if finished {
                            break;
                        }
                    }
                })
            })
            .collect();

        writer.join().expect("writer thread panicked");
        for reader in readers {
            reader.join().expect("reader thread panicked");
        }

        // After the churn settles the level still holds exactly the N makers,
        // with no duplicates and counters consistent with the queue.
        assert_counters_match_queue(&level);
        let final_ids: HashSet<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(
            final_ids.len(),
            N,
            "the level must still hold exactly N distinct makers"
        );
    }

    #[test]
    fn test_snapshot_by_insertion_seq_empty_level() {
        let level = PriceLevel::new(10_000);
        assert!(level.snapshot_by_insertion_seq().is_empty());
    }

    #[test]
    fn test_snapshot_by_insertion_seq_partial_fill_keeps_front() {
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("add_order should succeed");
        level
            .add_order(create_standard_order(2, 10_000, 50))
            .expect("add_order should succeed");
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        // Small taker partially fills the front maker (id 1): KeepInPlace, same seq.
        let _ = level.match_order(
            30,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );
        let by_seq: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(
            by_seq,
            vec![Id::from_u64(1), Id::from_u64(2)],
            "a partially-filled front maker keeps the front"
        );
    }

    #[test]
    fn test_snapshot_by_insertion_seq_replenished_maker_moves_to_tail() {
        let level = PriceLevel::new(10_000);
        // Iceberg (id 1, Sell) added first: visible 10 over hidden 40.
        level
            .add_order(create_iceberg_order(1, 10_000, 10, 40))
            .expect("add_order should succeed");
        // A plain Sell maker (id 2) rests behind it.
        level
            .add_order(create_sell_standard_order(2, 10_000, 100))
            .expect("add_order should succeed");
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        // Fully consume the iceberg's visible tranche (10): it replenishes from
        // hidden and is re-queued at the TAIL (ReplaceAtTail, new sequence).
        let _ = level.match_order(
            10,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );
        let by_seq: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(
            by_seq,
            vec![Id::from_u64(2), Id::from_u64(1)],
            "a replenished maker moves to the tail"
        );
    }

    // ------------------------------------------------------------------
    // Issue #104 — snapshot_by_seq_into (buffer-reuse) + public
    // matchable_quantity
    // ------------------------------------------------------------------

    #[test]
    fn test_snapshot_by_seq_into_matches_snapshot_by_insertion_seq() {
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("add_order should succeed");
        level
            .add_order(create_standard_order(2, 10_000, 50))
            .expect("add_order should succeed");
        level
            .add_order(create_buy_iceberg_order(3, 10_000, 20, 30))
            .expect("add_order should succeed");

        let owned: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();

        let mut buf = Vec::new();
        level.snapshot_by_seq_into(&mut buf);
        let into: Vec<Id> = buf.iter().map(|o| o.id()).collect();

        assert_eq!(
            into, owned,
            "snapshot_by_seq_into must yield the same sequence as \
             snapshot_by_insertion_seq"
        );
        assert_eq!(
            into,
            vec![Id::from_u64(1), Id::from_u64(2), Id::from_u64(3)],
            "the sequence is ascending insertion order"
        );
    }

    #[test]
    fn test_snapshot_by_seq_into_reuses_buffer() {
        // Seed the scratch buffer from a level with THREE orders so the buffer
        // starts non-empty (proving `clear()` discards the prior contents
        // rather than appending to them).
        let big = PriceLevel::new(10_000);
        big.add_order(create_standard_order(1, 10_000, 100))
            .expect("add_order should succeed");
        big.add_order(create_standard_order(2, 10_000, 100))
            .expect("add_order should succeed");
        big.add_order(create_standard_order(3, 10_000, 100))
            .expect("add_order should succeed");
        let mut buf = big.snapshot_by_insertion_seq();
        assert_eq!(buf.len(), 3);

        // Reuse the same buffer on a SMALLER level: it must shrink to one entry
        // with no stale tail left over from the previous three.
        let small = PriceLevel::new(10_000);
        small
            .add_order(create_standard_order(10, 10_000, 100))
            .expect("add_order should succeed");
        small.snapshot_by_seq_into(&mut buf);
        let ids: Vec<Id> = buf.iter().map(|o| o.id()).collect();
        assert_eq!(
            ids,
            vec![Id::from_u64(10)],
            "buffer must be cleared, not appended to"
        );

        // Reuse the same buffer again on a LARGER level: it must grow and hold
        // exactly the new contents in insertion order.
        let bigger = PriceLevel::new(10_000);
        for id in [20_u64, 21, 22, 23] {
            bigger
                .add_order(create_standard_order(id, 10_000, 100))
                .expect("add_order should succeed");
        }
        bigger.snapshot_by_seq_into(&mut buf);
        let ids: Vec<Id> = buf.iter().map(|o| o.id()).collect();
        assert_eq!(
            ids,
            vec![
                Id::from_u64(20),
                Id::from_u64(21),
                Id::from_u64(22),
                Id::from_u64(23)
            ],
            "buffer must hold exactly the new contents, in insertion order"
        );
    }

    #[test]
    fn test_matchable_quantity_public_predicts_sweep() {
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();

        // Plain makers: 100 + 50 = 150 of depth.
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("add_order should succeed");
        level
            .add_order(create_standard_order(2, 10_000, 50))
            .expect("add_order should succeed");

        let taker = Id::from_u64(999);
        assert_eq!(
            level.matchable_quantity(0, taker),
            0,
            "zero taker fills nothing"
        );
        assert_eq!(
            level.matchable_quantity(120, taker),
            120,
            "taker below depth"
        );
        // A taker above the available depth is capped at the depth.
        let predicted = level.matchable_quantity(200, taker);
        assert_eq!(predicted, 150, "taker above depth is capped at depth");

        // The dry run does not mutate, so the real sweep on the same level must
        // consume exactly what was predicted.
        let generator = UuidGenerator::new(namespace);
        let result = level.match_order(
            200,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );
        assert_eq!(
            result.executed_quantity().unwrap_or_default().as_u64(),
            predicted,
            "match_order consumes exactly matchable_quantity"
        );

        // Iceberg replenish: visible 10 over hidden 40 = 50 of total depth, all
        // reachable across replenishment.
        let ice = PriceLevel::new(10_000);
        ice.add_order(create_iceberg_order(1, 10_000, 10, 40))
            .expect("add_order should succeed");
        let predicted_ice = ice.matchable_quantity(100, Id::from_u64(998));
        assert_eq!(
            predicted_ice, 50,
            "matchable_quantity reaches hidden depth via replenishment"
        );
        let generator = UuidGenerator::new(namespace);
        let result = ice.match_order(
            100,
            Id::from_u64(998),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );
        assert_eq!(
            result.executed_quantity().unwrap_or_default().as_u64(),
            predicted_ice,
            "match_order consumes exactly matchable_quantity for an iceberg"
        );
    }

    // ------------------------------------------------------------------
    // Issue #106 — MatchResult pre-alloc is bounded by the fill count,
    // not the whole level depth.
    // ------------------------------------------------------------------

    #[test]
    fn test_match_order_capacity_bounded_by_incoming_quantity() {
        // A deep level: 200 resting makers.
        let level = PriceLevel::new(10_000);
        for id in 1..=200_u64 {
            level
                .add_order(create_standard_order(id, 10_000, 100))
                .expect("add_order should succeed");
        }
        assert_eq!(level.order_count(), 200, "level is deep");

        // A qty-1 taker fills exactly one maker. Pre-#106 the result buffers
        // were reserved to `order_count` (200); now they are bounded by
        // `min(incoming_quantity, order_count) = 1`.
        let incoming = 1_u64;
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        let result = level.match_order(
            incoming,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        assert_eq!(result.trades().as_vec().len(), 1, "exactly one fill");
        assert!(
            result.trades().as_vec().capacity() <= incoming as usize,
            "trade buffer must be bounded by incoming quantity ({incoming}), not \
             level depth (200); was {}",
            result.trades().as_vec().capacity()
        );
    }

    #[test]
    fn test_match_order_capacity_bounded_by_order_count() {
        // A shallow level: 3 makers, 300 units of depth.
        let level = PriceLevel::new(10_000);
        for id in 1..=3_u64 {
            level
                .add_order(create_standard_order(id, 10_000, 100))
                .expect("add_order should succeed");
        }
        assert_eq!(level.order_count(), 3, "level is shallow");

        // A taker far larger than the level: the bound `min(incoming, depth)`
        // must pick the order count (3), never the huge incoming quantity.
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        let result = level.match_order(
            10_000,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        assert_eq!(result.trades().as_vec().len(), 3, "all three makers filled");
        assert!(
            result.trades().as_vec().capacity() <= 3,
            "trade buffer must be bounded by order count (3), not the incoming \
             quantity (10000); was {}",
            result.trades().as_vec().capacity()
        );
    }
    // ------------------------------------------------------------------
    // Issue #111 — reject quantity overflow BEFORE mutating level state
    // ------------------------------------------------------------------
    //
    // `order_count` overflow (usize::MAX resting orders) is not directly
    // testable — it would require ~1.8e19 live orders. It is covered by the
    // same checked `fetch_update` mechanism as the visible / hidden counters
    // below; a unit test cannot reach it, so it is exercised structurally
    // (identical code path) rather than by admitting that many orders.

    #[test]
    fn test_add_order_visible_quantity_overflow_rejected() {
        let level = PriceLevel::new(10_000);
        // Take the visible counter all the way to u64::MAX.
        level
            .add_order(create_standard_order(1, 10_000, u64::MAX))
            .expect("first admission at u64::MAX visible must succeed");

        // Capture the full level state before the failing admission.
        let before_json = level
            .snapshot_to_json()
            .expect("snapshot before must serialize");
        let before_visible = level.visible_quantity();
        let before_hidden = level.hidden_quantity();
        let before_count = level.order_count();

        // Admitting even one more unit would overflow the visible counter.
        match level.add_order(create_standard_order(2, 10_000, 1)) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(
                    message.contains("visible quantity overflow"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected visible-overflow InvalidOperation, got {other:?}"),
        }

        // Nothing mutated: counters, count, and a byte-identical snapshot.
        assert_eq!(level.visible_quantity(), before_visible);
        assert_eq!(level.hidden_quantity(), before_hidden);
        assert_eq!(level.order_count(), before_count);
        assert_eq!(before_count, 1);
        let after_json = level
            .snapshot_to_json()
            .expect("snapshot after must serialize");
        assert_eq!(
            before_json, after_json,
            "a rejected admission must leave the snapshot byte-identical"
        );

        // The snapshot still round-trips, and the rejected order is absent.
        let restored =
            PriceLevel::from_snapshot_json(&after_json).expect("snapshot must round-trip");
        assert_eq!(restored.visible_quantity(), u64::MAX);
        assert_eq!(restored.order_count(), 1);
        let ids: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(ids, vec![Id::from_u64(1)]);
    }

    #[test]
    fn test_add_order_hidden_quantity_overflow_rejected() {
        let level = PriceLevel::new(10_000);
        // Take the hidden counter to u64::MAX with a hidden-only iceberg.
        level
            .add_order(create_iceberg_order(1, 10_000, 0, u64::MAX))
            .expect("first admission at u64::MAX hidden must succeed");

        let before_visible = level.visible_quantity();
        let before_hidden = level.hidden_quantity();
        let before_count = level.order_count();

        match level.add_order(create_iceberg_order(2, 10_000, 0, 1)) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(
                    message.contains("hidden quantity overflow"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected hidden-overflow InvalidOperation, got {other:?}"),
        }

        // The visible reservation the failing call briefly took is rolled back,
        // so no counter drifts.
        assert_eq!(level.visible_quantity(), before_visible);
        assert_eq!(level.hidden_quantity(), before_hidden);
        assert_eq!(level.order_count(), before_count);
        assert_eq!(before_count, 1);
        assert_eq!(level.hidden_quantity(), u64::MAX);
    }

    #[test]
    fn test_add_order_boundary_sum_reaches_u64_max_succeeds() {
        let level = PriceLevel::new(10_000);
        // Two admissions whose visible quantities sum to EXACTLY u64::MAX must
        // both succeed — the boundary is inclusive.
        level
            .add_order(create_standard_order(1, 10_000, u64::MAX - 10))
            .expect("first admission must succeed");
        level
            .add_order(create_standard_order(2, 10_000, 10))
            .expect("admission reaching exactly u64::MAX must succeed");

        assert_eq!(level.visible_quantity(), u64::MAX);
        assert_eq!(level.order_count(), 2);

        // Counter == snapshot aggregate == sum over the queue contents.
        let snapshot = level.snapshot();
        assert_eq!(snapshot.visible_quantity().as_u64(), u64::MAX);
        let queue_sum = level
            .snapshot_by_insertion_seq()
            .iter()
            .try_fold(0u64, |acc, o| {
                acc.checked_add(o.visible_quantity().as_u64())
            })
            .expect("boundary sum is exactly u64::MAX, no overflow");
        assert_eq!(queue_sum, u64::MAX);
    }

    #[test]
    fn test_reserve_own_total_overflow_rejected_at_admission() {
        // A reserve whose OWN visible + hidden overflows u64 is now rejected at
        // admission (issue #111 follow-up): the level cannot hold an order whose
        // total quantity is not representable, and the match sweep's replenish
        // add relies on that invariant. Nothing is mutated on rejection.
        let level = PriceLevel::new(10_000);
        match level.add_order(create_reserve_order(
            1,
            10_000,
            u64::MAX,       // visible
            u64::MAX,       // hidden -> visible + hidden overflows u64
            u64::MAX,       // threshold
            true,           // auto_replenish
            Some(u64::MAX), // replenish amount
        )) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(
                    message.contains("order total quantity overflows u64"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected order-total-overflow InvalidOperation, got {other:?}"),
        }

        // The level is untouched: no counters, no order, no stats moved.
        assert_eq!(level.visible_quantity(), 0);
        assert_eq!(level.hidden_quantity(), 0);
        assert_eq!(level.order_count(), 0);
        assert_eq!(level.snapshot_by_insertion_seq().len(), 0);
    }

    #[test]
    fn test_iceberg_own_total_overflow_rejected_at_admission() {
        // Same per-order invariant for an iceberg: visible + hidden must fit u64.
        let level = PriceLevel::new(10_000);
        match level.add_order(create_iceberg_order(1, 10_000, u64::MAX, u64::MAX)) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(
                    message.contains("order total quantity overflows u64"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected order-total-overflow InvalidOperation, got {other:?}"),
        }

        assert_eq!(level.visible_quantity(), 0);
        assert_eq!(level.hidden_quantity(), 0);
        assert_eq!(level.order_count(), 0);
        assert_eq!(level.snapshot_by_insertion_seq().len(), 0);
    }

    #[test]
    fn test_from_snapshot_rejects_order_own_total_overflow() {
        // The restore path admits orders too, so it enforces the SAME per-order
        // total invariant as add_order: a snapshot carrying an order whose own
        // visible + hidden overflows u64 is rejected (via the topology scan in
        // `refresh_aggregates`, shared by `from_snapshot` and the checksum
        // package path) rather than smuggled in.
        let overflowing = create_iceberg_order(1, 10_000, u64::MAX, u64::MAX);
        let snapshot = crate::price_level::PriceLevelSnapshot::from_raw_parts(
            Price::new(10_000),
            // Stored aggregates are recomputed by `refresh_aggregates`; the
            // per-order scan rejects the order before they matter.
            Quantity::new(0),
            Quantity::new(0),
            1,
            vec![std::sync::Arc::new(overflowing)],
        );

        match PriceLevel::from_snapshot(snapshot) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(
                    message.contains("order total quantity overflows u64"),
                    "unexpected message: {message}"
                );
            }
            other => {
                panic!("expected order-total-overflow InvalidOperation on restore, got {other:?}")
            }
        }
    }

    #[test]
    fn test_replenish_would_wrap_level_counter_aborts_sweep_no_trade() {
        // Issue #111 follow-up, finding 2: even when every order's OWN total
        // fits u64, converting hidden depth to visible can push the LEVEL visible
        // counter (and the true queue visible sum) past u64::MAX. The sweep must
        // abort at the FIFO front rather than wrap the counter or trade a younger
        // maker.
        //
        // auto-reserve(visible 1, hidden 100, replenish 100) admitted FIRST
        // (own total 101, fits) + standard(visible u64::MAX - 1) admitted second
        // (own total fits). Level visible counter = 1 + (u64::MAX - 1) = u64::MAX.
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_reserve_order(
                1,
                10_000,
                1,
                100,
                100,
                true,
                Some(100),
            ))
            .expect("reserve own total fits u64");
        level
            // Sell to stay side-coherent with the reserve above (issue #120
            // pins the level side to its first resting maker).
            .add_order(create_sell_standard_order(2, 10_000, u64::MAX - 1))
            .expect("standard own total fits u64");
        assert_eq!(level.visible_quantity(), u64::MAX);

        let before_json = level.snapshot_to_json().expect("snapshot serializes");

        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        // A one-unit taker: consuming the reserve's visible 1 IS representable,
        // but the +100 replenish would take the level visible counter to
        // u64::MAX + 99 -> the sweep aborts at the reserve.
        let result = level.match_order(
            1,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        // Zero trades, taker remainder full: the reserve's 1-unit fill was NOT
        // emitted because it is inseparable from the overflowing replenish.
        assert_eq!(result.trades().len(), 0, "aborted sweep must emit no trade");
        assert_eq!(
            result.remaining_quantity().as_u64(),
            1,
            "the taker must be left fully unconsumed"
        );

        // NO wrap: the level visible counter is still exactly u64::MAX.
        assert_eq!(
            level.visible_quantity(),
            u64::MAX,
            "the level visible counter must not have wrapped"
        );
        assert_eq!(level.hidden_quantity(), 100, "hidden counter unchanged");
        assert_eq!(level.order_count(), 2, "both makers still rest");

        // Both makers are byte-identical, and FIFO is preserved: the younger
        // standard maker did NOT trade and the reserve is untouched.
        let resting = level.snapshot_by_insertion_seq();
        assert_eq!(resting.len(), 2);
        assert_eq!(resting[0].id(), Id::from_u64(1));
        assert_eq!(resting[0].visible_quantity().as_u64(), 1);
        assert_eq!(resting[0].hidden_quantity().as_u64(), 100);
        assert_eq!(resting[1].id(), Id::from_u64(2));
        assert_eq!(resting[1].visible_quantity().as_u64(), u64::MAX - 1);

        // counters == queue == snapshot: the snapshot is byte-identical to the
        // pre-match one, proving no counter drifted from the queue.
        let after_json = level.snapshot_to_json().expect("snapshot serializes");
        assert_eq!(
            before_json, after_json,
            "an aborted sweep must leave the level byte-identical"
        );
    }

    #[test]
    fn test_update_order_upsize_wrapping_level_counter_rejected() {
        // update_order must reject a quantity update whose counter delta would
        // wrap the level visible counter, leaving the level unchanged and
        // deterministic. Fill the visible counter to u64::MAX with a standard
        // maker, add a tiny second maker, then try to upsize the tiny one — its
        // +delta would take the counter past u64::MAX.
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, u64::MAX - 5))
            .expect("first admission ok");
        level
            .add_order(create_standard_order(2, 10_000, 5))
            .expect("second admission reaches exactly u64::MAX");
        assert_eq!(level.visible_quantity(), u64::MAX);

        let before_json = level.snapshot_to_json().expect("snapshot serializes");

        // Upsize maker 2 from 5 to 10: delta +5 would wrap the level counter.
        match level.update_order(OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(2),
            new_quantity: Quantity::new(10),
        }) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(
                    message.contains("price level quantity counter overflow on update"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected counter-overflow InvalidOperation, got {other:?}"),
        }

        // Nothing mutated: counters, order sizes, and a byte-identical snapshot.
        assert_eq!(level.visible_quantity(), u64::MAX);
        assert_eq!(level.order_count(), 2);
        let after_json = level.snapshot_to_json().expect("snapshot serializes");
        assert_eq!(
            before_json, after_json,
            "a rejected update must leave the level byte-identical"
        );
    }

    #[test]
    fn test_add_order_normal_flow_fifo_unchanged() {
        // Sanity: the now-fallible add_order preserves normal admission and
        // strict FIFO consumption for in-range quantities.
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, 30))
            .expect("admission ok");
        level
            .add_order(create_standard_order(2, 10_000, 20))
            .expect("admission ok");
        level
            .add_order(create_standard_order(3, 10_000, 50))
            .expect("admission ok");

        assert_eq!(level.visible_quantity(), 100);
        assert_eq!(level.order_count(), 3);

        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        let result = level.match_order(
            100,
            Id::from_u64(999),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );
        let makers: Vec<Id> = result
            .trades()
            .as_vec()
            .iter()
            .map(|t| t.maker_order_id())
            .collect();
        assert_eq!(
            makers,
            vec![Id::from_u64(1), Id::from_u64(2), Id::from_u64(3)],
            "FIFO consumption order must be unchanged"
        );
    }

    // ------------------------------------------------------------------
    // Issue #113 — reject duplicate order IDs atomically
    // ------------------------------------------------------------------

    #[test]
    fn test_add_order_duplicate_id_rejected_sequentially() {
        let level = PriceLevel::new(10_000);
        let first = level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("first admission must succeed");
        level
            .add_order(create_standard_order(2, 10_000, 50))
            .expect("second (distinct) admission must succeed");

        // Snapshot the full state before the duplicate attempt.
        let before_json = level.snapshot_to_json().expect("snapshot before");
        let before_visible = level.visible_quantity();
        let before_hidden = level.hidden_quantity();
        let before_count = level.order_count();
        let before_ids: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();

        // Re-submit id 1 with a DIFFERENT quantity: must be rejected, never
        // overwrite the live order.
        match level.add_order(create_standard_order(1, 10_000, 999)) {
            Err(PriceLevelError::DuplicateOrderId(id)) => {
                assert_eq!(id, Id::from_u64(1).to_string())
            }
            other => panic!("expected DuplicateOrderId, got {other:?}"),
        }

        // Nothing changed: counters, order, FIFO order, and a byte-identical
        // snapshot.
        assert_eq!(level.visible_quantity(), before_visible);
        assert_eq!(level.hidden_quantity(), before_hidden);
        assert_eq!(level.order_count(), before_count);
        assert_eq!(
            level.snapshot_to_json().expect("snapshot after"),
            before_json,
            "a rejected duplicate must leave the snapshot byte-identical"
        );
        let after_ids: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(after_ids, before_ids);

        // The original order 1 kept its quantity (100), not the rejected 999.
        let order1 = level
            .snapshot_by_insertion_seq()
            .into_iter()
            .find(|o| o.id() == Id::from_u64(1))
            .expect("order 1 must still rest");
        assert_eq!(order1.visible_quantity().as_u64(), 100);
        assert_eq!(first.id(), Id::from_u64(1));
    }

    #[test]
    fn test_add_order_duplicate_id_across_variants_rejected() {
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("standard admission must succeed");

        // The same id as a DIFFERENT order variant is still a duplicate.
        let duplicates = [
            create_buy_iceberg_order(1, 10_000, 50, 50),
            create_buy_reserve_order(1, 10_000, 30, 60, 10, true, Some(20)),
            create_standard_order(1, 10_000, 5),
        ];
        for dup in duplicates {
            match level.add_order(dup) {
                Err(PriceLevelError::DuplicateOrderId(id)) => {
                    assert_eq!(id, Id::from_u64(1).to_string())
                }
                other => panic!("expected DuplicateOrderId across variants, got {other:?}"),
            }
        }

        // The original standard order 1 is intact; the level still holds one.
        assert_eq!(level.order_count(), 1);
        let resting = level.snapshot_by_insertion_seq();
        assert_eq!(resting.len(), 1);
        assert_eq!(resting[0].id(), Id::from_u64(1));
        assert_eq!(resting[0].visible_quantity().as_u64(), 100);

        // A genuinely distinct id still admits fine.
        level
            .add_order(create_buy_iceberg_order(2, 10_000, 50, 50))
            .expect("distinct id must admit");
        assert_eq!(level.order_count(), 2);
    }

    #[test]
    fn test_add_order_duplicate_id_concurrent_exactly_one_wins() {
        use std::sync::{Arc as StdArc, Barrier};
        use std::thread;

        const THREADS: usize = 8;
        const ITERATIONS: usize = 50;
        const DUP_ID: u64 = 1;

        for iter in 0..ITERATIONS {
            let level = StdArc::new(PriceLevel::new(10_000));
            let barrier = StdArc::new(Barrier::new(THREADS));

            let handles: Vec<_> = (0..THREADS)
                .map(|t| {
                    let level = StdArc::clone(&level);
                    let barrier = StdArc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        // All threads submit the SAME id with distinct sizes.
                        level
                            .add_order(OrderType::Standard {
                                id: Id::from_u64(DUP_ID),
                                price: Price::new(10_000),
                                quantity: Quantity::new(10 + t as u64),
                                side: Side::Buy,
                                user_id: Hash32::zero(),
                                timestamp: TimestampMs::new(1_600_000_000_000 + t as u64),
                                time_in_force: TimeInForce::Gtc,
                                extra_fields: (),
                            })
                            .is_ok()
                    })
                })
                .collect();

            let successes: usize = handles
                .into_iter()
                .map(|h| usize::from(h.join().expect("thread panicked")))
                .sum();
            assert_eq!(successes, 1, "iter {iter}: exactly one admission must win");

            // The level is consistent: exactly one order, counters == queue ==
            // snapshot, and the id-keyed map / ordered index are 1:1.
            assert_eq!(level.order_count(), 1, "iter {iter}: order_count must be 1");
            let ids: Vec<Id> = level
                .snapshot_by_insertion_seq()
                .iter()
                .map(|o| o.id())
                .collect();
            assert_eq!(
                ids,
                vec![Id::from_u64(DUP_ID)],
                "iter {iter}: exactly one id rests, once"
            );
            let snapshot = level.snapshot();
            assert_eq!(snapshot.order_count(), 1);
            assert_eq!(snapshot.orders().len(), 1);
            assert_eq!(
                level.visible_quantity(),
                snapshot.visible_quantity().as_u64(),
                "iter {iter}: counter must equal the snapshot aggregate"
            );
            assert_eq!(
                snapshot.visible_quantity().as_u64(),
                snapshot.orders()[0].visible_quantity().as_u64(),
                "iter {iter}: aggregate must equal the single resting order"
            );

            // Draining consumes the single maker exactly once — proof there is
            // no phantom second index entry pointing at the same map value.
            let generator = UuidGenerator::new(Uuid::from_u128(0xD00D_0000 + iter as u128));
            let result = level.match_order(
                10_000,
                Id::from_u64(9_999),
                TimeInForce::Gtc,
                TakerKind::Standard,
                TimestampMs::new(1_700_000_000_000),
                &generator,
            );
            let makers: Vec<Id> = result
                .trades()
                .as_vec()
                .iter()
                .map(|t| t.maker_order_id())
                .collect();
            assert_eq!(
                makers,
                vec![Id::from_u64(DUP_ID)],
                "iter {iter}: the maker must be consumed exactly once"
            );
        }
    }

    #[test]
    fn test_from_snapshot_rejects_duplicate_ids() {
        // Build a snapshot whose orders vector repeats id 1.
        let dup_a = std::sync::Arc::new(create_standard_order(1, 10_000, 100));
        let dup_b = std::sync::Arc::new(create_standard_order(1, 10_000, 50));
        let snapshot = crate::price_level::PriceLevelSnapshot::with_orders(
            Price::new(10_000),
            vec![dup_a, dup_b],
        )
        .expect("snapshot construction must succeed");

        // Direct from_snapshot rejects deterministically — no level built.
        match PriceLevel::from_snapshot(snapshot.clone()) {
            Err(PriceLevelError::DuplicateOrderId(id)) => {
                assert_eq!(id, Id::from_u64(1).to_string())
            }
            other => panic!("expected DuplicateOrderId from from_snapshot, got {other:?}"),
        }

        // The checksum-protected JSON path rejects too — with a DuplicateOrderId
        // (the checksum is valid), not a ChecksumMismatch.
        let json = PriceLevelSnapshotPackage::new(snapshot)
            .expect("package must build")
            .to_json()
            .expect("package must serialize");
        assert!(
            matches!(
                PriceLevel::from_snapshot_json(&json),
                Err(PriceLevelError::DuplicateOrderId(_))
            ),
            "from_snapshot_json must reject a duplicate-id snapshot"
        );
    }

    #[test]
    fn test_add_order_duplicate_id_at_counter_capacity_returns_duplicate() {
        // Finding 2 (PR #125): admission decides id IDENTITY before reserving
        // any counter, so a duplicate id submitted when the level's visible
        // counter is already at u64::MAX reports DuplicateOrderId — NOT a
        // spurious visible-overflow InvalidOperation — and leaves every counter
        // byte-identical (no transient inflation an overflow-first order would
        // cause).
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, u64::MAX))
            .expect("first admission fills the visible counter to u64::MAX");
        assert_eq!(level.visible_quantity(), u64::MAX);

        let before_json = level.snapshot_to_json().expect("snapshot before");

        // Re-submit id 1 with a positive quantity: reserving it WOULD overflow
        // the visible counter, but the duplicate id takes precedence.
        match level.add_order(create_standard_order(1, 10_000, 100)) {
            Err(PriceLevelError::DuplicateOrderId(id)) => {
                assert_eq!(id, Id::from_u64(1).to_string());
            }
            other => {
                panic!("expected DuplicateOrderId (identity before counters), got {other:?}")
            }
        }

        // Byte-identical: counters, count, and snapshot unchanged.
        assert_eq!(level.visible_quantity(), u64::MAX);
        assert_eq!(level.hidden_quantity(), 0);
        assert_eq!(level.order_count(), 1);
        assert_eq!(
            level.snapshot_to_json().expect("snapshot after"),
            before_json,
            "a duplicate at counter capacity must leave the level byte-identical"
        );
    }

    // ------------------------------------------------------------------
    // Issue #120 — admission and trade topology invariants
    // ------------------------------------------------------------------

    #[test]
    fn test_add_order_wrong_price_rejected() {
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("in-price admission must succeed");

        let before = level.snapshot_to_json().expect("snapshot before");
        // An order at a different price must be rejected, level unchanged.
        match level.add_order(create_standard_order(2, 10_001, 50)) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(message.contains("price"), "unexpected message: {message}");
            }
            other => panic!("expected wrong-price InvalidOperation, got {other:?}"),
        }
        assert_eq!(level.order_count(), 1);
        assert_eq!(
            level.snapshot_to_json().expect("snapshot after"),
            before,
            "a rejected wrong-price admission must leave the level unchanged"
        );
    }

    #[test]
    fn test_try_from_snapshot_propagates_duplicate_order_id() {
        // Finding 3 (PR #125): the infallible `From<&PriceLevelSnapshot>` (which
        // silently kept-first on a duplicate id while restoring counters over
        // every copy) is replaced by `TryFrom`, which delegates to
        // `from_snapshot` and propagates DuplicateOrderId.
        let dup_a = std::sync::Arc::new(create_standard_order(7, 10_000, 100));
        let dup_b = std::sync::Arc::new(create_standard_order(7, 10_000, 50));
        let snapshot = crate::price_level::PriceLevelSnapshot::with_orders(
            Price::new(10_000),
            vec![dup_a, dup_b],
        )
        .expect("snapshot construction must succeed");

        match PriceLevel::try_from(&snapshot) {
            Err(PriceLevelError::DuplicateOrderId(id)) => {
                assert_eq!(id, Id::from_u64(7).to_string());
            }
            other => panic!("expected DuplicateOrderId from TryFrom<&Snapshot>, got {other:?}"),
        }

        // A duplicate-free snapshot restores successfully through TryFrom.
        let ok_snapshot = crate::price_level::PriceLevelSnapshot::with_orders(
            Price::new(10_000),
            vec![
                std::sync::Arc::new(create_standard_order(1, 10_000, 10)),
                std::sync::Arc::new(create_standard_order(2, 10_000, 20)),
            ],
        )
        .expect("snapshot construction must succeed");
        let restored = PriceLevel::try_from(&ok_snapshot).expect("distinct ids restore");
        assert_eq!(restored.order_count(), 2);
        assert_eq!(restored.visible_quantity(), 30);
    }

    #[test]
    fn test_add_order_mixed_side_rejected_then_readmissible_after_drain() {
        let level = PriceLevel::new(10_000);
        // First maker pins the level side to Buy.
        level
            .add_order(create_standard_order(1, 10_000, 100))
            .expect("first (Buy) admission must succeed");

        let before = level.snapshot_to_json().expect("snapshot before");
        // A Sell maker is incompatible with the Buy level.
        match level.add_order(create_sell_standard_order(2, 10_000, 50)) {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(message.contains("side"), "unexpected message: {message}");
            }
            other => panic!("expected mixed-side InvalidOperation, got {other:?}"),
        }
        assert_eq!(level.order_count(), 1);
        assert_eq!(
            level.snapshot_to_json().expect("snapshot after"),
            before,
            "a rejected mixed-side admission must leave the level unchanged"
        );

        // Drain the level to empty via a full match.
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let generator = UuidGenerator::new(namespace);
        let _ = level.match_order(
            100,
            Id::from_u64(900),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );
        assert_eq!(level.order_count(), 0, "the level must be drained empty");

        // A drained level accepts either side again: the opposite side now admits.
        level
            .add_order(create_sell_standard_order(3, 10_000, 70))
            .expect("a drained level must re-accept the opposite side");
        assert_eq!(level.order_count(), 1);
    }

    #[test]
    fn test_match_order_self_match_terminal_rejected_all_tifs() {
        // Issue #126: a self-match is TERMINAL. If the taker's own id rests at
        // the level, the match emits NO trades and leaves the level
        // byte-identical for EVERY TIF and kind — it does NOT walk past its own
        // resting order to trade with the other makers (the old skip behaviour).
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();

        // Every TIF, plus the post-only kind, must reject identically.
        let cases: [(TimeInForce, TakerKind); 6] = [
            (TimeInForce::Gtc, TakerKind::Standard),
            (TimeInForce::Ioc, TakerKind::Standard),
            (TimeInForce::Fok, TakerKind::Standard),
            (TimeInForce::Day, TakerKind::Standard),
            (TimeInForce::Gtc, TakerKind::PostOnly),
            (TimeInForce::Fok, TakerKind::PostOnly),
        ];

        for (tif, kind) in cases {
            // Makers 1, 2, 3 rest in FIFO order; the taker shares maker 1's id.
            let level = PriceLevel::new(10_000);
            level
                .add_order(create_standard_order(1, 10_000, 40))
                .expect("maker 1 admits");
            level
                .add_order(create_standard_order(2, 10_000, 30))
                .expect("maker 2 admits");
            level
                .add_order(create_standard_order(3, 10_000, 50))
                .expect("maker 3 admits");
            let before = level.snapshot_by_insertion_seq();

            let result = level.match_order(
                1_000,
                Id::from_u64(1),
                tif,
                kind,
                TimestampMs::new(1_700_000_000_000),
                &UuidGenerator::new(namespace),
            );

            // Terminal Rejected: zero trades, full remaining, nothing executed.
            assert!(
                result.was_rejected(),
                "self-match must be Rejected for {tif:?}/{kind:?}"
            );
            assert_eq!(result.trades().len(), 0, "{tif:?}/{kind:?}: no trades");
            assert_eq!(
                result.remaining_quantity().as_u64(),
                1_000,
                "{tif:?}/{kind:?}: full remaining"
            );
            assert_eq!(
                result.executed_quantity().expect("no overflow").as_u64(),
                0,
                "{tif:?}/{kind:?}: nothing executed"
            );

            // The level is byte-identical: all three makers still rest in order.
            let after = level.snapshot_by_insertion_seq();
            assert_eq!(
                before.iter().map(|o| o.id()).collect::<Vec<_>>(),
                after.iter().map(|o| o.id()).collect::<Vec<_>>(),
                "{tif:?}/{kind:?}: queue unchanged"
            );
            assert_eq!(level.order_count(), 3, "{tif:?}/{kind:?}: count unchanged");
            assert_counters_match_queue(&level);
        }
    }

    #[test]
    fn test_matchable_quantity_self_skip_but_match_order_rejects_self() {
        // Maker 1 (shares the taker id) has 40; maker 2 has 60. The dry-run
        // helper `matchable_quantity` still skips the self-trade maker (it backs
        // the in-sweep defense-in-depth path, where the taker's order is admitted
        // mid-sweep), so it reports 60 takeable.
        let level = PriceLevel::new(10_000);
        level
            .add_order(create_standard_order(1, 10_000, 40))
            .expect("maker 1 admits");
        level
            .add_order(create_standard_order(2, 10_000, 60))
            .expect("maker 2 admits");

        assert_eq!(level.matchable_quantity(100, Id::from_u64(1)), 60);
        assert_eq!(level.matchable_quantity(60, Id::from_u64(1)), 60);

        // But `match_order` is TERMINAL when the taker id already rests (issue
        // #126): it rejects up front for every TIF, dominating the FOK dry run —
        // a self-match FOK is Rejected, NOT killed and NOT filled from maker 2.
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        for qty in [100u64, 60] {
            let result = level.match_order(
                qty,
                Id::from_u64(1),
                TimeInForce::Fok,
                TakerKind::Standard,
                TimestampMs::new(1_700_000_000_000),
                &UuidGenerator::new(namespace),
            );
            assert!(
                result.was_rejected(),
                "self-match FOK({qty}) is Rejected, not killed/filled"
            );
            assert!(!result.was_killed(), "self-match is Rejected, not Killed");
            assert_eq!(result.trades().len(), 0, "no trades on self-match");
            assert_eq!(result.remaining_quantity().as_u64(), qty);
            assert_eq!(
                level.order_count(),
                2,
                "a rejected self-match leaves the queue untouched"
            );
        }

        // A taker with a DISTINCT id (id 3) does take maker 1 + maker 2 normally.
        let filled = level.match_order(
            100,
            Id::from_u64(3),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_001),
            &UuidGenerator::new(namespace),
        );
        assert!(filled.is_complete(), "non-self FOK 100 fills 40 + 60");
        let makers: Vec<Id> = filled
            .trades()
            .as_vec()
            .iter()
            .map(|t| t.maker_order_id())
            .collect();
        assert_eq!(makers, vec![Id::from_u64(1), Id::from_u64(2)]);
    }

    #[test]
    fn test_opposite_side_admissions_race_exactly_one_wins() {
        // Issue #126 🔴: two opposite-side admissions racing into a genuinely
        // empty level must NEVER both admit — the atomic side pin serializes
        // them so exactly one wins and the other is rejected. Under the old
        // derive-from-queue scheme both could observe an empty queue and admit.
        use std::sync::{Arc, Barrier};
        use std::thread;

        const ITERATIONS: usize = 3_000;
        const PRICE: u128 = 10_000;

        for iter in 0..ITERATIONS {
            let level = Arc::new(PriceLevel::new(PRICE));
            let barrier = Arc::new(Barrier::new(2));
            let buy_id = iter as u64 * 2 + 1;
            let sell_id = buy_id + 1;

            let buyer = {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    level.add_order(create_standard_order(buy_id, PRICE, 10))
                })
            };
            let seller = {
                let level = Arc::clone(&level);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    level.add_order(create_sell_standard_order(sell_id, PRICE, 10))
                })
            };

            let buy_res = buyer.join().expect("buyer thread panicked");
            let sell_res = seller.join().expect("seller thread panicked");

            // Exactly one admission wins.
            let admitted = usize::from(buy_res.is_ok()) + usize::from(sell_res.is_ok());
            assert_eq!(
                admitted,
                1,
                "iter {iter}: exactly one opposite-side admission may win (buy_ok={}, sell_ok={})",
                buy_res.is_ok(),
                sell_res.is_ok()
            );
            // The loser is rejected with an incompatible-side error.
            if let Err(err) = &buy_res {
                assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
            }
            if let Err(err) = &sell_res {
                assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
            }

            // The level holds exactly one order; snapshot is single-side; the
            // advisory counters agree with the queue.
            assert_eq!(level.order_count(), 1, "iter {iter}");
            let snap = level.snapshot();
            assert_eq!(snap.orders().len(), 1, "iter {iter}");
            assert_counters_match_queue(&level);

            // A drained level re-accepts EITHER side (the pin un-pinned on drain).
            let winner_id = if buy_res.is_ok() { buy_id } else { sell_id };
            level
                .update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(winner_id),
                })
                .expect("cancel winner")
                .expect("winner was resting");
            assert_eq!(level.order_count(), 0, "iter {iter}: drained");
            // Whichever side lost the race can now be admitted into the empty level.
            let readmit = if buy_res.is_ok() {
                level.add_order(create_sell_standard_order(sell_id, PRICE, 7))
            } else {
                level.add_order(create_standard_order(buy_id, PRICE, 7))
            };
            assert!(
                readmit.is_ok(),
                "iter {iter}: a drained level must re-accept the opposite side"
            );
        }
    }

    #[test]
    fn test_snapshot_never_captures_torn_side_under_flips() {
        // Issue #126 🔴: a snapshot walk that spans a drain-then-re-admit to the
        // opposite side must never capture a torn old-side/new-side view (which
        // `from_snapshot` would reject for mixed sides). The topology epoch makes
        // `snapshot` retry across such a transition; here a flipper thread churns
        // the level Buy-batch -> drained -> Sell-batch -> drained while the main
        // thread takes many snapshots and asserts each is single-side.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;

        const PRICE: u128 = 10_000;
        const BATCH: u64 = 8;

        let level = Arc::new(PriceLevel::new(PRICE));
        let done = Arc::new(AtomicBool::new(false));

        let flipper = {
            let level = Arc::clone(&level);
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let mut round = 0u64;
                while !done.load(Ordering::Relaxed) {
                    let buy = round.is_multiple_of(2);
                    let base = 1_000 + round * BATCH;
                    for i in 0..BATCH {
                        let id = base + i;
                        let order = if buy {
                            create_standard_order(id, PRICE, 5)
                        } else {
                            create_sell_standard_order(id, PRICE, 5)
                        };
                        // May transiently fail if the opposite side is still
                        // draining; that is fine, we just churn the topology.
                        let _ = level.add_order(order);
                    }
                    for i in 0..BATCH {
                        let _ = level.update_order(OrderUpdate::Cancel {
                            order_id: Id::from_u64(base + i),
                        });
                    }
                    round += 1;
                }
            })
        };

        for _ in 0..50_000 {
            let snap = level.snapshot();
            let mut side = None;
            for order in snap.orders() {
                match side {
                    None => side = Some(order.side()),
                    Some(s) => assert_eq!(
                        s,
                        order.side(),
                        "snapshot captured a torn mixed-side view (issue #126)"
                    ),
                }
            }
        }

        done.store(true, Ordering::Relaxed);
        flipper.join().expect("flipper thread panicked");
    }

    #[test]
    fn test_from_snapshot_rejects_wrong_price_and_mixed_side() {
        // Wrong price: an order whose price differs from the level's.
        let wrong_price = crate::price_level::PriceLevelSnapshot::with_orders(
            Price::new(10_000),
            vec![
                std::sync::Arc::new(create_standard_order(1, 10_000, 100)),
                std::sync::Arc::new(create_standard_order(2, 10_001, 50)),
            ],
        )
        .expect("snapshot construction succeeds");
        assert!(
            matches!(
                PriceLevel::from_snapshot(wrong_price),
                Err(PriceLevelError::InvalidOperation { .. })
            ),
            "from_snapshot must reject a wrong-price order"
        );

        // Mixed side: Buy and Sell orders in one snapshot.
        let mixed_side = crate::price_level::PriceLevelSnapshot::with_orders(
            Price::new(10_000),
            vec![
                std::sync::Arc::new(create_standard_order(1, 10_000, 100)),
                std::sync::Arc::new(create_sell_standard_order(2, 10_000, 50)),
            ],
        )
        .expect("snapshot construction succeeds");
        assert!(
            matches!(
                PriceLevel::from_snapshot(mixed_side),
                Err(PriceLevelError::InvalidOperation { .. })
            ),
            "from_snapshot must reject a mixed-side snapshot"
        );
    }
}

#[cfg(test)]
mod tests_eq {
    use crate::PriceLevel;

    #[test]
    fn test_price_level_partial_eq() {
        // Create two price levels with the same price
        let price_level1 = PriceLevel::new(10000);
        let price_level2 = PriceLevel::new(10000);

        // Create a price level with a different price
        let price_level3 = PriceLevel::new(10001);

        // Test equality
        assert_eq!(price_level1, price_level2);

        // Test inequality
        assert_ne!(price_level1, price_level3);
        assert_ne!(price_level2, price_level3);
    }

    #[test]
    fn test_price_level_eq() {
        // Test Eq trait (reflexivity, symmetry, transitivity)
        let price_level1 = PriceLevel::new(10000);
        let price_level2 = PriceLevel::new(10000);
        let price_level3 = PriceLevel::new(10000);

        // Reflexivity: a == a
        assert_eq!(price_level1, price_level1);

        // Symmetry: if a == b then b == a
        assert_eq!(price_level1, price_level2);
        assert_eq!(price_level2, price_level1);

        // Transitivity: if a == b and b == c then a == c
        assert_eq!(price_level1, price_level2);
        assert_eq!(price_level2, price_level3);
        assert_eq!(price_level1, price_level3);
    }

    #[test]
    fn test_price_level_partial_ord() {
        let price_level1 = PriceLevel::new(10000);
        let price_level2 = PriceLevel::new(10500);
        let price_level3 = PriceLevel::new(9500);

        // Test comparisons
        assert!(price_level1 < price_level2);
        assert!(price_level3 < price_level1);
        assert!(price_level3 < price_level2);

        assert!(price_level2 > price_level1);
        assert!(price_level1 > price_level3);
        assert!(price_level2 > price_level3);

        assert!(price_level1 <= price_level2);
        assert!(price_level1 <= price_level1); // Equality case

        assert!(price_level2 >= price_level1);
        assert!(price_level1 >= price_level1); // Equality case
    }

    #[test]
    fn test_price_level_ord() {
        // Create some price levels
        let price_level1 = PriceLevel::new(9000);
        let price_level2 = PriceLevel::new(10000);
        let price_level3 = PriceLevel::new(11000);

        // Create a vector of price level references
        let mut price_level_refs = [&price_level3, &price_level1, &price_level2];

        // Sort the vector - this uses the Ord implementation
        price_level_refs.sort();

        // Verify the sorting order (ascending by price)
        assert_eq!(price_level_refs[0].price(), 9000);
        assert_eq!(price_level_refs[1].price(), 10000);
        assert_eq!(price_level_refs[2].price(), 11000);

        // Test the comparison methods directly
        assert_eq!(price_level1.cmp(&price_level2), std::cmp::Ordering::Less);
        assert_eq!(price_level2.cmp(&price_level1), std::cmp::Ordering::Greater);
        assert_eq!(price_level2.cmp(&price_level2), std::cmp::Ordering::Equal);
    }
}
