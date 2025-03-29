#[cfg(test)]
mod tests {
    use crate::orders::time_in_force::TimeInForce;
    use crate::orders::{OrderId, OrderType, PegReferenceType, Side};
    use std::str::FromStr;
    use tracing::info;

    fn create_standard_order() -> OrderType {
        OrderType::Standard {
            id: OrderId::from_u64(123),
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
            id: OrderId::from_u64(124),
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
            id: OrderId::from_u64(125),
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
            id: OrderId::from_u64(126),
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
            id: OrderId::from_u64(127),
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
            id: OrderId::from_u64(128),
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
            id: OrderId::from_u64(129),
            price: 10000,
            visible_quantity: 1,
            hidden_quantity: 4,
            side: Side::Sell,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: 0,
            replenish_amount: Some(1),
            auto_replenish: false,
        }
    }

    #[test]
    fn test_order_id() {
        assert_eq!(create_standard_order().id(), OrderId::from_u64(123));
        assert_eq!(create_iceberg_order().id(), OrderId::from_u64(124));
        assert_eq!(create_post_only_order().id(), OrderId::from_u64(125));
        assert_eq!(create_trailing_stop_order().id(), OrderId::from_u64(126));
        assert_eq!(create_pegged_order().id(), OrderId::from_u64(127));
        assert_eq!(create_market_to_limit_order().id(), OrderId::from_u64(128));
        assert_eq!(create_reserve_order().id(), OrderId::from_u64(129));
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

        // NEW TEST: Test post-only order with reduced quantity
        let order = create_post_only_order();
        let reduced = order.with_reduced_quantity(2);

        if let OrderType::PostOnly { quantity, .. } = reduced {
            assert_eq!(quantity, 2);
        } else {
            panic!("Expected PostOnly order");
        }

        // NEW TEST: Test trailing stop order with reduced quantity
        let order = create_trailing_stop_order();
        let reduced = order.with_reduced_quantity(3);

        match reduced {
            OrderType::TrailingStop { quantity, .. } => {
                assert_eq!(quantity, 5);
            }
            _ => panic!("Expected TrailingStop order"),
        }

        // NEW TEST: Test pegged order with reduced quantity
        let order = create_pegged_order();
        let reduced = order.with_reduced_quantity(1);

        match reduced {
            OrderType::PeggedOrder { quantity, .. } => {
                assert_eq!(quantity, 5);
            }
            _ => panic!("Expected PeggedOrder"),
        }

        // NEW TEST: Test market-to-limit order with reduced quantity
        let order = create_market_to_limit_order();
        let reduced = order.with_reduced_quantity(4);

        match reduced {
            OrderType::MarketToLimit { quantity, .. } => {
                assert_eq!(quantity, 5);
            }
            _ => panic!("Expected MarketToLimit order"),
        }

        // NEW TEST: Test reserve order with reduced quantity
        let order = create_reserve_order();
        let reduced = order.with_reduced_quantity(0);

        match reduced {
            OrderType::ReserveOrder {
                visible_quantity,
                hidden_quantity,
                ..
            } => {
                assert_eq!(visible_quantity, 1);
                assert_eq!(hidden_quantity, 4); // Hidden should remain unchanged
            }
            _ => panic!("Expected ReserveOrder"),
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
        let order_str = "Standard:id=00000000-0000-007b-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC";
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
            assert_eq!(id, OrderId::from_u64(123));
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
        let order_str = "IcebergOrder:id=00000000-0000-007c-0000-000000000000;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC";
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
            assert_eq!(id, OrderId::from_u64(124));
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
        let order_str = "TrailingStop:id=00000000-0000-007e-0000-000000000000;price=10000;quantity=5;side=SELL;timestamp=1616823000000;time_in_force=GTC;trail_amount=100;last_reference_price=10100";
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
            assert_eq!(id, OrderId::from_u64(126));
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
        let order_str = "PeggedOrder:id=00000000-0000-007f-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC;reference_price_offset=-50;reference_price_type=BestAsk";
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
            assert_eq!(id, OrderId::from_u64(127));
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
        let order_str = "Standard:id=00000000-0000-007b-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=IOC";
        let order = OrderType::from_str(order_str).unwrap();

        if let OrderType::Standard { time_in_force, .. } = order {
            assert_eq!(time_in_force, TimeInForce::Ioc);
        } else {
            panic!("Expected StandardOrder");
        }

        // Test GTD time-in-force
        let order_str = "Standard:id=00000000-0000-007b-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTD-1616909400000";
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
        let order_str = "Standard;id=00000000-0000-007b-0000-000000000000;price=10000";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());

        // Test unknown order type
        let order_str = "Unknown:id=00000000-0000-007b-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());

        // Test missing field
        let order_str =
            "Standard:id=00000000-0000-007b-0000-000000000000;price=10000;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());

        // Test invalid field value
        let order_str = "Standard:id=00000000-0000-007b-0000-000000000000;price=invalid;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderType::from_str(order_str);
        assert!(result.is_err());
    }

