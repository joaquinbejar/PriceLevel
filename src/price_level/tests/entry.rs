#[cfg(test)]
mod tests {
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use crate::{Hash32, Id, OrderType, Side, TimeInForce};
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

    #[test]
    fn test_order_book_entry_json_serialization() {
        let level = Arc::new(PriceLevel::new(10000));
        let entry = OrderBookEntry::new(level, 5);

        // Serialize to JSON
        let json = serde_json::to_string(&entry).unwrap();

        // Check JSON structure
        assert!(json.contains("\"price\":10000"));
        assert!(json.contains("\"index\":5"));
        assert!(json.contains("\"visible_quantity\":0"));
        assert!(json.contains("\"total_quantity\":0"));
    }

    #[test]
    fn test_order_book_entry_wrapper_struct() {
        // Directly test the wrapper struct used for deserialization
        #[derive(serde::Deserialize)]
        struct Wrapper {
            price: u64,
            index: usize,
        }

        let json = r#"{"price":10000,"index":5}"#;
        let wrapper: Wrapper = serde_json::from_str(json).unwrap();

        assert_eq!(wrapper.price, 10000);
        assert_eq!(wrapper.index, 5);
    }

    #[test]
    fn test_order_book_entry_equality_hash() {
        // Test line 76 - Testing Eq trait implementation
        let level1 = Arc::new(PriceLevel::new(1000));
        let level2 = Arc::new(PriceLevel::new(1000));

        let entry1 = OrderBookEntry::new(level1.clone(), 1);
        let entry2 = OrderBookEntry::new(level2.clone(), 2);

        // Test Eq trait implementation
        assert_eq!(entry1, entry2); // They should be equal as they have the same price

        // Create a hash set to test the Eq trait's blanket implementation
        let mut set = std::collections::HashSet::new();
        set.insert(entry1.price());
        assert!(set.contains(&entry2.price()));
    }

