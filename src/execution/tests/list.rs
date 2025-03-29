#[cfg(test)]
mod tests {
    use crate::execution::list::TransactionList;
    use crate::execution::transaction::Transaction;
    use crate::orders::{OrderId, Side};
    use std::str::FromStr;

    fn create_test_transactions() -> Vec<Transaction> {
        vec![
            Transaction {
                transaction_id: 12345,
                taker_order_id: OrderId::from_u64(1),
                maker_order_id: OrderId::from_u64(2),
                price: 10000,
                quantity: 5,
                taker_side: Side::Buy,
                timestamp: 1616823000000,
            },
            Transaction {
                transaction_id: 12346,
                taker_order_id: OrderId::from_u64(3),
                maker_order_id: OrderId::from_u64(4),
                price: 10001,
                quantity: 10,
                taker_side: Side::Sell,
                timestamp: 1616823000001,
            },
        ]
    }

    #[test]
    fn test_transaction_list_new() {
        let list = TransactionList::new();
        assert_eq!(list.transactions.len(), 0);
    }

    #[test]
    fn test_transaction_list_from_vec() {
        let transactions = create_test_transactions();
        let list = TransactionList::from_vec(transactions.clone());
        assert_eq!(list.transactions, transactions);
    }

    #[test]
    fn test_transaction_list_add() {
        let mut list = TransactionList::new();
        let transaction = create_test_transactions()[0];
        list.add(transaction);

        assert_eq!(list.transactions.len(), 1);
        assert_eq!(list.transactions[0].transaction_id, 12345);
    }

    #[test]
    fn test_transaction_list_as_vec() {
        let transactions = create_test_transactions();
        let list = TransactionList::from_vec(transactions.clone());
        let vec_ref = list.as_vec();

        assert_eq!(vec_ref, &transactions);
    }

    #[test]
    fn test_transaction_list_into_vec() {
        let transactions = create_test_transactions();
        let list = TransactionList::from_vec(transactions.clone());
        let vec = list.into_vec();

        assert_eq!(vec, transactions);
    }

    #[test]
    fn test_transaction_list_display() {
        let transactions = create_test_transactions();
        let list = TransactionList::from_vec(transactions);
        let display_str = list.to_string();

        assert!(display_str.starts_with("Transactions:["));
        assert!(display_str.ends_with("]"));
        assert!(display_str.contains("transaction_id=12345"));
        assert!(display_str.contains("transaction_id=12346"));
    }

    #[test]
    fn test_transaction_list_from_str_valid() {
        let input = "Transactions:[Transaction:transaction_id=12345;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000,Transaction:transaction_id=12346;taker_order_id=00000000-0000-0003-0000-000000000000;maker_order_id=00000000-0000-0004-0000-000000000000;price=10001;quantity=10;taker_side=SELL;timestamp=1616823000001]";
        let list = TransactionList::from_str(input).unwrap();

        assert_eq!(list.transactions.len(), 2);

        assert_eq!(list.transactions[0].transaction_id, 12345);
        assert_eq!(list.transactions[0].taker_order_id, OrderId::from_u64(1));
        assert_eq!(list.transactions[0].maker_order_id, OrderId::from_u64(2));
        assert_eq!(list.transactions[0].price, 10000);
        assert_eq!(list.transactions[0].quantity, 5);
        assert_eq!(list.transactions[0].taker_side, Side::Buy);
        assert_eq!(list.transactions[0].timestamp, 1616823000000);

        assert_eq!(list.transactions[1].transaction_id, 12346);
        assert_eq!(list.transactions[1].taker_order_id, OrderId::from_u64(3));
        assert_eq!(list.transactions[1].maker_order_id, OrderId::from_u64(4));
        assert_eq!(list.transactions[1].price, 10001);
        assert_eq!(list.transactions[1].quantity, 10);
        assert_eq!(list.transactions[1].taker_side, Side::Sell);
        assert_eq!(list.transactions[1].timestamp, 1616823000001);
    }

    #[test]
    fn test_transaction_list_from_str_empty() {
        let input = "Transactions:[]";
        let list = TransactionList::from_str(input).unwrap();

        assert_eq!(list.transactions.len(), 0);
    }

    #[test]
    fn test_transaction_list_from_str_invalid_format() {
        let input = "Transacciones:[]";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());

