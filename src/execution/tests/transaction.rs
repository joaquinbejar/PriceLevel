#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::execution::trade::Trade;
    use crate::orders::{Id, Side};
    use crate::utils::{Price, Quantity, TimestampMs, UnixClock};
    use std::cell::Cell;
    use std::str::FromStr;
    use std::time::{Duration, UNIX_EPOCH};
    use uuid::Uuid;

    fn create_test_trade() -> Trade {
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        Trade::with_timestamp(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(10000),
            Quantity::new(5),
            Side::Buy,
            TimestampMs::new(1616823000000),
        )
    }

    #[test]
    fn test_transaction_display() {
        let transaction = create_test_trade();
        let display_str = transaction.to_string();

        assert!(display_str.starts_with("Trade:"));
        assert!(display_str.contains("trade_id=6ba7b810-9dad-11d1-80b4-00c04fd430c8"));
        assert!(display_str.contains("taker_order_id=00000000-0000-0001-0000-000000000000"));
        assert!(display_str.contains("maker_order_id=00000000-0000-0002-0000-000000000000"));
        assert!(display_str.contains("price=10000"));
        assert!(display_str.contains("quantity=5"));
        assert!(display_str.contains("taker_side=BUY"));
        assert!(display_str.contains("timestamp=1616823000000"));
    }

    #[test]
    fn test_transaction_from_str_valid() {
        let input = "Trade:trade_id=6ba7b810-9dad-11d1-80b4-00c04fd430c8;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let transaction = Trade::from_str(input).unwrap();
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(transaction.trade_id(), Id::from_uuid(uuid));
        assert_eq!(transaction.taker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.maker_order_id(), Id::from_u64(2));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(5));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(transaction.timestamp(), TimestampMs::new(1616823000000));
    }

    #[test]
    fn test_transaction_from_str_invalid_format() {
        let input = "InvalidFormat";
        let result = Trade::from_str(input);
        assert!(result.is_err());

        let input = "Trade;trade_id=12345";
        let result = Trade::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_from_str_missing_field() {
        // Missing quantity field
        let input = "Trade:trade_id=6ba7b810-9dad-11d1-80b4-00c04fd430c8;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;taker_side=BUY;timestamp=1616823000000";
        let result = Trade::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::MissingField(field) => {
                assert_eq!(field, "quantity");
            }
            err => panic!("Expected MissingField error, got {err:?}"),
        }
    }

    #[test]
    fn test_transaction_from_str_invalid_field_value() {
        // Invalid trade_id
        let input = "Trade:trade_id=abc;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Trade::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::InvalidFieldValue { field, value } => {
                assert_eq!(field, "trade_id");
                assert_eq!(value, "abc");
            }
            err => panic!("Expected InvalidFieldValue error, got {err:?}"),
        }

        // Invalid taker_order_id
        let input = "Trade:trade_id=12345;taker_order_id=abc;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Trade::from_str(input);
        assert!(result.is_err());

        // Invalid side
        let input = "Trade:trade_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=INVALID;timestamp=1616823000000";
        let result = Trade::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_round_trip() {
        let original = create_test_trade();
        let string_representation = original.to_string();
        let parsed = Trade::from_str(&string_representation).unwrap();

        assert_eq!(parsed.trade_id(), original.trade_id());
        assert_eq!(parsed.taker_order_id(), original.taker_order_id());
        assert_eq!(parsed.maker_order_id(), original.maker_order_id());
        assert_eq!(parsed.price(), original.price());
        assert_eq!(parsed.quantity(), original.quantity());
        assert_eq!(parsed.taker_side(), original.taker_side());
        assert_eq!(parsed.timestamp(), original.timestamp());
    }

    #[test]
    fn test_maker_side() {
        // Test when taker is buyer
        let buy_trade = create_test_trade();
        assert_eq!(buy_trade.taker_side(), Side::Buy);
        assert_eq!(buy_trade.maker_side(), Side::Sell);

        // Test when taker is seller
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let sell_trade = Trade::with_timestamp(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(10000),
            Quantity::new(5),
            Side::Sell,
            TimestampMs::new(1616823000000),
        );
        assert_eq!(sell_trade.maker_side(), Side::Buy);
    }

    #[test]
    fn test_total_value() {
        let transaction = create_test_trade();
        assert_eq!(transaction.total_value().unwrap(), 50000);

        // Test with larger values
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let large_trade = Trade::with_timestamp(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(123456),
            Quantity::new(789),
            Side::Buy,
            TimestampMs::new(1616823000000),
        );
        assert_eq!(large_trade.total_value().unwrap(), 97406784);
    }

    #[test]
    fn test_total_value_overflow_returns_invalid_operation() {
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        // price * quantity overflows u128 -> checked_mul yields InvalidOperation.
        let trade = Trade::with_timestamp(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(u128::MAX),
            Quantity::new(2),
            Side::Buy,
            TimestampMs::new(1616823000000),
        );
        assert!(matches!(
            trade.total_value(),
            Err(crate::errors::PriceLevelError::InvalidOperation { .. })
        ));
    }

    /// Test clock returning a fixed reading and counting how often it is read.
    struct FixedClock {
        now: u64,
        reads: Cell<u32>,
    }

    impl UnixClock for FixedClock {
        fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
            self.reads.set(self.reads.get() + 1);
            Ok(TimestampMs::new(self.now))
        }
    }

    /// Test clock whose reading is a caller-chosen `SystemTime` offset, run
    /// through the crate's checked conversion.
    struct SystemTimeClock(std::time::SystemTime);

    impl UnixClock for SystemTimeClock {
        fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
            TimestampMs::try_from_system_time(self.0)
        }
    }

    fn try_new_with<C: UnixClock + ?Sized>(clock: &C) -> Result<Trade, PriceLevelError> {
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        Trade::try_new(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(10000),
            Quantity::new(5),
            Side::Buy,
            clock,
        )
    }

    #[test]
    fn test_try_new_trade_stamps_clock_reading_once() {
        let clock = FixedClock {
            now: 1_716_000_000_000,
            reads: Cell::new(0),
        };
        let transaction = try_new_with(&clock).unwrap();
        assert_eq!(clock.reads.get(), 1);

        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(transaction.trade_id(), Id::from_uuid(uuid));
        assert_eq!(transaction.taker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.maker_order_id(), Id::from_u64(2));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(5));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(transaction.timestamp(), TimestampMs::new(1_716_000_000_000));

        // Identical to the explicit-timestamp constructor.
        assert_eq!(
            transaction,
            Trade::with_timestamp(
                Id::from_uuid(uuid),
                Id::from_u64(1),
                Id::from_u64(2),
                Price::new(10000),
                Quantity::new(5),
                Side::Buy,
                TimestampMs::new(1_716_000_000_000),
            )
        );

        // Works through a trait object too.
        let dyn_clock: &dyn UnixClock = &clock;
        assert!(try_new_with(dyn_clock).is_ok());
    }

    #[test]
    fn test_try_new_trade_propagates_pre_epoch_clock() {
        let before = UNIX_EPOCH.checked_sub(Duration::from_millis(1)).unwrap();
        let err = try_new_with(&SystemTimeClock(before)).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
    }

    #[test]
    fn test_try_new_trade_epoch_is_not_a_fallback() {
        // The epoch is a legitimate reading (0 ms), distinct from a failure.
        let trade = try_new_with(&SystemTimeClock(UNIX_EPOCH)).unwrap();
        assert_eq!(trade.timestamp(), TimestampMs::ZERO);
    }

    #[test]
    fn test_try_new_trade_rejects_unrepresentable_millis() {
        let span = Duration::from_millis(u64::MAX) + Duration::from_millis(1);
        // Only reachable where the platform `SystemTime` can represent it.
        if let Some(far_future) = UNIX_EPOCH.checked_add(span) {
            let err = try_new_with(&SystemTimeClock(far_future)).unwrap_err();
            assert!(matches!(err, PriceLevelError::InvalidFieldValue { .. }));
        }
        if let Some(max) = UNIX_EPOCH.checked_add(Duration::from_millis(u64::MAX)) {
            let trade = try_new_with(&SystemTimeClock(max)).unwrap();
            assert_eq!(trade.timestamp().as_u64(), u64::MAX);
        }
    }

    // In execution/transaction.rs test module or in a separate test file

    #[test]
    fn test_transaction_from_str_all_fields() {
        let input = "Trade:trade_id=6ba7b810-9dad-11d1-80b4-00c04fd430c8;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";

        let transaction = Trade::from_str(input).unwrap();

        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(transaction.trade_id(), Id::from_uuid(uuid));
        assert_eq!(transaction.taker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.maker_order_id(), Id::from_u64(2));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(5));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(transaction.timestamp(), TimestampMs::new(1616823000000));
    }

    #[test]
    fn test_transaction_get_field_helper() {
        // Simulate get_field function being used in the from_str implementation
        let mut fields = std::collections::HashMap::new();
        fields.insert("trade_id", "6ba7b810-9dad-11d1-80b4-00c04fd430c8");
        fields.insert("price", "10000");

        // Test successful field retrieval
        let get_field = |field: &str| -> Result<&str, PriceLevelError> {
            match fields.get(field) {
                Some(result) => Ok(*result),
                None => Err(PriceLevelError::MissingField(field.to_string())),
            }
        };

        assert_eq!(
            get_field("trade_id").unwrap(),
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
        );
        assert_eq!(get_field("price").unwrap(), "10000");

        // Test missing field error
        let missing_result = get_field("missing_field");
        assert!(missing_result.is_err());
        if let Err(PriceLevelError::MissingField(field)) = missing_result {
            assert_eq!(field, "missing_field");
        } else {
            panic!("Expected MissingField error");
        }
    }

    #[test]
    fn test_transaction_parse_u64_helper() {
        // Simulate parse_u64 function being used in the from_str implementation
        let parse_u64 = |field: &str, value: &str| -> Result<u64, PriceLevelError> {
            value
                .parse::<u64>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        // Test successful parsing
        assert_eq!(parse_u64("price", "10000").unwrap(), 10000);

        // Test failed parsing
        let invalid_result = parse_u64("price", "invalid");
        assert!(invalid_result.is_err());
        if let Err(PriceLevelError::InvalidFieldValue { field, value }) = invalid_result {
            assert_eq!(field, "price");
            assert_eq!(value, "invalid");
        } else {
            panic!("Expected InvalidFieldValue error");
        }
    }
}

