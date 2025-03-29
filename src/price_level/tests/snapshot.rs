#[cfg(test)]
mod tests {
    use crate::orders::{OrderId, OrderType, Side, TimeInForce};
    use crate::price_level::PriceLevelSnapshot;
    use std::str::FromStr;
    use std::sync::Arc;

    fn create_sample_orders() -> Vec<Arc<OrderType>> {
        vec![
            Arc::new(OrderType::Standard {
                id: OrderId::from_u64(1),
                price: 1000,
                quantity: 10,
                side: Side::Buy,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Gtc,
            }),
            Arc::new(OrderType::IcebergOrder {
                id: OrderId::from_u64(2),
                price: 1000,
                visible_quantity: 5,
                hidden_quantity: 15,
                side: Side::Buy,
                timestamp: 1616823000001,
                time_in_force: TimeInForce::Gtc,
            }),
        ]
    }

    #[test]
    fn test_new() {
        let snapshot = PriceLevelSnapshot::new(1000);
        assert_eq!(snapshot.price, 1000);
        assert_eq!(snapshot.visible_quantity, 0);
        assert_eq!(snapshot.hidden_quantity, 0);
        assert_eq!(snapshot.order_count, 0);
        assert!(snapshot.orders.is_empty());
    }

    #[test]
    fn test_default() {
        let snapshot = PriceLevelSnapshot::default();
        assert_eq!(snapshot.price, 0);
        assert_eq!(snapshot.visible_quantity, 0);
        assert_eq!(snapshot.hidden_quantity, 0);
        assert_eq!(snapshot.order_count, 0);
        assert!(snapshot.orders.is_empty());
    }

    #[test]
    fn test_total_quantity() {
        let mut snapshot = PriceLevelSnapshot::new(1000);
        snapshot.visible_quantity = 50;
        snapshot.hidden_quantity = 150;
        assert_eq!(snapshot.total_quantity(), 200);
    }

    #[test]
    fn test_iter_orders() {
        let mut snapshot = PriceLevelSnapshot::new(1000);
        let orders = create_sample_orders();
        snapshot.orders = orders.clone();
        snapshot.order_count = orders.len();

        let collected: Vec<_> = snapshot.iter_orders().collect();
        assert_eq!(collected.len(), 2);

        // Verify first order
        if let OrderType::Standard { id, .. } = **collected[0] {
            assert_eq!(id, OrderId::from_u64(1));
        } else {
            panic!("Expected StandardOrder");
        }

        // Verify second order
        if let OrderType::IcebergOrder { id, .. } = **collected[1] {
            assert_eq!(id, OrderId::from_u64(2));
        } else {
            panic!("Expected IcebergOrder");
        }
    }

    #[test]
    fn test_clone() {
        let mut original = PriceLevelSnapshot::new(1000);
        original.visible_quantity = 50;
        original.hidden_quantity = 150;
        original.order_count = 2;
        original.orders = create_sample_orders();

        let cloned = original.clone();
        assert_eq!(cloned.price, 1000);
        assert_eq!(cloned.visible_quantity, 50);
        assert_eq!(cloned.hidden_quantity, 150);
        assert_eq!(cloned.order_count, 2);
        assert_eq!(cloned.orders.len(), 2);
    }

    #[test]
    fn test_display() {
        let mut snapshot = PriceLevelSnapshot::new(1000);
        snapshot.visible_quantity = 50;
        snapshot.hidden_quantity = 150;
        snapshot.order_count = 2;

        let display_str = snapshot.to_string();
        assert!(display_str.contains("price=1000"));
        assert!(display_str.contains("visible_quantity=50"));
        assert!(display_str.contains("hidden_quantity=150"));
        assert!(display_str.contains("order_count=2"));
    }

    #[test]
    fn test_from_str() {
        let input =
            "PriceLevelSnapshot:price=1000;visible_quantity=50;hidden_quantity=150;order_count=2";
        let snapshot = PriceLevelSnapshot::from_str(input).unwrap();

        assert_eq!(snapshot.price, 1000);
        assert_eq!(snapshot.visible_quantity, 50);
        assert_eq!(snapshot.hidden_quantity, 150);
        assert_eq!(snapshot.order_count, 2);
        assert!(snapshot.orders.is_empty()); // Orders can't be parsed from string representation
    }

