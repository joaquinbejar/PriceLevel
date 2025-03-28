#[cfg(test)]
mod tests {
    use crate::orders::time_in_force::TimeInForce;
    use crate::orders::{OrderId, OrderType, PegReferenceType, Side};
    use std::str::FromStr;

    fn create_standard_order() -> OrderType {
        OrderType::Standard {
            id: OrderId(123),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        }
    }

    // Helper function to create an iceberg order for testing
    fn create_iceberg_order() -> OrderType {
        OrderType::IcebergOrder {
            id: OrderId(124),
            price: 10000,
            visible_quantity: 1,
            hidden_quantity: 4,
            side: Side::Sell,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        }
    }

    // Helper function to create a post-only order for testing
    fn create_post_only_order() -> OrderType {
        OrderType::PostOnly {
            id: OrderId(125),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        }
    }

    // Helper function to create a trailing stop order for testing
    fn create_trailing_stop_order() -> OrderType {
        OrderType::TrailingStop {
            id: OrderId(126),
            price: 10000,
            quantity: 5,
            side: Side::Sell,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
            trail_amount: 100,
            last_reference_price: 10100,
        }
    }

    // Helper function to create a pegged order for testing
    fn create_pegged_order() -> OrderType {
        OrderType::PeggedOrder {
            id: OrderId(127),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
            reference_price_offset: -50,
            reference_price_type: PegReferenceType::BestAsk,
        }
    }

    // Helper function to create a market-to-limit order for testing
    fn create_market_to_limit_order() -> OrderType {
        OrderType::MarketToLimit {
            id: OrderId(128),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        }
    }

    // Helper function to create a reserve order for testing
    fn create_reserve_order() -> OrderType {
        OrderType::ReserveOrder {
            id: OrderId(129),
            price: 10000,
            visible_quantity: 1,
            hidden_quantity: 4,
            side: Side::Sell,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: 0,
        }
    }

    #[test]
    fn test_order_id() {
        assert_eq!(create_standard_order().id(), OrderId(123));
        assert_eq!(create_iceberg_order().id(), OrderId(124));
        assert_eq!(create_post_only_order().id(), OrderId(125));
        assert_eq!(create_trailing_stop_order().id(), OrderId(126));
        assert_eq!(create_pegged_order().id(), OrderId(127));
        assert_eq!(create_market_to_limit_order().id(), OrderId(128));
        assert_eq!(create_reserve_order().id(), OrderId(129));
    }

    #[test]
    fn test_order_price() {
        assert_eq!(create_standard_order().price(), 10000);
        assert_eq!(create_iceberg_order().price(), 10000);
        assert_eq!(create_post_only_order().price(), 10000);
        assert_eq!(create_trailing_stop_order().price(), 10000);
        assert_eq!(create_pegged_order().price(), 10000);
        assert_eq!(create_market_to_limit_order().price(), 10000);
        assert_eq!(create_reserve_order().price(), 10000);
    }

    #[test]
    fn test_visible_quantity() {
        assert_eq!(create_standard_order().visible_quantity(), 5);
        assert_eq!(create_iceberg_order().visible_quantity(), 1);
        assert_eq!(create_post_only_order().visible_quantity(), 5);
        assert_eq!(create_trailing_stop_order().visible_quantity(), 5);
        assert_eq!(create_pegged_order().visible_quantity(), 5);
        assert_eq!(create_market_to_limit_order().visible_quantity(), 5);
        assert_eq!(create_reserve_order().visible_quantity(), 1);
    }

    #[test]
    fn test_hidden_quantity() {
        assert_eq!(create_standard_order().hidden_quantity(), 0);
        assert_eq!(create_iceberg_order().hidden_quantity(), 4);
        assert_eq!(create_post_only_order().hidden_quantity(), 0);
        assert_eq!(create_trailing_stop_order().hidden_quantity(), 0);
        assert_eq!(create_pegged_order().hidden_quantity(), 0);
        assert_eq!(create_market_to_limit_order().hidden_quantity(), 0);
        assert_eq!(create_reserve_order().hidden_quantity(), 4);
    }

    #[test]
    fn test_order_side() {
        assert_eq!(create_standard_order().side(), Side::Buy);
        assert_eq!(create_iceberg_order().side(), Side::Sell);
        assert_eq!(create_post_only_order().side(), Side::Buy);
        assert_eq!(create_trailing_stop_order().side(), Side::Sell);
        assert_eq!(create_pegged_order().side(), Side::Buy);
        assert_eq!(create_market_to_limit_order().side(), Side::Buy);
        assert_eq!(create_reserve_order().side(), Side::Sell);
    }

