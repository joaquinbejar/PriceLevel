// Pre-release hardening: allocation-free case folding in the `FromStr`
// parsers, bounded input echo in error messages, the strict `Hash32` hex
// grammar, and the allocation-free `OrderType` `Display`.
#[cfg(test)]
mod hardening_tests {
    use crate::errors::PriceLevelError;
    use crate::orders::status::OrderStatus;
    use crate::orders::{Hash32, Id, OrderType, PegReferenceType, Side, TimeInForce};
    use crate::utils::text::MAX_ECHOED_INPUT_CHARS;
    use crate::utils::{Price, Quantity, TimestampMs};
    use proptest::prelude::*;
    use std::num::NonZeroU64;
    use std::str::FromStr;

    // ---- Pre-hardening reference parsers (test-only, allocating) ----------

    fn reference_side(s: &str) -> Option<Side> {
        match s.to_uppercase().as_str() {
            "BUY" => Some(Side::Buy),
            "SELL" => Some(Side::Sell),
            _ => None,
        }
    }

    fn reference_status(s: &str) -> Option<OrderStatus> {
        match s.to_uppercase().as_str() {
            "NEW" => Some(OrderStatus::New),
            "ACTIVE" => Some(OrderStatus::Active),
            "PARTIALLYFILLED" => Some(OrderStatus::PartiallyFilled),
            "FILLED" => Some(OrderStatus::Filled),
            "CANCELED" => Some(OrderStatus::Canceled),
            "REJECTED" => Some(OrderStatus::Rejected),
            "EXPIRED" => Some(OrderStatus::Expired),
            _ => None,
        }
    }

    fn reference_tif(s: &str) -> Option<TimeInForce> {
        match s.to_uppercase().as_str() {
            "GTC" => Some(TimeInForce::Gtc),
            "IOC" => Some(TimeInForce::Ioc),
            "FOK" => Some(TimeInForce::Fok),
            "DAY" => Some(TimeInForce::Day),
            s if s.starts_with("GTD-") => {
                let parts: Vec<&str> = s.split('-').collect();
                if parts.len() != 2 {
                    return None;
                }
                parts[1].parse::<u64>().ok().map(TimeInForce::Gtd)
            }
            _ => None,
        }
    }

    /// The pre-hardening `Hash32::from_hex` (per-pair `u8::from_str_radix`).
    fn reference_hash32(s: &str) -> Option<Hash32> {
        if s.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (slot, pair) in bytes.iter_mut().zip(s.as_bytes().chunks(2)) {
            let pair = std::str::from_utf8(pair).ok()?;
            *slot = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Hash32::new(bytes))
    }

    /// Scalars around the vocabularies: every ASCII letter of the literals in
    /// both cases, the non-ASCII scalars whose uppercase is ASCII, and a few
    /// that uppercase to a longer / non-ASCII form.
    const FOLD_CHARS: &str = "[bBuUyYsSeElLnNwWaAcCtTiIvVpPrRfFdDxXgGoOkK0-9+\\-ſıßﬀﬁﬂﬃﬄﬅﬆẗİéÉ ]";

