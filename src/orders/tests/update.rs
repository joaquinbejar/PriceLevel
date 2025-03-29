#[cfg(test)]
mod tests_order_update {
    use crate::errors::PriceLevelError;
    use crate::orders::base::{OrderId, Side};
    use crate::orders::update::OrderUpdate;
    use std::str::FromStr;

    #[test]
    fn test_update_price_from_str() {
        let input = "UpdatePrice:order_id=123;new_price=1000";
        let result = OrderUpdate::from_str(input).unwrap();

        match result {
            OrderUpdate::UpdatePrice {
                order_id,
                new_price,
            } => {
                assert_eq!(order_id, OrderId::from_u64(123));
                assert_eq!(new_price, 1000);
            }
            _ => panic!("Expected UpdatePrice variant"),
        }
    }

    #[test]
    fn test_update_quantity_from_str() {
        let input = "UpdateQuantity:order_id=456;new_quantity=50";
        let result = OrderUpdate::from_str(input).unwrap();

        match result {
            OrderUpdate::UpdateQuantity {
                order_id,
                new_quantity,
            } => {
                assert_eq!(order_id, OrderId::from_u64(456));
                assert_eq!(new_quantity, 50);
            }
            _ => panic!("Expected UpdateQuantity variant"),
        }
    }

    #[test]
    fn test_update_price_and_quantity_from_str() {
        let input = "UpdatePriceAndQuantity:order_id=789;new_price=2000;new_quantity=30";
        let result = OrderUpdate::from_str(input).unwrap();

        match result {
            OrderUpdate::UpdatePriceAndQuantity {
                order_id,
                new_price,
                new_quantity,
            } => {
                assert_eq!(order_id, OrderId::from_u64(789));
                assert_eq!(new_price, 2000);
                assert_eq!(new_quantity, 30);
            }
            _ => panic!("Expected UpdatePriceAndQuantity variant"),
        }
    }

    #[test]
    fn test_cancel_from_str() {
        let input = "Cancel:order_id=101";
        let result = OrderUpdate::from_str(input).unwrap();

        match result {
            OrderUpdate::Cancel { order_id } => {
                assert_eq!(order_id, OrderId::from_u64(101));
            }
            _ => panic!("Expected Cancel variant"),
        }
    }

    #[test]
    fn test_replace_from_str() {
        let input = "Replace:order_id=202;price=3000;quantity=40;side=BUY";
        let result = OrderUpdate::from_str(input).unwrap();

        match result {
            OrderUpdate::Replace {
                order_id,
                price,
                quantity,
                side,
            } => {
                assert_eq!(order_id, OrderId::from_u64(202));
                assert_eq!(price, 3000);
                assert_eq!(quantity, 40);
                assert_eq!(side, Side::Buy);
            }
            _ => panic!("Expected Replace variant"),
        }
    }

    #[test]
    fn test_replace_with_sell_side_from_str() {
        let input = "Replace:order_id=303;price=4000;quantity=60;side=SELL";
        let result = OrderUpdate::from_str(input).unwrap();

        match result {
            OrderUpdate::Replace {
                order_id,
                price,
                quantity,
                side,
            } => {
                assert_eq!(order_id, OrderId::from_u64(303));
                assert_eq!(price, 4000);
                assert_eq!(quantity, 60);
                assert_eq!(side, Side::Sell);
            }
            _ => panic!("Expected Replace variant"),
        }
    }

    #[test]
    fn test_invalid_format() {
        let input = "UpdatePrice;order_id=123;new_price=1000";
        let result = OrderUpdate::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::InvalidFormat => {}
            err => panic!("Expected InvalidFormat error, got {:?}", err),
        }
    }

    #[test]
    fn test_unknown_order_type() {
        let input = "Unknown:order_id=123;new_price=1000";
        let result = OrderUpdate::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::UnknownOrderType(type_name) => {
                assert_eq!(type_name, "Unknown");
            }
            err => panic!("Expected UnknownOrderType error, got {:?}", err),
        }
    }

    #[test]
    fn test_missing_field() {
        let input = "UpdatePrice:order_id=123"; // missing new_price
        let result = OrderUpdate::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::MissingField(field) => {
                assert_eq!(field, "new_price");
            }
            err => panic!("Expected MissingField error, got {:?}", err),
        }
    }

    #[test]
    fn test_invalid_field_value() {
        let input = "UpdatePrice:order_id=abc;new_price=1000"; // invalid order_id
        let result = OrderUpdate::from_str(input);

        assert!(result.is_err());
        match result.unwrap_err() {
            PriceLevelError::InvalidFieldValue { field, value } => {
                assert_eq!(field, "order_id");
                assert_eq!(value, "abc");
            }
            err => panic!("Expected InvalidFieldValue error, got {:?}", err),
        }
    }

    #[test]
    fn test_display_update_price() {
        let update = OrderUpdate::UpdatePrice {
            order_id: OrderId::from_u64(123),
            new_price: 1000,
        };

        assert_eq!(
            update.to_string(),
            "UpdatePrice:order_id=123;new_price=1000"
        );
    }

    #[test]
    fn test_display_update_quantity() {
        let update = OrderUpdate::UpdateQuantity {
            order_id: OrderId::from_u64(456),
            new_quantity: 50,
        };

        assert_eq!(
            update.to_string(),
            "UpdateQuantity:order_id=456;new_quantity=50"
        );
    }

    #[test]
    fn test_display_update_price_and_quantity() {
        let update = OrderUpdate::UpdatePriceAndQuantity {
            order_id: OrderId::from_u64(789),
            new_price: 2000,
            new_quantity: 30,
        };

        assert_eq!(
            update.to_string(),
            "UpdatePriceAndQuantity:order_id=789;new_price=2000;new_quantity=30"
        );
    }

    #[test]
    fn test_display_cancel() {
        let update = OrderUpdate::Cancel {
            order_id: OrderId::from_u64(101),
        };

        assert_eq!(update.to_string(), "Cancel:order_id=101");
    }

    #[test]
    fn test_display_replace() {
        let update = OrderUpdate::Replace {
            order_id: OrderId::from_u64(202),
            price: 3000,
            quantity: 40,
            side: Side::Buy,
        };

        assert_eq!(
            update.to_string(),
            "Replace:order_id=202;price=3000;quantity=40;side=BUY"
        );
    }

    #[test]
    fn test_roundtrip_parsing() {
        // Create instances of each variant
        let updates = vec![
            OrderUpdate::UpdatePrice {
                order_id: OrderId::from_u64(123),
                new_price: 1000,
            },
            OrderUpdate::UpdateQuantity {
                order_id: OrderId::from_u64(456),
                new_quantity: 50,
            },
            OrderUpdate::UpdatePriceAndQuantity {
                order_id: OrderId::from_u64(789),
                new_price: 2000,
                new_quantity: 30,
            },
            OrderUpdate::Cancel {
                order_id: OrderId::from_u64(101),
            },
            OrderUpdate::Replace {
                order_id: OrderId::from_u64(202),
                price: 3000,
                quantity: 40,
                side: Side::Buy,
            },
            OrderUpdate::Replace {
                order_id: OrderId::from_u64(303),
                price: 4000,
                quantity: 60,
                side: Side::Sell,
            },
        ];

        // Test round-trip for each variant
        for update in updates {
            let string_representation = update.to_string();
            let parsed_update = OrderUpdate::from_str(&string_representation).unwrap();

            // Compare the debug representation since OrderUpdate doesn't implement PartialEq
            assert_eq!(format!("{:?}", update), format!("{:?}", parsed_update));
        }
    }
}