    #[test]
    fn test_order_book_entry_serialization() {
        // Test lines 100, 102-104 - Serialize implementation
        let level = Arc::new(PriceLevel::new(1000));
        let entry = OrderBookEntry::new(level.clone(), 5);

        // Add an order to make the test more meaningful
        let order = OrderType::Standard {
            id: Id::from_u64(1),
            price: Price::new(1000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        level.add_order(order).expect("add_order should succeed");

        // Serialize the entry
        let json = serde_json::to_string(&entry).unwrap();

        // Verify the serialized output contains expected fields
        assert!(json.contains("\"price\":1000"));
        assert!(json.contains("\"visible_quantity\":10"));
        assert!(json.contains("\"total_quantity\":10"));
        assert!(json.contains("\"index\":5"));
    }

    #[test]
    fn test_order_book_entry_deserialization() {
        // Test lines 130, 144, 152-153, 161-162 - Deserialize implementation
        let json = r#"{"price":1500,"index":10,"visible_quantity":50,"total_quantity":150}"#;

        // Deserialize into OrderBookEntry
        let entry: OrderBookEntry = serde_json::from_str(json).unwrap();

        // Verify deserialized values
        assert_eq!(entry.price(), 1500);
        assert_eq!(entry.index, 10);

        // The visible quantity and total quantity cannot be verified directly
        // as they come from the PriceLevel which is freshly created in deserialization
        // and not populated with orders
    }

    #[test]
    fn test_order_book_entry_from_str_with_invalid_input() {
        // Create a string with invalid format
        let invalid_input = "NotAnOrderBookEntry:price=1000;index=5";

        // Attempt to parse the invalid input
        let result = OrderBookEntry::from_str(invalid_input);

        // Verify parsing fails as expected
        assert!(result.is_err());

        // Test missing fields
        let missing_index = "OrderBookEntry:price=1000";
        let result = OrderBookEntry::from_str(missing_index);
        assert!(result.is_err());

        // Test invalid field values
        let invalid_price = "OrderBookEntry:price=invalid;index=5";
        let result = OrderBookEntry::from_str(invalid_price);
        assert!(result.is_err());

        let invalid_index = "OrderBookEntry:price=1000;index=invalid";
        let result = OrderBookEntry::from_str(invalid_index);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod tests_order_book_entry {
    use crate::orders::Hash32;
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::cmp::Ordering;
    use std::sync::Arc;

    /// Create a test OrderBookEntry with specified price and index
    fn create_test_entry(price: u128, index: usize) -> OrderBookEntry {
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
            id: crate::orders::Id::from_u64(1),
            price: Price::new(1000),
            quantity: Quantity::new(10),
            side: crate::orders::Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: crate::orders::TimeInForce::Gtc,
            extra_fields: (),
        };

        level1
            .add_order(order_type)
            .expect("add_order should succeed");
        assert_eq!(entry1.order_count(), 1);

        // Add another order
        let order_type2 = crate::orders::OrderType::Standard {
            id: crate::orders::Id::from_u64(2),
            price: Price::new(1000),
            quantity: Quantity::new(20),
            side: crate::orders::Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000001),
            time_in_force: crate::orders::TimeInForce::Gtc,
            extra_fields: (),
        };

        level1
            .add_order(order_type2)
            .expect("add_order should succeed");
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
        assert!(matches!(entry.total_quantity(), Ok(0)));

        // Add an order with visible quantity
        let standard_order = crate::orders::OrderType::Standard {
            id: crate::orders::Id::from_u64(1),
            price: Price::new(1000),
            quantity: Quantity::new(10),
            side: crate::orders::Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: crate::orders::TimeInForce::Gtc,
            extra_fields: (),
        };
        level
            .add_order(standard_order)
            .expect("add_order should succeed");

        // Check quantities after adding order
        assert_eq!(entry.visible_quantity(), 10);
        assert!(matches!(entry.total_quantity(), Ok(10)));

        // Add an iceberg order with hidden quantity
        let iceberg_order = crate::orders::OrderType::IcebergOrder {
            id: crate::orders::Id::from_u64(2),
            price: Price::new(1000),
            visible_quantity: Quantity::new(5),
            hidden_quantity: Quantity::new(15),
            side: crate::orders::Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000001),
            time_in_force: crate::orders::TimeInForce::Gtc,
            extra_fields: (),
        };
        level
            .add_order(iceberg_order)
            .expect("add_order should succeed");

        // Check quantities after adding iceberg order
        assert_eq!(entry.visible_quantity(), 15); // 10 + 5
        assert!(matches!(entry.total_quantity(), Ok(30))); // 10 + 5 + 15
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
            price: u128,
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

#[cfg(test)]
mod tests_order_book_entry_text_and_serde_contract {
    use crate::errors::PriceLevelError;
    use crate::orders::Hash32;
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use crate::{Id, OrderType, Side, TimeInForce};
    use serde::Serialize;
    use serde::ser::{self, Impossible};
    use std::fmt::{self, Write as _};
    use std::str::FromStr;
    use std::sync::Arc;

    const PRICE: u128 = 1000;

    /// A valid level whose `visible + hidden` total overflows `u64`: a
    /// standard order of `u64::MAX - 1` plus a same-side, same-price iceberg
    /// with visible 1 and hidden 1. Each order total and each counter fits.
    fn overflowing_total_level() -> Arc<PriceLevel> {
        let level = Arc::new(PriceLevel::new(PRICE));
        level
            .add_order(OrderType::Standard {
                id: Id::from_u64(1),
                price: Price::new(PRICE),
                quantity: Quantity::new(u64::MAX - 1),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            })
            .expect("standard order admits");
        level
            .add_order(OrderType::IcebergOrder {
                id: Id::from_u64(2),
                price: Price::new(PRICE),
                visible_quantity: Quantity::new(1),
                hidden_quantity: Quantity::new(1),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_001),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            })
            .expect("iceberg order admits");
        assert_eq!(level.visible_quantity(), u64::MAX);
        assert_eq!(level.hidden_quantity(), 1);
        level
    }

    fn populated_level() -> Arc<PriceLevel> {
        let level = Arc::new(PriceLevel::new(PRICE));
        level
            .add_order(OrderType::IcebergOrder {
                id: Id::from_u64(7),
                price: Price::new(PRICE),
                visible_quantity: Quantity::new(10),
                hidden_quantity: Quantity::new(30),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            })
            .expect("iceberg order admits");
        level
    }

    fn resting_ids(level: &PriceLevel) -> Vec<Id> {
        level
            .snapshot_orders()
            .expect("materialize")
            .iter()
            .map(|o| o.id())
            .collect()
    }

    #[test]
    fn test_order_book_entry_display_overflowing_total_formats_without_panic() {
        let entry = OrderBookEntry::new(overflowing_total_level(), 3);

        let via_to_string = entry.to_string();
        let via_format = format!("{entry}");

        let expected = format!(
            "OrderBookEntry:price={PRICE};visible_quantity={};index=3",
            u64::MAX
        );
        assert_eq!(via_to_string, expected);
        assert_eq!(via_format, expected);
    }

    #[test]
    fn test_order_book_entry_to_full_string_overflowing_total_returns_invalid_operation() {
        let level = overflowing_total_level();
        let ids_before = resting_ids(&level);
        let entry = OrderBookEntry::new(Arc::clone(&level), 3);

        let result = entry.to_full_string();

        match result {
            Err(PriceLevelError::InvalidOperation { message }) => {
                assert!(message.contains("total quantity overflow"), "{message}");
            }
            other => panic!("expected InvalidOperation, got {other:?}"),
        }
        // The referenced level is left unchanged.
        assert_eq!(level.visible_quantity(), u64::MAX);
        assert_eq!(level.hidden_quantity(), 1);
        assert_eq!(level.order_count(), 2);
        assert_eq!(resting_ids(&level), ids_before);
    }

    #[test]
    fn test_order_book_entry_to_full_string_normal_level_includes_all_fields() {
        let entry = OrderBookEntry::new(populated_level(), 5);

        let text = entry.to_full_string().expect("total fits in u64");

        assert_eq!(
            text,
            "OrderBookEntry:price=1000;visible_quantity=10;total_quantity=40;index=5"
        );
    }

    #[test]
    fn test_order_book_entry_display_normal_level_omits_total_quantity() {
        let entry = OrderBookEntry::new(populated_level(), 5);

        assert_eq!(
            entry.to_string(),
            "OrderBookEntry:price=1000;visible_quantity=10;index=5"
        );
    }

    #[test]
    fn test_order_book_entry_to_full_string_round_trip_parses() {
        let original = OrderBookEntry::new(populated_level(), 5);

        let text = original.to_full_string().expect("total fits in u64");
        let parsed = OrderBookEntry::from_str(&text).expect("full text parses");

        assert_eq!(parsed.price(), original.price());
        assert_eq!(parsed.index, original.index);
    }

    #[test]
    fn test_order_book_entry_to_full_string_max_values_fit_reserved_capacity() {
        let level = Arc::new(PriceLevel::new(u128::MAX));
        level
            .add_order(OrderType::Standard {
                id: Id::from_u64(1),
                price: Price::new(u128::MAX),
                quantity: Quantity::new(u64::MAX),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            })
            .expect("standard order admits");
        let entry = OrderBookEntry::new(level, usize::MAX);

        let text = entry.to_full_string().expect("total fits in u64");

        let expected = format!(
            "OrderBookEntry:price={};visible_quantity={};total_quantity={};index={}",
            u128::MAX,
            u64::MAX,
            u64::MAX,
            usize::MAX
        );
        assert_eq!(text, expected);
        // The digit bounds used for the up-front reservation hold.
        assert!(u128::MAX.to_string().len() <= 39);
        assert!(u64::MAX.to_string().len() <= 20);
        assert!(usize::MAX.to_string().len() <= 20);
    }

    /// `fmt::Write` sink that always fails, to prove real sink errors still
    /// propagate through `Display` as `fmt::Error` (and nothing panics).
    struct FailingSink;

    impl fmt::Write for FailingSink {
        fn write_str(&mut self, _s: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[test]
    fn test_order_book_entry_display_failing_sink_propagates_fmt_error() {
        let entry = OrderBookEntry::new(overflowing_total_level(), 3);

        let result = write!(FailingSink, "{entry}");

        assert_eq!(result, Err(fmt::Error));
    }

    // ---- Count-aware serializer ------------------------------------------

    #[derive(Debug, PartialEq)]
    struct CountError(String);

    impl fmt::Display for CountError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for CountError {}

    impl ser::Error for CountError {
        fn custom<T: fmt::Display>(msg: T) -> Self {
            CountError(msg.to_string())
        }
    }

    /// Records what a struct serialization declared and emitted.
    #[derive(Debug, Default, PartialEq)]
    struct StructRecord {
        name: &'static str,
        declared_len: usize,
        fields: Vec<&'static str>,
        ended: bool,
    }

    /// Minimal serializer that only accepts a top-level struct and records
    /// the declared length and the emitted field names, in order.
    struct CountingSerializer<'a> {
        record: &'a mut StructRecord,
    }

    struct CountingStruct<'a> {
        record: &'a mut StructRecord,
    }

    impl ser::SerializeStruct for CountingStruct<'_> {
        type Ok = ();
        type Error = CountError;

        fn serialize_field<T: ?Sized + Serialize>(
            &mut self,
            key: &'static str,
            _value: &T,
        ) -> Result<(), CountError> {
            self.record.fields.push(key);
            Ok(())
        }

        fn end(self) -> Result<(), CountError> {
            self.record.ended = true;
            Ok(())
        }
    }

    fn unsupported<T>() -> Result<T, CountError> {
        Err(CountError("unsupported by CountingSerializer".to_string()))
    }

    impl<'a> ser::Serializer for CountingSerializer<'a> {
        type Ok = ();
        type Error = CountError;
        type SerializeSeq = Impossible<(), CountError>;
        type SerializeTuple = Impossible<(), CountError>;
        type SerializeTupleStruct = Impossible<(), CountError>;
        type SerializeTupleVariant = Impossible<(), CountError>;
        type SerializeMap = Impossible<(), CountError>;
        type SerializeStruct = CountingStruct<'a>;
        type SerializeStructVariant = Impossible<(), CountError>;

        fn serialize_struct(
            self,
            name: &'static str,
            len: usize,
        ) -> Result<CountingStruct<'a>, CountError> {
            self.record.name = name;
            self.record.declared_len = len;
            Ok(CountingStruct {
                record: self.record,
            })
        }

        fn serialize_bool(self, _v: bool) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_i8(self, _v: i8) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_i16(self, _v: i16) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_i32(self, _v: i32) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_i64(self, _v: i64) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_u8(self, _v: u8) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_u16(self, _v: u16) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_u32(self, _v: u32) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_u64(self, _v: u64) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_f32(self, _v: f32) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_f64(self, _v: f64) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_char(self, _v: char) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_str(self, _v: &str) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_bytes(self, _v: &[u8]) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_none(self) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_some<T: ?Sized + Serialize>(self, _value: &T) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_unit(self) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_unit_struct(self, _name: &'static str) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_unit_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
        ) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_newtype_struct<T: ?Sized + Serialize>(
            self,
            _name: &'static str,
            _value: &T,
        ) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_newtype_variant<T: ?Sized + Serialize>(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _value: &T,
        ) -> Result<(), CountError> {
            unsupported()
        }
        fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, CountError> {
            unsupported()
        }
        fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, CountError> {
            unsupported()
        }
        fn serialize_tuple_struct(
            self,
            _name: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleStruct, CountError> {
            unsupported()
        }
        fn serialize_tuple_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleVariant, CountError> {
            unsupported()
        }
        fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, CountError> {
            unsupported()
        }
        fn serialize_struct_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeStructVariant, CountError> {
            unsupported()
        }
    }

    #[test]
    fn test_order_book_entry_serialize_declared_len_matches_emitted_fields() {
        let entry = OrderBookEntry::new(populated_level(), 5);
        let mut record = StructRecord::default();

        entry
            .serialize(CountingSerializer {
                record: &mut record,
            })
            .expect("counting serialization succeeds");

        assert_eq!(record.name, "OrderBookEntry");
        assert_eq!(
            record.fields,
            ["price", "visible_quantity", "total_quantity", "index"]
        );
        assert_eq!(record.declared_len, record.fields.len());
        assert_eq!(record.declared_len, 4);
        assert!(record.ended);
    }

    #[test]
    fn test_order_book_entry_serialize_overflowing_total_returns_serializer_error() {
        let level = overflowing_total_level();
        let entry = OrderBookEntry::new(Arc::clone(&level), 3);
        let mut record = StructRecord::default();

        let result = entry.serialize(CountingSerializer {
            record: &mut record,
        });

        match result {
            Err(CountError(message)) => {
                assert!(message.contains("total quantity overflow"), "{message}");
            }
            Ok(()) => panic!("expected serializer error"),
        }
        assert_eq!(record.declared_len, 4);
        assert_eq!(record.fields, ["price", "visible_quantity"]);
        assert!(!record.ended);
        assert_eq!(level.order_count(), 2);

        assert!(serde_json::to_string(&entry).is_err());
    }
}