#[cfg(test)]
mod transaction_serialization_tests {
    use crate::execution::trade::Trade;
    use crate::orders::{Id, Side};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::str::FromStr;
    use uuid::Uuid;

    fn create_test_trade() -> Trade {
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        Trade::with_timestamp(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(10000),
            Quantity::new(5),
            Side::Buy,
            TimestampMs::new(1616823000000),
        )
    }

    #[test]
    fn test_serde_json_serialization() {
        let transaction = create_test_trade();
        let json = serde_json::to_string(&transaction).unwrap();
        assert!(json.contains("\"trade_id\":\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\""));
        assert!(json.contains("\"taker_order_id\":\"00000000-0000-0001-0000-000000000000\""));
        assert!(json.contains("\"maker_order_id\":\"00000000-0000-0002-0000-000000000000\""));
        assert!(json.contains("\"price\":10000"));
        assert!(json.contains("\"quantity\":5"));
        assert!(json.contains("\"taker_side\":\"BUY\""));
        assert!(json.contains("\"timestamp\":1616823000000"));
    }

    #[test]
    fn test_serde_json_deserialization() {
        let json = r#"{
            "trade_id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            "taker_order_id": "00000000-0000-0001-0000-000000000000",
            "maker_order_id": "00000000-0000-0002-0000-000000000000",
            "price": 10000,
            "quantity": 5,
            "taker_side": "BUY",
            "timestamp": 1616823000000
        }"#;