        let input = "Transactions:";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());

        let input = "[Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000]";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_list_from_str_invalid_transaction() {
        let input = "Transactions:[Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000,Transaction:transaction_id=invalid;taker_order_id=3;maker_order_id=4;price=10001;quantity=10;taker_side=SELL;timestamp=1616823000001]";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_list_round_trip() {
        let original_transactions = create_test_transactions();
        let original = TransactionList::from_vec(original_transactions);

        let string_representation = original.to_string();
        let parsed = TransactionList::from_str(&string_representation).unwrap();

        assert_eq!(parsed.transactions.len(), original.transactions.len());

        for i in 0..parsed.transactions.len() {
            assert_eq!(
                parsed.transactions[i].transaction_id,
                original.transactions[i].transaction_id
            );
            assert_eq!(
                parsed.transactions[i].taker_order_id,
                original.transactions[i].taker_order_id
            );
            assert_eq!(
                parsed.transactions[i].maker_order_id,
                original.transactions[i].maker_order_id
            );
            assert_eq!(parsed.transactions[i].price, original.transactions[i].price);
            assert_eq!(
                parsed.transactions[i].quantity,
                original.transactions[i].quantity
            );
            assert_eq!(
                parsed.transactions[i].taker_side,
                original.transactions[i].taker_side
            );
            assert_eq!(
                parsed.transactions[i].timestamp,
                original.transactions[i].timestamp
            );
        }
    }

    #[test]
    fn test_from_into_conversions() {
        // Vec<Transaction> -> TransactionList
        let transactions = create_test_transactions();
        let list: TransactionList = transactions.clone().into();
        assert_eq!(list.transactions, transactions);

        // TransactionList -> Vec<Transaction>
        let list = TransactionList::from_vec(transactions.clone());
        let vec: Vec<Transaction> = list.into();
        assert_eq!(vec, transactions);
    }
}

#[cfg(test)]
mod transaction_list_serialization_tests {

    use crate::orders::{OrderId, Side};
    use std::str::FromStr;

    use crate::execution::list::TransactionList;
    use crate::execution::transaction::Transaction;

    fn create_test_transactions() -> Vec<Transaction> {
        vec![
            Transaction {
                transaction_id: 12345,
                taker_order_id: OrderId::from_u64(1),
                maker_order_id: OrderId::from_u64(2),
                price: 10000,
                quantity: 5,
                taker_side: Side::Buy,
                timestamp: 1616823000000,
            },
            Transaction {
                transaction_id: 12346,
                taker_order_id: OrderId::from_u64(3),
                maker_order_id: OrderId::from_u64(4),
                price: 10001,
                quantity: 10,
                taker_side: Side::Sell,
                timestamp: 1616823000001,
            },
        ]
    }

    fn create_test_transaction_list() -> TransactionList {
        TransactionList::from_vec(create_test_transactions())
    }

    #[test]
    fn test_custom_display_format() {
        let list = create_test_transaction_list();
        let display_str = list.to_string();

        assert!(display_str.starts_with("Transactions:["));
        assert!(display_str.ends_with("]"));

        assert!(display_str.contains("transaction_id=12345"));
        assert!(display_str.contains("taker_order_id=00000000-0000-0001-0000-000000000000"));
        assert!(display_str.contains("maker_order_id=00000000-0000-0002-0000-000000000000"));
        assert!(display_str.contains("transaction_id=12346"));
        assert!(display_str.contains("taker_order_id=00000000-0000-0003-0000-000000000000"));
        assert!(display_str.contains("maker_order_id=00000000-0000-0004-0000-000000000000"));
    }

    #[test]
    fn test_empty_list_display() {
        let list = TransactionList::new();
        let display_str = list.to_string();

        assert_eq!(display_str, "Transactions:[]");
    }

    #[test]
    fn test_from_str_valid() {
        let input = "Transactions:[Transaction:transaction_id=12345;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000,Transaction:transaction_id=12346;taker_order_id=00000000-0000-0003-0000-000000000000;maker_order_id=00000000-0000-0004-0000-000000000000;price=10001;quantity=10;taker_side=SELL;timestamp=1616823000001]";
        let list = TransactionList::from_str(input).unwrap();

        assert_eq!(list.len(), 2);

        let tx1 = &list.transactions[0];
        assert_eq!(tx1.transaction_id, 12345);
        assert_eq!(tx1.taker_order_id, OrderId::from_u64(1));
        assert_eq!(tx1.maker_order_id, OrderId::from_u64(2));
        assert_eq!(tx1.price, 10000);
        assert_eq!(tx1.quantity, 5);
        assert_eq!(tx1.taker_side, Side::Buy);
        assert_eq!(tx1.timestamp, 1616823000000);

        let tx2 = &list.transactions[1];
        assert_eq!(tx2.transaction_id, 12346);
        assert_eq!(tx2.taker_order_id, OrderId::from_u64(3));
        assert_eq!(tx2.maker_order_id, OrderId::from_u64(4));
        assert_eq!(tx2.price, 10001);
        assert_eq!(tx2.quantity, 10);
        assert_eq!(tx2.taker_side, Side::Sell);
        assert_eq!(tx2.timestamp, 1616823000001);
    }

    #[test]
    fn test_from_str_empty() {
        let input = "Transactions:[]";
        let list = TransactionList::from_str(input).unwrap();

        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
    }

