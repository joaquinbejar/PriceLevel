#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::execution::transaction::Transaction;
    use crate::orders::{OrderId, Side};
    use std::str::FromStr;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_test_transaction() -> Transaction {
        Transaction {
            transaction_id: 12345,
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
        let transaction = create_test_transaction();
        let display_str = transaction.to_string();

        assert!(display_str.starts_with("Transaction:"));
        assert!(display_str.contains("transaction_id=12345"));
        assert!(display_str.contains("taker_order_id=1"));
        assert!(display_str.contains("maker_order_id=2"));
        assert!(display_str.contains("price=10000"));
        assert!(display_str.contains("quantity=5"));
        assert!(display_str.contains("taker_side=BUY"));
        assert!(display_str.contains("timestamp=1616823000000"));
    }

    #[test]
    fn test_transaction_from_str_valid() {
        let input = "Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let transaction = Transaction::from_str(input).unwrap();

        assert_eq!(transaction.transaction_id, 12345);
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
        let result = Transaction::from_str(input);
        assert!(result.is_err());

        let input = "Transaction;transaction_id=12345";
        let result = Transaction::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_from_str_missing_field() {
        // Missing quantity field
        let input = "Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;taker_side=BUY;timestamp=1616823000000";
        let result = Transaction::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::MissingField(field) => {
                assert_eq!(field, "quantity");
            }
            err => panic!("Expected MissingField error, got {:?}", err),
        }
    }

    #[test]
    fn test_transaction_from_str_invalid_field_value() {
        // Invalid transaction_id (not a number)
        let input = "Transaction:transaction_id=abc;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Transaction::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::InvalidFieldValue { field, value } => {
                assert_eq!(field, "transaction_id");
                assert_eq!(value, "abc");
            }
            err => panic!("Expected InvalidFieldValue error, got {:?}", err),
        }

        // Invalid taker_order_id
        let input = "Transaction:transaction_id=12345;taker_order_id=abc;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Transaction::from_str(input);
        assert!(result.is_err());

        // Invalid side
        let input = "Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=INVALID;timestamp=1616823000000";
        let result = Transaction::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_round_trip() {
        let original = create_test_transaction();
        let string_representation = original.to_string();
        let parsed = Transaction::from_str(&string_representation).unwrap();

        assert_eq!(parsed.transaction_id, original.transaction_id);
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
        let mut transaction = create_test_transaction();
        transaction.taker_side = Side::Buy;
        assert_eq!(transaction.maker_side(), Side::Sell);

        // Test when taker is seller
        transaction.taker_side = Side::Sell;
        assert_eq!(transaction.maker_side(), Side::Buy);
    }

    #[test]
    fn test_total_value() {
        let mut transaction = create_test_transaction();
        transaction.price = 10000;
        transaction.quantity = 5;

        assert_eq!(transaction.total_value(), 50000);

        // Test with larger values
        transaction.price = 123456;
        transaction.quantity = 789;
        assert_eq!(transaction.total_value(), 97406784);
    }

    #[test]
    fn test_new_transaction() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let transaction = Transaction::new(12345, OrderId::from_u64(1), OrderId::from_u64(2), 10000, 5, Side::Buy);

        assert_eq!(transaction.transaction_id, 12345);
        assert_eq!(transaction.taker_order_id, OrderId::from_u64(1));
        assert_eq!(transaction.maker_order_id, OrderId::from_u64(2));
        assert_eq!(transaction.price, 10000);
        assert_eq!(transaction.quantity, 5);
        assert_eq!(transaction.taker_side, Side::Buy);

        // The timestamp should be approximately now
        let timestamp_diff = if transaction.timestamp > now {
            transaction.timestamp - now
        } else {
            now - transaction.timestamp
        };

        // Timestamp should be within 100ms of current time
        assert!(
            timestamp_diff < 100,
            "Timestamp difference is too large: {}",
            timestamp_diff
        );
    }
}

#[cfg(test)]
mod transaction_serialization_tests {
    use crate::orders::{OrderId, Side};
    use std::str::FromStr;

    use crate::execution::transaction::Transaction;