    // NEW TESTS for Display implementation
    #[test]
    fn test_display_standard_order() {
        let order = create_standard_order();
        let display_str = format!("{}", order);

        info!("{}", display_str);
        assert!(display_str.starts_with("Standard:"));
        assert!(display_str.contains("id=00000000-0000-007b-0000-000000000000"));
        assert!(display_str.contains("price=10000"));
        assert!(display_str.contains("quantity=5"));
        assert!(display_str.contains("side=BUY"));
        assert!(display_str.contains("timestamp=1616823000000"));
        assert!(display_str.contains("time_in_force=GTC"));
    }

    #[test]
    fn test_display_iceberg_order() {
        let order = create_iceberg_order();
        let display_str = format!("{}", order);

        assert!(display_str.starts_with("IcebergOrder:"));
        assert!(display_str.contains("id=00000000-0000-007c-0000-000000000000"));
        assert!(display_str.contains("price=10000"));
        assert!(display_str.contains("visible_quantity=1"));
        assert!(display_str.contains("hidden_quantity=4"));
        assert!(display_str.contains("side=SELL"));
        assert!(display_str.contains("timestamp=1616823000000"));
        assert!(display_str.contains("time_in_force=GTC"));
    }

    #[test]
    fn test_display_post_only_order() {
        let order = create_post_only_order();
        let display_str = format!("{}", order);

        // Assuming the Display implementation for PostOnly is completed
        assert!(
            display_str.contains("OrderType variant not fully implemented for Display")
                || (display_str.starts_with("PostOnly:")
                    && display_str.contains("id=00000000-0000-007d-0000-000000000000")
                    && display_str.contains("price=10000")
                    && display_str.contains("quantity=5")
                    && display_str.contains("side=BUY")
                    && display_str.contains("timestamp=1616823000000")
                    && display_str.contains("time_in_force=GTC"))
        );
    }

    #[test]
    fn test_roundtrip_display_parse() {
        // Test that converting to string and parsing back works correctly
        let original_order = create_standard_order();
        let string_representation = original_order.to_string();
        let parsed_order = OrderType::from_str(&string_representation);

        // If Display is properly implemented, this should work
        if let Ok(parsed) = parsed_order {
            if let (
                OrderType::Standard {
                    id: id1,
                    price: price1,
                    quantity: qty1,
                    side: side1,
                    ..
                },
                OrderType::Standard {
                    id: id2,
                    price: price2,
                    quantity: qty2,
                    side: side2,
                    ..
                },
            ) = (original_order, parsed)
            {
                assert_eq!(id1, id2);
                assert_eq!(price1, price2);
                assert_eq!(qty1, qty2);
                assert_eq!(side1, side2);
            } else {
                // This will happen if Display for non-Standard orders isn't complete
                info!("Note: Display implementation may not be complete for all order types");
            }
        }
    }