    #[test]
    fn test_from_str_invalid_format() {
        let input = "InvalidFormat";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());

        let input = "TransactionsList:[Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000]";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());

        let input = "Transactions:";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());

        let input = "Transactions:[Transaction:transaction_id=12345;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_invalid_transaction() {
        let input = "Transactions:[Transaction:transaction_id=abc;taker_order_id=1;maker_order_id=2;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000]";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());

        let input = "Transactions:[InvalidTransaction]";
        let result = TransactionList::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_serialization_round_trip() {
        let original = create_test_transaction_list();
        let string_representation = original.to_string();
        let parsed = TransactionList::from_str(&string_representation).unwrap();

        assert_eq!(parsed.len(), original.len());

        for i in 0..parsed.len() {
            assert_eq!(
                parsed.transactions[i].transaction_id,
                original.transactions[i].transaction_id
            );
            assert_eq!(
                parsed.transactions[i].taker_order_id,
                original.transactions[i].taker_order_id
            );
            assert_eq!(
                parsed.transactions[i].maker_order_id,
                original.transactions[i].maker_order_id
            );
            assert_eq!(parsed.transactions[i].price, original.transactions[i].price);
            assert_eq!(
                parsed.transactions[i].quantity,
                original.transactions[i].quantity
            );
            assert_eq!(
                parsed.transactions[i].taker_side,
                original.transactions[i].taker_side
            );
            assert_eq!(
                parsed.transactions[i].timestamp,
                original.transactions[i].timestamp
            );
        }
    }

    #[test]
    fn test_from_vec_and_into_vec() {
        let transactions = create_test_transactions();
        let list = TransactionList::from_vec(transactions.clone());
        assert_eq!(list.len(), transactions.len());

        let result_vec = list.into_vec();
        assert_eq!(result_vec.len(), transactions.len());

        for i in 0..result_vec.len() {
            assert_eq!(result_vec[i].transaction_id, transactions[i].transaction_id);
        }
    }

    #[test]
    fn test_from_into_trait_implementations() {
        let transactions = create_test_transactions();

        let list: TransactionList = transactions.clone().into();
        assert_eq!(list.len(), transactions.len());

        let list = TransactionList::from_vec(transactions.clone());
        let vec: Vec<Transaction> = list.into();
        assert_eq!(vec.len(), transactions.len());

        for i in 0..vec.len() {
            assert_eq!(vec[i].transaction_id, transactions[i].transaction_id);
        }
    }

    #[test]
    fn test_add_transaction() {
        let mut list = TransactionList::new();
        assert_eq!(list.len(), 0);

        let tx1 = create_test_transactions()[0];
        list.add(tx1);
        assert_eq!(list.len(), 1);
        assert_eq!(list.transactions[0].transaction_id, 12345);

        let tx2 = create_test_transactions()[1];
        list.add(tx2);
        assert_eq!(list.len(), 2);
        assert_eq!(list.transactions[1].transaction_id, 12346);
    }

    #[test]
    fn test_as_vec() {
        let list = create_test_transaction_list();
        let vec_ref = list.as_vec();

        assert_eq!(vec_ref.len(), 2);
        assert_eq!(vec_ref[0].transaction_id, 12345);
        assert_eq!(vec_ref[1].transaction_id, 12346);
    }

    #[test]
    fn test_default_implementation() {
        let list = TransactionList::default();
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
    }

    #[test]
    fn test_is_empty() {
        let empty_list = TransactionList::new();
        assert!(empty_list.is_empty());

        let non_empty_list = create_test_transaction_list();
        assert!(!non_empty_list.is_empty());
    }

    #[test]
    fn test_complex_transaction_list_parsing() {
        let input = "Transactions:[Transaction:transaction_id=12345;taker_order_id=00000000-0000-0001-0000-000000000000;maker_order_id=00000000-0000-0002-0000-000000000000;price=10000;quantity=5;taker_side=BUY;timestamp=1616823000000,Transaction:transaction_id=12346;taker_order_id=00000000-0000-0003-0000-000000000000;maker_order_id=00000000-0000-0004-0000-000000000000;price=10001;quantity=10;taker_side=SELL;timestamp=1616823000001,Transaction:transaction_id=12347;taker_order_id=00000000-0000-0005-0000-000000000000;maker_order_id=00000000-0000-0006-0000-000000000000;price=10002;quantity=15;taker_side=BUY;timestamp=1616823000002]";

        let list = TransactionList::from_str(input).unwrap();

        assert_eq!(list.len(), 3);
        assert_eq!(list.transactions[0].transaction_id, 12345);
        assert_eq!(list.transactions[1].transaction_id, 12346);
        assert_eq!(list.transactions[2].transaction_id, 12347);
    }
}
