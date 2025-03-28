#[cfg(test)]
mod tests {
    use crate::execution::list::TransactionList;
    use crate::execution::match_result::MatchResult;
    use crate::execution::transaction::Transaction;
    use crate::orders::OrderId;
    use crate::orders::Side;
    use std::str::FromStr;

    // Helper function to create a test transaction
    fn create_test_transaction(
        id: u64,
        taker_id: u64,
        maker_id: u64,
        price: u64,
        quantity: u64,
    ) -> Transaction {
        Transaction {
            transaction_id: id,
            taker_order_id: OrderId(taker_id),
            maker_order_id: OrderId(maker_id),
            price,
            quantity,
            taker_side: Side::Buy,
            timestamp: 1616823000000 + id, // Create unique timestamps
        }
    }

    #[test]
    fn test_match_result_new() {
        let result = MatchResult::new(OrderId(123), 100);

        assert_eq!(result.order_id, OrderId(123));
        assert_eq!(result.remaining_quantity, 100);
        assert!(!result.is_complete);
        assert!(result.transactions.is_empty());
        assert!(result.filled_order_ids.is_empty());
    }

    #[test]
    fn test_add_transaction() {
        let mut result = MatchResult::new(OrderId(123), 100);

        // Add a transaction for 30 quantity
        let transaction1 = create_test_transaction(1, 123, 456, 1000, 30);
        result.add_transaction(transaction1);

        assert_eq!(result.remaining_quantity, 70); // 100 - 30
        assert!(!result.is_complete);
        assert_eq!(result.transactions.len(), 1);

        // Add another transaction that will complete the match
        let transaction2 = create_test_transaction(2, 123, 789, 1000, 70);
        result.add_transaction(transaction2);

        assert_eq!(result.remaining_quantity, 0);
        assert!(result.is_complete);
        assert_eq!(result.transactions.len(), 2);

        // Add a transaction that would exceed the remaining quantity
        // This is normally prevented by validation logic elsewhere, but testing the method
        let transaction3 = create_test_transaction(3, 123, 101, 1000, 20);
        result.add_transaction(transaction3);

        // Should remain at 0 due to saturating_sub
        assert_eq!(result.remaining_quantity, 0);
        assert!(result.is_complete);
        assert_eq!(result.transactions.len(), 3);
    }

    #[test]
    fn test_add_filled_order_id() {
        let mut result = MatchResult::new(OrderId(123), 100);

        result.add_filled_order_id(OrderId(456));
        result.add_filled_order_id(OrderId(789));

        assert_eq!(result.filled_order_ids.len(), 2);
        assert_eq!(result.filled_order_ids[0], OrderId(456));
        assert_eq!(result.filled_order_ids[1], OrderId(789));
    }

    #[test]
    fn test_executed_quantity() {
        let mut result = MatchResult::new(OrderId(123), 100);

        // No transactions yet
        assert_eq!(result.executed_quantity(), 0);

        // Add some transactions
        result.add_transaction(create_test_transaction(1, 123, 456, 1000, 30));
        result.add_transaction(create_test_transaction(2, 123, 789, 1000, 20));

        assert_eq!(result.executed_quantity(), 50); // 30 + 20
    }

    #[test]
    fn test_executed_value() {
        let mut result = MatchResult::new(OrderId(123), 100);

        // No transactions yet
        assert_eq!(result.executed_value(), 0);

        // Add transactions with different prices
        result.add_transaction(create_test_transaction(1, 123, 456, 1000, 30)); // Value: 30,000
        result.add_transaction(create_test_transaction(2, 123, 789, 1200, 20)); // Value: 24,000

        assert_eq!(result.executed_value(), 54000); // 30,000 + 24,000
    }

    #[test]
    fn test_average_price() {
        let mut result = MatchResult::new(OrderId(123), 100);

        // No transactions yet
        assert_eq!(result.average_price(), None);

        // Add transactions with different prices
        result.add_transaction(create_test_transaction(1, 123, 456, 1000, 30)); // Value: 30,000
        result.add_transaction(create_test_transaction(2, 123, 789, 1200, 20)); // Value: 24,000

        // Average price: 54,000 / 50 = 1,080
        assert_eq!(result.average_price(), Some(1080.0));
    }

    #[test]
    fn test_display() {
        let mut result = MatchResult::new(OrderId(123), 100);

        // Test display with empty transactions and filled_order_ids
        let display_str = result.to_string();
        assert!(
            display_str
                .starts_with("MatchResult:order_id=123;remaining_quantity=100;is_complete=false")
        );
        assert!(display_str.contains("transactions=Transactions:[]"));
        assert!(display_str.contains("filled_order_ids=[]"));

        // Add some transactions and filled order IDs
        result.add_transaction(create_test_transaction(1, 123, 456, 1000, 30));
        result.add_filled_order_id(OrderId(456));

        let display_str = result.to_string();
        assert!(
            display_str
                .starts_with("MatchResult:order_id=123;remaining_quantity=70;is_complete=false")
        );
        assert!(display_str.contains("Transaction:transaction_id=1"));
        assert!(display_str.contains("filled_order_ids=[456]"));
    }

