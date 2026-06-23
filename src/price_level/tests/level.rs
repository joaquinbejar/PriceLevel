#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
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
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_iceberg_order(2, 10000, 50, 200));

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
        price_level.add_order(create_standard_order(1, 20000, 100));

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
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 100));
        price_level.add_order(create_standard_order(3, 10000, 100));

        // Execution timestamp is comfortably after the order arrival timestamps
        // (TIMESTAMP_COUNTER starts at 1_616_823_000_000), so waiting times are
        // positive and deterministic.
        let execution_ts = TimestampMs::new(1_716_000_000_000);

        // First match: fully consumes maker 1 and partially maker 2.
        let _ = price_level.match_order(150, Id::from_u64(900), execution_ts, &trade_id_generator);
        // Second match: consumes the rest of maker 2 and part of maker 3.
        let _ = price_level.match_order(60, Id::from_u64(901), execution_ts, &trade_id_generator);

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
        price_level.add_order(create_standard_order(1, 10000, 100));

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
        price_level.add_order(create_standard_order(1, 15000, 100));
        price_level.add_order(create_iceberg_order(2, 15000, 40, 120));
        price_level.add_order(create_post_only_order(3, 15000, 60));
        price_level.add_order(create_reserve_order(4, 15000, 30, 90, 15, true, Some(20)));

        let snapshot = price_level.snapshot();
        let restored = PriceLevel::from(&snapshot);

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
        price_level.add_order(create_standard_order(10, 17500, 80));
        price_level.add_order(create_trailing_stop_order(11, 17500, 50));
        price_level.add_order(create_pegged_order(12, 17500, 40));
        price_level.add_order(create_market_to_limit_order(13, 17500, 70));

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

        let order_arc = price_level.add_order(order);

        assert_eq!(price_level.visible_quantity(), 100);
        assert_eq!(price_level.hidden_quantity(), 0);
        assert_eq!(price_level.order_count(), 1);
        assert!(matches!(price_level.total_quantity(), Ok(100)));

        // Verify the returned Arc contains the expected order
        assert_eq!(order_arc.id(), Id::from_u64(1));
        assert_eq!(order_arc.price(), Price::new(10000));
        assert_eq!(order_arc.visible_quantity(), 100);

        // Verify stats
        assert_eq!(price_level.stats().orders_added(), 1);
    }

    #[test]
    fn test_add_iceberg_order() {
        let price_level = PriceLevel::new(10000);
        let order = create_iceberg_order(2, 10000, 50, 200);

        price_level.add_order(order);

        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.hidden_quantity(), 200);
        assert_eq!(price_level.order_count(), 1);
        assert!(matches!(price_level.total_quantity(), Ok(250)));
    }

    #[test]
    fn test_add_multiple_orders() {
        let price_level = PriceLevel::new(10000);

        // Add different order types
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_iceberg_order(2, 10000, 50, 200));
        price_level.add_order(create_post_only_order(3, 10000, 75));
        price_level.add_order(create_reserve_order(4, 10000, 25, 100, 100, true, None));

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

        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_iceberg_order(2, 10000, 50, 200));

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

        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_iceberg_order(2, 10000, 50, 200));

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

        price_level.add_order(create_standard_order(1, 10000, 100));

        // Match the entire order
        let taker_id = Id::from_u64(999); // market order ID
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity(), 0);
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
            price_level.add_order(mk(1, 40));
            price_level.add_order(mk(2, 30));
            price_level.add_order(mk(3, 50));

            let trade_id_generator = UuidGenerator::new(namespace);
            price_level.match_order(90, taker_id, timestamp, &trade_id_generator)
        };

        let first = run();
        let second = run();

        let first_trades = first.trades().as_vec();
        let second_trades = second.trades().as_vec();

        // Crossed two full makers (40 + 30) and partially filled the third (20).
        assert_eq!(first_trades.len(), 3);
        assert_eq!(first.executed_quantity().unwrap_or_default(), 90);
        assert_eq!(second.executed_quantity().unwrap_or_default(), 90);

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

        price_level.add_order(create_standard_order(1, 10000, 100));

        // Match part of the order
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            60,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Verificar el resultado de matching
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity(), 0);
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

        price_level.add_order(create_standard_order(1, 10000, 100));

        // Match with quantity exceeding available
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            150,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity(), 50); // 150 - 100 = 50 remaining
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
        price_level.add_order(create_iceberg_order(1, 10000, 50, 100));

        // Match the visible portion of the iceberg order.
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Assertions to validate the match result.
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity(), 0);
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
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity(), 0);
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
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_iceberg_order(1, 10000, 100, 100));

        // Match the visible portion of the iceberg order.
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Assertions to validate the match result.
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity(), 0);
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
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity(), 0);
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
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );
        assert_eq!(match_result.remaining_quantity(), 50);
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

        price_level.add_order(create_iceberg_order(1, 10000, 50, 150));

        // Match part of the visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            30,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 150, 20, false, None));

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 150, 20, true, None));

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 150, 20, false, None));

        // Match partially, but still above threshold
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            25,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 25); // 50 - 25 = 25
        assert_eq!(price_level.hidden_quantity(), 150); // No change to hidden quantity

        // Match more to go below threshold
        let taker_id = Id::from_u64(1000);
        let match_result = price_level.match_order(
            10,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(
            1,
            10000,
            50,
            150,
            20,
            true,
            Some(custom_amount),
        ));

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 150, 0, true, None));

        // Match partially
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            49,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 150, 0, false, None));

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 150, 1, false, None));

        // Match the entire visible portion
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 150, 20, false, None));

        // Match part of the visible portion, but still above threshold
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            25,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 25); // 50 - 25 = 25
        assert_eq!(price_level.hidden_quantity(), 150); // No replenishment yet

        // Match more to go below threshold
        let taker_id = Id::from_u64(1000);
        let match_result = price_level.match_order(
            10,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
        price_level.add_order(create_reserve_order(1, 10000, 100, 100, 20, true, None));

        // Match 80 units, which is above the replenish threshold
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            80,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Validate the match result
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity(), 0);
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
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
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
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 40); // 150 - 90 - 20 = 40
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

        price_level.add_order(create_post_only_order(1, 10000, 100));

        // Post-only orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            60,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 40);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_match_trailing_stop_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_trailing_stop_order(1, 10000, 100));

        // Trailing stop orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_pegged_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_pegged_order(1, 10000, 100));

        // Pegged orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_match_market_to_limit_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_market_to_limit_order(1, 10000, 100));

        // Market-to-limit orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_fill_or_kill_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_fill_or_kill_order(1, 10000, 100));

        // For the price level, FOK behaves like standard orders
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_immediate_or_cancel_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_immediate_or_cancel_order(1, 10000, 100));

        // For the price level, IOC behaves like standard orders
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            50,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 50);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_match_good_till_date_order() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_good_till_date_order(1, 10000, 100, 1617000000000));

        // GTD orders behave like standard orders for matching
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            100,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 0);
        assert_eq!(price_level.order_count(), 0);
    }

    #[test]
    fn test_match_multiple_orders() {
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_standard_order(1, 10000, 50));
        price_level.add_order(create_standard_order(2, 10000, 75));
        price_level.add_order(create_standard_order(3, 10000, 25));

        // Match first two orders completely and third partially
        let taker_id = Id::from_u64(999);
        let match_result = price_level.match_order(
            140,
            taker_id,
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        // Verificar el resultado de matching
        assert_eq!(match_result.order_id(), taker_id);
        assert_eq!(match_result.remaining_quantity(), 0);
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
        assert_eq!(orders[0].visible_quantity(), 10);
        assert_eq!(orders[0].hidden_quantity(), 0);
    }

    #[test]
    fn test_snapshot() {
        let price_level = PriceLevel::new(10000);

        // Add some orders
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 50));

        // Create a snapshot
        let snapshot = price_level.snapshot();

        // Verify snapshot data
        assert_eq!(snapshot.price(), 10000);
        assert_eq!(snapshot.visible_quantity(), 150); // 100 + 50
        assert_eq!(snapshot.hidden_quantity(), 0);
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
        price_level.add_order(order);

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
        price_level.add_order(order);

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
        price_level.add_order(order);

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
        assert_eq!(updated_order.unwrap().visible_quantity(), 150);

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
        assert_eq!(updated_order.unwrap().visible_quantity(), 50);

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
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 100));

        // Reduce A's quantity (decrease). A must keep its front position.
        let result = price_level.update_order(OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(40),
        });
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().visible_quantity(), 40);

        // Match a quantity that only consumes the first resting order. A (id 1)
        // must be hit before B (id 2).
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);
        let execution_ts = TimestampMs::new(1_716_000_000_000);
        let match_result =
            price_level.match_order(40, Id::from_u64(900), execution_ts, &trade_id_generator);

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
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 100));

        // Increase A's quantity (Standard orders support resizing). This must
        // demote A to the back of the queue, behind B.
        let result = price_level.update_order(OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64(1),
            new_quantity: Quantity::new(150),
        });
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().visible_quantity(), 150);

        // A subsequent match that only consumes the first resting order must
        // now hit B (id 2) before the resized A (id 1).
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);
        let execution_ts = TimestampMs::new(1_716_000_000_000);
        let match_result =
            price_level.match_order(100, Id::from_u64(900), execution_ts, &trade_id_generator);

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
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 100));
        price_level.add_order(create_iceberg_order(3, 10000, 50, 200));

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
        let expected_visible: u64 = snapshot.iter().map(|o| o.visible_quantity()).sum();
        let expected_hidden: u64 = snapshot.iter().map(|o| o.hidden_quantity()).sum();

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
        price_level.add_order(order);

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
        price_level.add_order(order);

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
        assert_eq!(updated_order.unwrap().visible_quantity(), 150);

        // The price level should reflect the new quantity
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_update_order_replace() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        let order = create_standard_order(1, 10000, 100);
        price_level.add_order(order);

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
        price_level.add_order(order);

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
        assert_eq!(updated_order.unwrap().visible_quantity(), 150);

        // The price level should reflect the new quantity
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);
    }

    // Test the From<&PriceLevel> implementation for PriceLevelData
    #[test]
    fn test_price_level_data_from_price_level() {
        let price_level = PriceLevel::new(10000);

        // Add some orders
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 50));

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
        price_level.add_order(create_standard_order(1, 10000, 100));

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
        price_level.add_order(create_standard_order(1, 10000, 50));
        price_level.add_order(create_standard_order(2, 10000, 75));
        price_level.add_order(create_good_till_date_order(3, 10000, 100, 1617000000000));
        price_level.add_order(create_reserve_order(4, 10000, 100, 100, 20, true, None));
        price_level.add_order(create_iceberg_order(5, 10000, 50, 100));

        let input = "PriceLevel:price=10000;visible_quantity=375;hidden_quantity=200;order_count=5;orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=10000;quantity=50;side=BUY;timestamp=1616823000000;time_in_force=GTC,Standard:id=00000000-0000-0002-0000-000000000000;price=10000;quantity=75;side=BUY;timestamp=1616823000001;time_in_force=GTC,Standard:id=00000000-0000-0003-0000-000000000000;price=10000;quantity=100;side=BUY;timestamp=1616823000002;time_in_force=GTD-1617000000000,ReserveOrder:id=00000000-0000-0004-0000-000000000000;price=10000;visible_quantity=100;hidden_quantity=100;side=SELL;timestamp=1616823000003;time_in_force=GTC;replenish_threshold=20;replenish_amount=None;auto_replenish=true,IcebergOrder:id=00000000-0000-0005-0000-000000000000;price=10000;visible_quantity=50;hidden_quantity=100;side=SELL;timestamp=1616823000004;time_in_force=GTC]";
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
        assert_eq!(orders[0].visible_quantity(), 50);
    }

    // Test serialization and deserialization for PriceLevel
    #[test]
    fn test_price_level_serde() {
        let price_level = PriceLevel::new(10000);
        price_level.add_order(create_standard_order(1, 10000, 100));

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
        assert_eq!(orders[0].visible_quantity(), 100);
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
        price_level.add_order(create_standard_order(1, 10000, 200));

        // Match only part of what's available
        let match_result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimestampMs::new(1_716_000_000_000),
            &transaction_id_generator,
        );

        assert_eq!(match_result.remaining_quantity(), 0);
        assert!(match_result.is_complete());
        assert_eq!(price_level.visible_quantity(), 100); // 200 - 100 = 100
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_level_update_price_different_price() {
        let price_level = PriceLevel::new(10000);

        // Add an order
        price_level.add_order(create_standard_order(1, 10000, 100));

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
        price_level.add_order(create_standard_order(1, 10000, 100));

        // Update the quantity but keep the same price
        let result = price_level.update_order(OrderUpdate::UpdatePriceAndQuantity {
            order_id: Id::from_u64(1),
            new_price: Price::new(10000), // Same price
            new_quantity: Quantity::new(150),
        });

        assert!(result.is_ok());
        let updated_order = result.unwrap().unwrap();
        assert_eq!(updated_order.visible_quantity(), 150);
        assert_eq!(price_level.visible_quantity(), 150);
        assert_eq!(price_level.order_count(), 1);
    }

    #[test]
    fn test_serialize_deserialize_with_orders() {
        let price_level = PriceLevel::new(10000);

        // Add some orders
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_iceberg_order(2, 10000, 50, 150));

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
        price_level.add_order(order);

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
        price_level.add_order(order);

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
        price_level.add_order(order);

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
        price_level.add_order(order);

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
        price_level.add_order(new_order);

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
        price_level.add_order(order);

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
        price_level.add_order(order1);

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
        price_level.add_order(order2);

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
        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 100));

        // First aggressor partially fills A (60 of 100). A's residual = 40.
        let first = price_level.match_order(
            60,
            Id::from_u64(901),
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

        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 100));
        let total_before = match price_level.total_quantity() {
            Ok(q) => q,
            Err(e) => panic!("total_quantity failed: {e}"),
        };
        assert_eq!(total_before, 200);

        let _ = price_level.match_order(
            60,
            Id::from_u64(901),
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
        price_level.add_order(create_iceberg_order(1, 10000, 50, 100));
        // O (id=2) arrives later: a plain 50 (iceberg with no hidden).
        price_level.add_order(create_iceberg_order(2, 10000, 50, 0));

        // Aggressor consumes I's visible tip (50) → I refreshes from hidden and
        // moves to the tail. remaining hits 0, so this call stops there.
        let first = price_level.match_order(
            50,
            Id::from_u64(901),
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

        price_level.add_order(create_standard_order(1, 10000, 100));
        price_level.add_order(create_standard_order(2, 10000, 100));
        let _ = price_level.match_order(
            60,
            Id::from_u64(901),
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

        let visible_sum: u64 = orders.iter().map(|order| order.visible_quantity()).sum();
        let hidden_sum: u64 = orders.iter().map(|order| order.hidden_quantity()).sum();

        assert_eq!(
            snapshot.visible_quantity(),
            visible_sum,
            "snapshot visible_quantity must equal the sum over its own orders"
        );
        assert_eq!(
            snapshot.hidden_quantity(),
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
                        level.add_order(create_standard_order(id, PRICE, 1 + (base % 7)));
                    } else {
                        level.add_order(create_iceberg_order(
                            id,
                            PRICE,
                            1 + (base % 5),
                            1 + (base % 11),
                        ));
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
    /// - `filled_order_ids().len()` equals the count of fully-consumed makers.
    fn assert_match_result_consistent(result: &crate::execution::MatchResult, level_price: u128) {
        // is_complete <=> remaining_quantity == 0
        assert_eq!(
            result.is_complete(),
            result.remaining_quantity() == 0,
            "is_complete must agree with remaining_quantity == 0"
        );

        let trades = result.trades().as_vec();

        // executed_quantity == sum of trade quantities.
        let expected_qty: u64 = trades.iter().map(|t| t.quantity().as_u64()).sum();
        let executed_qty = match result.executed_quantity() {
            Ok(q) => q,
            Err(e) => panic!("executed_quantity must not error on real output: {e}"),
        };
        assert_eq!(
            executed_qty, expected_qty,
            "executed_quantity must equal the sum of trade quantities"
        );

        // executed_value == sum of each trade's price * quantity.
        let expected_value: u128 = trades
            .iter()
            .map(|t| t.price().as_u128() * u128::from(t.quantity().as_u64()))
            .sum();
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

        assert_match_result_trades_valid(result, level_price);
    }

    /// Assert the structural invariants on every `Trade` emitted by a real
    /// `match_order` call: maker != taker, price == level price, quantity > 0,
    /// and taker side is the opposite of the maker side.
    ///
    /// All makers in these scenarios rest on a single side per level, so the
    /// taker side is uniform across the sweep and equals that maker side's
    /// opposite. The trade already records `taker_side`; we cross-check it
    /// against the known resting side.
    fn assert_match_result_trades_valid(result: &crate::execution::MatchResult, level_price: u128) {
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
            // taker_side == maker_side.opposite(). `maker_side()` is derived as
            // `taker_side().opposite()`, so this also pins the relationship.
            assert_eq!(
                trade.taker_side(),
                trade.maker_side().opposite(),
                "taker side must be the opposite of the maker side"
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

        price_level.add_order(create_standard_order(1, 10000, 100));

        let result = price_level.match_order(
            40,
            Id::from_u64(999),
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        // The taker (40) is exhausted against the maker (100): complete.
        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity(), 0);
        assert_eq!(result.trades().len(), 1);
        // The maker is only partially filled and remains resting.
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_eq!(price_level.order_count(), 1);
        assert_match_result_consistent(&result, 10000);
    }

    #[test]
    fn test_match_order_exact_full_fill_result_invariants_hold() {
        // Taker exactly equals total resting depth across two makers: every
        // maker is fully consumed and the taker is complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_standard_order(1, 10000, 60));
        price_level.add_order(create_standard_order(2, 10000, 40));

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity(), 0);
        assert_eq!(result.trades().len(), 2);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_match_result_consistent(&result, 10000);
    }

    #[test]
    fn test_match_order_taker_larger_than_depth_result_invariants_hold() {
        // Taker exceeds resting depth: queue drained, all makers filled, and a
        // positive remainder is left so the result is NOT complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_standard_order(1, 10000, 30));
        price_level.add_order(create_standard_order(2, 10000, 30));

        let result = price_level.match_order(
            100,
            Id::from_u64(999),
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity(), 40);
        assert_eq!(result.trades().len(), 2);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_eq!(price_level.order_count(), 0);
        assert_match_result_consistent(&result, 10000);
    }

    #[test]
    fn test_match_order_multi_maker_sweep_result_invariants_hold() {
        // Sweep three makers, partially filling the last: two fully-consumed
        // makers, three trades, taker complete.
        let price_level = PriceLevel::new(10000);
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let trade_id_generator = UuidGenerator::new(namespace);

        price_level.add_order(create_standard_order(1, 10000, 40));
        price_level.add_order(create_standard_order(2, 10000, 30));
        price_level.add_order(create_standard_order(3, 10000, 50));

        let result = price_level.match_order(
            90,
            Id::from_u64(999),
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity(), 0);
        assert_eq!(result.trades().len(), 3);
        assert_eq!(result.filled_order_ids().len(), 2);
        assert_match_result_consistent(&result, 10000);
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
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(!result.is_complete());
        assert_eq!(result.remaining_quantity(), 50);
        assert_eq!(result.trades().len(), 0);
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_match_result_consistent(&result, 10000);
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
        price_level.add_order(create_iceberg_order(1, 10000, 50, 200));

        // Consume the full visible tranche; the maker replenishes and keeps
        // resting, so it is not in filled_order_ids.
        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity(), 0);
        assert!(!result.trades().as_vec().is_empty());
        assert_eq!(result.filled_order_ids().len(), 0);
        assert_match_result_consistent(&result, 10000);
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
        price_level.add_order(create_reserve_order(1, 10000, 50, 200, 10, true, Some(50)));

        let result = price_level.match_order(
            50,
            Id::from_u64(999),
            TimestampMs::new(1_716_000_000_000),
            &trade_id_generator,
        );

        assert!(result.is_complete());
        assert_eq!(result.remaining_quantity(), 0);
        assert!(!result.trades().as_vec().is_empty());
        assert_match_result_consistent(&result, 10000);
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
            price_level.add_order(mk());
            let trade_id_generator = UuidGenerator::new(namespace);
            // Cross more than the visible tranche to force replenishment and a
            // multi-trade stream.
            price_level.match_order(120, taker_id, timestamp, &trade_id_generator)
        };

        let first = run();
        let second = run();

        assert_eq!(first.trades().as_vec(), second.trades().as_vec());
        assert_match_result_consistent(&first, 10000);
        assert_match_result_consistent(&second, 10000);
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