    fn vocabulary() -> Vec<&'static str> {
        vec![
            "BUY",
            "buy",
            "Buy",
            "SELL",
            "sell",
            "sElL",
            "ſell",
            "SELſ",
            "NEW",
            "new",
            "ACTIVE",
            "actıve",
            "PARTIALLYFILLED",
            "partiallyﬁlled",
            "FILLED",
            "ﬁlled",
            "ﬁﬂed",
            "CANCELED",
            "canceled",
            "REJECTED",
            "EXPIRED",
            "expıred",
            "GTC",
            "gtc",
            "IOC",
            "ıoc",
            "FOK",
            "fok",
            "DAY",
            "day",
            "GTD-0",
            "gtd-1",
            "Gtd-18446744073709551615",
            "GTD-18446744073709551616",
            "GTD-+5",
            "GTD--5",
            "GTD-5-",
            "GTD-",
            "GTD",
            "gẗd-1",
            "ǵtd-1",
            "",
            " BUY",
            "BUY ",
            "BUYY",
            "ß",
            "İoc",
        ]
    }

    #[test]
    fn test_fold_parsers_match_reference_on_vocabulary() {
        for s in vocabulary() {
            assert_eq!(Side::from_str(s).ok(), reference_side(s), "Side {s:?}");
            assert_eq!(
                OrderStatus::from_str(s).ok(),
                reference_status(s),
                "OrderStatus {s:?}"
            );
            assert_eq!(
                TimeInForce::from_str(s).ok(),
                reference_tif(s),
                "TimeInForce {s:?}"
            );
        }
    }

    /// Non-ASCII inputs the former `to_uppercase` parsers accepted by
    /// coincidence: still accepted (documented on each `FromStr`).
    #[test]
    fn test_unicode_folds_are_preserved() {
        assert_eq!(Side::from_str("ſell").ok(), Some(Side::Sell));
        assert_eq!(
            OrderStatus::from_str("ﬁlled").ok(),
            Some(OrderStatus::Filled)
        );
        assert_eq!(
            OrderStatus::from_str("actıve").ok(),
            Some(OrderStatus::Active)
        );
        assert_eq!(TimeInForce::from_str("ıoc").ok(), Some(TimeInForce::Ioc));
    }

    /// The `GTD-` branch compares ASCII case-insensitively and parses the raw
    /// tail. That is equivalent to the former uppercase-first parser only if
    /// no non-ASCII scalar can uppercase into (a piece of) `GTD-`, a `-`, a
    /// sign or a digit. Checked over every scalar.
    #[test]
    fn test_no_non_ascii_scalar_uppercases_into_gtd_grammar() {
        const PREFIX: &str = "GTD-";
        let suffixes: Vec<&str> = (0..PREFIX.len()).filter_map(|i| PREFIX.get(i..)).collect();
        for c in (0u32..=0x10_FFFF)
            .filter_map(char::from_u32)
            .filter(|c| !c.is_ascii())
        {
            let upper: String = c.to_uppercase().collect();
            assert!(!PREFIX.contains(upper.as_str()), "{c:?} -> {upper:?}");
            assert!(
                !suffixes.iter().any(|suffix| upper.starts_with(suffix)),
                "{c:?} -> {upper:?}"
            );
            assert!(
                !upper
                    .chars()
                    .any(|u| u == '-' || u == '+' || u.is_ascii_digit()),
                "{c:?} -> {upper:?}"
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

        #[test]
        fn prop_fold_parsers_match_reference(
            s in prop_oneof![
                proptest::string::string_regex(&format!("{FOLD_CHARS}{{0,16}}")).unwrap(),
                "(?i:gtd)-[0-9+\\-ſı]{0,22}",
                ".{0,20}",
            ]
        ) {
            prop_assert_eq!(Side::from_str(&s).ok(), reference_side(&s));
            prop_assert_eq!(OrderStatus::from_str(&s).ok(), reference_status(&s));
            prop_assert_eq!(TimeInForce::from_str(&s).ok(), reference_tif(&s));
        }

        #[test]
        fn prop_hash32_matches_reference_except_signs(
            s in prop_oneof![
                "[0-9a-fA-F]{64}",
                "[0-9a-fA-F+\\-]{64}",
                ".{60,66}",
            ]
        ) {
            let actual = Hash32::from_hex(&s).ok();
            if s.contains('+') {
                prop_assert_eq!(actual, None);
            } else {
                prop_assert_eq!(actual, reference_hash32(&s));
            }
        }
    }

    // ---- Hash32 grammar ----------------------------------------------------

    #[test]
    fn test_hash32_rejects_plus_sign_pairs() {
        let plus = "+f".repeat(32);
        assert_eq!(reference_hash32(&plus), Some(Hash32::new([0x0f; 32])));
        let err = Hash32::from_hex(&plus).unwrap_err();
        assert_eq!(
            err,
            PriceLevelError::ParseError {
                message: "Invalid hex character in Hash32: +f".to_string(),
            }
        );
        let mut one_plus = "a5".repeat(32);
        one_plus.replace_range(62..63, "+");
        assert!(Hash32::from_hex(&one_plus).is_err());
    }

    #[test]
    fn test_hash32_accepts_canonical_lower_upper_and_mixed_case() {
        let expected = Hash32::new([0xab; 32]);
        assert_eq!(Hash32::from_hex(&"ab".repeat(32)).ok(), Some(expected));
        assert_eq!(Hash32::from_hex(&"AB".repeat(32)).ok(), Some(expected));
        assert_eq!(Hash32::from_hex(&"aB".repeat(32)).ok(), Some(expected));
        let all: String = (0u8..32).map(|b| format!("{b:02x}")).collect();
        let mut bytes = [0u8; 32];
        for (b, value) in bytes.iter_mut().zip(0u8..) {
            *b = value;
        }
        assert_eq!(Hash32::from_hex(&all).ok(), Some(Hash32::new(bytes)));
    }

    #[test]
    fn test_hash32_error_messages_unchanged() {
        let mut bad = "00".repeat(32);
        bad.replace_range(0..2, "zz");
        assert_eq!(
            Hash32::from_hex(&bad).unwrap_err(),
            PriceLevelError::ParseError {
                message: "Invalid hex character in Hash32: zz".to_string(),
            }
        );
        // `é` is two bytes and straddles no pair boundary here: pair `é`.
        let mut multibyte = "00".repeat(31);
        multibyte.insert(0, 'é');
        assert_eq!(
            Hash32::from_hex(&multibyte).unwrap_err(),
            PriceLevelError::ParseError {
                message: "Invalid hex character in Hash32: é".to_string(),
            }
        );
        // `é` at an odd offset splits across two pairs: invalid UTF-8 pair.
        let mut split = "0".to_string();
        split.push('é');
        split.push_str(&"0".repeat(61));
        assert_eq!(
            Hash32::from_hex(&split).unwrap_err(),
            PriceLevelError::ParseError {
                message: "Invalid UTF-8 in hex string".to_string(),
            }
        );
    }

    // ---- Bounded echo ------------------------------------------------------

    fn assert_bounded(message: &str, input_len: usize) {
        assert!(
            message.len() < 4 * MAX_ECHOED_INPUT_CHARS + 128,
            "message not bounded: {} bytes",
            message.len()
        );
        assert!(
            message.ends_with(&format!("... ({input_len} bytes total)")),
            "{message}"
        );
    }

    #[test]
    fn test_parse_errors_echo_a_bounded_prefix() {
        let huge = "Z".repeat(1 << 16);
        let parse_message = |err: PriceLevelError| match err {
            PriceLevelError::ParseError { message } => message,
            other => panic!("expected ParseError, got {other:?}"),
        };

        assert_bounded(
            &parse_message(TimeInForce::from_str(&huge).unwrap_err()),
            huge.len(),
        );
        let gtd = format!("GTD-{huge}");
        assert_bounded(
            &parse_message(TimeInForce::from_str(&gtd).unwrap_err()),
            huge.len(),
        );
        let gtd_two = format!("GTD-1-{huge}");
        assert_bounded(
            &parse_message(TimeInForce::from_str(&gtd_two).unwrap_err()),
            gtd_two.len(),
        );
        assert_bounded(
            &parse_message(OrderStatus::from_str(&huge).unwrap_err()),
            huge.len(),
        );
        assert_bounded(
            &parse_message(PegReferenceType::from_str(&huge).unwrap_err()),
            huge.len(),
        );
        assert_bounded(&parse_message(Id::from_str(&huge).unwrap_err()), huge.len());
    }

    #[test]
    fn test_field_value_errors_echo_a_bounded_prefix() {
        let huge = "Z".repeat(1 << 16);
        let value_of = |err: PriceLevelError| match err {
            PriceLevelError::InvalidFieldValue { value, .. } => value,
            other => panic!("expected InvalidFieldValue, got {other:?}"),
        };
        assert_bounded(&value_of(Price::from_str(&huge).unwrap_err()), huge.len());
        assert_bounded(
            &value_of(Quantity::from_str(&huge).unwrap_err()),
            huge.len(),
        );
        assert_bounded(
            &value_of(TimestampMs::from_str(&huge).unwrap_err()),
            huge.len(),
        );
        let order = format!(
            "Standard:id=1;price={huge};quantity=1;side=BUY;user_id={};timestamp=1;time_in_force=GTC",
            "00".repeat(32)
        );
        assert_bounded(
            &value_of(OrderType::<()>::from_str(&order).unwrap_err()),
            huge.len(),
        );
        let unknown = format!(
            "{huge}:id=1;price=1;quantity=1;side=BUY;user_id={};timestamp=1;time_in_force=GTC",
            "00".repeat(32)
        );
        match OrderType::<()>::from_str(&unknown).unwrap_err() {
            PriceLevelError::UnknownOrderType(name) => assert_bounded(&name, huge.len()),
            other => panic!("expected UnknownOrderType, got {other:?}"),
        }
    }

    #[test]
    fn test_short_inputs_are_echoed_verbatim() {
        assert_eq!(
            TimeInForce::from_str("nope").unwrap_err(),
            PriceLevelError::ParseError {
                message: "Invalid TimeInForce: nope".to_string(),
            }
        );
        assert_eq!(
            OrderStatus::from_str("nope").unwrap_err(),
            PriceLevelError::ParseError {
                message: "Invalid OrderStatus: nope".to_string(),
            }
        );
    }

    // ---- Display -----------------------------------------------------------

    #[test]
    fn test_side_display_matches_former_debug_uppercase() {
        for side in [Side::Buy, Side::Sell] {
            assert_eq!(side.to_string(), format!("{side:?}").to_uppercase());
        }
    }

    #[test]
    fn test_reserve_display_replenish_amount_forms() {
        let make = |replenish_amount| OrderType::<()>::ReserveOrder {
            id: Id::Sequential(7),
            price: Price::new(100),
            visible_quantity: Quantity::new(5),
            hidden_quantity: Quantity::new(10),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(9),
            time_in_force: TimeInForce::Gtd(42),
            replenish_threshold: Quantity::new(1),
            replenish_amount,
            auto_replenish: true,
            extra_fields: (),
        };
        let zero = "0".repeat(64);
        assert_eq!(
            make(None).to_string(),
            format!(
                "ReserveOrder:id=7;price=100;visible_quantity=5;hidden_quantity=10;side=SELL;user_id={zero};timestamp=9;time_in_force=GTD-42;replenish_threshold=1;replenish_amount=None;auto_replenish=true"
            )
        );
        assert_eq!(
            make(NonZeroU64::new(3)).to_string(),
            format!(
                "ReserveOrder:id=7;price=100;visible_quantity=5;hidden_quantity=10;side=SELL;user_id={zero};timestamp=9;time_in_force=GTD-42;replenish_threshold=1;replenish_amount=3;auto_replenish=true"
            )
        );
        for order in [make(None), make(NonZeroU64::new(3))] {
            let text = order.to_string();
            let back = OrderType::<()>::from_str(&text).unwrap();
            assert_eq!(back.to_string(), text);
        }
    }
}
