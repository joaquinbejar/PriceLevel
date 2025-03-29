#[cfg(test)]
mod tests {
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::level::PriceLevel;
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

#[cfg(test)]
mod tests_order_book_entry {
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::level::PriceLevel;
    use std::cmp::Ordering;
    use std::sync::Arc;

    /// Create a test OrderBookEntry with specified price and index
    fn create_test_entry(price: u64, index: usize) -> OrderBookEntry {
        let level = Arc::new(PriceLevel::new(price));
        OrderBookEntry::new(level, index)
    }

    #[test]
    /// Test the order_count method returns the correct count
    fn test_order_count() {
        // Create two price levels with different characteristics
        let level1 = Arc::new(PriceLevel::new(1000));
        let entry1 = OrderBookEntry::new(level1.clone(), 5);

        // Initially should have zero orders
        assert_eq!(entry1.order_count(), 0);

        // Add some orders and check again
        let order_type = crate::orders::OrderType::Standard {
            id: crate::orders::OrderId::from_u64(1),
            price: 1000,
            quantity: 10,
            side: crate::orders::Side::Buy,
            timestamp: 1616823000000,
            time_in_force: crate::orders::TimeInForce::Gtc,
        };

        level1.add_order(order_type);
        assert_eq!(entry1.order_count(), 1);

        // Add another order
        let order_type2 = crate::orders::OrderType::Standard {
            id: crate::orders::OrderId::from_u64(2),
            price: 1000,
            quantity: 20,
            side: crate::orders::Side::Buy,
            timestamp: 1616823000001,
            time_in_force: crate::orders::TimeInForce::Gtc,
        };

        level1.add_order(order_type2);
        assert_eq!(entry1.order_count(), 2);
    }

    #[test]
    /// Test the equality comparison between entries
    fn test_partial_eq() {
        // Create entries with same price but different indices
        let entry1 = create_test_entry(1000, 5);
        let entry2 = create_test_entry(1000, 10);

        // Entries should be equal because they have the same price
        assert_eq!(entry1, entry2);

        // Create an entry with different price
        let entry3 = create_test_entry(2000, 5);

        // Entries should not be equal because they have different prices
        assert_ne!(entry1, entry3);
    }

    #[test]
    /// Test that Eq trait is implemented correctly
    fn test_eq() {
        // This test is mostly to verify the Eq trait's blanket implementation
        let entry1 = create_test_entry(1000, 5);
        let entry2 = create_test_entry(1000, 10);

        // Use in a context requiring Eq
        let mut entries = std::collections::HashSet::new();
        entries.insert(entry1.price());
        entries.insert(entry2.price());

        // Should only have one entry because prices are the same
        assert_eq!(entries.len(), 1);
    }

    #[test]
    /// Test partial ordering comparison
    fn test_partial_ord() {
        let entry1 = create_test_entry(1000, 5);
        let entry2 = create_test_entry(2000, 10);

        // entry1 should be less than entry2
        assert!(entry1.partial_cmp(&entry2) == Some(Ordering::Less));
        // entry2 should be greater than entry1
        assert!(entry2.partial_cmp(&entry1) == Some(Ordering::Greater));
        // entry1 should be equal to itself
        assert!(entry1.partial_cmp(&entry1) == Some(Ordering::Equal));
    }

    #[test]
    /// Test total ordering comparison
    fn test_ord() {
        let entry1 = create_test_entry(1000, 5);
        let entry2 = create_test_entry(2000, 10);
        let entry3 = create_test_entry(500, 15);

        // Direct comparisons
        assert!(entry1 < entry2);
        assert!(entry2 > entry1);
        assert!(entry3 < entry1);

        // Test sorting behavior
        let mut entries = [entry2, entry1, entry3];
        entries.sort();

        // After sorting, should be in order of increasing price
        assert_eq!(entries[0].price(), 500);
        assert_eq!(entries[1].price(), 1000);
        assert_eq!(entries[2].price(), 2000);
    }

