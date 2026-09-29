//! Issue #219: `MatchResult::try_absorb`, folding one price level's result
//! into a multi-level aggregate. Covers the validation refusals (aggregate
//! unchanged, source intact), the lossless buffer move into an empty
//! aggregate, the reserve-and-append path, first-error-wins, kill / reject
//! adoption, and a property over sequences of level results.

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): raw arithmetic is
// permitted inside `mod tests` per the Testing section of
// `rules/global_rules.md`. Production code outside this module keeps the
// full deny list.
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use crate::errors::{CapacityResource, ExhaustedCounter, PriceLevelError};
    use crate::execution::match_result::{MatchOutcome, MatchResult};
    use crate::execution::trade::Trade;
    use crate::execution::trade_list_seam;
    use crate::orders::{Id, Side};
    use crate::utils::{Price, Quantity, TimestampMs};
    use proptest::prelude::*;
    use std::str::FromStr;

    const TAKER: u64 = 10;

    fn trade(taker: u64, maker: u64, quantity: u64) -> Trade {
        Trade::with_timestamp(
            Id::from_u64(1_000_000 + maker),
            Id::from_u64(taker),
            Id::from_u64(maker),
            Price::new(1_000),
            Quantity::new(quantity),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        )
    }

    /// A level result for `incoming`: one trade per `(maker, quantity,
    /// consumed)`, a filled id for each consumed maker, finalized like the
    /// engine does.
    fn level(incoming: u64, fills: &[(u64, u64, bool)]) -> MatchResult {
        let mut result = MatchResult::new(Id::from_u64(TAKER), Quantity::new(incoming));
        for &(maker, quantity, consumed) in fills {
            result
                .add_trade(trade(TAKER, maker, quantity))
                .expect("add_trade");
            if consumed {
                result
                    .add_filled_order_id(Id::from_u64(maker))
                    .expect("filled id");
            }
        }
        let remaining = result.remaining_quantity();
        result.finalize(remaining);
        result
    }

    /// As [`level`], with the trade buffer sized exactly for its fills (no
    /// spare capacity).
    fn exact_level(incoming: u64, fills: &[(u64, u64, bool)]) -> MatchResult {
        let mut result = MatchResult::try_with_capacity(
            Id::from_u64(TAKER),
            Quantity::new(incoming),
            fills.len(),
        )
        .expect("capacity");
        for &(maker, quantity, consumed) in fills {
            result
                .add_trade(trade(TAKER, maker, quantity))
                .expect("add_trade");
            if consumed {
                result
                    .add_filled_order_id(Id::from_u64(maker))
                    .expect("filled id");
            }
        }
        assert_eq!(result.trades().capacity(), fills.len());
        let remaining = result.remaining_quantity();
        result.finalize(remaining);
        result
    }

    fn json(result: &MatchResult) -> String {
        serde_json::to_string(result).expect("serialize")
    }

    /// The aggregate decodes through the validator (serde and text forms).
    fn assert_round_trips(result: &MatchResult) {
        let decoded: MatchResult = serde_json::from_str(&json(result)).expect("validated decode");
        assert_eq!(json(&decoded), json(result));
        MatchResult::from_str(&result.to_string()).expect("validated text decode");
    }

    fn maker_ids(result: &MatchResult) -> Vec<Id> {
        result
            .trades()
            .as_vec()
            .iter()
            .map(Trade::maker_order_id)
            .collect()
    }

    fn assert_refused_unchanged(
        aggregate: &mut MatchResult,
        source: &mut MatchResult,
    ) -> PriceLevelError {
        let aggregate_before = json(aggregate);
        let source_before = json(source);
        let err = aggregate.try_absorb(source).expect_err("must refuse");
        assert_eq!(json(aggregate), aggregate_before, "aggregate unchanged");
        assert_eq!(json(source), source_before, "source intact");
        err
    }

    #[test]
    fn test_try_absorb_taker_mismatch_refused_unchanged() {
        let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(10));
        let mut other = MatchResult::new(Id::from_u64(TAKER + 1), Quantity::new(10));
        other.add_trade(trade(TAKER + 1, 1, 4)).expect("add_trade");
        let err = assert_refused_unchanged(&mut aggregate, &mut other);
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
    }

    #[test]
    fn test_try_absorb_quantity_mismatch_refused_unchanged() {
        // The level was matched with more, and with less, than the aggregate
        // has left.
        for incoming in [9, 11] {
            let mut aggregate = level(20, &[(1, 10, true)]);
            let mut source = level(incoming, &[(2, 3, false)]);
            let err = assert_refused_unchanged(&mut aggregate, &mut source);
            assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
        }
    }

    #[test]
    fn test_try_absorb_terminal_aggregate_refused_unchanged() {
        let mut failed = level(20, &[(1, 5, true)]);
        failed.set_error(PriceLevelError::counter_exhausted(
            ExhaustedCounter::QueueSequence,
        ));
        let mut killed = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        killed.mark_killed(20);
        let mut rejected = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        rejected.mark_rejected(20);

        for mut aggregate in [failed, killed, rejected] {
            let remaining = aggregate.remaining_quantity().as_u64();
            let mut source = level(remaining, &[(2, 1, false)]);
            let err = assert_refused_unchanged(&mut aggregate, &mut source);
            assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
        }
    }

    #[test]
    fn test_try_absorb_empty_aggregate_adopts_level_buffers() {
        let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        let mut source =
            MatchResult::try_with_capacity(Id::from_u64(TAKER), Quantity::new(20), 4).expect("cap");
        source.add_trade(trade(TAKER, 1, 6)).expect("t1");
        source.add_filled_order_id(Id::from_u64(1)).expect("f1");
        source.add_trade(trade(TAKER, 2, 4)).expect("t2");
        let remaining = source.remaining_quantity();
        source.finalize(remaining);
        let trades_ptr = source.trades().as_vec().as_ptr();
        let trades_capacity = source.trades().capacity();
        let filled_ptr = source.filled_order_ids().as_ptr();
        let filled_capacity = source.test_filled_order_ids_capacity();

        aggregate.try_absorb(&mut source).expect("absorb");

        // The level's buffers moved: same allocation, same capacity, so the
        // absorb itself allocated nothing.
        assert_eq!(aggregate.trades().as_vec().as_ptr(), trades_ptr);
        assert_eq!(aggregate.trades().capacity(), trades_capacity);
        assert_eq!(aggregate.filled_order_ids().as_ptr(), filled_ptr);
        assert_eq!(aggregate.test_filled_order_ids_capacity(), filled_capacity);
        assert_eq!(
            maker_ids(&aggregate),
            vec![Id::from_u64(1), Id::from_u64(2)]
        );
        assert_eq!(aggregate.filled_order_ids(), &[Id::from_u64(1)]);
        assert_eq!(aggregate.remaining_quantity().as_u64(), 10);
        assert_eq!(aggregate.outcome(), MatchOutcome::PartiallyFilled);
        assert_round_trips(&aggregate);

        // The source is drained into a consistent empty result.
        assert!(source.trades().is_empty());
        assert!(source.filled_order_ids().is_empty());
        assert!(source.error().is_none());
        assert_eq!(source.remaining_quantity().as_u64(), 10);
        assert_eq!(source.outcome(), MatchOutcome::NotFilled);
        assert_round_trips(&source);
    }

    #[test]
    fn test_try_absorb_sufficient_reservation_appended_into() {
        let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        aggregate.try_reserve(8).expect("reserve");
        let trades_ptr = aggregate.trades().as_vec().as_ptr();
        let filled_ptr = aggregate.filled_order_ids().as_ptr();
        let mut source = level(20, &[(1, 6, true), (2, 4, true)]);

        aggregate.try_absorb(&mut source).expect("absorb");

        // Appended into the caller's reservation; nothing moved or grew.
        assert_eq!(aggregate.trades().as_vec().as_ptr(), trades_ptr);
        assert_eq!(aggregate.filled_order_ids().as_ptr(), filled_ptr);
        assert!(aggregate.trades().capacity() >= 8);
        assert_eq!(
            maker_ids(&aggregate),
            vec![Id::from_u64(1), Id::from_u64(2)]
        );
        assert_eq!(
            aggregate.filled_order_ids(),
            &[Id::from_u64(1), Id::from_u64(2)]
        );
        assert_round_trips(&aggregate);
    }

    #[test]
    fn test_try_absorb_existing_filled_ids_kept() {
        // The aggregate holds a filled id but no trades: its trade vector is
        // moved in, its id vector is appended to.
        let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        aggregate
            .add_filled_order_id(Id::from_u64(7))
            .expect("filled id");
        let mut source = level(20, &[(1, 5, true)]);

        aggregate.try_absorb(&mut source).expect("absorb");

        assert_eq!(maker_ids(&aggregate), vec![Id::from_u64(1)]);
        assert_eq!(
            aggregate.filled_order_ids(),
            &[Id::from_u64(7), Id::from_u64(1)]
        );
        assert_eq!(aggregate.remaining_quantity().as_u64(), 15);
    }

    #[test]
    fn test_try_absorb_growth_refusal_unchanged_then_appends_in_order() {
        // Both buffers are exactly full, so neither the append nor the adopt
        // plan fits and the aggregate must grow.
        let mut aggregate = exact_level(20, &[(1, 5, true)]);
        let mut source = exact_level(15, &[(2, 3, true), (3, 2, false)]);

        // A capped trade list refuses the growth: nothing changes.
        {
            let _limit = trade_list_seam::limit_trades(2);
            let err = assert_refused_unchanged(&mut aggregate, &mut source);
            assert!(matches!(
                err,
                PriceLevelError::CapacityExceeded {
                    resource: CapacityResource::Trades,
                    ..
                }
            ));
        }

        aggregate.try_absorb(&mut source).expect("absorb");
        assert_eq!(
            maker_ids(&aggregate),
            vec![Id::from_u64(1), Id::from_u64(2), Id::from_u64(3)]
        );
        assert_eq!(
            aggregate.filled_order_ids(),
            &[Id::from_u64(1), Id::from_u64(2)]
        );
        assert_eq!(aggregate.remaining_quantity().as_u64(), 10);
        assert_eq!(aggregate.executed_quantity().expect("sum").as_u64(), 10);
        assert_eq!(aggregate.outcome(), MatchOutcome::PartiallyFilled);
        assert!(!aggregate.is_complete());
        assert_round_trips(&aggregate);
    }

    #[test]
    fn test_try_absorb_filling_level_completes_aggregate() {
        let mut aggregate = level(10, &[(1, 4, true)]);
        let mut source = level(6, &[(2, 6, true)]);
        aggregate.try_absorb(&mut source).expect("absorb");
        assert!(aggregate.is_complete());
        assert_eq!(aggregate.outcome(), MatchOutcome::Filled);
        assert_eq!(source.outcome(), MatchOutcome::Filled);
        assert_round_trips(&aggregate);
        assert_round_trips(&source);
    }

    #[test]
    fn test_try_absorb_level_error_first_error_wins() {
        let first = PriceLevelError::counter_exhausted(ExhaustedCounter::QueueSequence);
        let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        let mut source = level(20, &[(1, 5, true)]);
        source.set_error(first.clone());

        aggregate.try_absorb(&mut source).expect("absorb");
        assert_eq!(aggregate.error(), Some(&first));
        assert!(source.error().is_none(), "the error was taken");
        assert_round_trips(&aggregate);

        // The failed aggregate is terminal: a later level cannot replace it.
        let mut later = level(15, &[(2, 1, false)]);
        later.set_error(PriceLevelError::capacity_exceeded(
            CapacityResource::Trades,
            1,
        ));
        let _ = assert_refused_unchanged(&mut aggregate, &mut later);
        assert_eq!(aggregate.error(), Some(&first));
    }

    #[test]
    fn test_try_absorb_kill_or_reject_adopted_only_without_trades() {
        for reject in [false, true] {
            let mark = |result: &mut MatchResult, quantity: u64| {
                if reject {
                    result.mark_rejected(quantity);
                } else {
                    result.mark_killed(quantity);
                }
            };
            let expected = if reject {
                MatchOutcome::Rejected
            } else {
                MatchOutcome::Killed
            };

            // Nothing traded anywhere: the level's verdict is adopted.
            let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
            let mut source = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
            mark(&mut source, 20);
            aggregate.try_absorb(&mut source).expect("absorb");
            assert_eq!(aggregate.outcome(), expected);
            assert_eq!(aggregate.remaining_quantity().as_u64(), 20);
            assert!(!aggregate.is_complete());
            assert_round_trips(&aggregate);

            // Earlier trades: the aggregate stays partially filled.
            let mut aggregate = level(20, &[(1, 5, true)]);
            let mut source = MatchResult::new(Id::from_u64(TAKER), Quantity::new(15));
            mark(&mut source, 15);
            aggregate.try_absorb(&mut source).expect("absorb");
            assert_eq!(aggregate.outcome(), MatchOutcome::PartiallyFilled);
            assert_round_trips(&aggregate);
        }
    }

    #[test]
    fn test_try_absorb_nonempty_aggregate_adopts_roomy_level_buffers() {
        // The aggregate's buffers are full; the level's have room for both
        // sets of entries, so the aggregate's entries move to the front of
        // the level's buffers and nothing is allocated.
        let mut aggregate =
            MatchResult::try_with_capacity(Id::from_u64(TAKER), Quantity::new(20), 1).expect("cap");
        aggregate.add_trade(trade(TAKER, 1, 5)).expect("t1");
        aggregate.add_filled_order_id(Id::from_u64(1)).expect("f1");
        let mut source =
            MatchResult::try_with_capacity(Id::from_u64(TAKER), Quantity::new(15), 8).expect("cap");
        source.add_trade(trade(TAKER, 2, 3)).expect("t2");
        source.add_filled_order_id(Id::from_u64(2)).expect("f2");
        source.add_trade(trade(TAKER, 3, 2)).expect("t3");
        let trades_ptr = source.trades().as_vec().as_ptr();
        let filled_ptr = source.filled_order_ids().as_ptr();
        let aggregate_trades_ptr = aggregate.trades().as_vec().as_ptr();

        aggregate.try_absorb(&mut source).expect("absorb");

        assert_eq!(aggregate.trades().as_vec().as_ptr(), trades_ptr);
        assert_eq!(aggregate.filled_order_ids().as_ptr(), filled_ptr);
        assert_eq!(aggregate.trades().capacity(), 8);
        assert_eq!(
            maker_ids(&aggregate),
            vec![Id::from_u64(1), Id::from_u64(2), Id::from_u64(3)]
        );
        assert_eq!(
            aggregate.filled_order_ids(),
            &[Id::from_u64(1), Id::from_u64(2)]
        );
        assert_eq!(aggregate.remaining_quantity().as_u64(), 10);
        // The level got the aggregate's emptied buffer back.
        assert_eq!(source.trades().as_vec().as_ptr(), aggregate_trades_ptr);
        assert!(source.trades().is_empty());
        assert_round_trips(&aggregate);
        assert_round_trips(&source);
    }

    #[test]
    fn test_try_absorb_killed_level_into_ids_without_trades_not_filled() {
        // `add_filled_order_id` can leave ids without trades; a kill must
        // not be adopted then (Killed carries no filled ids).
        let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        aggregate
            .add_filled_order_id(Id::from_u64(7))
            .expect("filled id");
        let mut source = MatchResult::new(Id::from_u64(TAKER), Quantity::new(20));
        source.mark_killed(20);

        aggregate.try_absorb(&mut source).expect("absorb");

        assert_eq!(aggregate.outcome(), MatchOutcome::NotFilled);
        assert!(!aggregate.was_killed());
        assert_eq!(aggregate.filled_order_ids(), &[Id::from_u64(7)]);
        // The outcome agrees with the fields; the decoder still rejects the
        // payload, but only because the caller-added id is backed by no
        // trade (the same rejection the aggregate drew before absorbing).
        let err = serde_json::from_str::<MatchResult>(&json(&aggregate))
            .expect_err("an id without a trade is not a decodable result");
        let message = err.to_string();
        assert!(message.contains("not an in-order maker"), "{message}");
        assert!(!message.contains("outcome contradicts"), "{message}");
    }

    #[test]
    fn test_try_absorb_killed_level_after_trades_partially_filled_and_open() {
        // A kill after earlier trades does not make the aggregate terminal:
        // it stays PartiallyFilled and accepts a further level. Stopping the
        // sweep is the caller's decision.
        let mut aggregate = level(20, &[(1, 5, true)]);
        let mut killed = MatchResult::new(Id::from_u64(TAKER), Quantity::new(15));
        killed.mark_killed(15);

        aggregate
            .try_absorb(&mut killed)
            .expect("absorb killed level");
        assert_eq!(aggregate.outcome(), MatchOutcome::PartiallyFilled);
        assert!(!aggregate.was_killed());

        let mut next = level(15, &[(2, 15, true)]);
        aggregate
            .try_absorb(&mut next)
            .expect("a later level is accepted");
        assert!(aggregate.is_complete());
        assert_eq!(aggregate.outcome(), MatchOutcome::Filled);
        assert_round_trips(&aggregate);
    }

    #[test]
    fn test_try_absorb_zero_quantity_vacuously_filled() {
        let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(0));
        let mut source = MatchResult::new(Id::from_u64(TAKER), Quantity::new(0));
        aggregate.try_absorb(&mut source).expect("absorb");
        assert!(aggregate.is_complete());
        assert_eq!(aggregate.outcome(), MatchOutcome::Filled);
        assert_round_trips(&aggregate);
    }

    /// One generated level: fills `(quantity, consumed)`, then an optional
    /// kill (only when nothing traded) and an optional error.
    #[derive(Debug, Clone)]
    struct LevelSpec {
        fills: Vec<(u64, bool)>,
        killed: bool,
        error: bool,
        /// Capacity the level result is created with (exercises the append,
        /// adopt and grow plans).
        capacity: usize,
    }

    fn level_spec() -> impl Strategy<Value = LevelSpec> {
        (
            prop::collection::vec((1u64..=8, any::<bool>()), 0..4),
            any::<bool>(),
            prop::bool::weighted(0.15),
            0usize..10,
        )
            .prop_map(|(fills, killed, error, capacity)| LevelSpec {
                fills,
                killed,
                error,
                capacity,
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        #[test]
        fn test_try_absorb_level_sequences_keep_invariants(
            initial in 0u64..=40,
            specs in prop::collection::vec(level_spec(), 0..6),
            // Filled ids the caller added to the destination without trades.
            prefix_ids in 0u64..3,
            reservation in 0usize..6,
        ) {
            let mut aggregate = MatchResult::new(Id::from_u64(TAKER), Quantity::new(initial));
            aggregate.try_reserve_trades(reservation).expect("reserve");
            let mut expected_filled: Vec<Id> = Vec::new();
            for id in 0..prefix_ids {
                let id = Id::from_u64(900 + id);
                aggregate.add_filled_order_id(id).expect("prefix id");
                expected_filled.push(id);
            }
            let mut next_maker = 1u64;
            let mut expected_makers: Vec<Id> = Vec::new();
            let mut first_error: Option<PriceLevelError> = None;

            for spec in specs {
                let remaining = aggregate.remaining_quantity().as_u64();
                let mut source = MatchResult::try_with_capacity(
                    Id::from_u64(TAKER),
                    Quantity::new(remaining),
                    spec.capacity,
                )
                .expect("level capacity");
                let mut level_makers = Vec::new();
                let mut level_filled = Vec::new();
                let mut left = remaining;
                for (quantity, consumed) in spec.fills {
                    let quantity = quantity.min(left);
                    if quantity == 0 {
                        break;
                    }
                    let maker = next_maker;
                    next_maker += 1;
                    source.add_trade(trade(TAKER, maker, quantity)).expect("add_trade");
                    left -= quantity;
                    level_makers.push(Id::from_u64(maker));
                    if consumed {
                        source.add_filled_order_id(Id::from_u64(maker)).expect("filled id");
                        level_filled.push(Id::from_u64(maker));
                    }
                }
                let source_remaining = source.remaining_quantity();
                source.finalize(source_remaining);
                if spec.killed && level_makers.is_empty() && remaining > 0 {
                    source.mark_killed(remaining);
                }
                let level_error = spec.error.then(|| {
                    PriceLevelError::capacity_exceeded(CapacityResource::Trades, expected_makers.len() + 1)
                });
                if let Some(error) = &level_error {
                    source.set_error(error.clone());
                }

                let terminal = aggregate.is_failed()
                    || aggregate.was_killed()
                    || aggregate.was_rejected();
                let before = json(&aggregate);
                let source_before = json(&source);
                let absorbed = aggregate.try_absorb(&mut source);
                if terminal {
                    prop_assert!(absorbed.is_err());
                    prop_assert_eq!(json(&aggregate), before);
                    prop_assert_eq!(json(&source), source_before);
                    continue;
                }
                prop_assert!(absorbed.is_ok());
                expected_makers.extend(level_makers);
                expected_filled.extend(level_filled);
                if first_error.is_none() {
                    first_error = level_error;
                }

                let remaining = aggregate.remaining_quantity().as_u64();
                let executed = aggregate.executed_quantity().expect("sum").as_u64();
                prop_assert_eq!(aggregate.is_complete(), remaining == 0);
                prop_assert_eq!(executed + remaining, initial);
                prop_assert_eq!(maker_ids(&aggregate), expected_makers.clone());
                prop_assert_eq!(aggregate.filled_order_ids(), expected_filled.as_slice());
                prop_assert_eq!(aggregate.error(), first_error.as_ref());
                let consistent = match aggregate.outcome() {
                    MatchOutcome::Filled => remaining == 0,
                    MatchOutcome::PartiallyFilled => {
                        remaining > 0 && !aggregate.trades().is_empty()
                    }
                    MatchOutcome::NotFilled => remaining > 0 && aggregate.trades().is_empty(),
                    MatchOutcome::Killed | MatchOutcome::Rejected => {
                        remaining > 0
                            && aggregate.trades().is_empty()
                            && aggregate.filled_order_ids().is_empty()
                    }
                };
                prop_assert!(consistent, "{:?}", aggregate.outcome());
                // Caller-added ids without trades are outside the decodable
                // domain from the start; otherwise the aggregate round-trips.
                if prefix_ids == 0 {
                    let decoded: MatchResult =
                        serde_json::from_str(&json(&aggregate)).expect("validated decode");
                    prop_assert_eq!(json(&decoded), json(&aggregate));
                }
                // The drained source is a consistent empty result too.
                prop_assert!(source.trades().is_empty());
                prop_assert!(source.filled_order_ids().is_empty());
                let _: MatchResult =
                    serde_json::from_str(&json(&source)).expect("drained source decodes");
            }
        }
    }
}