    #[test]
    fn test_time_in_force() {
        assert_eq!(create_standard_order().time_in_force(), TimeInForce::Gtc);
        assert_eq!(create_iceberg_order().time_in_force(), TimeInForce::Gtc);
        assert_eq!(create_post_only_order().time_in_force(), TimeInForce::Gtc);
        assert_eq!(
            create_trailing_stop_order().time_in_force(),
            TimeInForce::Gtc
        );
        assert_eq!(create_pegged_order().time_in_force(), TimeInForce::Gtc);
        assert_eq!(
            create_market_to_limit_order().time_in_force(),
            TimeInForce::Gtc
        );
        assert_eq!(create_reserve_order().time_in_force(), TimeInForce::Gtc);
    }

    #[test]
    fn test_timestamp() {
        assert_eq!(create_standard_order().timestamp(), 1616823000000);
        assert_eq!(create_iceberg_order().timestamp(), 1616823000000);
        assert_eq!(create_post_only_order().timestamp(), 1616823000000);
        assert_eq!(create_trailing_stop_order().timestamp(), 1616823000000);
        assert_eq!(create_pegged_order().timestamp(), 1616823000000);
        assert_eq!(create_market_to_limit_order().timestamp(), 1616823000000);
        assert_eq!(create_reserve_order().timestamp(), 1616823000000);
    }

    #[test]
    fn test_is_immediate() {
        let mut order = create_standard_order();
        assert!(!order.is_immediate());

        // Test with IOC time-in-force
        if let OrderType::Standard { time_in_force, .. } = &mut order {
            *time_in_force = TimeInForce::Ioc;
        }
        assert!(order.is_immediate());
    }

    #[test]
    fn test_is_fill_or_kill() {
        let mut order = create_standard_order();
        assert!(!order.is_fill_or_kill());

        // Test with FOK time-in-force
        if let OrderType::Standard { time_in_force, .. } = &mut order {
            *time_in_force = TimeInForce::Fok;
        }
        assert!(order.is_fill_or_kill());
    }

    #[test]
    fn test_is_post_only() {
        assert!(!create_standard_order().is_post_only());
        assert!(!create_iceberg_order().is_post_only());
        assert!(create_post_only_order().is_post_only());
        assert!(!create_trailing_stop_order().is_post_only());
        assert!(!create_pegged_order().is_post_only());
        assert!(!create_market_to_limit_order().is_post_only());
        assert!(!create_reserve_order().is_post_only());
    }

    #[test]
    fn test_with_reduced_quantity() {
        // Test standard order
        let order = create_standard_order();
        let reduced = order.with_reduced_quantity(3);

        if let OrderType::Standard { quantity, .. } = reduced {
            assert_eq!(quantity, 3);
        } else {
            panic!("Expected StandardOrder");
        }

        // Test iceberg order
        let order = create_iceberg_order();
        let reduced = order.with_reduced_quantity(0);

        if let OrderType::IcebergOrder {
            visible_quantity,
            hidden_quantity,
            ..
        } = reduced
        {
            assert_eq!(visible_quantity, 0);
            assert_eq!(hidden_quantity, 4); // Hidden quantity should remain unchanged
        } else {
            panic!("Expected IcebergOrder");
        }
    }

    #[test]
    fn test_refresh_iceberg() {
        // Test iceberg order refresh
        let order = create_iceberg_order();
        let (refreshed, used) = order.refresh_iceberg(2);

        if let OrderType::IcebergOrder {
            visible_quantity,
            hidden_quantity,
            ..
        } = refreshed
        {
            assert_eq!(visible_quantity, 2);
            assert_eq!(hidden_quantity, 2); // 4 - 2 = 2
            assert_eq!(used, 2);
        } else {
            panic!("Expected IcebergOrder");
        }

        // Test reserve order refresh
        let order = create_reserve_order();
        let (refreshed, used) = order.refresh_iceberg(3);

        if let OrderType::ReserveOrder {
            visible_quantity,
            hidden_quantity,
            ..
        } = refreshed
        {
            assert_eq!(visible_quantity, 3);
            assert_eq!(hidden_quantity, 1); // 4 - 3 = 1
            assert_eq!(used, 3);
        } else {
            panic!("Expected ReserveOrder");
        }

        // Test non-iceberg order (should not refresh)
        let order = create_standard_order();
        let (refreshed, used) = order.refresh_iceberg(2);

        if let OrderType::Standard { quantity, .. } = refreshed {
            assert_eq!(quantity, 5); // Should remain unchanged
            assert_eq!(used, 0);
        } else {
            panic!("Expected StandardOrder");
        }
    }

