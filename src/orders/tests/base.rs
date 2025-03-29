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
    use std::str::FromStr;
    use crate::orders::OrderId;
    use uuid::Uuid;

    #[test]
    fn test_order_id_creation() {
        // Create using from_u64 for backward compatibility
        let id = OrderId::from_u64(12345);
        assert_eq!(id.0.as_u64_pair().1 & 0xFFFFFFFFFFFFFF00, 0); // Upper bytes should contain the ID

        // Create random UUIDs
        let id1 = OrderId::new();
        let id2 = OrderId::new();
        assert_ne!(id1, id2); // Random UUIDs should be different

        // Create from existing UUID
        let uuid = Uuid::new_v4();
        let id = OrderId::from_uuid(uuid);
        assert_eq!(id.0, uuid);

        // Create nil UUID
        let nil_id = OrderId::nil();
        assert_eq!(nil_id.0, Uuid::nil());
    }

    #[test]
    fn test_order_id_equality() {
        let id1 = OrderId::from_u64(12345);
        let id2 = OrderId::from_u64(12345);
        let id3 = OrderId::from_u64(54321);
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_order_id_clone() {
        let id = OrderId::from_u64(12345);
        let cloned_id = id;
        assert_eq!(id, cloned_id);
    }

    #[test]
    fn test_order_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(OrderId::from_u64(12345));
        set.insert(OrderId::from_u64(54321));
        assert!(set.contains(&OrderId::from_u64(12345)));
        assert!(set.contains(&OrderId::from_u64(54321)));
        assert!(!set.contains(&OrderId::from_u64(99999)));
        set.insert(OrderId::from_u64(12345));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_serialize_deserialize() {
        let id = OrderId::from_u64(12345);
        let serialized = serde_json::to_string(&id).unwrap();
        let expected_uuid = id.0.to_string();
        assert!(serialized.contains(&expected_uuid));

        let deserialized: OrderId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn test_from_str_valid() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let order_id = OrderId::from_str(uuid_str).unwrap();
        assert_eq!(order_id.0.to_string(), uuid_str);

        // Test that legacy conversions still work through string format
        let u64_id = 12345;
        let order_id_from_u64 = OrderId::from_u64(u64_id);
        let order_id_str = order_id_from_u64.to_string();
        let order_id_parsed = OrderId::from_str(&order_id_str).unwrap();
        assert_eq!(order_id_from_u64, order_id_parsed);
    }

    #[test]
    fn test_from_str_invalid() {
        assert!(OrderId::from_str("").is_err());
        assert!(OrderId::from_str("not-a-uuid").is_err());
        assert!(OrderId::from_str("550e8400-e29b-41d4-a716").is_err()); // Incomplete UUID
    }

    #[test]
    fn test_display() {
        let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let id = OrderId::from_uuid(uuid);
        assert_eq!(format!("{}", id), "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_roundtrip() {
        // Test U64 round trip
        let original = 12345u64;
        let id = OrderId::from_u64(original);
        let string = id.to_string();
        let parsed = string.parse::<OrderId>().unwrap();
        assert_eq!(parsed, id);

        // Test UUID round trip
        let uuid = Uuid::new_v4();
        let id = OrderId::from_uuid(uuid);
        let string = id.to_string();
        let parsed = string.parse::<OrderId>().unwrap();
        assert_eq!(parsed, id);
    }
}
