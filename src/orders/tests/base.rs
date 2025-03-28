#[cfg(test)]
mod tests_side {
    use crate::orders::Side;

    #[test]
    fn test_side_equality() {
        assert_eq!(Side::Buy, Side::Buy);
        assert_eq!(Side::Sell, Side::Sell);
        assert_ne!(Side::Buy, Side::Sell);
    }

    #[test]
    fn test_side_clone() {
        let buy = Side::Buy;
        let cloned_buy = buy;
        assert_eq!(buy, cloned_buy);

        let sell = Side::Sell;
        let cloned_sell = sell;
        assert_eq!(sell, cloned_sell);
    }

    #[test]
    fn test_serialize_to_uppercase() {
        assert_eq!(serde_json::to_string(&Side::Buy).unwrap(), "\"BUY\"");
        assert_eq!(serde_json::to_string(&Side::Sell).unwrap(), "\"SELL\"");
    }

    #[test]
    fn test_deserialize_uppercase() {
        assert_eq!(serde_json::from_str::<Side>("\"BUY\"").unwrap(), Side::Buy);
        assert_eq!(
            serde_json::from_str::<Side>("\"SELL\"").unwrap(),
            Side::Sell
        );
    }

    #[test]
    fn test_deserialize_lowercase() {
        assert_eq!(serde_json::from_str::<Side>("\"buy\"").unwrap(), Side::Buy);
        assert_eq!(
            serde_json::from_str::<Side>("\"sell\"").unwrap(),
            Side::Sell
        );
    }

    #[test]
    fn test_deserialize_capitalized() {
        assert_eq!(serde_json::from_str::<Side>("\"Buy\"").unwrap(), Side::Buy);
        assert_eq!(
            serde_json::from_str::<Side>("\"Sell\"").unwrap(),
            Side::Sell
        );
    }

    #[test]
    fn test_round_trip_serialization() {
        let sides = vec![Side::Buy, Side::Sell];

        for side in sides {
            let serialized = serde_json::to_string(&side).unwrap();
            let deserialized: Side = serde_json::from_str(&serialized).unwrap();
            assert_eq!(side, deserialized);
        }
    }

    #[test]
    fn test_invalid_deserialization() {
        assert!(serde_json::from_str::<Side>("\"INVALID\"").is_err());
        assert!(serde_json::from_str::<Side>("\"BUYING\"").is_err());
        assert!(serde_json::from_str::<Side>("\"SELLING\"").is_err());
        assert!(serde_json::from_str::<Side>("123").is_err());
        assert!(serde_json::from_str::<Side>("null").is_err());
    }

    #[test]
    fn test_from_string() {
        assert_eq!("BUY".parse::<Side>().unwrap(), Side::Buy);
        assert_eq!("SELL".parse::<Side>().unwrap(), Side::Sell);
        assert_eq!("buy".parse::<Side>().unwrap(), Side::Buy);
        assert_eq!("sell".parse::<Side>().unwrap(), Side::Sell);
    }

    #[test]
    fn test_serialized_size() {
        assert_eq!(serde_json::to_string(&Side::Buy).unwrap().len(), 5); // "BUY"
        assert_eq!(serde_json::to_string(&Side::Sell).unwrap().len(), 6); // "SELL"
    }
}

#[cfg(test)]
mod tests_orderid {
    use crate::orders::OrderId;

    #[test]
    fn test_order_id_creation() {
        let id = OrderId(12345);
        assert_eq!(id.0, 12345);
    }

