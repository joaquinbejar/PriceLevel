#[cfg(test)]
mod tests_side {
    use crate::orders::Side;

    #[test]
    fn test_side_equality() {
        assert_eq!(Side::Buy, Side::Buy);
        assert_eq!(Side::Sell, Side::Sell);
        assert_ne!(Side::Buy, Side::Sell);
    }

    #[test]
    fn test_side_clone() {
        let buy = Side::Buy;
        let cloned_buy = buy;
        assert_eq!(buy, cloned_buy);

        let sell = Side::Sell;
        let cloned_sell = sell;
        assert_eq!(sell, cloned_sell);
    }

    #[test]
    fn test_serialize_to_uppercase() {
        assert_eq!(serde_json::to_string(&Side::Buy).unwrap(), "\"BUY\"");
        assert_eq!(serde_json::to_string(&Side::Sell).unwrap(), "\"SELL\"");
    }

    #[test]
    fn test_deserialize_uppercase() {
        assert_eq!(serde_json::from_str::<Side>("\"BUY\"").unwrap(), Side::Buy);
        assert_eq!(
            serde_json::from_str::<Side>("\"SELL\"").unwrap(),
            Side::Sell
        );
    }

    #[test]
    fn test_deserialize_lowercase() {
        assert_eq!(serde_json::from_str::<Side>("\"buy\"").unwrap(), Side::Buy);
        assert_eq!(
            serde_json::from_str::<Side>("\"sell\"").unwrap(),
            Side::Sell
        );
    }

    #[test]
    fn test_deserialize_capitalized() {
        assert_eq!(serde_json::from_str::<Side>("\"Buy\"").unwrap(), Side::Buy);
        assert_eq!(
            serde_json::from_str::<Side>("\"Sell\"").unwrap(),
            Side::Sell
        );
    }

    #[test]
    fn test_round_trip_serialization() {
        let sides = vec![Side::Buy, Side::Sell];

        for side in sides {
            let serialized = serde_json::to_string(&side).unwrap();
            let deserialized: Side = serde_json::from_str(&serialized).unwrap();
            assert_eq!(side, deserialized);
        }
    }

    #[test]
    fn test_invalid_deserialization() {
        assert!(serde_json::from_str::<Side>("\"INVALID\"").is_err());
        assert!(serde_json::from_str::<Side>("\"BUYING\"").is_err());
        assert!(serde_json::from_str::<Side>("\"SELLING\"").is_err());
        assert!(serde_json::from_str::<Side>("123").is_err());
        assert!(serde_json::from_str::<Side>("null").is_err());
    }

    #[test]
    fn test_from_string() {
        assert_eq!("BUY".parse::<Side>().unwrap(), Side::Buy);
        assert_eq!("SELL".parse::<Side>().unwrap(), Side::Sell);
        assert_eq!("buy".parse::<Side>().unwrap(), Side::Buy);
        assert_eq!("sell".parse::<Side>().unwrap(), Side::Sell);
    }

    #[test]
    fn test_serialized_size() {
        assert_eq!(serde_json::to_string(&Side::Buy).unwrap().len(), 5); // "BUY"
        assert_eq!(serde_json::to_string(&Side::Sell).unwrap().len(), 6); // "SELL"
    }

    // After dropping the redundant `alias = "Buy"` / `alias = "Sell"` (the
    // variant names serde already accepts on deserialize by default), the
    // serialize form ("BUY"/"SELL", kept as an alias), the lowercase form
    // (kept as an alias), and the bare variant name must all still deserialize.
    #[test]
    fn test_deserialize_all_accepted_forms_after_alias_cleanup() {
        for s in ["\"BUY\"", "\"buy\"", "\"Buy\""] {
            assert_eq!(
                serde_json::from_str::<Side>(s).unwrap(),
                Side::Buy,
                "Side::Buy should deserialize from {s}"
            );
        }
        for s in ["\"SELL\"", "\"sell\"", "\"Sell\""] {
            assert_eq!(
                serde_json::from_str::<Side>(s).unwrap(),
                Side::Sell,
                "Side::Sell should deserialize from {s}"
            );
        }

        // Wire format proof: serialize emits the uppercase form and it
        // round-trips back via the kept uppercase alias.
        for side in [Side::Buy, Side::Sell] {
            let wire = serde_json::to_string(&side).unwrap();
            assert_eq!(serde_json::from_str::<Side>(&wire).unwrap(), side);
        }
    }
}

