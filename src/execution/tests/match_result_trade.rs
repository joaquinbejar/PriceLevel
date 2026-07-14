#[cfg(test)]
mod tests {
    use crate::execution::list::TradeList;
    use crate::execution::match_result::{MatchOutcome, MatchResult};
    use crate::execution::trade::Trade;
    use crate::orders::{Id, Side};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::str::FromStr;
    use uuid::Uuid;

    fn parse_uuid(input: &str) -> Uuid {
        match Uuid::parse_str(input) {
            Ok(value) => value,
            Err(error) => panic!("failed to parse uuid: {error}"),
        }
    }

    fn sample_trade(quantity: u64) -> Trade {
        sample_trade_with_maker(20, quantity)
    }

    fn sample_trade_with_maker(maker_id: u64, quantity: u64) -> Trade {
        Trade::with_timestamp(
            Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            Id::from_u64(10),
            Id::from_u64(maker_id),
            Price::new(1_000),
            Quantity::new(quantity),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        )
    }

    #[test]
    fn add_trade_updates_remaining_and_trades() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(result.add_trade(sample_trade(25)).is_ok());

        assert_eq!(result.remaining_quantity().as_u64(), 75);
        assert_eq!(result.trades().len(), 1);
        assert!(!result.is_complete());
    }

    #[test]
    fn display_and_parse_use_trades_field() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(result.add_trade(sample_trade(40)).is_ok());

        let rendered = result.to_string();
        assert!(rendered.contains(";trades=Trades:[Trade:"));

        let parsed = match MatchResult::from_str(&rendered) {
            Ok(value) => value,
            Err(error) => panic!("failed to parse match result: {error:?}"),
        };

        assert_eq!(parsed.trades().len(), 1);
        assert_eq!(parsed.remaining_quantity().as_u64(), 60);
    }

    #[test]
    fn add_trade_keeps_outcome_in_sync() {
        // No trades yet -> the default benign classification.
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert_eq!(result.outcome(), MatchOutcome::NotFilled);

        // Partial fill.
        assert!(result.add_trade(sample_trade(40)).is_ok());
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);
        assert!(!result.was_killed());
        assert!(!result.was_rejected());

        // Complete fill.
        assert!(result.add_trade(sample_trade(60)).is_ok());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert!(result.is_complete());
    }

    #[test]
    fn outcome_survives_serde_json_roundtrip() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(result.add_trade(sample_trade(40)).is_ok());

        let json = serde_json::to_string(&result).expect("serialize match result");
        let parsed: MatchResult = serde_json::from_str(&json).expect("deserialize match result");

        assert_eq!(parsed.outcome(), MatchOutcome::PartiallyFilled);
        assert_eq!(parsed.remaining_quantity().as_u64(), 60);
        assert_eq!(parsed.trades().len(), 1);
    }

    #[test]
    fn outcome_defaults_when_absent_from_json() {
        // A JSON payload written before the `outcome` field existed must still
        // deserialize (the field is `#[serde(default)]`). Build a current JSON,
        // then strip the `outcome` key to emulate the legacy shape.
        let result = MatchResult::new(Id::from_u64(10), Quantity::new(70));
        let mut value: serde_json::Value =
            serde_json::to_value(&result).expect("serialize match result");
        value
            .as_object_mut()
            .expect("object")
            .remove("outcome")
            .expect("current payload carries an outcome field");
        let legacy = serde_json::to_string(&value).expect("re-serialize legacy payload");

        let parsed: MatchResult =
            serde_json::from_str(&legacy).expect("legacy match result must deserialize");
        assert_eq!(parsed.outcome(), MatchOutcome::NotFilled);
        assert_eq!(parsed.remaining_quantity().as_u64(), 70);
    }

    #[test]
    fn from_str_rejects_old_transactions_field() {
        let old_payload = "MatchResult:order_id=1;remaining_quantity=1;is_complete=false;transactions=Transactions:[];filled_order_ids=[]";
        let parsed = MatchResult::from_str(old_payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn add_trade_rejects_underflow() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(10));
        let error = result.add_trade(sample_trade(11));
        assert!(error.is_err());
        assert_eq!(result.remaining_quantity().as_u64(), 10);
        assert_eq!(result.trades().len(), 0);
    }

    #[test]
    fn executed_value_rejects_overflow() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(4));

        let trade = Trade::with_timestamp(
            Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            Id::from_u64(10),
            Id::from_u64(20),
            Price::new(u128::MAX),
            Quantity::new(2),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        );

        assert!(result.add_trade(trade).is_ok());
        assert!(result.executed_value().is_err());
    }

    // ----- average_price edge cases -----
    //
    // `average_price()` returns `Result<Option<f64>, PriceLevelError>` and
    // computes `executed_value as f64 / executed_quantity as f64`, guarding
    // the zero-quantity case so it never divides by zero. These tests pin the
    // zero-quantity, precision-loss, and never-NaN/Inf behavior. The
    // happy-path (exact) average is covered by
    // `test_average_price_exact_small_values_is_precise` below.

    /// No trades have been added, so executed quantity is zero and there is no
    /// average price to report: `average_price()` must return `Ok(None)`
    /// (never an error, never a division by zero, never NaN).
    #[test]
    fn test_average_price_zero_executed_quantity_returns_none() {
        let result = MatchResult::new(Id::from_u64(10), Quantity::new(100));

        match result.average_price() {
            Ok(None) => {}
            other => panic!("expected Ok(None) for zero executed quantity, got {other:?}"),
        }
    }

    /// A single trade whose `price` and `quantity` are exactly representable in
    /// `f64` yields an exact average. Pins that the common case is precise and
    /// never NaN/Inf.
    #[test]
    fn test_average_price_exact_small_values_is_precise() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));

        // price 1000, quantity 30 -> value 30_000, avg 1000.0 exactly.
        assert!(result.add_trade(sample_trade(30)).is_ok());

        match result.average_price() {
            Ok(Some(avg)) => {
                assert!(avg.is_finite(), "average price must be finite");
                assert!((avg - 1000.0_f64).abs() < f64::EPSILON);
            }
            other => panic!("expected Ok(Some(1000.0)), got {other:?}"),
        }
    }

    /// Large value/quantity case where `f64`'s 53-bit integer mantissa cannot
    /// represent the exact integer average.
    ///
    /// A single trade with `price = 2^53 + 1` and `quantity = 1` has an exact
    /// integer average of `9_007_199_254_740_993`, but `average_price()`
    /// returns `9_007_199_254_740_992.0` because `2^53 + 1` is not
    /// representable as an `f64` (it rounds to `2^53`). This is a documented,
    /// intrinsic limitation of using `f64` for the analytics average — the
    /// matching path itself keeps exact integer `Price`/`Quantity` math and
    /// only `average_price` drops to `f64`.
    ///
    /// We therefore do NOT assert exact equality with the integer average.
    /// Instead we assert the result is finite and within a small absolute
    /// tolerance (well under 1 ULP-worth of drift at this magnitude, here a
    /// fixed difference of exactly 1).
    #[test]
    fn test_average_price_large_values_loses_f64_precision_but_stays_finite() {
        const PRICE: u128 = (1_u128 << 53) + 1; // 9_007_199_254_740_993
        const EXACT_INT_AVG: u128 = PRICE; // quantity == 1, so average == price

        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(1));
        let trade = Trade::with_timestamp(
            Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            Id::from_u64(10),
            Id::from_u64(20),
            Price::new(PRICE),
            Quantity::new(1),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        );
        assert!(result.add_trade(trade).is_ok());

        match result.average_price() {
            Ok(Some(avg)) => {
                // Never NaN/Inf for a valid, in-range input.
                assert!(avg.is_finite(), "average price must be finite");
                assert!(!avg.is_nan(), "average price must not be NaN");
                assert!(!avg.is_infinite(), "average price must not be Inf");

                // Document the observed drift: the exact integer average is
                // 2^53 + 1, but f64 has only a 53-bit mantissa and the division
                // rounds it down to 2^53.
                assert_eq!(
                    avg, 9_007_199_254_740_992.0_f64,
                    "observed f64 average rounds 2^53 + 1 down to 2^53"
                );

                // Show the loss in INTEGER space. Comparing `EXACT_INT_AVG as
                // f64` would round the same way and hide the drift (abs_err
                // would be 0), so convert the f64 result back to an integer and
                // prove it is short of the exact integer average by exactly 1.
                let avg_as_int = avg as u128;
                assert_eq!(
                    EXACT_INT_AVG - avg_as_int,
                    1,
                    "f64 average loses exactly 1 vs the exact integer average (2^53 + 1)"
                );
            }
            other => panic!("expected Ok(Some(_)), got {other:?}"),
        }
    }

    /// For valid inputs producing trades, `average_price()` must never yield a
    /// NaN or infinite value. Sweeps a handful of (price, quantity) inputs and
    /// asserts finiteness on each.
    #[test]
    fn test_average_price_valid_inputs_never_nan_or_inf() {
        let cases: [(u128, u64); 4] = [
            (1, 1),
            (1_000, 7),
            (u64::MAX as u128, 3),
            ((1_u128 << 60) + 5, 11),
        ];

        for (idx, (price, quantity)) in cases.into_iter().enumerate() {
            let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(quantity));
            let trade = Trade::with_timestamp(
                Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
                Id::from_u64(10),
                Id::from_u64(20),
                Price::new(price),
                Quantity::new(quantity),
                Side::Buy,
                TimestampMs::new(1_616_823_000_000),
            );
            assert!(result.add_trade(trade).is_ok());

            match result.average_price() {
                Ok(Some(avg)) => {
                    assert!(
                        avg.is_finite(),
                        "case {idx}: average must be finite (price={price}, qty={quantity})"
                    );
                }
                other => panic!("case {idx}: expected Ok(Some(_)), got {other:?}"),
            }
        }
    }

    // ----- issue #114: malformed UTF-8 must parse to Err, never panic -----
    //
    // `MatchResult::from_str` accepts untrusted text and must return a
    // `Result`, so a multibyte Unicode scalar sitting before the next ASCII
    // delimiter must not make an internal byte offset land inside the scalar
    // and panic on a non-char-boundary slice. Each case below simply calls
    // `from_str` and asserts `Err`: a plain call is enough to prove no panic,
    // because a panic would unwind the test rather than reach the assertion.

    /// Every field position that the byte scanners walk — a plain field value,
    /// a value that runs to the end of the string with no trailing `;`, a
    /// field name, the `trades` bracket body, and the `filled_order_ids`
    /// bracket body — must reject 2-, 3-, and 4-byte scalars deterministically.
    #[test]
    fn from_str_rejects_multibyte_without_panicking() {
        // 'é' = 2 bytes, '→' = 3 bytes, '😀' = 4 bytes: cover every UTF-8 width
        // and a scalar straddling each delimiter position.
        let malformed = [
            // Multibyte in a field value immediately before the ';' delimiter.
            "MatchResult:order_id=é;remaining_quantity=60;is_complete=false;\
             trades=Trades:[];filled_order_ids=[]",
            "MatchResult:order_id=→;remaining_quantity=60;is_complete=false;\
             trades=Trades:[];filled_order_ids=[]",
            // Multibyte in the LAST-scanned value that runs to end-of-string
            // (no trailing ';'): exercises the find-to-end branch.
            "MatchResult:order_id=10😀",
            // Multibyte in a field NAME (before the '=').
            "MatchResult:é=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[];filled_order_ids=[]",
            // Multibyte inside the trades bracket body.
            "MatchResult:order_id=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[é];filled_order_ids=[]",
            "MatchResult:order_id=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[😀];filled_order_ids=[]",
            // Multibyte inside the filled_order_ids bracket list.
            "MatchResult:order_id=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[];filled_order_ids=[é]",
            "MatchResult:order_id=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[];filled_order_ids=[1,→,2]",
            // Multibyte right where the '=' after a field name is expected.
            "MatchResult:order_idé=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[];filled_order_ids=[]",
        ];

        for input in malformed {
            let parsed = MatchResult::from_str(input);
            assert!(
                parsed.is_err(),
                "malformed multibyte input must parse to Err, got Ok for {input:?}"
            );
        }
    }

    /// A multibyte scalar that lands exactly on the byte offset where the
    /// scanner probes for the closing `]` of each bracket must not panic — the
    /// bracket depth stays unbalanced and the parse fails cleanly.
    #[test]
    fn from_str_rejects_unterminated_multibyte_bracket() {
        let inputs = [
            // trades bracket opened, then a multibyte scalar, then end.
            "MatchResult:order_id=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[é",
            // filled_order_ids bracket opened, then a multibyte scalar, then end.
            "MatchResult:order_id=10;remaining_quantity=60;is_complete=false;\
             trades=Trades:[];filled_order_ids=[😀",
        ];
        for input in inputs {
            assert!(
                MatchResult::from_str(input).is_err(),
                "unterminated multibyte bracket must parse to Err for {input:?}"
            );
        }
    }

    /// Canonical ASCII output from `Display` — including a populated
    /// `filled_order_ids` list and multiple trades — must keep round-tripping
    /// unchanged through `FromStr`.
    #[test]
    fn display_round_trips_with_filled_ids_and_trades() {
        // Two makers (20 and 21) each fully consumed, so each is both a trade
        // maker and a filled id — the shape the engine actually produces.
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(result.add_trade(sample_trade_with_maker(20, 30)).is_ok());
        assert!(result.add_trade(sample_trade_with_maker(21, 20)).is_ok());
        result.add_filled_order_id(Id::from_u64(20));
        result.add_filled_order_id(Id::from_u64(21));

        let rendered = result.to_string();
        let parsed = match MatchResult::from_str(&rendered) {
            Ok(value) => value,
            Err(error) => panic!("valid canonical output must round-trip: {error:?}"),
        };

        assert_eq!(parsed.order_id(), Id::from_u64(10));
        assert_eq!(parsed.remaining_quantity().as_u64(), 50);
        assert!(!parsed.is_complete());
        assert_eq!(parsed.trades().len(), 2);
        assert_eq!(
            parsed.filled_order_ids(),
            &[Id::from_u64(20), Id::from_u64(21)]
        );
        // Rendering the parsed result reproduces the exact canonical text.
        assert_eq!(parsed.to_string(), rendered);
    }

    // ----- issue #116: invariants enforced during decoding -----
    //
    // Private fields protect Rust-API construction, but `Deserialize` and
    // `FromStr` reconstruct fields directly. Both now route through
    // `MatchResult::validated`, so a self-contradictory payload is rejected
    // (serde error / `PriceLevelError`) instead of minting an impossible value.
    // Every payload a public-API-built result can produce still decodes.

    /// A valid `MatchResult` carrying trades and filled ids round-trips through
    /// serde JSON with all fields (including `outcome`) intact.
    #[test]
    fn valid_result_with_filled_ids_round_trips_serde_json() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(result.add_trade(sample_trade_with_maker(20, 40)).is_ok());
        result.add_filled_order_id(Id::from_u64(20));

        let json = serde_json::to_string(&result).expect("serialize");
        let parsed: MatchResult = serde_json::from_str(&json).expect("valid payload must decode");

        assert_eq!(parsed.order_id(), Id::from_u64(10));
        assert_eq!(parsed.remaining_quantity().as_u64(), 60);
        assert!(!parsed.is_complete());
        assert_eq!(parsed.outcome(), MatchOutcome::PartiallyFilled);
        assert_eq!(parsed.trades().len(), 1);
        assert_eq!(parsed.filled_order_ids(), &[Id::from_u64(20)]);
    }

    /// A genuinely killed / rejected result (no trades, no filled ids) still
    /// decodes — the outcome invariant only forbids a *populated* kill/reject.
    #[test]
    fn valid_killed_and_rejected_round_trip_serde_json() {
        for build in [
            MatchResult::mark_killed as fn(&mut MatchResult, u64),
            MatchResult::mark_rejected,
        ] {
            let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
            build(&mut result, 100);

            let json = serde_json::to_string(&result).expect("serialize");
            let parsed: MatchResult =
                serde_json::from_str(&json).expect("empty kill/reject must decode");

            assert_eq!(parsed.remaining_quantity().as_u64(), 100);
            assert!(!parsed.is_complete());
            assert!(parsed.trades().is_empty());
            assert!(parsed.filled_order_ids().is_empty());
            assert!(parsed.was_killed() || parsed.was_rejected());
        }
    }

    /// Helper: serialize a valid result, mutate the JSON object, and return the
    /// mutated JSON string so a negative test can feed it back to `from_str`.
    fn mutated_json(base: &MatchResult, mutate: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut value = serde_json::to_value(base).expect("serialize base");
        mutate(&mut value);
        serde_json::to_string(&value).expect("re-serialize mutated payload")
    }

    /// Invariant 1: `is_complete == true` with a positive remainder is rejected.
    #[test]
    fn deserialize_rejects_complete_with_remainder() {
        // new(0) is complete with remainder 0; force a positive remainder.
        let base = MatchResult::new(Id::from_u64(10), Quantity::new(0));
        let json = mutated_json(&base, |v| {
            v["remaining_quantity"] = serde_json::json!(5);
        });
        assert!(serde_json::from_str::<MatchResult>(&json).is_err());
    }

    /// Invariant 3: a `Rejected` / `Killed` outcome may not carry trades.
    #[test]
    fn deserialize_rejects_trades_on_killed_or_rejected() {
        let mut base = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(base.add_trade(sample_trade_with_maker(20, 40)).is_ok());

        for outcome in ["killed", "rejected"] {
            let json = mutated_json(&base, |v| {
                v["outcome"] = serde_json::json!(outcome);
            });
            assert!(
                serde_json::from_str::<MatchResult>(&json).is_err(),
                "{outcome} with trades must be rejected"
            );
        }
    }

    /// Invariant 3 (filled ids variant): a `Killed` outcome may not carry
    /// filled order ids either.
    #[test]
    fn deserialize_rejects_filled_ids_on_killed() {
        let mut base = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        base.mark_killed(100);
        let json = mutated_json(&base, |v| {
            // Killed carries no trades; inject a filled id with no backing trade.
            v["filled_order_ids"] = serde_json::json!(["20"]);
        });
        assert!(serde_json::from_str::<MatchResult>(&json).is_err());
    }

    /// Invariant 2: trade quantities whose sum overflows `u64` are rejected.
    #[test]
    fn deserialize_rejects_executed_quantity_sum_overflow() {
        let mut base = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(base.add_trade(sample_trade_with_maker(20, 40)).is_ok());
        assert!(base.add_trade(sample_trade_with_maker(21, 20)).is_ok());
        // Two trades each at u64::MAX overflow the checked sum.
        let json = mutated_json(&base, |v| {
            v["trades"]["trades"][0]["quantity"] = serde_json::json!(u64::MAX);
            v["trades"]["trades"][1]["quantity"] = serde_json::json!(u64::MAX);
        });
        assert!(serde_json::from_str::<MatchResult>(&json).is_err());
    }

    /// Invariant 2 (implied initial): executed quantity + remaining overflowing
    /// `u64` (an impossible initial taker quantity) is rejected.
    #[test]
    fn deserialize_rejects_executed_plus_remaining_overflow() {
        let mut base = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(base.add_trade(sample_trade_with_maker(20, 40)).is_ok());
        let json = mutated_json(&base, |v| {
            v["trades"]["trades"][0]["quantity"] = serde_json::json!(u64::MAX);
            // Keep is_complete consistent with a positive remainder.
            v["remaining_quantity"] = serde_json::json!(5);
            v["is_complete"] = serde_json::json!(false);
        });
        assert!(serde_json::from_str::<MatchResult>(&json).is_err());
    }

    /// Invariant 4: a filled order id with no backing trade maker is rejected.
    #[test]
    fn deserialize_rejects_filled_id_absent_from_trades() {
        let mut base = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        assert!(base.add_trade(sample_trade_with_maker(20, 40)).is_ok());
        base.add_filled_order_id(Id::from_u64(20));
        let json = mutated_json(&base, |v| {
            // 99 never traded.
            v["filled_order_ids"] = serde_json::json!(["20", "99"]);
        });
        assert!(serde_json::from_str::<MatchResult>(&json).is_err());
    }

    // ----- issue #116: the same invariants via FromStr -----

    /// `FromStr` rejects a structurally-valid text whose `is_complete` claim
    /// contradicts the remainder.
    #[test]
    fn from_str_rejects_complete_with_remainder() {
        let text = "MatchResult:order_id=10;remaining_quantity=5;is_complete=true;\
                    trades=Trades:[];filled_order_ids=[]";
        assert!(MatchResult::from_str(text).is_err());
    }

    /// `FromStr` rejects text whose trade quantities sum past `u64::MAX`.
    #[test]
    fn from_str_rejects_executed_quantity_sum_overflow() {
        let trades = TradeList::from_vec(vec![
            sample_trade_with_maker(20, u64::MAX),
            sample_trade_with_maker(21, u64::MAX),
        ]);
        let text = format!(
            "MatchResult:order_id=10;remaining_quantity=1;is_complete=false;\
             trades={trades};filled_order_ids=[]"
        );
        assert!(MatchResult::from_str(&text).is_err());
    }

    /// `FromStr` rejects text listing a filled id that is not a trade maker.
    #[test]
    fn from_str_rejects_filled_id_absent_from_trades() {
        let trades = TradeList::from_vec(vec![sample_trade_with_maker(20, 40)]);
        let text = format!(
            "MatchResult:order_id=10;remaining_quantity=60;is_complete=false;\
             trades={trades};filled_order_ids=[20,99]"
        );
        assert!(MatchResult::from_str(&text).is_err());
    }

    /// Structural tightening (#114 review follow-up): trailing content after the
    /// `filled_order_ids` closing `]` is now rejected, symmetric with the
    /// `trades` branch. Previously the `filled_order_ids` branch silently
    /// tolerated a missing `;` separator and parsed the trailing text as another
    /// field.
    #[test]
    fn from_str_rejects_trailing_content_after_filled_ids() {
        // A second `order_id=...` glued directly to the closing `]` with no `;`.
        let text = "MatchResult:order_id=10;remaining_quantity=0;is_complete=true;\
                    trades=Trades:[];filled_order_ids=[]order_id=10";
        assert!(
            MatchResult::from_str(text).is_err(),
            "trailing content after filled_order_ids ']' must be rejected"
        );
        // The canonical form (nothing after the ']') still parses.
        let ok = "MatchResult:order_id=10;remaining_quantity=0;is_complete=true;\
                  trades=Trades:[];filled_order_ids=[]";
        assert!(MatchResult::from_str(ok).is_ok());
    }

    // ----- property: from_str never panics on arbitrary UTF-8 -----
    //
    // A co-located `proptest` block (the `tests/proptest/` harness is reserved
    // for the nine matching invariants and drives `PriceLevel`, not the
    // parser, so a parser-robustness property does not fit there). `proptest`
    // reports any panic in the closure as a failing, shrinking case, so the
    // bodies just call `from_str` and drop the result.
    use proptest::prelude::*;

    /// Concatenate the format's structural delimiters with arbitrary Unicode
    /// scalars so multibyte characters land adjacent to (and straddling) every
    /// delimiter the byte scanners probe — the worst case for boundary safety.
    fn structural_fuzz() -> impl Strategy<Value = String> {
        let token = prop_oneof![
            Just("=".to_string()),
            Just(";".to_string()),
            Just("[".to_string()),
            Just("]".to_string()),
            Just("Trades:".to_string()),
            Just("order_id".to_string()),
            Just("remaining_quantity".to_string()),
            Just("is_complete".to_string()),
            Just("trades".to_string()),
            Just("filled_order_ids".to_string()),
            any::<char>().prop_map(|c| c.to_string()),
        ];
        prop::collection::vec(token, 0..24)
            .prop_map(|parts| format!("MatchResult:{}", parts.concat()))
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

        /// Arbitrary UTF-8 strings (with the required prefix and bare) never
        /// panic; they either parse or return `Err`.
        #[test]
        fn from_str_never_panics_on_arbitrary_utf8(suffix in any::<String>()) {
            let with_prefix = format!("MatchResult:{suffix}");
            // The result is deliberately ignored — a panic (not an Err) is the
            // only way this can fail.
            let _ = MatchResult::from_str(&with_prefix);
            let _ = MatchResult::from_str(&suffix);
        }

        /// Delimiter-dense fuzz: multibyte scalars adjacent to every structural
        /// delimiter still never panic.
        #[test]
        fn from_str_never_panics_on_structural_fuzz(input in structural_fuzz()) {
            let _ = MatchResult::from_str(&input);
        }
    }

    /// Build a valid `MatchResult` purely through the public API from a seed, so
    /// every invariant `validated` checks holds by construction: `add_trade`
    /// keeps `is_complete` / `remaining` / `outcome` in lockstep, and each
    /// filled id is a maker that just traded.
    fn build_valid_result(initial: u64, specs: &[(u64, u64, bool)]) -> MatchResult {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(initial));
        let mut remaining = initial;
        for &(maker, raw_qty, fill) in specs {
            if remaining == 0 {
                break;
            }
            // A trade of 1..=remaining keeps add_trade from underflowing.
            let qty = raw_qty % remaining + 1;
            if result
                .add_trade(sample_trade_with_maker(maker, qty))
                .is_ok()
            {
                remaining -= qty;
                if fill {
                    result.add_filled_order_id(Id::from_u64(maker));
                }
            }
        }
        result
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

        /// Any valid result built through the public API round-trips both serde
        /// JSON (outcome preserved) and Display/FromStr (benign outcome
        /// re-derived identically) with no field drift — the validator never
        /// rejects a legitimately-constructed value.
        #[test]
        fn valid_result_round_trips_both_encodings(
            initial in 1u64..=100_000,
            specs in prop::collection::vec((20u64..=40, 1u64..=100_000, any::<bool>()), 0..12),
        ) {
            let original = build_valid_result(initial, &specs);

            // serde JSON must decode and preserve every field, outcome included.
            let json = serde_json::to_string(&original)
                .map_err(|e| TestCaseError::fail(format!("serialize: {e}")))?;
            let decoded: MatchResult = serde_json::from_str(&json)
                .map_err(|e| TestCaseError::fail(format!("valid json must decode: {e}")))?;
            prop_assert_eq!(decoded.order_id(), original.order_id());
            prop_assert_eq!(
                decoded.remaining_quantity().as_u64(),
                original.remaining_quantity().as_u64()
            );
            prop_assert_eq!(decoded.is_complete(), original.is_complete());
            prop_assert_eq!(decoded.outcome(), original.outcome());
            prop_assert_eq!(decoded.trades().len(), original.trades().len());
            prop_assert_eq!(decoded.filled_order_ids(), original.filled_order_ids());

            // Display / FromStr must reparse and preserve the text-carried fields.
            let text = original.to_string();
            let reparsed = MatchResult::from_str(&text)
                .map_err(|e| TestCaseError::fail(format!("valid text must reparse: {e}")))?;
            prop_assert_eq!(
                reparsed.remaining_quantity().as_u64(),
                original.remaining_quantity().as_u64()
            );
            prop_assert_eq!(reparsed.is_complete(), original.is_complete());
            prop_assert_eq!(reparsed.trades().len(), original.trades().len());
            prop_assert_eq!(reparsed.filled_order_ids(), original.filled_order_ids());
        }
    }
}