    #[test]
    fn test_from_str_invalid_format() {
        let input = "InvalidFormat";
        let result = PriceLevelSnapshot::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_missing_field() {
        let input = "PriceLevelSnapshot:price=1000;visible_quantity=50;hidden_quantity=150";
        let result = PriceLevelSnapshot::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_invalid_field_value() {
        let input = "PriceLevelSnapshot:price=invalid;visible_quantity=50;hidden_quantity=150;order_count=2";
        let result = PriceLevelSnapshot::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_display_fromstr() {
        let mut original = PriceLevelSnapshot::new(1000);
        original.visible_quantity = 50;
        original.hidden_quantity = 150;
        original.order_count = 2;

        let string_representation = original.to_string();
        let parsed = PriceLevelSnapshot::from_str(&string_representation).unwrap();

        assert_eq!(parsed.price, original.price);
        assert_eq!(parsed.visible_quantity, original.visible_quantity);
        assert_eq!(parsed.hidden_quantity, original.hidden_quantity);
        assert_eq!(parsed.order_count, original.order_count);
    }
}

#[cfg(test)]
mod pricelevel_snapshot_serialization_tests {
    use crate::orders::{OrderId, OrderType, Side, TimeInForce};
    use crate::price_level::PriceLevelSnapshot;

    use std::str::FromStr;
    use std::sync::Arc;

    // Helper function to create sample orders for testing
    fn create_sample_orders() -> Vec<Arc<OrderType>> {
        vec![
            Arc::new(OrderType::Standard {
                id: OrderId::from_u64(1),
                price: 1000,
                quantity: 10,
                side: Side::Buy,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Gtc,
            }),
            Arc::new(OrderType::IcebergOrder {
                id: OrderId::from_u64(2),
                price: 1000,
                visible_quantity: 5,
                hidden_quantity: 15,
                side: Side::Sell,
                timestamp: 1616823000001,
                time_in_force: TimeInForce::Gtc,
            }),
            Arc::new(OrderType::PostOnly {
                id: OrderId::from_u64(3),
                price: 1000,
                quantity: 8,
                side: Side::Buy,
                timestamp: 1616823000002,
                time_in_force: TimeInForce::Ioc,
            }),
        ]
    }

    // Helper function to create a sample snapshot for testing
    fn create_sample_snapshot() -> PriceLevelSnapshot {
        let mut snapshot = PriceLevelSnapshot::new(1000);
        snapshot.visible_quantity = 15; // 10 + 5 (first two orders)
        snapshot.hidden_quantity = 15; // hidden quantity from iceberg order
        snapshot.order_count = 3;
        snapshot.orders = create_sample_orders();
        snapshot
    }

    #[test]
    fn test_snapshot_json_serialization() {
        let snapshot = create_sample_snapshot();

        // Serialize to JSON
        let json = serde_json::to_string(&snapshot)
            .expect("Failed to serialize PriceLevelSnapshot to JSON");

        // Verify basic JSON properties
        assert!(json.contains("\"price\":1000"));
        assert!(json.contains("\"visible_quantity\":15"));
        assert!(json.contains("\"hidden_quantity\":15"));
        assert!(json.contains("\"order_count\":3"));

        // Verify orders array
        assert!(json.contains("\"orders\":["));

        // Check for order details
        assert!(json.contains("\"Standard\":{"));
        assert!(json.contains("\"id\":\"00000000-0000-0001-0000-000000000000\""));
        assert!(json.contains("\"IcebergOrder\":{"));
        assert!(json.contains("\"visible_quantity\":5"));
        assert!(json.contains("\"hidden_quantity\":15"));
        assert!(json.contains("\"PostOnly\":{"));
    }

    #[test]
    fn test_snapshot_json_deserialization() {
        let snapshot = create_sample_snapshot();

        // Serialize to JSON
        let json =
            serde_json::to_string(&snapshot).expect("Failed to serialize PriceLevelSnapshot");

        // Deserialize back to PriceLevelSnapshot
        let deserialized: PriceLevelSnapshot = serde_json::from_str(&json)
            .expect("Failed to deserialize PriceLevelSnapshot from JSON");

        // Verify basic fields
        assert_eq!(deserialized.price, 1000);
        assert_eq!(deserialized.visible_quantity, 15);
        assert_eq!(deserialized.hidden_quantity, 15);
        assert_eq!(deserialized.order_count, 3);

        // Verify orders array length
        assert_eq!(deserialized.orders.len(), 3);

        // Check specific order details
        let standard_order = &deserialized.orders[0];
        match **standard_order {
            OrderType::Standard {
                id,
                price,
                quantity,
                side,
                ..
            } => {
                assert_eq!(id, OrderId::from_u64(1));
                assert_eq!(price, 1000);
                assert_eq!(quantity, 10);
                assert_eq!(side, Side::Buy);
            }
            _ => panic!("Expected Standard order"),
        }

        let iceberg_order = &deserialized.orders[1];
        match **iceberg_order {
            OrderType::IcebergOrder {
                id,
                visible_quantity,
                hidden_quantity,
                side,
                ..
            } => {
                assert_eq!(id, OrderId::from_u64(2));
                assert_eq!(visible_quantity, 5);
                assert_eq!(hidden_quantity, 15);
                assert_eq!(side, Side::Sell);
            }
            _ => panic!("Expected IcebergOrder"),
        }

        let post_only_order = &deserialized.orders[2];
        match **post_only_order {
            OrderType::PostOnly {
                id, quantity, side, ..
            } => {
                assert_eq!(id, OrderId::from_u64(3));
                assert_eq!(quantity, 8);
                assert_eq!(side, Side::Buy);
            }
            _ => panic!("Expected PostOnly order"),
        }
    }

    #[test]
    fn test_snapshot_string_format_serialization() {
        let snapshot = create_sample_snapshot();

        // Convert to string representation
        let display_str = snapshot.to_string();

        // Verify string format
        assert!(display_str.starts_with("PriceLevelSnapshot:"));
        assert!(display_str.contains("price=1000"));
        assert!(display_str.contains("visible_quantity=15"));
        assert!(display_str.contains("hidden_quantity=15"));
        assert!(display_str.contains("order_count=3"));

        // Note: The string format doesn't include orders as shown in the FromStr implementation
    }

    #[test]
    fn test_snapshot_string_format_deserialization() {
        // Create string representation
        let input =
            "PriceLevelSnapshot:price=1000;visible_quantity=15;hidden_quantity=15;order_count=3";

        // Parse from string
        let snapshot =
            PriceLevelSnapshot::from_str(input).expect("Failed to parse PriceLevelSnapshot");

        // Verify basic fields
        assert_eq!(snapshot.price, 1000);
        assert_eq!(snapshot.visible_quantity, 15);
        assert_eq!(snapshot.hidden_quantity, 15);
        assert_eq!(snapshot.order_count, 3);

        // Orders array should be empty when deserialized from string format (per FromStr implementation)
        assert!(snapshot.orders.is_empty());
    }

    #[test]
    fn test_snapshot_string_format_invalid_inputs() {
        // Test missing price field
        let input = "PriceLevelSnapshot:visible_quantity=15;hidden_quantity=15;order_count=3";
        let result = PriceLevelSnapshot::from_str(input);
        assert!(result.is_err());

        // Test invalid prefix
        let input = "InvalidPrefix:price=1000;visible_quantity=15;hidden_quantity=15;order_count=3";
        let result = PriceLevelSnapshot::from_str(input);
        assert!(result.is_err());

        // Test invalid field value
        let input =
            "PriceLevelSnapshot:price=invalid;visible_quantity=15;hidden_quantity=15;order_count=3";
        let result = PriceLevelSnapshot::from_str(input);
        assert!(result.is_err());

        // Test missing field separator
        let input =
            "PriceLevelSnapshot:price=1000visible_quantity=15;hidden_quantity=15;order_count=3";
        let result = PriceLevelSnapshot::from_str(input);
        assert!(result.is_err());

        // Test with unknown field
        let input = "PriceLevelSnapshot:price=1000;visible_quantity=15;hidden_quantity=15;order_count=3;unknown_field=value";
        let result = PriceLevelSnapshot::from_str(input);
        // This should still succeed as FromStr implementation doesn't validate for unknown fields
        assert!(result.is_ok());
    }

    #[test]
    fn test_snapshot_string_format_roundtrip() {
        // Create a snapshot with only basic fields (no orders)
        let mut original = PriceLevelSnapshot::new(1000);
        original.visible_quantity = 15;
        original.hidden_quantity = 15;
        original.order_count = 3;

        // Convert to string
        let string_representation = original.to_string();

        // Parse back to snapshot
        let parsed = PriceLevelSnapshot::from_str(&string_representation)
            .expect("Failed to parse PriceLevelSnapshot");

        // Verify all fields match
        assert_eq!(parsed.price, original.price);
        assert_eq!(parsed.visible_quantity, original.visible_quantity);
        assert_eq!(parsed.hidden_quantity, original.hidden_quantity);
        assert_eq!(parsed.order_count, original.order_count);
    }

    #[test]
    fn test_snapshot_edge_cases() {
        // Test with zero values
        let mut snapshot = PriceLevelSnapshot::new(0);
        snapshot.visible_quantity = 0;
        snapshot.hidden_quantity = 0;
        snapshot.order_count = 0;

        let json = serde_json::to_string(&snapshot).expect("Failed to serialize");
        let deserialized: PriceLevelSnapshot =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.price, 0);
        assert_eq!(deserialized.visible_quantity, 0);
        assert_eq!(deserialized.hidden_quantity, 0);
        assert_eq!(deserialized.order_count, 0);

        // Test with maximum values
        let mut snapshot = PriceLevelSnapshot::new(u64::MAX);
        snapshot.visible_quantity = u64::MAX;
        snapshot.hidden_quantity = u64::MAX;
        snapshot.order_count = usize::MAX;

        let json = serde_json::to_string(&snapshot).expect("Failed to serialize max values");
        let deserialized: PriceLevelSnapshot =
            serde_json::from_str(&json).expect("Failed to deserialize max values");

        assert_eq!(deserialized.price, u64::MAX);
        assert_eq!(deserialized.visible_quantity, u64::MAX);
        assert_eq!(deserialized.hidden_quantity, u64::MAX);
        assert_eq!(deserialized.order_count, usize::MAX);
    }

    #[test]
    fn test_snapshot_deserialization_unknown_field() {
        // Create JSON with an unknown field "unknown_field"
        let json = r#"{
            "price": 1000,
            "visible_quantity": 15,
            "hidden_quantity": 15,
            "order_count": 3,
            "orders": [],
            "unknown_field": "some value"
        }"#;

        // Attempt to deserialize - this should fail because of the unknown field
        let result = serde_json::from_str::<PriceLevelSnapshot>(json);

        // Verify that the error is of the expected type
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_string = err.to_string();

        // Verify the error message mentions the unknown field
        assert!(err_string.contains("unknown field"));
        assert!(err_string.contains("unknown_field"));

        // Verify the error message mentions the expected fields
        assert!(err_string.contains("price"));
        assert!(err_string.contains("visible_quantity"));
        assert!(err_string.contains("hidden_quantity"));
        assert!(err_string.contains("order_count"));
        assert!(err_string.contains("orders"));
    }

    #[test]
    fn test_snapshot_empty_orders() {
        // Test with an empty orders array
        let mut snapshot = PriceLevelSnapshot::new(1000);
        snapshot.visible_quantity = 15;
        snapshot.hidden_quantity = 15;
        snapshot.order_count = 0;
        snapshot.orders = Vec::new();

        let json = serde_json::to_string(&snapshot).expect("Failed to serialize");
        let deserialized: PriceLevelSnapshot =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.price, 1000);
        assert_eq!(deserialized.orders.len(), 0);
    }

