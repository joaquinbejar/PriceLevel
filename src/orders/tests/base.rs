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