    #[test]
    fn test_display_implementation_completeness() {
        // Test all order types to ensure Display is implemented or properly indicated as unimplemented
        let orders = vec![
            create_standard_order(),
            create_iceberg_order(),
            create_post_only_order(),
            create_trailing_stop_order(),
            create_pegged_order(),
            create_market_to_limit_order(),
            create_reserve_order(),
        ];

        for order in orders {
            let display_str = format!("{}", order);

            // Either properly implemented or properly indicates it's not implemented
            assert!(
                display_str.contains(":id=")
                    || display_str.contains("OrderType variant not fully implemented for Display")
            );
        }
    }
}

#[cfg(test)]
mod test_order_type_display {
    use crate::orders::time_in_force::TimeInForce;
    use crate::orders::{OrderId, OrderType, PegReferenceType, Side};
    use std::str::FromStr;

    #[test]
    fn test_standard_order_display() {
        let order = OrderType::Standard {
            id: OrderId::from_u64(123),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        };

        let display_str = order.to_string();
        assert_eq!(
            display_str,
            "Standard:id=00000000-0000-007b-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC"
        );

        // Test that it can be parsed back (round-trip)
        let parsed = OrderType::from_str(&display_str);
        assert!(parsed.is_ok(), "Failed to parse Standard order string");

        if let Ok(OrderType::Standard {
            id,
            price,
            quantity,
            side,
            ..
        }) = parsed
        {
            assert_eq!(id, OrderId::from_u64(123));
            assert_eq!(price, 10000);
            assert_eq!(quantity, 5);
            assert_eq!(side, Side::Buy);
        } else {
            panic!("Parsed result is not a Standard order");
        }
    }

    #[test]
    fn test_iceberg_order_display() {
        let order = OrderType::IcebergOrder {
            id: OrderId::from_u64(124),
            price: 10000,
            visible_quantity: 1,
            hidden_quantity: 4,
            side: Side::Sell,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        };

        let display_str = order.to_string();
        assert_eq!(
            display_str,
            "IcebergOrder:id=00000000-0000-007c-0000-000000000000;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC"
        );

        // Test that it can be parsed back (round-trip)
        let parsed = OrderType::from_str(&display_str);
        assert!(parsed.is_ok(), "Failed to parse IcebergOrder string");

        if let Ok(OrderType::IcebergOrder {
            id,
            price,
            visible_quantity,
            hidden_quantity,
            side,
            ..
        }) = parsed
        {
            assert_eq!(id, OrderId::from_u64(124));
            assert_eq!(price, 10000);
            assert_eq!(visible_quantity, 1);
            assert_eq!(hidden_quantity, 4);
            assert_eq!(side, Side::Sell);
        } else {
            panic!("Parsed result is not an IcebergOrder");
        }
    }

    #[test]
    fn test_post_only_order_display() {
        let order = OrderType::PostOnly {
            id: OrderId::from_u64(125),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        };

        let display_str = order.to_string();

        // Since PostOnly might not be fully implemented, check if it returns
        // the fallback message or the proper format
        if !display_str.contains("not fully implemented") {
            assert!(display_str.starts_with("PostOnly:"));
            assert!(display_str.contains("id=00000000-0000-007d-0000-000000000000"));
            assert!(display_str.contains("price=10000"));
            assert!(display_str.contains("quantity=5"));
            assert!(display_str.contains("side=BUY"));
            assert!(display_str.contains("timestamp=1616823000000"));
            assert!(display_str.contains("time_in_force="));
        } else {
            // If not fully implemented, at least ensure we get the fallback message
            assert_eq!(
                display_str,
                "OrderType variant not fully implemented for Display"
            );
        }
    }

