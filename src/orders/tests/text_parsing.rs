//! Text-parser regression tests for `Hash32`, `TimeInForce`, `OrderType` and
//! `OrderUpdate` (issue #174): checked access, preserved separator /
//! duplicate-field rules and typed errors on multibyte input.

#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::orders::{Hash32, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::utils::{Id, Price, Quantity, TimestampMs};
    use std::str::FromStr;

    fn parse_message<T: std::fmt::Debug>(r: Result<T, PriceLevelError>) -> String {
        match r {
            Err(PriceLevelError::ParseError { message }) => message,
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn test_hash32_from_hex_round_trip_and_every_byte_position() {
        let mut bytes = [0_u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from(i * 7 + 1).expect("fits");
        }
        let hash = Hash32::new(bytes);
        assert_eq!(Hash32::from_hex(&hash.to_hex()).expect("round trip"), hash);
        assert_eq!(
            Hash32::from_str(&"ff".repeat(32)).expect("max"),
            Hash32::new([0xff; 32])
        );
    }

    #[test]
    fn test_hash32_from_hex_rejects_length_and_multibyte_with_typed_errors() {
        for s in ["", "0", &"0".repeat(63), &"0".repeat(65)] {
            let msg = parse_message(Hash32::from_hex(s));
            assert!(msg.contains("must be 64 characters"), "{msg}");
        }
        // 62 ASCII hex digits + one 2-byte scalar = 64 bytes. Aligned on a
        // pair, the scalar is valid UTF-8 but not hex.
        let aligned = format!("{}é", "0".repeat(62));
        assert_eq!(aligned.len(), 64);
        assert_eq!(
            parse_message(Hash32::from_hex(&aligned)),
            "Invalid hex character in Hash32: é"
        );
        // Straddling two pairs, each half is invalid UTF-8 on its own.
        let straddling = format!("0é{}", "0".repeat(61));
        assert_eq!(straddling.len(), 64);
        assert_eq!(
            parse_message(Hash32::from_hex(&straddling)),
            "Invalid UTF-8 in hex string"
        );
        let bad = format!("{}zz", "0".repeat(62));
        assert!(parse_message(Hash32::from_hex(&bad)).contains("Invalid hex character"));
    }

    #[test]
    fn test_time_in_force_gtd_separator_rules() {
        assert_eq!(
            TimeInForce::from_str("GTD-123").expect("gtd"),
            TimeInForce::Gtd(123)
        );
        assert_eq!(
            TimeInForce::from_str("gtd-7").expect("gtd"),
            TimeInForce::Gtd(7)
        );
        assert_eq!(
            TimeInForce::from_str(&TimeInForce::Gtd(u64::MAX).to_string()).expect("max"),
            TimeInForce::Gtd(u64::MAX)
        );
        assert_eq!(
            parse_message(TimeInForce::from_str("GTD-1-2")),
            "Invalid GTD format: GTD-1-2"
        );
        assert_eq!(
            parse_message(TimeInForce::from_str("GTD-")),
            "Invalid expiry timestamp in GTD: "
        );
        assert_eq!(
            parse_message(TimeInForce::from_str("GTD-18446744073709551616")),
            "Invalid expiry timestamp in GTD: 18446744073709551616"
        );
        assert!(TimeInForce::from_str("GTD-é").is_err());
    }

    fn standard() -> OrderType<()> {
        OrderType::Standard {
            id: Id::sequential(1),
            price: Price::new(100),
            quantity: Quantity::new(5),
            side: Side::Sell,
            user_id: Hash32::new([0xab; 32]),
            timestamp: TimestampMs::new(9),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    #[test]
    fn test_order_type_from_str_separators_and_duplicates() {
        let o = standard();
        let text = o.to_string();
        assert_eq!(OrderType::<()>::from_str(&text).expect("round trip"), o);
        assert!(matches!(
            OrderType::<()>::from_str(&format!("{text};x=a:b")),
            Err(PriceLevelError::InvalidFormat)
        ));
        assert!(matches!(
            OrderType::<()>::from_str("Standard"),
            Err(PriceLevelError::InvalidFormat)
        ));
        let dup = OrderType::<()>::from_str(&format!("{text};quantity=42")).expect("dup");
        assert_eq!(dup.visible_quantity(), Quantity::new(42));
        assert!(matches!(
            OrderType::<()>::from_str(&text.replace("quantity=5", "quantity=5=5")),
            Err(PriceLevelError::MissingField(ref f)) if f == "quantity"
        ));
        assert!(matches!(
            OrderType::<()>::from_str(&text.replace("price=100", "price=1é")),
            Err(PriceLevelError::InvalidFieldValue { ref field, .. }) if field == "price"
        ));
        assert!(matches!(
            OrderType::<()>::from_str(&text.replacen("Standard", "Stañdard", 1)),
            Err(PriceLevelError::UnknownOrderType(_))
        ));
    }

    #[test]
    fn test_order_update_from_str_separators_and_duplicates() {
        let text = "UpdateQuantity:order_id=1;new_quantity=5";
        let parsed = OrderUpdate::from_str(text).expect("parse");
        assert_eq!(parsed.to_string(), text);
        let dup = OrderUpdate::from_str(&format!("{text};new_quantity=6")).expect("dup");
        assert_eq!(dup.to_string(), "UpdateQuantity:order_id=1;new_quantity=6");
        assert!(matches!(
            OrderUpdate::from_str("UpdateQuantity:order_id=1:x"),
            Err(PriceLevelError::InvalidFormat)
        ));
        assert!(matches!(
            OrderUpdate::from_str("Cancel:order_id=é"),
            Err(PriceLevelError::InvalidFieldValue { .. })
        ));
        assert!(matches!(
            OrderUpdate::from_str("Cancel:order_id=1=1"),
            Err(PriceLevelError::MissingField(_))
        ));
    }
}
