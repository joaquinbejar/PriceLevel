#[cfg(test)]
mod tests {
    use crate::orders::{OrderId, OrderType, Side, TimeInForce};
    use crate::price_level::order_queue::OrderQueue;
    use std::str::FromStr;
    use std::sync::Arc;
    use tracing::info;

    fn create_test_order(id: u64, price: u64, quantity: u64) -> OrderType {
        OrderType::Standard {
            id: OrderId::from_u64(id),
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
        info!("Display: {}", display_string);

        assert!(display_string.starts_with("OrderQueue:orders=["));
        assert!(display_string.contains("id=00000000-0000-0001-0000-000000000000"));
        assert!(display_string.contains("id=00000000-0000-0002-0000-000000000000"));
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
        assert!(display_string.contains("id=00000000-0000-0001-0000-000000000000"));
        assert!(display_string.contains("id=00000000-0000-0002-0000-000000000000"));
        assert!(display_string.contains("price=1000"));
        assert!(display_string.contains("price=1100"));

        // Example input string format (manually constructed to match expected format)
        let input = "OrderQueue:orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC,Standard:id=00000000-0000-0002-0000-000000000000;price=1100;quantity=20;side=BUY;timestamp=1616823000000;time_in_force=GTC]";

        // Try parsing
        let parsed_queue = match OrderQueue::from_str(input) {
            Ok(q) => q,
            Err(e) => {
                info!("Parse error: {:?}", e);
                info!("Input string: {}", input);
                panic!("Failed to parse OrderQueue from string");
            }
        };

        // Verify the parsed queue
        assert!(!parsed_queue.is_empty());
        let orders = parsed_queue.to_vec();

        // Should have both orders
        assert_eq!(orders.len(), 2, "Expected 2 orders in parsed queue");

        // Verify individual orders (order might not be preserved)
        let has_order1 = orders.iter().any(|o| {
            o.id() == OrderId::from_u64(1) && o.price() == 1000 && o.visible_quantity() == 10
        });
        let has_order2 = orders.iter().any(|o| {
            o.id() == OrderId::from_u64(2) && o.price() == 1100 && o.visible_quantity() == 20
        });

        assert!(has_order1, "First order not found or incorrect");
        assert!(has_order2, "Second order not found or incorrect");

        // Test round-trip parsing
        let round_trip_queue = OrderQueue::from_str(&display_string).unwrap();
        let round_trip_orders = round_trip_queue.to_vec();

        assert_eq!(
            round_trip_orders.len(),
            2,
            "Round-trip parsing should preserve order count"
        );

        let round_trip_has_order1 = round_trip_orders.iter().any(|o| {
            o.id() == OrderId::from_u64(1) && o.price() == 1000 && o.visible_quantity() == 10
        });
        let round_trip_has_order2 = round_trip_orders.iter().any(|o| {
            o.id() == OrderId::from_u64(2) && o.price() == 1100 && o.visible_quantity() == 20
        });

        assert!(
            round_trip_has_order1,
            "First order not preserved in round-trip"
        );
        assert!(
            round_trip_has_order2,
            "Second order not preserved in round-trip"
        );
    }

    #[test]
    fn test_serialize_deserialize() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000, 10)));
        queue.push(Arc::new(create_test_order(2, 1100, 20)));

        // Serialize to JSON
        let serialized = serde_json::to_string(&queue).unwrap();
        info!("Serialized: {}", serialized);

        // Deserialize back
        let deserialized: OrderQueue = serde_json::from_str(&serialized).unwrap();

        // Verify
        let original_orders = queue.to_vec();
        let deserialized_orders = deserialized.to_vec();

        assert_eq!(original_orders.len(), deserialized_orders.len());

        // Since the order of orders might not be preserved, compare individual orders
        for order in original_orders {
            let found = deserialized_orders.iter().any(|o| o.id() == order.id());
            assert!(
                found,
                "Order with ID {} not found after deserialization",
                order.id()
            );
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
                info!("Parse error: {:?}", e);
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

    // In price_level/order_queue.rs test module or in a separate test file

    #[test]
    fn test_order_queue_to_vec_empty() {
        let queue = OrderQueue::new();

        // test_to_vec on empty queue
        let vec = queue.to_vec();
        assert!(vec.is_empty());

        // Verify queue is still empty after to_vec
        assert!(queue.is_empty());
    }

    #[test]
    fn test_order_queue_from_str_complex() {
        // Test with a complex order string format
        let complex_order = "Standard:id=00000000-0000-0001-0000-000000000000;price=10000;quantity=100;side=BUY;timestamp=1616823000000;time_in_force=GTD-1617000000000";

        let input = format!("OrderQueue:orders=[{}]", complex_order);
        let queue = OrderQueue::from_str(&input).unwrap();

        assert_eq!(queue.len(), 1);

        // Verify the order's details
        let order = &queue.to_vec()[0];

        if let OrderType::Standard {
            id,
            price,
            quantity,
            time_in_force,
            ..
        } = **order
        {
            assert_eq!(id, OrderId::from_u64(1));
            assert_eq!(price, 10000);
            assert_eq!(quantity, 100);
            assert!(matches!(time_in_force, TimeInForce::Gtd(1617000000000)));
        } else {
            panic!("Expected Standard order");
        }
    }

    #[test]
    fn test_order_queue_from_str_invalid_order() {
        // Test with an invalid order format
        let input = "OrderQueue:orders=[InvalidOrder:id=1]";
        let result = OrderQueue::from_str(input);

        assert!(result.is_err());
    }

    #[test]
    fn test_order_queue_serialization() {
        fn create_standard_order(id: u64, price: u64, quantity: u64) -> OrderType {
            OrderType::Standard {
                id: OrderId::from_u64(id),
                price,
                quantity,
                side: Side::Buy,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Gtc,
            }
        }
        let queue = OrderQueue::new();

        // Add an order
        let order = create_standard_order(1, 10000, 100);
        queue.push(Arc::new(order));

        // Serialize
        let serialized = serde_json::to_string(&queue).unwrap();

        // Check that it contains the expected order data
        assert!(serialized.contains("\"Standard\""));
        assert!(serialized.contains("\"id\":\"00000000-0000-0001-0000-000000000000\""));
        assert!(serialized.contains("\"price\":10000"));
        assert!(serialized.contains("\"quantity\":100"));

        // Deserialize and verify
        let deserialized: OrderQueue = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.len(), 1);

        let deserialized_order = &deserialized.to_vec()[0];

        if let OrderType::Standard {
            id,
            price,
            quantity,
            ..
        } = **deserialized_order
        {
            assert_eq!(id, OrderId::from_u64(1));
            assert_eq!(price, 10000);
            assert_eq!(quantity, 100);
        } else {
            panic!("Expected Standard order");
        }
    }

    #[test]
    fn test_order_queue_empty_check() {
        // Test lines 123-124
        let queue = OrderQueue::new();

        // Queue should be empty initially
        assert!(queue.is_empty());

        // Add an order and check again
        let order = OrderType::Standard {
            id: OrderId::from_u64(1),
            price: 1000,
            quantity: 10,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        };
        queue.push(Arc::new(order));

        // Queue should not be empty after adding an order
        assert!(!queue.is_empty());

        // Remove the order and check again
        let _ = queue.pop();
        assert!(queue.is_empty());

        // Push the order back and then try a different approach to check emptiness
        queue.push(Arc::new(order));
        assert!(!queue.is_empty());
    }

    // #[test]
    // fn test_order_queue_from_vec() {
    //     // Test lines 170, 178
    //     // Create a vector of orders
    //     let order1 = Arc::new(OrderType::Standard {
    //         id: OrderId::from_u64(1),
    //         price: 1000,
    //         quantity: 10,
    //         side: Side::Buy,
    //         timestamp: 1616823000000,
    //         time_in_force: TimeInForce::Gtc,
    //     });
    //
    //     let order2 = Arc::new(OrderType::Standard {
    //         id: OrderId::from_u64(2),
    //         price: 1000,
    //         quantity: 20,
    //         side: Side::Buy,
    //         timestamp: 1616823000001,
    //         time_in_force: TimeInForce::Gtc,
    //     });
    //
    //     let orders = vec![order1.clone(), order2.clone()];
    //
    //     // Create a queue from the vector
    //     let queue = OrderQueue::from_vec(orders.clone());
    //
    //     // Verify the queue contains the orders
    //     assert_eq!(queue.to_vec().len(), 2);
    //     assert!(queue.to_vec().contains(&order1));
    //     assert!(queue.to_vec().contains(&order2));
    //
    //     // Test the From implementation
    //     let queue_from_trait: OrderQueue = orders.clone().into();
    //     assert_eq!(queue_from_trait.to_vec().len(), 2);
    //
    //     // Test the Into implementation
    //     let orders_from_queue: Vec<Arc<OrderType>> = queue.into();
    //     assert_eq!(orders_from_queue.len(), 2);
    //     assert!(orders_from_queue.contains(&order1));
    //     assert!(orders_from_queue.contains(&order2));
    // }

    #[test]
    fn test_order_queue_from_str_parsing_with_complex_content() {
        // Test lines 196-198, 200-202, 241, 266-267

        // Create a complex string with nested delimiters
        let complex_input = "OrderQueue:orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC,IcebergOrder:id=00000000-0000-0002-0000-000000000000;price=1000;visible_quantity=5;hidden_quantity=15;side=SELL;timestamp=1616823000001;time_in_force=GTC]";

        // Parse the complex input
        let result = OrderQueue::from_str(complex_input);
        assert!(result.is_ok());

        let queue = result.unwrap();
        assert_eq!(queue.to_vec().len(), 2);

        // Verify the parsed orders have the expected IDs
        let order_ids: Vec<OrderId> = queue.to_vec().iter().map(|order| order.id()).collect();
        assert!(order_ids.contains(&OrderId::from_u64(1)));
        assert!(order_ids.contains(&OrderId::from_u64(2)));

        // Test parsing with empty orders
        let empty_orders = "OrderQueue:orders=[]";
        let result = OrderQueue::from_str(empty_orders);
        assert!(result.is_ok());
        let queue = result.unwrap();
        assert!(queue.is_empty());

        // Test parsing with invalid format (no "OrderQueue:" prefix)
        let invalid_input = "orders=[Standard:id=1;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC]";
        let result = OrderQueue::from_str(invalid_input);
        assert!(result.is_err());

        // Test parsing with malformed content (missing closing bracket)
        let malformed_input = "OrderQueue:orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderQueue::from_str(malformed_input);
        assert!(result.is_err());

        // Test parsing with invalid order type
        let invalid_order = "OrderQueue:orders=[InvalidOrder:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC]";
        let result = OrderQueue::from_str(invalid_order);
        assert!(result.is_err());
    }

    #[test]
    fn test_order_queue_serialization_deserialization() {
        // Create a queue with orders
        let queue = OrderQueue::new();

        let order1 = OrderType::Standard {
            id: OrderId::from_u64(1),
            price: 1000,
            quantity: 10,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        };

        let order2 = OrderType::IcebergOrder {
            id: OrderId::from_u64(2),
            price: 1000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Sell,
            timestamp: 1616823000001,
            time_in_force: TimeInForce::Gtc,
        };

        queue.push(Arc::new(order1));
        queue.push(Arc::new(order2));

        // Serialize the queue
        let serialized = serde_json::to_string(&queue).unwrap();

        // Verify the serialized format contains the orders
        assert!(serialized.contains("\"id\":\"00000000-0000-0001-0000-000000000000\""));
        assert!(serialized.contains("\"id\":\"00000000-0000-0002-0000-000000000000\""));

        // Deserialize back to OrderQueue
        let deserialized: OrderQueue = serde_json::from_str(&serialized).unwrap();

        // Verify the deserialized queue has the same orders
        assert_eq!(deserialized.to_vec().len(), 2);

        // Verify the order IDs
        let order_ids: Vec<OrderId> = deserialized
            .to_vec()
            .iter()
            .map(|order| order.id())
            .collect();
        assert!(order_ids.contains(&OrderId::from_u64(1)));
        assert!(order_ids.contains(&OrderId::from_u64(2)));
    }
}
