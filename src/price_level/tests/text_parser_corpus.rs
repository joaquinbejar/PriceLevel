//! Accepted / rejected corpus for every text (`FromStr`) parser in the crate
//! (issue #174, #152).
//!
//! The corpus is generated deterministically from the `Display` output of
//! representative values plus hand-written malformed inputs, then mutated
//! (every truncation, every single-character deletion, token insertion next
//! to every structural delimiter, duplicated fields). Each input's outcome —
//! `ok <Display of the parsed value>` or `err <Debug of the error>` — is
//! compared against `fixtures/text_parser_corpus.txt`, which was captured from
//! the parser implementation that predates #174. Intentional contract changes
//! are recorded in that fixture and in the CHANGELOG / migration guide.
//!
//! Regenerate (only after an intentional, documented change) with
//! `PRICELEVEL_REGEN_PARSER_CORPUS=1 cargo test text_parser_corpus`.

#[cfg(test)]
mod tests {
    use crate::execution::{MatchResult, Trade, TradeList};
    use crate::orders::{Hash32, OrderType, OrderUpdate, PegReferenceType, Side, TimeInForce};
    use crate::price_level::entry::OrderBookEntry;
    use crate::price_level::{OrderQueue, PriceLevel, PriceLevelSnapshot, PriceLevelStatistics};
    use crate::utils::{Id, Price, Quantity, TimestampMs};
    use std::fmt::{Debug, Display};
    use std::num::NonZeroU64;
    use std::str::FromStr;
    use std::sync::Arc;

    const FIXTURE: &str = include_str!("fixtures/text_parser_corpus.txt");
    const FIXTURE_PATH: &str = "src/price_level/tests/fixtures/text_parser_corpus.txt";

    /// Tokens inserted next to structural delimiters: every delimiter the
    /// grammars use, brackets / parentheses, and multibyte scalars of 2, 3 and
    /// 4 UTF-8 bytes.
    const TOKENS: &[&str] = &[
        ";",
        "=",
        ":",
        ",",
        "[",
        "]",
        "(",
        ")",
        "-",
        "é",
        "日",
        "\u{1F600}",
    ];

    fn fnv1a(s: &str) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in s.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// Full outcome text: `ok <Display>` or `err <Debug>`.
    fn outcome<T: Display, E: Debug>(r: Result<T, E>) -> String {
        match r {
            Ok(v) => format!("ok {v}"),
            Err(e) => format!("err {e:?}"),
        }
    }

    /// Compact fixture code for an outcome: `O` (accepted) / `E` (rejected)
    /// followed by a 32-bit FNV-1a digest of the full outcome text.
    fn code(outcome: &str) -> String {
        let kind = if outcome.starts_with("ok ") { 'O' } else { 'E' };
        format!("{kind}{:08x}", fnv1a(outcome) as u32)
    }

    type Parser = fn(&str) -> String;

    fn parsers() -> Vec<(&'static str, Parser)> {
        vec![
            ("Id", |s| outcome(Id::from_str(s))),
            ("Hash32", |s| outcome(Hash32::from_str(s))),
            ("TimeInForce", |s| outcome(TimeInForce::from_str(s))),
            ("OrderType", |s| outcome(OrderType::<()>::from_str(s))),
            ("OrderUpdate", |s| outcome(OrderUpdate::from_str(s))),
            ("Trade", |s| outcome(Trade::from_str(s))),
            ("TradeList", |s| outcome(TradeList::from_str(s))),
            ("MatchResult", |s| outcome(MatchResult::from_str(s))),
            ("PriceLevelSnapshot", |s| {
                outcome(PriceLevelSnapshot::from_str(s))
            }),
            ("PriceLevelStatistics", |s| {
                outcome(PriceLevelStatistics::from_str(s))
            }),
            ("OrderBookEntry", |s| outcome(OrderBookEntry::from_str(s))),
            ("OrderQueue", |s| outcome(OrderQueue::from_str(s))),
            ("PriceLevel", |s| outcome(PriceLevel::from_str(s))),
        ]
    }

    fn uuid_id(n: u8) -> Id {
        Id::from_uuid(uuid::Uuid::from_bytes([n; 16]))
    }

    fn ulid_id(n: u128) -> Id {
        Id::from_ulid(ulid::Ulid::from(n))
    }

    fn trade(trade_id: Id, maker: Id, qty: u64) -> Trade {
        Trade::with_timestamp(
            trade_id,
            Id::sequential(900),
            maker,
            Price::new(10_000),
            Quantity::new(qty),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        )
    }

    fn user() -> Hash32 {
        Hash32::new([0xab; 32])
    }