    #[test]
    fn test_snapshot_with_many_order_types() {
        // Create a snapshot with all supported order types
        let mut snapshot = PriceLevelSnapshot::new(1000);

        // Add sample orders of different types
        snapshot.orders = vec![
            // Standard order
            Arc::new(OrderType::Standard {
                id: OrderId::from_u64(1),
                price: 1000,
                quantity: 10,
                side: Side::Buy,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Gtc,
            }),
            // Iceberg order
            Arc::new(OrderType::IcebergOrder {
                id: OrderId::from_u64(2),
                price: 1000,
                visible_quantity: 5,
                hidden_quantity: 15,
                side: Side::Sell,
                timestamp: 1616823000001,
                time_in_force: TimeInForce::Gtc,
            }),
            // Post-only order
            Arc::new(OrderType::PostOnly {
                id: OrderId::from_u64(3),
                price: 1000,
                quantity: 8,
                side: Side::Buy,
                timestamp: 1616823000002,
                time_in_force: TimeInForce::Ioc,
            }),
            // Fill-or-kill order (as Standard with FOK time-in-force)
            Arc::new(OrderType::Standard {
                id: OrderId::from_u64(4),
                price: 1000,
                quantity: 12,
                side: Side::Buy,
                timestamp: 1616823000003,
                time_in_force: TimeInForce::Fok,
            }),
            // Good-till-date order (as Standard with GTD time-in-force)
            Arc::new(OrderType::Standard {
                id: OrderId::from_u64(5),
                price: 1000,
                quantity: 7,
                side: Side::Sell,
                timestamp: 1616823000004,
                time_in_force: TimeInForce::Gtd(1617000000000),
            }),
            // Reserve order
            Arc::new(OrderType::ReserveOrder {
                id: OrderId::from_u64(6),
                price: 1000,
                visible_quantity: 3,
                hidden_quantity: 12,
                side: Side::Buy,
                timestamp: 1616823000005,
                time_in_force: TimeInForce::Gtc,
                replenish_threshold: 1,
                replenish_amount: Some(2),
                auto_replenish: true,
            }),
        ];

        snapshot.order_count = snapshot.orders.len();
        snapshot.visible_quantity = 45; // Sum of all visible quantities
        snapshot.hidden_quantity = 27; // Sum of all hidden quantities

        // Serialize to JSON
        let json = serde_json::to_string(&snapshot).expect("Failed to serialize complex snapshot");

        // Deserialize back
        let deserialized: PriceLevelSnapshot =
            serde_json::from_str(&json).expect("Failed to deserialize complex snapshot");

        // Verify basic fields
        assert_eq!(deserialized.price, 1000);
        assert_eq!(deserialized.visible_quantity, 45);
        assert_eq!(deserialized.hidden_quantity, 27);
        assert_eq!(deserialized.order_count, 6);
        assert_eq!(deserialized.orders.len(), 6);

        // Verify specific order types were preserved
        let order_types = deserialized
            .orders
            .iter()
            .map(|order| match **order {
                OrderType::Standard { .. } => "Standard",
                OrderType::IcebergOrder { .. } => "IcebergOrder",
                OrderType::PostOnly { .. } => "PostOnly",
                OrderType::ReserveOrder { .. } => "ReserveOrder",
                _ => "Other",
            })
            .collect::<Vec<_>>();

        // Count the occurrences of each order type
        let standard_count = order_types.iter().filter(|&&t| t == "Standard").count();
        let iceberg_count = order_types.iter().filter(|&&t| t == "IcebergOrder").count();
        let post_only_count = order_types.iter().filter(|&&t| t == "PostOnly").count();
        let reserve_count = order_types.iter().filter(|&&t| t == "ReserveOrder").count();

        // Verify we have the expected number of each order type
        assert_eq!(standard_count, 3); // 1 standard + 1 FOK + 1 GTD
        assert_eq!(iceberg_count, 1);
        assert_eq!(post_only_count, 1);
        assert_eq!(reserve_count, 1);

        // Check a few specific properties to ensure proper deserialization
        let reserve_order = deserialized
            .orders
            .iter()
            .find(|order| matches!(***order, OrderType::ReserveOrder { .. }))
            .expect("Reserve order not found");

        if let OrderType::ReserveOrder {
            replenish_threshold,
            auto_replenish,
            ..
        } = **reserve_order
        {
            assert_eq!(replenish_threshold, 1);
            assert!(auto_replenish);
        }

        let gtd_order = deserialized
            .orders
            .iter()
            .find(|order| {
                matches!(
                    ***order,
                    OrderType::Standard {
                        time_in_force: TimeInForce::Gtd(_),
                        ..
                    }
                )
            })
            .expect("GTD order not found");

        if let OrderType::Standard {
            time_in_force: TimeInForce::Gtd(expiry),
            ..
        } = **gtd_order
        {
            assert_eq!(expiry, 1617000000000);
        }
    }
}