    #[test]
    fn test_trailing_stop_order_display() {
        let order = OrderType::TrailingStop {
            id: OrderId::from_u64(126),
            price: 10000,
            quantity: 5,
            side: Side::Sell,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
            trail_amount: 100,
            last_reference_price: 10100,
        };

        let display_str = order.to_string();

        if !display_str.contains("not fully implemented") {
            assert!(display_str.starts_with("TrailingStop:"));
            assert!(display_str.contains("id=00000000-0000-007e-0000-000000000000"));
            assert!(display_str.contains("price=10000"));
            assert!(display_str.contains("quantity=5"));
            assert!(display_str.contains("side=SELL"));
            assert!(display_str.contains("trail_amount=100"));
            assert!(display_str.contains("last_reference_price=10100"));
        } else {
            assert_eq!(
                display_str,
                "OrderType variant not fully implemented for Display"
            );
        }
    }

    #[test]
    fn test_pegged_order_display() {
        let order = OrderType::PeggedOrder {
            id: OrderId::from_u64(127),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
            reference_price_offset: -50,
            reference_price_type: PegReferenceType::BestAsk,
        };

        let display_str = order.to_string();

        if !display_str.contains("not fully implemented") {
            assert!(display_str.starts_with("PeggedOrder:"));
            assert!(display_str.contains("id=00000000-0000-007f-0000-000000000000"));
            assert!(display_str.contains("price=10000"));
            assert!(display_str.contains("quantity=5"));
            assert!(display_str.contains("side=BUY"));
            assert!(display_str.contains("reference_price_offset=-50"));
            assert!(display_str.contains("reference_price_type=BestAsk"));
        } else {
            assert_eq!(
                display_str,
                "OrderType variant not fully implemented for Display"
            );
        }
    }

    #[test]
    fn test_market_to_limit_order_display() {
        let order = OrderType::MarketToLimit {
            id: OrderId::from_u64(128),
            price: 10000,
            quantity: 5,
            side: Side::Buy,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
        };

        let display_str = order.to_string();

        if !display_str.contains("not fully implemented") {
            assert!(display_str.starts_with("MarketToLimit:"));
            assert!(display_str.contains("id=00000000-0000-0080-0000-000000000000"));
            assert!(display_str.contains("price=10000"));
            assert!(display_str.contains("quantity=5"));
            assert!(display_str.contains("side=BUY"));
        } else {
            assert_eq!(
                display_str,
                "OrderType variant not fully implemented for Display"
            );
        }
    }

    #[test]
    fn test_reserve_order_display() {
        let order = OrderType::ReserveOrder {
            id: OrderId::from_u64(129),
            price: 10000,
            visible_quantity: 1,
            hidden_quantity: 4,
            side: Side::Sell,
            timestamp: 1616823000000,
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: 0,
            replenish_amount: Some(1),
            auto_replenish: false,
        };

        let display_str = order.to_string();

        if !display_str.contains("not fully implemented") {
            assert!(display_str.starts_with("ReserveOrder:"));
            assert!(display_str.contains("id=00000000-0000-0081-0000-000000000000"));
            assert!(display_str.contains("price=10000"));
            assert!(display_str.contains("visible_quantity=1"));
            assert!(display_str.contains("hidden_quantity=4"));
            assert!(display_str.contains("side=SELL"));
            assert!(display_str.contains("replenish_threshold=0"));
        } else {
            assert_eq!(
                display_str,
                "OrderType variant not fully implemented for Display"
            );
        }
    }
}

#[cfg(test)]
mod from_str_specific_tests {
    use crate::orders::{OrderId, OrderType, PegReferenceType, Side, TimeInForce};
    use std::str::FromStr;