    #[test]
    /// Test ordering works correctly with binary search
    fn test_binary_search() {
        // Create sorted entries
        let entries = [
            create_test_entry(500, 1),
            create_test_entry(1000, 2),
            create_test_entry(1500, 3),
            create_test_entry(2000, 4),
            create_test_entry(2500, 5),
        ];

        // Search for existing entry
        let search_entry = create_test_entry(1500, 100); // Different index, same price
        let result = entries.binary_search(&search_entry);
        assert_eq!(result, Ok(2)); // Should find at index 2

        // Search for entry that doesn't exist but would be inserted at index 3
        let search_entry = create_test_entry(1800, 100);
        let result = entries.binary_search(&search_entry);
        assert_eq!(result, Err(3)); // Should suggest insertion at index 3
    }

    #[test]
    /// Test price accessor method returns correct value
    fn test_price() {
        let entry = create_test_entry(1234, 5);
        assert_eq!(entry.price(), 1234);
    }

    #[test]
    /// Test that index is stored and accessible
    fn test_index() {
        let entry = create_test_entry(1000, 42);
        assert_eq!(entry.index, 42);
    }

    #[test]
    /// Test that visible_quantity and total_quantity are correctly delegated to PriceLevel
    fn test_quantity_methods() {
        let level = Arc::new(PriceLevel::new(1000));
        let entry = OrderBookEntry::new(level.clone(), 5);

        // Initially quantities should be zero
        assert_eq!(entry.visible_quantity(), 0);
        assert_eq!(entry.total_quantity(), 0);

        // Add an order with visible quantity
        let standard_order = crate::orders::OrderType::Standard {
            id: crate::orders::OrderId::from_u64(1),
            price: 1000,
            quantity: 10,
            side: crate::orders::Side::Buy,
            timestamp: 1616823000000,
            time_in_force: crate::orders::TimeInForce::Gtc,
        };
        level.add_order(standard_order);

        // Check quantities after adding order
        assert_eq!(entry.visible_quantity(), 10);
        assert_eq!(entry.total_quantity(), 10);

        // Add an iceberg order with hidden quantity
        let iceberg_order = crate::orders::OrderType::IcebergOrder {
            id: crate::orders::OrderId::from_u64(2),
            price: 1000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: crate::orders::Side::Buy,
            timestamp: 1616823000001,
            time_in_force: crate::orders::TimeInForce::Gtc,
        };
        level.add_order(iceberg_order);

        // Check quantities after adding iceberg order
        assert_eq!(entry.visible_quantity(), 15); // 10 + 5
        assert_eq!(entry.total_quantity(), 30); // 10 + 5 + 15
    }
}

#[cfg(test)]
mod tests_order_book_entry_deserialize {
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::level::PriceLevel;
    use std::sync::Arc;

    #[test]
    /// Test deserialization from JSON with minimum fields
    fn test_deserialize_from_json_basic() {
        // Create a simple JSON representation
        let json = r#"{"price":1000,"index":5}"#;

        // Deserialize into OrderBookEntry
        let entry: OrderBookEntry = serde_json::from_str(json).unwrap();

        // Assert the deserialized values match expected values
        assert_eq!(entry.price(), 1000);
        assert_eq!(entry.index, 5);
        assert_eq!(entry.order_count(), 0); // New PriceLevel should have 0 orders
    }

    #[test]
    /// Test deserialization handles additional fields gracefully
    fn test_deserialize_with_extra_fields() {
        // JSON with additional fields that should be ignored
        let json = r#"{
            "price": 1500,
            "index": 10,
            "visible_quantity": 100,
            "total_quantity": 200,
            "unknown_field": "value"
        }"#;

        // Deserialize should work despite extra fields
        let entry: OrderBookEntry = serde_json::from_str(json).unwrap();