    fn create_test_transaction() -> Transaction {
        Transaction {
            transaction_id: 12345,
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
        let transaction = create_test_transaction();
        let json = serde_json::to_string(&transaction).unwrap();
        assert!(json.contains("\"transaction_id\":12345"));
        assert!(json.contains("\"taker_order_id\":1"));
        assert!(json.contains("\"maker_order_id\":2"));
        assert!(json.contains("\"price\":10000"));
        assert!(json.contains("\"quantity\":5"));
        assert!(json.contains("\"taker_side\":\"BUY\"")); // Asumiendo que Side serializa a "BUY"
        assert!(json.contains("\"timestamp\":1616823000000"));
    }

    #[test]
    fn test_serde_json_deserialization() {
        let json = r#"{
            "transaction_id": 12345,
            "taker_order_id": 1,
            "maker_order_id": 2,
            "price": 10000,
            "quantity": 5,
            "taker_side": "BUY",
            "timestamp": 1616823000000
        }"#;

        let transaction: Transaction = serde_json::from_str(json).unwrap();

        assert_eq!(transaction.transaction_id, 12345);
        assert_eq!(transaction.taker_order_id, OrderId::from_u64(1));
        assert_eq!(transaction.maker_order_id, OrderId::from_u64(2));
        assert_eq!(transaction.price, 10000);
        assert_eq!(transaction.quantity, 5);
        assert_eq!(transaction.taker_side, Side::Buy);
        assert_eq!(transaction.timestamp, 1616823000000);
    }

    #[test]
    fn test_serde_json_round_trip() {
        let original = create_test_transaction();

        let json = serde_json::to_string(&original).unwrap();

        let deserialized: Transaction = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.transaction_id, original.transaction_id);
        assert_eq!(deserialized.taker_order_id, original.taker_order_id);
        assert_eq!(deserialized.maker_order_id, original.maker_order_id);
        assert_eq!(deserialized.price, original.price);
        assert_eq!(deserialized.quantity, original.quantity);
        assert_eq!(deserialized.taker_side, original.taker_side);
        assert_eq!(deserialized.timestamp, original.timestamp);
    }

    #[test]
    fn test_custom_display_format() {
        let transaction = create_test_transaction();
        let display_str = transaction.to_string();

        assert!(display_str.starts_with("Transaction:"));
        assert!(display_str.contains("transaction_id=12345"));
        assert!(display_str.contains("taker_order_id=1"));
        assert!(display_str.contains("maker_order_id=2"));
        assert!(display_str.contains("price=10000"));
        assert!(display_str.contains("quantity=5"));
        assert!(display_str.contains("taker_side=BUY"));
        assert!(display_str.contains("timestamp=1616823000000"));
    }

    #[test]
    fn test_from_str_valid() {
        let input = "Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let transaction = Transaction::from_str(input).unwrap();

        assert_eq!(transaction.transaction_id, 12345);
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
        let result = Transaction::from_str(input);
        assert!(result.is_err());

        let input = "TransactionX:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Transaction::from_str(input);
        assert!(result.is_err());

        let input = "Transaction:";
        let result = Transaction::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_missing_field() {
        let input = "Transaction:taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Transaction::from_str(input);
        assert!(result.is_err());

        let input = "Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Transaction::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_invalid_field_value() {
        let input = "Transaction:transaction_id=abc;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = Transaction::from_str(input);
        assert!(result.is_err());

        let input = "Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=INVALID;timestamp=1616823000000";
        let result = Transaction::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_serialization_round_trip() {
        let original = create_test_transaction();
        let string_representation = original.to_string();
        let parsed = Transaction::from_str(&string_representation).unwrap();

        assert_eq!(parsed.transaction_id, original.transaction_id);
        assert_eq!(parsed.taker_order_id, original.taker_order_id);
        assert_eq!(parsed.maker_order_id, original.maker_order_id);
        assert_eq!(parsed.price, original.price);
        assert_eq!(parsed.quantity, original.quantity);
        assert_eq!(parsed.taker_side, original.taker_side);
        assert_eq!(parsed.timestamp, original.timestamp);
    }

    #[test]
    fn test_maker_side_when_taker_is_buyer() {
        let mut transaction = create_test_transaction();
        transaction.taker_side = Side::Buy;

        assert_eq!(transaction.maker_side(), Side::Sell);
    }

    #[test]
    fn test_maker_side_when_taker_is_seller() {
        let mut transaction = create_test_transaction();
        transaction.taker_side = Side::Sell;

        assert_eq!(transaction.maker_side(), Side::Buy);
    }

    #[test]
    fn test_total_value_calculation() {
        let mut transaction = create_test_transaction();
        transaction.price = 10000;
        transaction.quantity = 5;

        assert_eq!(transaction.total_value(), 50000);

        transaction.price = 12345;
        transaction.quantity = 67;

        assert_eq!(transaction.total_value(), 827115);
    }
}