        let transaction: Trade = serde_json::from_str(json).unwrap();
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(transaction.trade_id(), Id::from_uuid(uuid));
        assert_eq!(transaction.taker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.maker_order_id(), Id::from_u64(2));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(5));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(transaction.timestamp(), TimestampMs::new(1616823000000));
    }

    #[test]
    fn test_serde_json_round_trip() {
        let original = create_test_trade();

        let json = serde_json::to_string(&original).unwrap();

        let deserialized: Trade = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.trade_id(), original.trade_id());
        assert_eq!(deserialized.taker_order_id(), original.taker_order_id());
        assert_eq!(deserialized.maker_order_id(), original.maker_order_id());
        assert_eq!(deserialized.price(), original.price());
        assert_eq!(deserialized.quantity(), original.quantity());
        assert_eq!(deserialized.taker_side(), original.taker_side());
        assert_eq!(deserialized.timestamp(), original.timestamp());
    }

    #[test]
    fn test_custom_display_format() {
        let transaction = create_test_trade();
        let display_str = transaction.to_string();

        assert!(display_str.starts_with("Trade:"));
        assert!(display_str.contains("trade_id=6ba7b810-9dad-11d1-80b4-00c04fd430c8"));
        assert!(display_str.contains("taker_order_id=00000000-0000-0001-0000-000000000000"));
        assert!(display_str.contains("maker_order_id=00000000-0000-0002-0000-000000000000"));
        assert!(display_str.contains("price=10000"));
        assert!(display_str.contains("quantity=5"));
        assert!(display_str.contains("taker_side=BUY"));
        assert!(display_str.contains("timestamp=1616823000000"));
    }

    #[test]
    fn test_from_str_valid() {
        let input = "Trade:trade_id=6ba7b810-9dad-11d1-80b4-00c04fd430c8;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let transaction = Trade::from_str(input).unwrap();
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(transaction.trade_id(), Id::from_uuid(uuid));
        assert_eq!(transaction.taker_order_id(), Id::from_u64(1));
        assert_eq!(transaction.maker_order_id(), Id::from_u64(2));
        assert_eq!(transaction.price(), Price::new(10000));
        assert_eq!(transaction.quantity(), Quantity::new(5));
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(transaction.timestamp(), TimestampMs::new(1616823000000));
    }

    #[test]
    fn test_from_str_invalid_format() {
        let input = "InvalidFormat";
        let result = Trade::from_str(input);
        assert!(result.is_err());

        let input = "TradeX:trade_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Trade::from_str(input);
        assert!(result.is_err());

        let input = "Trade:";
        let result = Trade::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_missing_field() {
        let input = "Trade:taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Trade::from_str(input);
        assert!(result.is_err());

        let input = "Trade:trade_id=12345;taker_order_id=1;maker_order_id=2;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Trade::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_invalid_field_value() {
        let input = "Trade:trade_id=abc;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Trade::from_str(input);
        assert!(result.is_err());

        let input = "Trade:trade_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=INVALID;timestamp=1616823000000";
        let result = Trade::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_serialization_round_trip() {
        let original = create_test_trade();
        let string_representation = original.to_string();
        let parsed = Trade::from_str(&string_representation).unwrap();

        assert_eq!(parsed.trade_id(), original.trade_id());
        assert_eq!(parsed.taker_order_id(), original.taker_order_id());
        assert_eq!(parsed.maker_order_id(), original.maker_order_id());
        assert_eq!(parsed.price(), original.price());
        assert_eq!(parsed.quantity(), original.quantity());
        assert_eq!(parsed.taker_side(), original.taker_side());
        assert_eq!(parsed.timestamp(), original.timestamp());
    }

    #[test]
    fn test_maker_side_when_taker_is_buyer() {
        let transaction = create_test_trade();
        assert_eq!(transaction.taker_side(), Side::Buy);
        assert_eq!(transaction.maker_side(), Side::Sell);
    }

    #[test]
    fn test_maker_side_when_taker_is_seller() {
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction = Trade::with_timestamp(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(10000),
            Quantity::new(5),
            Side::Sell,
            TimestampMs::new(1616823000000),
        );
        assert_eq!(transaction.maker_side(), Side::Buy);
    }

    #[test]
    fn test_total_value_calculation() {
        let transaction = create_test_trade();
        assert_eq!(transaction.total_value().unwrap(), 50000);

        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let large_trade = Trade::with_timestamp(
            Id::from_uuid(uuid),
            Id::from_u64(1),
            Id::from_u64(2),
            Price::new(12345),
            Quantity::new(67),
            Side::Buy,
            TimestampMs::new(1616823000000),
        );
        assert_eq!(large_trade.total_value().unwrap(), 827115);
    }
}