    #[test]
    fn test_order_id_equality() {
        let id1 = OrderId(12345);
        let id2 = OrderId(12345);
        let id3 = OrderId(54321);
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_order_id_clone() {
        let id = OrderId(12345);
        let cloned_id = id;
        assert_eq!(id, cloned_id);
    }

    #[test]
    fn test_order_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(OrderId(12345));
        set.insert(OrderId(54321));
        assert!(set.contains(&OrderId(12345)));
        assert!(set.contains(&OrderId(54321)));
        assert!(!set.contains(&OrderId(99999)));
        set.insert(OrderId(12345));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_serialize_deserialize() {
        let id = OrderId(12345);
        let serialized = serde_json::to_string(&id).unwrap();
        assert_eq!(serialized, "12345");

        let deserialized: OrderId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn test_from_str_valid() {
        assert_eq!("12345".parse::<OrderId>().unwrap(), OrderId(12345));
        assert_eq!("0".parse::<OrderId>().unwrap(), OrderId(0));
        assert_eq!(
            "18446744073709551615".parse::<OrderId>().unwrap(),
            OrderId(u64::MAX)
        );
    }

    #[test]
    fn test_from_str_invalid() {
        assert!("".parse::<OrderId>().is_err());
        assert!("-1".parse::<OrderId>().is_err());
        assert!("abc".parse::<OrderId>().is_err());
        assert!("123.45".parse::<OrderId>().is_err());
        assert!("18446744073709551616".parse::<OrderId>().is_err()); // u64::MAX + 1
    }

    #[test]
    fn test_error_message() {
        let error = "abc".parse::<OrderId>().unwrap_err();
        assert!(error.to_string().contains("Failed to parse OrderId"));
    }

    #[test]
    fn test_display() {
        let id = OrderId(12345);
        assert_eq!(format!("{}", id), "12345");
        assert_eq!(id.to_string(), "12345");
    }

    #[test]
    fn test_roundtrip() {
        let original = 12345u64;
        let id = OrderId(original);
        let string = id.to_string();
        let parsed = string.parse::<OrderId>().unwrap();
        let final_value = parsed.0;
        assert_eq!(original, final_value);
    }
}

#[cfg(test)]
mod tests_order {
    use crate::errors::PriceLevelError::ParseError;
    use crate::orders::base::Order;
    use crate::orders::{OrderId, Side};

    #[test]
    fn test_order_creation() {
        let order = Order::new(1, 100, 10, Side::Buy, 1656789012345);
        assert_eq!(order.id, OrderId(1));
        assert_eq!(order.price, 100);
        assert_eq!(order.quantity, 10);
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.timestamp, 1656789012345);

        let buy_order = Order::buy(2, 200, 20, 1656789012346);
        assert_eq!(buy_order.id, OrderId(2));
        assert_eq!(buy_order.price, 200);
        assert_eq!(buy_order.quantity, 20);
        assert_eq!(buy_order.side, Side::Buy);
        assert_eq!(buy_order.timestamp, 1656789012346);

        let sell_order = Order::sell(3, 300, 30, 1656789012347);
        assert_eq!(sell_order.id, OrderId(3));
        assert_eq!(sell_order.price, 300);
        assert_eq!(sell_order.quantity, 30);
        assert_eq!(sell_order.side, Side::Sell);
        assert_eq!(sell_order.timestamp, 1656789012347);
    }

    #[test]
    fn test_order_equality() {
        let order1 = Order::new(1, 100, 10, Side::Buy, 1656789012345);
        let order2 = Order::new(1, 100, 10, Side::Buy, 1656789012345);
        let order3 = Order::new(2, 100, 10, Side::Buy, 1656789012345);
        let order4 = Order::new(1, 200, 10, Side::Buy, 1656789012345);
        let order5 = Order::new(1, 100, 20, Side::Buy, 1656789012345);
        let order6 = Order::new(1, 100, 10, Side::Sell, 1656789012345);
        let order7 = Order::new(1, 100, 10, Side::Buy, 1656789012346);

        assert_eq!(order1, order2);
        assert_ne!(order1, order3);
        assert_ne!(order1, order4);
        assert_ne!(order1, order5);
        assert_ne!(order1, order6);
        assert_ne!(order1, order7);
    }

    #[test]
    fn test_order_clone() {
        let order = Order::buy(1, 100, 10, 1656789012345);
        let cloned = order;
        assert_eq!(order, cloned);
    }

    #[test]
    fn test_serialize_deserialize() {
        let order = Order::new(1, 100, 10, Side::Buy, 1656789012345);

        let serialized = serde_json::to_string(&order).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["price"], 100);
        assert_eq!(parsed["quantity"], 10);
        assert_eq!(parsed["side"], "BUY");
        assert_eq!(parsed["timestamp"].as_u64().unwrap(), 1656789012345);
        let deserialized: Order = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, order);
    }

    #[test]
    fn test_display() {
        let buy_order = Order::buy(1, 100, 10, 1656789012345);
        let sell_order = Order::sell(2, 200, 20, 1656789012346);

        assert_eq!(buy_order.to_string(), "1:100:10:BUY:1656789012345");
        assert_eq!(sell_order.to_string(), "2:200:20:SELL:1656789012346");
    }

    #[test]
    fn test_from_str_valid() {
        let order1: Order = "1:100:10:BUY:1656789012345".parse().unwrap();
        let expected1 = Order::buy(1, 100, 10, 1656789012345);
        assert_eq!(order1, expected1);

        let order2: Order = "2:200:20:SELL:1656789012346".parse().unwrap();
        let expected2 = Order::sell(2, 200, 20, 1656789012346);
        assert_eq!(order2, expected2);

        let order3: Order = "3:300:30:buy:1656789012347".parse().unwrap();
        let expected3 = Order::buy(3, 300, 30, 1656789012347);
        assert_eq!(order3, expected3);

        let order4: Order = "4:400:40:sell:1656789012348".parse().unwrap();
        let expected4 = Order::sell(4, 400, 40, 1656789012348);
        assert_eq!(order4, expected4);
    }

    #[test]
    fn test_from_str_invalid() {
        assert!("1:100:10:BUY".parse::<Order>().is_err());
        assert!("abc:100:10:BUY:1656789012345".parse::<Order>().is_err());
        assert!("1:abc:10:BUY:1656789012345".parse::<Order>().is_err());
        assert!("1:100:abc:BUY:1656789012345".parse::<Order>().is_err());
        assert!("1:100:10:INVALID:1656789012345".parse::<Order>().is_err());
        assert!("1:100:10:BUY:abc".parse::<Order>().is_err());
        assert!("1:100:10:BUY:1656789012345:extra".parse::<Order>().is_err());
    }

    #[test]
    fn test_error_messages() {
        let error1 = "1:100:10".parse::<Order>().unwrap_err();
        matches!(error1, ParseError { .. });
        let message1 = error1.to_string();
        assert!(
            message1.contains("Expected 5 parts"),
            "Error message was: {}",
            message1
        );

        let error2 = "abc:100:10:BUY:1656789012345".parse::<Order>().unwrap_err();
        matches!(error2, ParseError { .. });
        let message2 = error2.to_string();
        assert!(
            message2.contains("Failed to parse id"),
            "Error message was: {}",
            message2
        );

        let error3 = "1:100:10:INVALID:1656789012345"
            .parse::<Order>()
            .unwrap_err();
        matches!(error3, ParseError { .. });
        let message3 = error3.to_string();
        assert!(
            message3.contains("Failed to parse side"),
            "Error message was: {}",
            message3
        );
    }

    #[test]
    fn test_roundtrip() {
        let original = Order::buy(12345, 9876, 54321, 1656789012345);
        let string = original.to_string();
        let parsed = string.parse::<Order>().unwrap();

        assert_eq!(original, parsed);
    }

    #[test]
    fn test_json_deserialization_case_insensitivity() {
        let json_upper =
            r#"{"id":1,"price":100,"quantity":10,"side":"BUY","timestamp":1656789012345}"#;
        let json_lower =
            r#"{"id":1,"price":100,"quantity":10,"side":"buy","timestamp":1656789012345}"#;
        let json_mixed =
            r#"{"id":1,"price":100,"quantity":10,"side":"Buy","timestamp":1656789012345}"#;

        let expected = Order::buy(1, 100, 10, 1656789012345);

        assert_eq!(serde_json::from_str::<Order>(json_upper).unwrap(), expected);
        assert_eq!(serde_json::from_str::<Order>(json_lower).unwrap(), expected);
        assert_eq!(serde_json::from_str::<Order>(json_mixed).unwrap(), expected);
    }
}
