#[cfg(test)]
mod tests {
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::price_level::PriceLevel;
    use std::str::FromStr;
    use std::sync::Arc;
    use tracing::info;

    #[test]
    fn test_display() {
        let level = Arc::new(PriceLevel::new(1000));
        let entry = OrderBookEntry::new(level.clone(), 5);

        let display_str = entry.to_string();
        info!("Display string: {}", display_str);

        assert!(display_str.starts_with("OrderBookEntry:"));
        assert!(display_str.contains("price=1000"));
        assert!(display_str.contains("index=5"));
    }

    #[test]
    fn test_from_str() {
        let input = "OrderBookEntry:price=1000;index=5";
        let entry = OrderBookEntry::from_str(input).unwrap();

        assert_eq!(entry.price(), 1000);
        assert_eq!(entry.index, 5);
    }

    #[test]
    fn test_roundtrip_display_parse() {
        let level = Arc::new(PriceLevel::new(1000));
        let original = OrderBookEntry::new(level.clone(), 5);

        let string_rep = original.to_string();
        let parsed = OrderBookEntry::from_str(&string_rep).unwrap();

        assert_eq!(original.price(), parsed.price());
        assert_eq!(original.index, parsed.index);
    }

    #[test]
    fn test_serialization() {
        use serde_json;

        let level = Arc::new(PriceLevel::new(1000));
        let entry = OrderBookEntry::new(level.clone(), 5);

        let serialized = serde_json::to_string(&entry).unwrap();
        info!("Serialized: {}", serialized);

        // Verify basic structure of JSON
        assert!(serialized.contains("\"price\":1000"));
        assert!(serialized.contains("\"index\":5"));
    }

    #[test]
    fn test_deserialization() {
        use serde_json;

        let json = r#"{"price":1000,"index":5}"#;
        let entry: OrderBookEntry = serde_json::from_str(json).unwrap();

        assert_eq!(entry.price(), 1000);
        assert_eq!(entry.index, 5);
    }
}