        // Check the values were properly deserialized
        assert_eq!(entry.price(), 1500);
        assert_eq!(entry.index, 10);
    }

    #[test]
    /// Test deserialization fails when required fields are missing
    fn test_deserialize_missing_fields() {
        // Missing price field
        let json_missing_price = r#"{"index": 5}"#;
        let result = serde_json::from_str::<OrderBookEntry>(json_missing_price);
        assert!(result.is_err());

        // Missing index field
        let json_missing_index = r#"{"price": 1000}"#;
        let result = serde_json::from_str::<OrderBookEntry>(json_missing_index);
        assert!(result.is_err());
    }

    #[test]
    /// Test deserialization fails with invalid field types
    fn test_deserialize_invalid_types() {
        // Invalid type for price (string instead of number)
        let json_invalid_price = r#"{"price":"invalid","index":5}"#;
        let result = serde_json::from_str::<OrderBookEntry>(json_invalid_price);
        assert!(result.is_err());

        // Invalid type for index (string instead of number)
        let json_invalid_index = r#"{"price":1000,"index":"invalid"}"#;
        let result = serde_json::from_str::<OrderBookEntry>(json_invalid_index);
        assert!(result.is_err());
    }

    #[test]
    /// Test deserialization from different JSON formats
    fn test_deserialize_different_formats() {
        // Test with integer index
        let json_int = r#"{"price":1000,"index":5}"#;
        let entry: OrderBookEntry = serde_json::from_str(json_int).unwrap();
        assert_eq!(entry.index, 5);

        // Test with larger integers
        let json_large_values = r#"{"price":18446744073709551615,"index":4294967295}"#; // max u64, max u32
        let entry: OrderBookEntry = serde_json::from_str(json_large_values).unwrap();
        assert_eq!(entry.price(), 18446744073709551615);
        assert_eq!(entry.index, 4294967295);
    }

    #[test]
    /// Test Wrapper struct directly used in deserialization implementation
    fn test_deserialize_wrapper_struct() {
        // Access the internal Wrapper struct - requires knowledge of implementation details
        // This is based on the Deserialize implementation shown earlier
        #[derive(serde::Deserialize)]
        struct Wrapper {
            price: u64,
            index: usize,
        }

        let json = r#"{"price":1000,"index":5}"#;
        let wrapper: Wrapper = serde_json::from_str(json).unwrap();

        assert_eq!(wrapper.price, 1000);
        assert_eq!(wrapper.index, 5);

        // Create an OrderBookEntry from the wrapper manually
        let level = Arc::new(PriceLevel::new(wrapper.price));
        let entry = OrderBookEntry::new(level, wrapper.index);

        assert_eq!(entry.price(), 1000);
        assert_eq!(entry.index, 5);
    }

    #[test]
    /// Test deserialization from a complete JSON data structure
    fn test_deserialize_from_complete_json() {
        // More complete JSON with nested structure similar to what might be used in practice
        let json = r#"{
            "price": 1000, 
            "index": 5,
            "level_data": {
                "visible_quantity": 10,
                "hidden_quantity": 20,
                "order_count": 2
            }
        }"#;

        // Despite extra nested fields, deserialization should still work
        let entry: OrderBookEntry = serde_json::from_str(json).unwrap();

        assert_eq!(entry.price(), 1000);
        assert_eq!(entry.index, 5);
    }

    #[test]
    /// Test round-trip serialization and deserialization
    fn test_serde_round_trip() {
        // Create an original entry
        let original_level = Arc::new(PriceLevel::new(1500));
        let original_entry = OrderBookEntry::new(original_level, 25);

        // Serialize to JSON
        let serialized = serde_json::to_string(&original_entry).unwrap();

        // Deserialize back
        let deserialized: OrderBookEntry = serde_json::from_str(&serialized).unwrap();

        // Compare values
        assert_eq!(deserialized.price(), original_entry.price());
        assert_eq!(deserialized.index, original_entry.index);
    }
}