    #[test]
    fn test_from_str_valid() {
        let input = "MatchResult:order_id=123;remaining_quantity=70;is_complete=false;transactions=Transactions:[];filled_order_ids=[]";
        let result = match MatchResult::from_str(input) {
            Ok(r) => r,
            Err(e) => {
                panic!("Test failed: {:?}", e);
            }
        };

        assert_eq!(result.order_id, OrderId(123));
        assert_eq!(result.remaining_quantity, 70);
        assert!(!result.is_complete);
        assert!(result.transactions.is_empty());
        assert!(result.filled_order_ids.is_empty());

        // Test parsing with transactions and filled order IDs
        let input = "MatchResult:order_id=123;remaining_quantity=70;is_complete=false;transactions=Transactions:[Transaction:transaction_id=1;taker_order_id=123;maker_order_id=456;price=1000;quantity=30;taker_side=BUY;timestamp=1616823000001];filled_order_ids=[456]";
        let result = MatchResult::from_str(input).unwrap();

        assert_eq!(result.order_id, OrderId(123));
        assert_eq!(result.remaining_quantity, 70);
        assert!(!result.is_complete);
        assert_eq!(result.transactions.len(), 1);
        assert_eq!(result.filled_order_ids.len(), 1);
        assert_eq!(result.filled_order_ids[0], OrderId(456));
    }

    #[test]
    fn test_from_str_invalid_format() {
        // Test invalid prefix
        let input = "InvalidPrefix:order_id=123;remaining_quantity=70;is_complete=false;transactions=Transactions:[];filled_order_ids=[]";
        let result = MatchResult::from_str(input);
        assert!(result.is_err());

        // Test missing field
        let input =
            "MatchResult:order_id=123;remaining_quantity=70;is_complete=false;filled_order_ids=[]";
        let result = MatchResult::from_str(input);
        assert!(result.is_err());

        // Test invalid value type
        let input = "MatchResult:order_id=abc;remaining_quantity=70;is_complete=false;transactions=Transactions:[];filled_order_ids=[]";
        let result = MatchResult::from_str(input);
        assert!(result.is_err());

        // Test invalid boolean
        let input = "MatchResult:order_id=123;remaining_quantity=70;is_complete=invalidbool;transactions=Transactions:[];filled_order_ids=[]";
        let result = MatchResult::from_str(input);
        assert!(result.is_err());

        // Test invalid filled_order_ids format
        let input = "MatchResult:order_id=123;remaining_quantity=70;is_complete=false;transactions=Transactions:[];filled_order_ids=invalid";
        let result = MatchResult::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip() {
        // Create a match result with some data
        let mut original = MatchResult::new(OrderId(123), 100);
        original.add_transaction(create_test_transaction(1, 123, 456, 1000, 30));
        original.add_transaction(create_test_transaction(2, 123, 789, 1200, 20));
        original.add_filled_order_id(OrderId(456));
        original.add_filled_order_id(OrderId(789));

        // Convert to string
        let string_representation = original.to_string();
        println!("Cadena generada: '{}'", string_representation);

        // Parse back
        let parsed = match MatchResult::from_str(&string_representation) {
            Ok(r) => {
                println!("Análisis exitoso");
                r
            }
            Err(e) => {
                println!("Error en el análisis: {:?}", e);
                panic!("Test falló en el roundtrip");
            }
        };

        // Verify all fields match
        assert_eq!(parsed.order_id, original.order_id);
        assert_eq!(parsed.remaining_quantity, original.remaining_quantity);
        assert_eq!(parsed.is_complete, original.is_complete);
        assert_eq!(parsed.filled_order_ids, original.filled_order_ids);

        // Verify transactions (need to check each one since Transaction might not implement PartialEq)
        assert_eq!(parsed.transactions.len(), original.transactions.len());
        for (i, transaction) in original.transactions.as_vec().iter().enumerate() {
            let parsed_transaction = &parsed.transactions.as_vec()[i];
            assert_eq!(
                parsed_transaction.transaction_id,
                transaction.transaction_id
            );
            assert_eq!(
                parsed_transaction.taker_order_id,
                transaction.taker_order_id
            );
            assert_eq!(
                parsed_transaction.maker_order_id,
                transaction.maker_order_id
            );
            assert_eq!(parsed_transaction.price, transaction.price);
            assert_eq!(parsed_transaction.quantity, transaction.quantity);
            assert_eq!(parsed_transaction.taker_side, transaction.taker_side);
            assert_eq!(parsed_transaction.timestamp, transaction.timestamp);
        }
    }

    #[test]
    fn test_with_multiple_filled_order_ids() {
        // Create a match result with multiple filled order IDs
        let mut result = MatchResult::new(OrderId(123), 100);
        result.add_filled_order_id(OrderId(456));
        result.add_filled_order_id(OrderId(789));
        result.add_filled_order_id(OrderId(101));

        // Convert to string
        let string_representation = result.to_string();

        // Verify filled_order_ids format
        assert!(string_representation.contains("filled_order_ids=[456,789,101]"));

        // Parse back
        let parsed = MatchResult::from_str(&string_representation).unwrap();

        // Verify filled_order_ids were parsed correctly
        assert_eq!(parsed.filled_order_ids.len(), 3);
        assert_eq!(parsed.filled_order_ids[0], OrderId(456));
        assert_eq!(parsed.filled_order_ids[1], OrderId(789));
        assert_eq!(parsed.filled_order_ids[2], OrderId(101));
    }

    #[test]
    fn test_with_empty_transactions_and_filled_ids() {
        // Test with explicitly empty collections
        let mut result = MatchResult::new(OrderId(123), 100);
        result.transactions = TransactionList::new(); // Explicitly empty
        result.filled_order_ids = Vec::new(); // Explicitly empty

        // Convert to string
        let string_representation = result.to_string();

        // Parse back
        let parsed = MatchResult::from_str(&string_representation).unwrap();

        // Verify
        assert!(parsed.transactions.is_empty());
        assert!(parsed.filled_order_ids.is_empty());
    }
}