// Issue #201: `Hash32` text / serde through a stack buffer and a borrowed
// visitor must be byte-identical to the pre-#201 allocating forms.
#[cfg(test)]
mod tests_hash32_issue_201 {
    use crate::orders::Hash32;
    use proptest::prelude::*;
    use std::str::FromStr;

    /// The pre-#201 allocating `to_hex` form (test-only reference).
    fn reference_hex(hash: &Hash32) -> String {
        hash.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `hex` with its first character replaced by `prefix`.
    fn with_first_replaced(hex: &str, prefix: &str) -> String {
        let mut chars = hex.chars();
        chars.next();
        format!("{prefix}{}", chars.as_str())
    }

    fn hex_like_text() -> impl Strategy<Value = String> {
        prop_oneof![
            any::<[u8; 32]>().prop_map(|b| reference_hex(&Hash32::new(b))),
            any::<[u8; 32]>().prop_map(|b| reference_hex(&Hash32::new(b)).to_uppercase()),
            // `from_hex` keeps `u8::from_str_radix`'s leading `+` per pair.
            any::<[u8; 32]>()
                .prop_map(|b| with_first_replaced(&reference_hex(&Hash32::new(b)), "+")),
            "[0-9a-fA-F]{62,66}",
            ".{0,70}",
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

        #[test]
        fn prop_hash32_text_and_json_are_byte_identical(bytes in any::<[u8; 32]>()) {
            let hash = Hash32::new(bytes);
            let expected = reference_hex(&hash);
            prop_assert_eq!(hash.to_string(), expected.clone());
            prop_assert_eq!(hash.to_hex(), expected.clone());
            let json = serde_json::to_string(&hash)
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(json, format!("\"{expected}\""));
        }

        #[test]
        fn prop_hash32_deserialize_matches_from_hex(text in hex_like_text()) {
            let expected = Hash32::from_str(&text).ok();
            let json = serde_json::to_string(&text)
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
            // Borrowed (`visit_str`), reader scratch, and owned (`visit_string`).
            prop_assert_eq!(serde_json::from_str::<Hash32>(&json).ok(), expected);
            prop_assert_eq!(serde_json::from_reader::<_, Hash32>(json.as_bytes()).ok(), expected);
            let value = serde_json::Value::String(text.clone());
            prop_assert_eq!(serde_json::from_value::<Hash32>(value).ok(), expected);
        }
    }

    #[test]
    fn test_hash32_deserialize_rejects_non_string() {
        let err = serde_json::from_str::<Hash32>("7")
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            err.as_deref(),
            Some("invalid type: integer `7`, expected a string at line 1 column 1")
        );
    }

    #[test]
    fn test_hash32_deserialize_bytes_and_char_match_pre_201_string_path() {
        use crate::utils::encode::serde_parity_tests::assert_byte_and_char_parity;
        let valid = reference_hex(&Hash32::new([0xa7; 32]));
        let plus = with_first_replaced(&valid, "+");
        for text in [
            valid.as_str(),
            valid.to_uppercase().as_str(),
            plus.as_str(),
            "zz",
            "",
        ] {
            assert_byte_and_char_parity::<Hash32>(text.as_bytes(), &[]);
        }
        let mut invalid_utf8 = valid.clone().into_bytes();
        invalid_utf8[0] = 0xff;
        assert_byte_and_char_parity::<Hash32>(&invalid_utf8, &['a', '0', 'é']);
    }

    #[test]
    fn test_hash32_deserialize_bytes_accepts_utf8_like_base() {
        use serde::Deserialize;
        use serde::de::value::{BytesDeserializer, Error as ValueError};
        let hash = Hash32::new([0x3c; 32]);
        let hex = reference_hex(&hash);
        let de = BytesDeserializer::<ValueError>::new(hex.as_bytes());
        assert_eq!(Hash32::deserialize(de).ok(), Some(hash));
    }

    #[test]
    fn test_hash32_escaped_json_round_trips() {
        let hash = Hash32::new([0x5a; 32]);
        // `5` is `5`: forces serde_json's unescaping scratch path.
        let json = format!(
            "\"{}\"",
            with_first_replaced(&reference_hex(&hash), "\\u0035")
        );
        assert_eq!(serde_json::from_str::<Hash32>(&json).ok(), Some(hash));
    }
}