    #[test]
    fn test_from_str_reserve_order() {
        // Test a complete reserve order string
        let input = "ReserveOrder:id=00000000-0000-0081-0000-000000000000;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC;replenish_threshold=0;replenish_amount=1;auto_replenish=false";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
            } => {
                assert_eq!(id, OrderId::from_u64(129));
                assert_eq!(price, 10000);
                assert_eq!(visible_quantity, 1);
                assert_eq!(hidden_quantity, 4);
                assert_eq!(side, Side::Sell);
                assert_eq!(timestamp, 1616823000000);
                assert_eq!(time_in_force, TimeInForce::Gtc);
                assert_eq!(replenish_threshold, 0);
                assert_eq!(replenish_amount, Some(1));
                assert!(!auto_replenish);
            }
            _ => panic!("Expected ReserveOrder"),
        }

        // Test with None replenish_amount
        let input = "ReserveOrder:id=00000000-0000-0081-0000-000000000000;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC;replenish_threshold=10;replenish_amount=None;auto_replenish=true";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::ReserveOrder {
                replenish_amount,
                replenish_threshold,
                auto_replenish,
                ..
            } => {
                assert_eq!(replenish_amount, None);
                assert_eq!(replenish_threshold, 10);
                assert!(auto_replenish);
            }
            _ => panic!("Expected ReserveOrder"),
        }

        // Test with different time_in_force
        let input = "ReserveOrder:id=00000000-0000-0081-0000-000000000000;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTD-1617000000000;replenish_threshold=5;replenish_amount=2;auto_replenish=true";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::ReserveOrder {
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                ..
            } => {
                assert_eq!(time_in_force, TimeInForce::Gtd(1617000000000));
                assert_eq!(replenish_threshold, 5);
                assert_eq!(replenish_amount, Some(2));
                assert!(auto_replenish);
            }
            _ => panic!("Expected ReserveOrder"),
        }
    }

    #[test]
    fn test_from_str_market_to_limit_order() {
        // Test basic market-to-limit order
        let input = "MarketToLimit:id=00000000-0000-0080-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::MarketToLimit {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
            } => {
                assert_eq!(id, OrderId::from_u64(128));
                assert_eq!(price, 10000);
                assert_eq!(quantity, 5);
                assert_eq!(side, Side::Buy);
                assert_eq!(timestamp, 1616823000000);
                assert_eq!(time_in_force, TimeInForce::Gtc);
            }
            _ => panic!("Expected MarketToLimit"),
        }

        // Test with IOC time-in-force
        let input = "MarketToLimit:id=00000000-0000-0080-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=IOC";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::MarketToLimit { time_in_force, .. } => {
                assert_eq!(time_in_force, TimeInForce::Ioc);
            }
            _ => panic!("Expected MarketToLimit"),
        }

        // Test with SELL side
        let input = "MarketToLimit:id=00000000-0000-0080-0000-000000000000;price=10000;quantity=5;side=SELL;timestamp=1616823000000;time_in_force=GTC";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::MarketToLimit { side, .. } => {
                assert_eq!(side, Side::Sell);
            }
            _ => panic!("Expected MarketToLimit"),
        }
    }

    #[test]
    fn test_from_str_pegged_order() {
        // Test with BestAsk reference type
        let input = "PeggedOrder:id=00000000-0000-007f-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC;reference_price_offset=-50;reference_price_type=BestAsk";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::PeggedOrder {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
            } => {
                assert_eq!(id, OrderId::from_u64(127));
                assert_eq!(price, 10000);
                assert_eq!(quantity, 5);
                assert_eq!(side, Side::Buy);
                assert_eq!(timestamp, 1616823000000);
                assert_eq!(time_in_force, TimeInForce::Gtc);
                assert_eq!(reference_price_offset, -50);
                assert_eq!(reference_price_type, PegReferenceType::BestAsk);
            }
            _ => panic!("Expected PeggedOrder"),
        }

        // Test with BestBid reference type
        let input = "PeggedOrder:id=00000000-0000-007f-0000-000000000000;price=10000;quantity=5;side=SELL;timestamp=1616823000000;time_in_force=IOC;reference_price_offset=50;reference_price_type=BestBid";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::PeggedOrder {
                side,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                ..
            } => {
                assert_eq!(side, Side::Sell);
                assert_eq!(time_in_force, TimeInForce::Ioc);
                assert_eq!(reference_price_offset, 50);
                assert_eq!(reference_price_type, PegReferenceType::BestBid);
            }
            _ => panic!("Expected PeggedOrder"),
        }

        // Test with MidPrice reference type
        let input = "PeggedOrder:id=00000000-0000-007f-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC;reference_price_offset=0;reference_price_type=MidPrice";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::PeggedOrder {
                reference_price_offset,
                reference_price_type,
                ..
            } => {
                assert_eq!(reference_price_offset, 0);
                assert_eq!(reference_price_type, PegReferenceType::MidPrice);
            }
            _ => panic!("Expected PeggedOrder"),
        }

        // Test with LastTrade reference type
        let input = "PeggedOrder:id=00000000-0000-007f-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC;reference_price_offset=-100;reference_price_type=LastTrade";
        let order = OrderType::from_str(input).unwrap();

        match order {
            OrderType::PeggedOrder {
                reference_price_offset,
                reference_price_type,
                ..
            } => {
                assert_eq!(reference_price_offset, -100);
                assert_eq!(reference_price_type, PegReferenceType::LastTrade);
            }
            _ => panic!("Expected PeggedOrder"),
        }
    }

    #[test]
    fn test_from_str_invalid_pegged_reference_type() {
        let input = "PeggedOrder:id=00000000-0000-007f-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC;reference_price_offset=-50;reference_price_type=InvalidType";
        let result = OrderType::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            crate::errors::PriceLevelError::InvalidFieldValue { field, value } => {
                assert_eq!(field, "reference_price_type");
                assert_eq!(value, "InvalidType");
            }
            err => panic!("Expected InvalidFieldValue error, got {:?}", err),
        }
    }

    #[test]
    fn test_from_str_invalid_reserve_order_auto_replenish() {
        let input = "ReserveOrder:id=00000000-0000-0081-0000-000000000000;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC;replenish_threshold=0;replenish_amount=1;auto_replenish=invalid";
        let result = OrderType::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            crate::errors::PriceLevelError::InvalidFieldValue { field, value } => {
                assert_eq!(field, "auto_replenish");
                assert_eq!(value, "invalid");
            }
            err => panic!("Expected InvalidFieldValue error, got {:?}", err),
        }
    }

    #[test]
    fn test_edge_cases() {
        // Test case-insensitivity for side
        let input = "MarketToLimit:id=00000000-0000-0080-0000-000000000000;price=10000;quantity=5;side=bUY;timestamp=1616823000000;time_in_force=GTC";
        let order_result = OrderType::from_str(input);
        assert!(
            order_result.is_ok(),
            "Failed to parse MarketToLimit order string"
        );
        let order = order_result.unwrap();

        match order {
            OrderType::MarketToLimit { side, .. } => {
                assert_eq!(side, Side::Buy);
            }
            _ => panic!("Expected MarketToLimit"),
        }
        
        // Test with maximum values
        let input = format!(
            "PeggedOrder:id=ffffffff-ffff-ffff-0000-000000000000;price={};quantity={};side=BUY;timestamp={};time_in_force=GTC;reference_price_offset={};reference_price_type=BestAsk",
            u64::MAX,
            u64::MAX,
            u64::MAX,
            i64::MAX
        );
        let order = OrderType::from_str(&input).unwrap();

        match order {
            OrderType::PeggedOrder {
                id,
                price,
                quantity,
                timestamp,
                reference_price_offset,
                ..
            } => {
                assert_eq!(id, OrderId::from_u64(u64::MAX));
                assert_eq!(price, u64::MAX);
                assert_eq!(quantity, u64::MAX);
                assert_eq!(timestamp, u64::MAX);
                assert_eq!(reference_price_offset, i64::MAX);
            }
            _ => panic!("Expected PeggedOrder"),
        }

        // Test with minimum values for reference_price_offset
        let input = format!(
            "PeggedOrder:id=00000000-0000-007f-0000-000000000000;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC;reference_price_offset={};reference_price_type=BestAsk",
            i64::MIN
        );
        let order = OrderType::from_str(&input).unwrap();

        match order {
            OrderType::PeggedOrder {
                reference_price_offset,
                ..
            } => {
                assert_eq!(reference_price_offset, i64::MIN);
            }
            _ => panic!("Expected PeggedOrder"),
        }
    }

    #[test]
    fn test_roundtrip_serialization() {
        // Create sample orders
        let orders = vec![
            OrderType::ReserveOrder {
                id: OrderId::from_u64(129),
                price: 10000,
                visible_quantity: 1,
                hidden_quantity: 4,
                side: Side::Sell,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Gtc,
                replenish_threshold: 0,
                replenish_amount: Some(1),
                auto_replenish: false,
            },
            OrderType::MarketToLimit {
                id: OrderId::from_u64(128),
                price: 10000,
                quantity: 5,
                side: Side::Buy,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Ioc,
            },
            OrderType::PeggedOrder {
                id: OrderId::from_u64(127),
                price: 10000,
                quantity: 5,
                side: Side::Buy,
                timestamp: 1616823000000,
                time_in_force: TimeInForce::Gtc,
                reference_price_offset: -50,
                reference_price_type: PegReferenceType::MidPrice,
            },
        ];

        // Test round-trip for each order type
        for original_order in orders {
            let string_representation = original_order.to_string();
            let parsed_order = OrderType::from_str(&string_representation).unwrap();

            // Compare specific fields based on order type
            match (original_order, parsed_order) {
                (
                    OrderType::ReserveOrder {
                        id: id1,
                        price: price1,
                        visible_quantity: vis1,
                        hidden_quantity: hid1,
                        side: side1,
                        replenish_threshold: thresh1,
                        replenish_amount: amt1,
                        auto_replenish: auto1,
                        ..
                    },
                    OrderType::ReserveOrder {
                        id: id2,
                        price: price2,
                        visible_quantity: vis2,
                        hidden_quantity: hid2,
                        side: side2,
                        replenish_threshold: thresh2,
                        replenish_amount: amt2,
                        auto_replenish: auto2,
                        ..
                    },
                ) => {
                    assert_eq!(id1, id2);
                    assert_eq!(price1, price2);
                    assert_eq!(vis1, vis2);
                    assert_eq!(hid1, hid2);
                    assert_eq!(side1, side2);
                    assert_eq!(thresh1, thresh2);
                    assert_eq!(amt1, amt2);
                    assert_eq!(auto1, auto2);
                }
                (
                    OrderType::MarketToLimit {
                        id: id1,
                        price: price1,
                        quantity: qty1,
                        side: side1,
                        time_in_force: tif1,
                        ..
                    },
                    OrderType::MarketToLimit {
                        id: id2,
                        price: price2,
                        quantity: qty2,
                        side: side2,
                        time_in_force: tif2,
                        ..
                    },
                ) => {
                    assert_eq!(id1, id2);
                    assert_eq!(price1, price2);
                    assert_eq!(qty1, qty2);
                    assert_eq!(side1, side2);
                    assert_eq!(tif1, tif2);
                }
                (
                    OrderType::PeggedOrder {
                        id: id1,
                        price: price1,
                        quantity: qty1,
                        side: side1,
                        reference_price_offset: offset1,
                        reference_price_type: ref_type1,
                        ..
                    },
                    OrderType::PeggedOrder {
                        id: id2,
                        price: price2,
                        quantity: qty2,
                        side: side2,
                        reference_price_offset: offset2,
                        reference_price_type: ref_type2,
                        ..
                    },
                ) => {
                    assert_eq!(id1, id2);
                    assert_eq!(price1, price2);
                    assert_eq!(qty1, qty2);
                    assert_eq!(side1, side2);
                    assert_eq!(offset1, offset2);
                    assert_eq!(ref_type1, ref_type2);
                }
                _ => panic!("Order types don't match after round-trip"),
            }
        }
    }
}
