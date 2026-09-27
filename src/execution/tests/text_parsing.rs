//! Text-parser regression tests for `Trade`, `TradeList` and `MatchResult`
//! (issues #174, #152): checked access, borrowed-slice segmentation, bounded
//! nesting and typed errors.

#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::execution::{MatchResult, Trade, TradeList};
    use crate::orders::Side;
    use crate::utils::{Id, Price, Quantity, TimestampMs};
    use std::str::FromStr;

    fn trade(n: u64, id: Id) -> Trade {
        Trade::with_timestamp(
            id,
            Id::sequential(900),
            Id::sequential(n),
            Price::new(10_000 + u128::from(n)),
            Quantity::new(n + 1),
            if n.is_multiple_of(2) {
                Side::Buy
            } else {
                Side::Sell
            },
            TimestampMs::new(1_616_823_000_000 + n),
        )
    }

    fn list(n: u64) -> TradeList {
        let mut l = TradeList::new();
        for i in 0..n {
            let id = if i % 2 == 0 {
                Id::sequential(1_000 + i)
            } else {
                Id::from_uuid(uuid::Uuid::from_u128(u128::from(i) << 64 | 7))
            };
            l.add(trade(i, id)).expect("capacity available in test");
        }
        l
    }

    fn outcome<T>(r: Result<T, PriceLevelError>) -> Result<(), String> {
        r.map(|_| ()).map_err(|e| format!("{e:?}"))
    }

    fn err(e: PriceLevelError) -> Result<(), String> {
        Err(format!("{e:?}"))
    }

    fn is_too_deep(e: &PriceLevelError) -> bool {
        matches!(e, PriceLevelError::ParseError { message } if message.contains("nesting depth"))
    }

    #[test]
    fn test_trade_list_from_str_round_trips_every_field_and_order() {
        for n in [0_u64, 1, 2, 32] {
            let original = list(n);
            let parsed = TradeList::from_str(&original.to_string()).expect("round trip");
            assert_eq!(parsed, original, "n = {n}");
        }
    }

    #[test]
    fn test_trade_list_from_str_skips_empty_segments_and_trailing_delimiter() {
        let t = trade(1, Id::sequential(5));
        let expected = TradeList::from_vec(vec![t, t]);
        for text in [
            format!("Trades:[{t},{t}]"),
            format!("Trades:[,{t},,{t},]"),
            format!("Trades:[{t},,,{t},,]"),
        ] {
            assert_eq!(
                TradeList::from_str(&text).expect("parse"),
                expected,
                "{text}"
            );
        }
        assert_eq!(
            TradeList::from_str("Trades:[,,,]").expect("empty segments"),
            TradeList::new()
        );
        assert_eq!(
            TradeList::from_str("Trades:[]").expect("empty"),
            TradeList::new()
        );
    }

    #[test]
    fn test_trade_list_from_str_rejects_bad_envelope() {
        for text in [
            "",
            "Trades:[",
            "Trades:",
            "Trades]",
            "trades:[]",
            "Trades:[]x",
            "Trades:(]",
        ] {
            assert_eq!(
                outcome(TradeList::from_str(text)),
                err(PriceLevelError::InvalidFormat),
                "{text:?}"
            );
        }
    }

    #[test]
    fn test_trade_list_from_str_rejects_unbalanced_brackets() {
        let t = trade(1, Id::sequential(5));
        // Balanced brackets inside an ignored field are accepted as before.
        assert!(TradeList::from_str(&format!("Trades:[{t};x=[a,b]]")).is_ok());
        // Unbalanced ones are rejected even though the trade itself parses
        // (accepted before #174).
        for text in [
            format!("Trades:[{t};x=[]"),
            format!("Trades:[{t};x=]]"),
            format!("Trades:[{t};x=],foo]"),
            format!("Trades:[{t};x=][]"),
        ] {
            assert_eq!(
                outcome(TradeList::from_str(&text)),
                err(PriceLevelError::InvalidFormat),
                "{text}"
            );
        }
    }

    #[test]
    fn test_trade_list_from_str_element_error_precedes_imbalance() {
        let t = trade(1, Id::sequential(5));
        // The first malformed trade is reported, exactly as before, even when
        // the brackets are also unbalanced.
        let text = format!("Trades:[{t},Trade:trade_id=x;y=[]");
        assert!(matches!(
            TradeList::from_str(&text),
            Err(PriceLevelError::InvalidFieldValue { ref field, .. }) if field == "trade_id"
        ));
    }

    #[test]
    fn test_trade_list_from_str_depth_limit_bounded_input() {
        let t = trade(1, Id::sequential(5));
        // 127 nested levels inside the list bracket (128 total) is accepted.
        let ok = format!("Trades:[{t};x={}{}]", "[".repeat(127), "]".repeat(127));
        assert!(TradeList::from_str(&ok).is_ok());
        let deep = format!("Trades:[{t};x={}{}]", "[".repeat(128), "]".repeat(128));
        let err = TradeList::from_str(&deep).expect_err("too deep");
        assert!(is_too_deep(&err), "{err:?}");
        // A 1 MiB run of brackets fails fast with the same typed error.
        let huge = format!("Trades:[{}]", "[".repeat(1 << 20));
        assert!(is_too_deep(&TradeList::from_str(&huge).expect_err("huge")));
        let huge_close = format!("Trades:[{}]", "]".repeat(1 << 20));
        assert!(is_too_deep(
            &TradeList::from_str(&huge_close).expect_err("huge")
        ));
    }

    #[test]
    fn test_trade_list_from_str_multibyte_near_delimiters_is_typed() {
        let t = trade(1, Id::sequential(5));
        for text in [
            format!("Trades:[é{t}]"),
            format!("Trades:[{t}日]"),
            format!("Trades:[{t},\u{1F600}]"),
            format!("Trades:[{t},é,{t}]"),
            "Trades:[日]".to_string(),
        ] {
            assert!(TradeList::from_str(&text).is_err(), "{text}");
        }
        // Multibyte text in an ignored field is carried through untouched.
        let ok = format!("Trades:[{t};note=é日\u{1F600}]");
        assert_eq!(TradeList::from_str(&ok).expect("ok").len(), 1);
    }

    #[test]
    fn test_trade_from_str_extra_separators_and_duplicates() {
        let t = trade(3, Id::sequential(8));
        let text = t.to_string();
        assert_eq!(Trade::from_str(&text).expect("round trip"), t);
        // A second `:` is rejected.
        assert_eq!(
            outcome(Trade::from_str(&format!("{text};x=a:b"))),
            err(PriceLevelError::InvalidFormat)
        );
        assert_eq!(
            outcome(Trade::from_str(&text.replacen("Trade", "Trad", 1))),
            err(PriceLevelError::InvalidFormat)
        );
        // A duplicated field keeps its last value.
        let dup = Trade::from_str(&format!("{text};quantity=77")).expect("dup");
        assert_eq!(dup.quantity(), Quantity::new(77));
        // A pair with two `=` is ignored, so the field is missing.
        let two_eq = text.replace("quantity=4", "quantity=4=4");
        assert_eq!(
            outcome(Trade::from_str(&two_eq)),
            err(PriceLevelError::MissingField("quantity".to_string()))
        );
    }

    fn match_result_text() -> (MatchResult, String) {
        let mut r = MatchResult::new(Id::sequential(900), Quantity::new(10));
        r.add_trade(trade(1, Id::sequential(50))).expect("add");
        r.add_trade(trade(2, Id::sequential(51))).expect("add");
        r.add_filled_order_id(Id::sequential(1))
            .expect("capacity available in test");
        r.add_filled_order_id(Id::sequential(2))
            .expect("capacity available in test");
        r.finalize(Quantity::new(5));
        let text = r.to_string();
        (r, text)
    }

    #[test]
    fn test_match_result_from_str_round_trip_and_field_order() {
        let (r, text) = match_result_text();
        assert_eq!(
            MatchResult::from_str(&text)
                .expect("round trip")
                .to_string(),
            r.to_string()
        );
        // Fields in a different order are accepted.
        let (head, trades_and_rest) = text.split_once(";trades=").expect("split");
        let reordered = format!(
            "MatchResult:trades={trades_and_rest};{}",
            head.strip_prefix("MatchResult:").expect("prefix")
        );
        assert_eq!(
            MatchResult::from_str(&reordered)
                .expect("reordered")
                .to_string(),
            r.to_string()
        );
    }

    #[test]
    fn test_match_result_from_str_rejects_unclosed_and_trailing_sections() {
        let (_, text) = match_result_text();
        let unclosed = text.strip_suffix(']').expect("suffix");
        assert_eq!(
            outcome(MatchResult::from_str(unclosed)),
            err(PriceLevelError::InvalidFormat)
        );
        assert_eq!(
            outcome(MatchResult::from_str(&format!("{text}x"))),
            err(PriceLevelError::InvalidFormat)
        );
        let trades_unclosed = text.replacen("Trades:[", "Trades:[[", 1);
        assert!(MatchResult::from_str(&trades_unclosed).is_err());
        assert_eq!(
            outcome(MatchResult::from_str("MatchResult:order_id")),
            err(PriceLevelError::InvalidFormat)
        );
        assert_eq!(
            outcome(MatchResult::from_str("MatchResult:bogus=1")),
            err(PriceLevelError::InvalidFormat)
        );
    }

    #[test]
    fn test_match_result_from_str_depth_limit_bounded_input() {
        let prefix = "MatchResult:order_id=1;remaining_quantity=0;is_complete=true;trades=Trades:[";
        // The pathological input from #174: an unbounded run of `[` after the
        // trades prefix, bounded here to 1 MiB.
        let huge = format!("{prefix}{}", "[".repeat(1 << 20));
        assert!(is_too_deep(
            &MatchResult::from_str(&huge).expect_err("huge")
        ));
        let filled = format!(
            "MatchResult:order_id=1;remaining_quantity=0;is_complete=true;trades=Trades:[];filled_order_ids=[{}",
            "[".repeat(200)
        );
        assert!(is_too_deep(
            &MatchResult::from_str(&filled).expect_err("deep")
        ));
    }

    #[test]
    fn test_match_result_from_str_multibyte_fields_are_typed_errors() {
        let (_, text) = match_result_text();
        for (from, to) in [
            ("order_id=900", "order_id=9é0"),
            ("remaining_quantity=5", "remaining_quantity=日"),
            ("is_complete=false", "is_complete=\u{1F600}"),
            ("filled_order_ids=[1", "filled_order_ids=[é"),
        ] {
            let bad = text.replacen(from, to, 1);
            assert_ne!(bad, text, "{from}");
            assert!(
                matches!(
                    MatchResult::from_str(&bad),
                    Err(PriceLevelError::InvalidFieldValue { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn test_match_result_from_str_still_validates_field_agreement() {
        let (_, text) = match_result_text();
        let lying = text.replacen("is_complete=false", "is_complete=true", 1);
        assert!(matches!(
            MatchResult::from_str(&lying),
            Err(PriceLevelError::InvalidOperation { .. })
        ));
    }
}
