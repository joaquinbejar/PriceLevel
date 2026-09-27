//! Text-parser regression tests for `PriceLevel`, `OrderQueue`,
//! `PriceLevelSnapshot`, `PriceLevelStatistics` and `OrderBookEntry`
//! (issue #174): checked access, bounded nesting and typed errors.

#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::orders::{Hash32, OrderType, Side, TimeInForce};
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::{OrderQueue, PriceLevel, PriceLevelSnapshot, PriceLevelStatistics};
    use crate::utils::{Id, Price, Quantity, TimestampMs};
    use std::str::FromStr;
    use std::sync::Arc;

    fn order(id: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::sequential(id),
            price: Price::new(100),
            quantity: Quantity::new(id + 1),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(id),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn parse_error_message(r: Result<PriceLevel, PriceLevelError>) -> String {
        match r {
            Err(PriceLevelError::ParseError { message }) => message,
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn test_price_level_from_str_round_trip_preserves_fifo() {
        let level = PriceLevel::new(100);
        for id in 1..=5 {
            level.add_order(order(id)).expect("add");
        }
        let parsed = PriceLevel::from_str(&level.to_string()).expect("round trip");
        assert_eq!(parsed.to_string(), level.to_string());
        let ids: Vec<Id> = parsed
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(ids, (1..=5).map(Id::sequential).collect::<Vec<_>>());
    }

    #[test]
    fn test_price_level_from_str_empty_segments_and_trailing_comma() {
        let o = order(1);
        // A trailing `,` is skipped; an inner empty segment is rejected.
        let level = PriceLevel::from_str(&format!("PriceLevel:price=100;orders=[{o},]"))
            .expect("trailing comma");
        assert_eq!(level.order_count(), 1);
        let msg = parse_error_message(PriceLevel::from_str(&format!(
            "PriceLevel:price=100;orders=[{o},,{o}]"
        )));
        assert!(msg.starts_with("Order parse error"), "{msg}");
    }

    #[test]
    fn test_price_level_from_str_rejects_unbalanced_delimiters() {
        let o = order(1);
        let unclosed = parse_error_message(PriceLevel::from_str(&format!(
            "PriceLevel:price=100;orders=[{o};x=(]"
        )));
        assert!(unclosed.contains("unclosed delimiter"), "{unclosed}");
        let unmatched = parse_error_message(PriceLevel::from_str(&format!(
            "PriceLevel:price=100;orders=[{o};x=)]"
        )));
        assert!(unmatched.contains("unmatched closing"), "{unmatched}");
        // Balanced parentheses in an ignored field keep the old acceptance,
        // including the `,` they protect.
        let ok = PriceLevel::from_str(&format!("PriceLevel:price=100;orders=[{o};x=(a,b)]"))
            .expect("balanced");
        assert_eq!(ok.order_count(), 1);
        let bracket = parse_error_message(PriceLevel::from_str(
            "PriceLevel:price=100;orders=[Standard:id=1",
        ));
        assert!(bracket.contains("unclosed orders bracket"), "{bracket}");
    }

    #[test]
    fn test_price_level_from_str_depth_limit_bounded_input() {
        let o = order(1);
        let ok = format!(
            "PriceLevel:price=100;orders=[{o};x={}{}]",
            "(".repeat(127),
            ")".repeat(127)
        );
        assert_eq!(
            PriceLevel::from_str(&ok).expect("127 deep").order_count(),
            1
        );
        let deep = format!(
            "PriceLevel:price=100;orders=[{o};x={}{}]",
            "(".repeat(128),
            ")".repeat(128)
        );
        let msg = parse_error_message(PriceLevel::from_str(&deep));
        assert!(msg.contains("nesting depth"), "{msg}");
        let huge = format!("PriceLevel:price=100;orders=[{}]", "(".repeat(1 << 20));
        let msg = parse_error_message(PriceLevel::from_str(&huge));
        assert!(msg.contains("nesting depth"), "{msg}");
    }

    #[test]
    fn test_price_level_from_str_multibyte_near_delimiters_is_typed() {
        for text in [
            "PriceLevel:price=1é",
            "PriceLevel:é",
            "PriceLevel:price=1;orders=[日]",
            "PriceLevel:price=1;orders=[\u{1F600},]",
            "PriceLevé:price=1",
        ] {
            assert!(PriceLevel::from_str(text).is_err(), "{text}");
        }
        assert_eq!(
            PriceLevel::from_str("PriceLevel:note=é;price=7;x=日")
                .expect("ignored multibyte")
                .price(),
            7
        );
    }

    #[test]
    fn test_order_queue_from_str_round_trip_and_envelope() {
        let queue = OrderQueue::new();
        for id in 1..=3 {
            queue.try_push(Arc::new(order(id))).expect("push");
        }
        let parsed = OrderQueue::from_str(&queue.to_string()).expect("round trip");
        assert_eq!(parsed.to_string(), queue.to_string());
        assert!(
            OrderQueue::from_str("OrderQueue:orders=[]")
                .expect("empty")
                .is_empty()
        );
        for text in [
            "",
            "OrderQueue:orders=[",
            "OrderQueue:orders=]",
            "OrderQueue:orders=[é",
        ] {
            assert!(
                matches!(
                    OrderQueue::from_str(text),
                    Err(PriceLevelError::ParseError { .. })
                ),
                "{text:?}"
            );
        }
    }

    mod proptests {
        use crate::execution::{MatchResult, Trade, TradeList};
        use crate::orders::{Hash32, OrderType, OrderUpdate, TimeInForce};
        use crate::price_level::entry::OrderBookEntry;
        use crate::price_level::{
            OrderQueue, PriceLevel, PriceLevelSnapshot, PriceLevelStatistics,
        };
        use proptest::prelude::*;
        use std::str::FromStr;

        /// Parses `s` with every text parser; none may panic.
        fn parse_all(s: &str) {
            let _ = Hash32::from_str(s);
            let _ = TimeInForce::from_str(s);
            let _ = OrderType::<()>::from_str(s);
            let _ = OrderUpdate::from_str(s);
            let _ = Trade::from_str(s);
            let _ = TradeList::from_str(s);
            let _ = MatchResult::from_str(s);
            let _ = PriceLevelSnapshot::from_str(s);
            let _ = PriceLevelStatistics::from_str(s);
            let _ = OrderBookEntry::from_str(s);
            let _ = OrderQueue::from_str(s);
            let _ = PriceLevel::from_str(s);
        }

        /// Delimiter-dense fragments mixed with multibyte scalars.
        fn fragment() -> impl Strategy<Value = String> {
            prop_oneof![
                Just("Trades:[".to_string()),
                Just("MatchResult:".to_string()),
                Just("PriceLevel:price=1;orders=[".to_string()),
                Just("OrderQueue:orders=[".to_string()),
                Just("trades=".to_string()),
                Just("filled_order_ids=[".to_string()),
                Just("Trade:trade_id=1;taker_order_id=2;maker_order_id=3;price=4;quantity=5;taker_side=BUY;timestamp=6".to_string()),
                Just("GTD-".to_string()),
                prop::sample::select(vec![";", "=", ":", ",", "[", "]", "(", ")", "-"])
                    .prop_map(str::to_string),
                any::<char>().prop_map(|c| c.to_string()),
                "\\PC{0,4}",
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

            #[test]
            fn prop_text_parsers_never_panic_on_structured_noise(
                parts in prop::collection::vec(fragment(), 0..24)
            ) {
                parse_all(&parts.concat());
            }

            #[test]
            fn prop_text_parsers_never_panic_on_arbitrary_unicode(s in "\\PC{0,64}") {
                parse_all(&s);
            }
        }
    }

    #[test]
    fn test_snapshot_statistics_entry_from_str_separators_and_duplicates() {
        let snap = "PriceLevelSnapshot:price=10;visible_quantity=1;hidden_quantity=2;order_count=3";
        assert_eq!(
            PriceLevelSnapshot::from_str(snap)
                .expect("snapshot")
                .to_string(),
            snap
        );
        let dup = PriceLevelSnapshot::from_str(&format!("{snap};price=11")).expect("dup");
        assert_eq!(dup.price(), Price::new(11));
        assert!(matches!(
            PriceLevelSnapshot::from_str(&format!("{snap}:x")),
            Err(PriceLevelError::InvalidFormat)
        ));
        assert!(matches!(
            PriceLevelSnapshot::from_str("PriceLevelSnapshot:price=é"),
            Err(PriceLevelError::InvalidFieldValue { .. })
        ));

        let stats = PriceLevelStatistics::new().to_string();
        assert_eq!(
            PriceLevelStatistics::from_str(&stats)
                .expect("stats")
                .to_string(),
            stats
        );
        assert!(matches!(
            PriceLevelStatistics::from_str(&stats.replacen(':', "::", 1)),
            Err(PriceLevelError::InvalidFormat)
        ));

        let entry = "OrderBookEntry:price=1000;visible_quantity=0;index=5";
        assert_eq!(
            OrderBookEntry::from_str(entry).expect("entry").to_string(),
            entry
        );
        assert!(matches!(
            OrderBookEntry::from_str("OrderBookEntry:price=1000"),
            Err(PriceLevelError::MissingField(_))
        ));
        assert!(matches!(
            OrderBookEntry::from_str("OrderBookEntry:price=1000;index=5=5"),
            Err(PriceLevelError::MissingField(_))
        ));
    }
}
