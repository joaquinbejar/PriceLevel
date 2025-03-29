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
