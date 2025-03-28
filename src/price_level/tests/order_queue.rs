#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;
    use crate::orders::{OrderId, OrderType, Side, TimeInForce};
    use crate::price_level::order_queue::OrderQueue;

    fn create_test_order(id: u64, price: u64, quantity: u64) -> OrderType {
        OrderType::Standard {
            id: OrderId(id),
            price,
            quantity,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        }
    }

    #[test]
    fn test_display() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000, 10)));
        queue.push(Arc::new(create_test_order(2, 1100, 20)));

        let display_string = queue.to_string();
        println!("Display: {}", display_string);

        assert!(display_string.starts_with("OrderQueue:orders=["));
        assert!(display_string.contains("id=1"));
        assert!(display_string.contains("id=2"));
        assert!(display_string.contains("price=1000"));
        assert!(display_string.contains("price=1100"));
    }

    #[test]
    fn test_from_str() {
        // Create a queue directly for consistency check
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000, 10)));
        queue.push(Arc::new(create_test_order(2, 1100, 20)));

        // Get the display string
        let display_string = queue.to_string();

        // Verify display string format
        assert!(display_string.starts_with("OrderQueue:orders=["));
        assert!(display_string.contains("id=1"));
        assert!(display_string.contains("id=2"));
        assert!(display_string.contains("price=1000"));
        assert!(display_string.contains("price=1100"));

        // Example input string format (manually constructed to match expected format)
        let input = "OrderQueue:orders=[Standard:id=1;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC,Standard:id=2;price=1100;quantity=20;side=BUY;timestamp=1616823000000;time_in_force=GTC]";

        // Try parsing
        let parsed_queue = match OrderQueue::from_str(input) {
            Ok(q) => q,
            Err(e) => {
                println!("Parse error: {:?}", e);
                println!("Input string: {}", input);
                panic!("Failed to parse OrderQueue from string");
            }
        };

        // Verify the parsed queue
        assert!(!parsed_queue.is_empty());
        let orders = parsed_queue.to_vec();

        // Should have both orders
        assert_eq!(orders.len(), 2, "Expected 2 orders in parsed queue");

        // Verify individual orders (order might not be preserved)
        let has_order1 = orders.iter().any(|o| o.id() == OrderId(1) && o.price() == 1000 && o.visible_quantity() == 10);
        let has_order2 = orders.iter().any(|o| o.id() == OrderId(2) && o.price() == 1100 && o.visible_quantity() == 20);

        assert!(has_order1, "First order not found or incorrect");
        assert!(has_order2, "Second order not found or incorrect");

        // Test round-trip parsing
        let round_trip_queue = OrderQueue::from_str(&display_string).unwrap();
        let round_trip_orders = round_trip_queue.to_vec();

        assert_eq!(round_trip_orders.len(), 2, "Round-trip parsing should preserve order count");

        let round_trip_has_order1 = round_trip_orders.iter()
            .any(|o| o.id() == OrderId(1) && o.price() == 1000 && o.visible_quantity() == 10);
        let round_trip_has_order2 = round_trip_orders.iter()
            .any(|o| o.id() == OrderId(2) && o.price() == 1100 && o.visible_quantity() == 20);

        assert!(round_trip_has_order1, "First order not preserved in round-trip");
        assert!(round_trip_has_order2, "Second order not preserved in round-trip");
    }

    #[test]
    fn test_serialize_deserialize() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000, 10)));
        queue.push(Arc::new(create_test_order(2, 1100, 20)));

        // Serialize to JSON
        let serialized = serde_json::to_string(&queue).unwrap();
        println!("Serialized: {}", serialized);

        // Deserialize back
        let deserialized: OrderQueue = serde_json::from_str(&serialized).unwrap();

        // Verify
        let original_orders = queue.to_vec();
        let deserialized_orders = deserialized.to_vec();

        assert_eq!(original_orders.len(), deserialized_orders.len());

        // Since the order of orders might not be preserved, compare individual orders
        for order in original_orders {
            let found = deserialized_orders.iter().any(|o| o.id() == order.id());
            assert!(found, "Order with ID {} not found after deserialization", order.id());
        }
    }

    #[test]
    fn test_round_trip() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000, 10)));

        // Convert to string
        let string_rep = queue.to_string();

        // Parse back from string
        let parsed_queue = match OrderQueue::from_str(&string_rep) {
            Ok(q) => q,
            Err(e) => {
                println!("Parse error: {:?}", e);
                panic!("Failed to parse: {}", string_rep);
            }
        };

        // Verify
        let original_orders = queue.to_vec();
        let parsed_orders = parsed_queue.to_vec();

        assert_eq!(original_orders.len(), parsed_orders.len());
        assert_eq!(original_orders[0].id(), parsed_orders[0].id());
        assert_eq!(original_orders[0].price(), parsed_orders[0].price());
    }
}