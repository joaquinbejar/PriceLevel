#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::execution::transaction::Trade;
    use crate::orders::{OrderId, Side};
    use std::str::FromStr;
    use std::time::{SystemTime, UNIX_EPOCH};
    use uuid::Uuid;

    fn create_test_trade() -> Trade {
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();

        Trade {
            trade_id: uuid,
            taker_order_id: OrderId::from_u64(1),
            maker_order_id: OrderId::from_u64(2),
            price: 10000,
            quantity: 5,
            taker_side: Side::Buy,
            timestamp: 1616823000000,
        }
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
        assert_eq!(transaction.trade_id, uuid);
        assert_eq!(transaction.taker_order_id, OrderId::from_u64(1));
        assert_eq!(transaction.maker_order_id, OrderId::from_u64(2));
        assert_eq!(transaction.price, 10000);
        assert_eq!(transaction.quantity, 5);
        assert_eq!(transaction.taker_side, Side::Buy);
        assert_eq!(transaction.timestamp, 1616823000000);
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

        assert_eq!(parsed.trade_id, original.trade_id);
        assert_eq!(parsed.taker_order_id, original.taker_order_id);
        assert_eq!(parsed.maker_order_id, original.maker_order_id);
        assert_eq!(parsed.price, original.price);
        assert_eq!(parsed.quantity, original.quantity);
        assert_eq!(parsed.taker_side, original.taker_side);
        assert_eq!(parsed.timestamp, original.timestamp);
    }

    #[test]
    fn test_maker_side() {
        // Test when taker is buyer
        let mut transaction = create_test_trade();
        transaction.taker_side = Side::Buy;
        assert_eq!(transaction.maker_side(), Side::Sell);

        // Test when taker is seller
        transaction.taker_side = Side::Sell;
        assert_eq!(transaction.maker_side(), Side::Buy);
    }

    #[test]
    fn test_total_value() {
        let mut transaction = create_test_trade();
        transaction.price = 10000;
        transaction.quantity = 5;

        assert_eq!(transaction.total_value(), 50000);

        // Test with larger values
        transaction.price = 123456;
        transaction.quantity = 789;
        assert_eq!(transaction.total_value(), 97406784);
    }

    #[test]
    fn test_new_trade() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let transaction = Trade::new(
            uuid,
            OrderId::from_u64(1),
            OrderId::from_u64(2),
            10000,
            5,
            Side::Buy,
        );

        assert_eq!(transaction.trade_id, uuid);
        assert_eq!(transaction.taker_order_id, OrderId::from_u64(1));
        assert_eq!(transaction.maker_order_id, OrderId::from_u64(2));
        assert_eq!(transaction.price, 10000);
        assert_eq!(transaction.quantity, 5);
        assert_eq!(transaction.taker_side, Side::Buy);

        // The timestamp should be approximately now
        let timestamp_diff = transaction.timestamp.abs_diff(now);

        // Timestamp should be within 100ms of current time
        assert!(
            timestamp_diff < 100,
            "Timestamp difference is too large: {timestamp_diff}"
        );
    }

    // In execution/transaction.rs test module or in a separate test file

    #[test]
    fn test_transaction_from_str_all_fields() {
        let input = "Trade:trade_id=6ba7b810-9dad-11d1-80b4-00c04fd430c8;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";

        let transaction = Trade::from_str(input).unwrap();

        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(transaction.trade_id, uuid);
        assert_eq!(transaction.taker_order_id, OrderId::from_u64(1));
        assert_eq!(transaction.maker_order_id, OrderId::from_u64(2));
        assert_eq!(transaction.price, 10000);
        assert_eq!(transaction.quantity, 5);
        assert_eq!(transaction.taker_side, Side::Buy);
        assert_eq!(transaction.timestamp, 1616823000000);
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
    use crate::execution::transaction::Trade;
    use crate::orders::{OrderId, Side};
    use std::str::FromStr;
    use uuid::Uuid;

    fn create_test_trade() -> Trade {
        let uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        Trade {
            trade_id: uuid,
            taker_order_id: OrderId::from_u64(1),
            maker_order_id: OrderId::from_u64(2),
            price: 10000,
            quantity: 5,
            taker_side: Side::Buy,
            timestamp: 1616823000000,
        }
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
        assert_eq!(transaction.trade_id, uuid);
        assert_eq!(transaction.taker_order_id, OrderId::from_u64(1));
        assert_eq!(transaction.maker_order_id, OrderId::from_u64(2));
        assert_eq!(transaction.price, 10000);
        assert_eq!(transaction.quantity, 5);
        assert_eq!(transaction.taker_side, Side::Buy);
        assert_eq!(transaction.timestamp, 1616823000000);
    }

    #[test]
    fn test_serde_json_round_trip() {
        let original = create_test_trade();

        let json = serde_json::to_string(&original).unwrap();

        let deserialized: Trade = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.trade_id, original.trade_id);
        assert_eq!(deserialized.taker_order_id, original.taker_order_id);
        assert_eq!(deserialized.maker_order_id, original.maker_order_id);
        assert_eq!(deserialized.price, original.price);
        assert_eq!(deserialized.quantity, original.quantity);
        assert_eq!(deserialized.taker_side, original.taker_side);
        assert_eq!(deserialized.timestamp, original.timestamp);
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
        assert_eq!(transaction.trade_id, uuid);
        assert_eq!(transaction.taker_order_id, OrderId::from_u64(1));
        assert_eq!(transaction.maker_order_id, OrderId::from_u64(2));
        assert_eq!(transaction.price, 10000);
        assert_eq!(transaction.quantity, 5);
        assert_eq!(transaction.taker_side, Side::Buy);
        assert_eq!(transaction.timestamp, 1616823000000);
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

        assert_eq!(parsed.trade_id, original.trade_id);
        assert_eq!(parsed.taker_order_id, original.taker_order_id);
        assert_eq!(parsed.maker_order_id, original.maker_order_id);
        assert_eq!(parsed.price, original.price);
        assert_eq!(parsed.quantity, original.quantity);
        assert_eq!(parsed.taker_side, original.taker_side);
        assert_eq!(parsed.timestamp, original.timestamp);
    }

    #[test]
    fn test_maker_side_when_taker_is_buyer() {
        let mut transaction = create_test_trade();
        transaction.taker_side = Side::Buy;

        assert_eq!(transaction.maker_side(), Side::Sell);
    }

    #[test]
    fn test_maker_side_when_taker_is_seller() {
        let mut transaction = create_test_trade();
        transaction.taker_side = Side::Sell;

        assert_eq!(transaction.maker_side(), Side::Buy);
    }

    #[test]
    fn test_total_value_calculation() {
        let mut transaction = create_test_trade();
        transaction.price = 10000;
        transaction.quantity = 5;

        assert_eq!(transaction.total_value(), 50000);

        transaction.price = 12345;
        transaction.quantity = 67;

        assert_eq!(transaction.total_value(), 827115);
    }
}