    fn orders() -> Vec<OrderType<()>> {
        vec![
            OrderType::Standard {
                id: Id::sequential(1),
                price: Price::new(10_000),
                quantity: Quantity::new(5),
                side: Side::Sell,
                user_id: user(),
                timestamp: TimestampMs::new(1),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
            OrderType::IcebergOrder {
                id: uuid_id(2),
                price: Price::new(10_000),
                visible_quantity: Quantity::new(1),
                hidden_quantity: Quantity::new(4),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(2),
                time_in_force: TimeInForce::Gtd(99),
                extra_fields: (),
            },
            OrderType::PeggedOrder {
                id: ulid_id(3),
                price: Price::new(10_000),
                quantity: Quantity::new(3),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(3),
                time_in_force: TimeInForce::Day,
                reference_price_offset: -5,
                reference_price_type: PegReferenceType::BestAsk,
                extra_fields: (),
            },
            OrderType::ReserveOrder {
                id: Id::sequential(4),
                price: Price::new(10_000),
                visible_quantity: Quantity::new(2),
                hidden_quantity: Quantity::new(8),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(4),
                time_in_force: TimeInForce::Gtc,
                replenish_threshold: Quantity::new(1),
                replenish_amount: NonZeroU64::new(2),
                auto_replenish: true,
                extra_fields: (),
            },
            OrderType::TrailingStop {
                id: Id::sequential(5),
                price: Price::new(10_000),
                quantity: Quantity::new(7),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(5),
                time_in_force: TimeInForce::Ioc,
                trail_amount: Quantity::new(2),
                last_reference_price: Price::new(10_010),
                extra_fields: (),
            },
        ]
    }

    fn match_result(trades: &[Trade], filled: &[Id], remaining: u64) -> String {
        let mut total = remaining;
        for t in trades {
            total += t.quantity().as_u64();
        }
        let mut r = MatchResult::new(Id::sequential(900), Quantity::new(total));
        for t in trades {
            r.add_trade(*t).expect("add_trade");
        }
        for id in filled {
            r.add_filled_order_id(*id)
                .expect("capacity available in test");
        }
        r.finalize(Quantity::new(remaining));
        r.to_string()
    }

    /// `(parser name, seed inputs)` — valid `Display` output plus
    /// hand-written malformed seeds. Every seed is also mutated.
    fn seeds() -> Vec<(&'static str, Vec<String>)> {
        let t1 = trade(Id::sequential(100), Id::sequential(1), 5);
        let t2 = trade(uuid_id(7), uuid_id(2), 3);
        let t3 = trade(ulid_id(9), ulid_id(3), 2);

        let level = PriceLevel::new(10_000);
        for o in orders() {
            level.add_order(o).expect("add_order");
        }
        let queue = OrderQueue::new();
        for o in orders().into_iter().take(2) {
            queue.try_push(Arc::new(o)).expect("push");
        }

        let hex = Hash32::new([0x5a; 32]).to_string();

        vec![
            (
                "Id",
                vec![
                    Id::sequential(42).to_string(),
                    uuid_id(1).to_string(),
                    ulid_id(77).to_string(),
                    "18446744073709551616".to_string(),
                ],
            ),
            ("Hash32", vec![hex.clone(), format!("+{}", &hex[1..])]),
            (
                "TimeInForce",
                vec![
                    "GTC".into(),
                    "gtd-123".into(),
                    "GTD-1-2".into(),
                    "GTD-".into(),
                    "DAY".into(),
                    "GTD-18446744073709551616".into(),
                ],
            ),
            (
                "OrderType",
                orders().iter().map(ToString::to_string).collect(),
            ),
            (
                "OrderUpdate",
                vec![
                    "UpdatePrice:order_id=1;new_price=5".into(),
                    "UpdateQuantity:order_id=1;new_quantity=5".into(),
                    format!(
                        "UpdatePriceAndQuantity:order_id={};new_price=5;new_quantity=6",
                        uuid_id(3)
                    ),
                    "Cancel:order_id=1".into(),
                    "Replace:order_id=1;price=5;quantity=6;side=BUY".into(),
                ],
            ),
            (
                "Trade",
                vec![t1.to_string(), t2.to_string(), format!("{t3};x=[[(;y=])")],
            ),
            (
                "TradeList",
                vec![
                    TradeList::new().to_string(),
                    TradeList::from_vec(vec![t1]).to_string(),
                    TradeList::from_vec(vec![t1, t2, t3]).to_string(),
                    format!("Trades:[{t1};x=[a],,{t2};y=]"),
                    format!("Trades:[{t1};x=],foo]"),
                    format!("Trades:[{t1};x=[[[,{t2}]"),
                    format!("Trades:[{t1},]"),
                    format!("Trades:[,{t1}]"),
                ],
            ),
            (
                "MatchResult",
                vec![
                    match_result(&[t1, t2], &[Id::sequential(1), uuid_id(2)], 0),
                    match_result(&[t3], &[], 4),
                    match_result(&[], &[], 9),
                    format!(
                        "MatchResult:order_id=900;remaining_quantity=0;is_complete=true;trades=Trades:[{t1};x=[[]]];filled_order_ids=[]"
                    ),
                ],
            ),
            (
                "PriceLevelSnapshot",
                vec![level.snapshot().expect("snapshot").to_string()],
            ),
            ("PriceLevelStatistics", vec![level.stats().to_string()]),
            (
                "OrderBookEntry",
                vec!["OrderBookEntry:price=1000;visible_quantity=3;index=5".into()],
            ),
            (
                "OrderQueue",
                vec![queue.to_string(), "OrderQueue:orders=[]".into()],
            ),
            (
                "PriceLevel",
                vec![
                    level.to_string(),
                    PriceLevel::new(7).to_string(),
                    "PriceLevel:price=1orders=[]0".into(),
                    format!("PriceLevel:price=1;orders=[];orders={}", orders()[0]),
                    format!(
                        "PriceLevel:price=1;orders=[({},{}]",
                        orders()[0],
                        orders()[4]
                    ),
                    format!(
                        "PriceLevel:price=1;orders=[){},{}]",
                        orders()[0],
                        orders()[4]
                    ),
                ],
            ),
        ]
    }

    fn mutate(seed: &str) -> Vec<String> {
        let mut out = vec![seed.to_string(), String::new()];
        // Interesting positions: the start, the end, and both sides of every
        // ASCII punctuation delimiter or multibyte scalar.
        let mut positions = vec![0, seed.len()];
        for (i, c) in seed.char_indices() {
            if c.is_ascii_punctuation() || !c.is_ascii() {
                positions.push(i);
                positions.push(i + c.len_utf8());
            }
        }
        positions.sort_unstable();
        positions.dedup();
        for &p in &positions {
            // Truncation.
            out.push(seed[..p].to_string());
            // Deletion of the character starting here.
            if let Some(c) = seed[p..].chars().next() {
                let mut s = seed.to_string();
                s.replace_range(p..p + c.len_utf8(), "");
                out.push(s);
            }
            // Token insertion.
            for t in TOKENS {
                let mut s = seed.to_string();
                s.insert_str(p, t);
                out.push(s);
            }
        }
        // Duplicate every `;`-separated segment (duplicate-field rules).
        let segments: Vec<&str> = seed.split(';').collect();
        for i in 0..segments.len() {
            let mut v = segments.clone();
            v.insert(i + 1, segments[i]);
            out.push(v.join(";"));
        }
        out
    }

    /// `(parser, input, full outcome)` for every corpus entry, in a fixed order.
    fn corpus() -> Vec<(&'static str, String, String)> {
        let parsers = parsers();
        let mut entries = Vec::new();
        for (name, seeds) in seeds() {
            let parse = parsers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, p)| *p)
                .expect("parser registered");
            for seed in &seeds {
                for input in mutate(seed) {
                    let result = parse(&input);
                    entries.push((name, input, result));
                }
            }
        }
        entries
    }

    fn render(entries: &[(&'static str, String, String)]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current = "";
        for (name, _, result) in entries {
            if *name != current {
                lines.push(format!("## {name}"));
                current = name;
            }
            lines.push(code(result));
        }
        lines
    }

    #[test]
    fn test_text_parser_corpus_matches_recorded_outcomes() {
        let entries = corpus();
        if let Some(dump) = std::env::var_os("PRICELEVEL_DUMP_PARSER_CORPUS") {
            let body: Vec<String> = entries
                .iter()
                .map(|(name, input, result)| format!("{name}\t{input:?}\t{result:?}"))
                .collect();
            std::fs::write(dump, body.join("\n")).expect("dump corpus");
        }
        if std::env::var_os("PRICELEVEL_REGEN_PARSER_CORPUS").is_some() {
            let mut body = render(&entries).join("\n");
            body.push('\n');
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_PATH);
            std::fs::write(path, body).expect("write corpus fixture");
            return;
        }
        let recorded: Vec<&str> = FIXTURE.lines().filter(|l| !l.starts_with("##")).collect();
        assert_eq!(recorded.len(), entries.len(), "corpus size changed");
        let mismatches: Vec<String> = recorded
            .iter()
            .zip(&entries)
            .filter(|(a, (_, _, result))| **a != code(result))
            .map(|(a, (name, input, result))| {
                format!("{name} input {input:?}\n  recorded: {a}\n  actual: {result}")
            })
            .collect();
        assert!(
            mismatches.is_empty(),
            "{} corpus mismatches, first 40:\n{}",
            mismatches.len(),
            mismatches
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
