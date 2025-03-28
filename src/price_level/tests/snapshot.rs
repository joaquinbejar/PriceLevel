#[cfg(test)]
mod tests {
    use crate::orders::{OrderId, OrderType, Side, TimeInForce};
    use crate::price_level::PriceLevelSnapshot;
    use std::str::FromStr;
    use std::sync::Arc;

    fn create_sample_orders() -> Vec<Arc<OrderType>> {
        vec![
            Arc::new(OrderType::Standard {
                id: OrderId(1),
                price: 1000,
                quantity: 10,
                side: Side::Buy,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Gtc,
            }),
            Arc::new(OrderType::IcebergOrder {
                id: OrderId(2),
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
            assert_eq!(id, OrderId(1));
        } else {
            panic!("Expected StandardOrder");
        }

        // Verify second order
        if let OrderType::IcebergOrder { id, .. } = **collected[1] {
            assert_eq!(id, OrderId(2));
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