    #[test]
    fn test_from_str_standard() {
        let order_str = "Standard:id=123;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let order = OrderType::from_str(order_str).unwrap();

        if let OrderType::Standard {
            id,
            price,
            quantity,
            side,
            timestamp,
            time_in_force,
        } = order
        {
            assert_eq!(id, OrderId(123));
            assert_eq!(price, 10000);
            assert_eq!(quantity, 5);
            assert_eq!(side, Side::Buy);
            assert_eq!(timestamp, 1616823000000);
            assert_eq!(time_in_force, TimeInForce::Gtc);
        } else {
            panic!("Expected StandardOrder");
        }
    }

    #[test]
    fn test_from_str_iceberg() {
        let order_str = "IcebergOrder:id=124;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC";
        let order = OrderType::from_str(order_str).unwrap();

        if let OrderType::IcebergOrder {
            id,
            price,
            visible_quantity,
            hidden_quantity,
            side,
            timestamp,
            time_in_force,
        } = order
        {
            assert_eq!(id, OrderId(124));
            assert_eq!(price, 10000);
            assert_eq!(visible_quantity, 1);
            assert_eq!(hidden_quantity, 4);
            assert_eq!(side, Side::Sell);
            assert_eq!(timestamp, 1616823000000);
            assert_eq!(time_in_force, TimeInForce::Gtc);
        } else {
            panic!("Expected IcebergOrder");
        }
    }

    #[test]
    fn test_from_str_trailing_stop() {
        let order_str = "TrailingStop:id=126;price=10000;quantity=5;side=SELL;timestamp=1616823000000;time_in_force=GTC;trail_amount=100;last_reference_price=10100";
        let order = OrderType::from_str(order_str).unwrap();

        if let OrderType::TrailingStop {
            id,
            price,
            quantity,
            side,
            timestamp,
            time_in_force,
            trail_amount,
            last_reference_price,
        } = order
        {
            assert_eq!(id, OrderId(126));
            assert_eq!(price, 10000);
            assert_eq!(quantity, 5);
            assert_eq!(side, Side::Sell);
            assert_eq!(timestamp, 1616823000000);
            assert_eq!(time_in_force, TimeInForce::Gtc);
            assert_eq!(trail_amount, 100);
            assert_eq!(last_reference_price, 10100);
        } else {
            panic!("Expected TrailingStop");
        }
    }

    #[test]
    fn test_from_str_pegged() {
        let order_str = "PeggedOrder:id=127;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC;reference_price_offset=-50;reference_price_type=BestAsk";
        let order = OrderType::from_str(order_str).unwrap();

        if let OrderType::PeggedOrder {
            id,
            price,
            quantity,
            side,
            timestamp,
            time_in_force,
            reference_price_offset,
            reference_price_type,
        } = order
        {
            assert_eq!(id, OrderId(127));
            assert_eq!(price, 10000);
            assert_eq!(quantity, 5);
            assert_eq!(side, Side::Buy);
            assert_eq!(timestamp, 1616823000000);
            assert_eq!(time_in_force, TimeInForce::Gtc);
            assert_eq!(reference_price_offset, -50);
            assert_eq!(reference_price_type, PegReferenceType::BestAsk);
        } else {
            panic!("Expected PeggedOrder");
        }
    }

    #[test]
    fn test_from_str_different_time_in_force() {
        // Test IOC time-in-force
        let order_str = "Standard:id=123;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=IOC";
        let order = OrderType::from_str(order_str).unwrap();

        if let OrderType::Standard { time_in_force, .. } = order {
            assert_eq!(time_in_force, TimeInForce::Ioc);
        } else {
            panic!("Expected StandardOrder");
        }

        // Test GTD time-in-force
        let order_str = "Standard:id=123;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTD-1616909400000";
        let order = OrderType::from_str(order_str).unwrap();

        if let OrderType::Standard { time_in_force, .. } = order {
            assert_eq!(time_in_force, TimeInForce::Gtd(1616909400000));
        } else {
            panic!("Expected StandardOrder");
        }
    }

    #[test]
    fn test_from_str_errors() {
        // Test invalid format
        let order_str = "Standard;id=123;price=10000";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());

        // Test unknown order type
        let order_str = "Unknown:id=123;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());

        // Test missing field
        let order_str =
            "Standard:id=123;price=10000;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());

        // Test invalid field value
        let order_str = "Standard:id=123;price=invalid;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());
    }
}
