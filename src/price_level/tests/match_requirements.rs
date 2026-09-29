//! Issue #218: `PriceLevel::counter_headroom` and
//! `PriceLevel::match_requirements`. The requirement must equal what the real
//! sweep consumes (filled quantity, trades, FIFO sequences, epoch bumps), and
//! at the counter boundaries `MatchRequirements::check` must agree with what
//! `match_order` does: exactly sufficient headroom never yields
//! `CounterExhausted`, one short is refused by both.

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): raw arithmetic is
// permitted inside `mod tests` per the Testing section of
// `rules/global_rules.md`. Production code outside this module keeps the
// full deny list.
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::{ExhaustedCounter, PriceLevelError};
    use crate::execution::{MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::num::NonZeroU64;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const TAKER: u64 = 999;
    /// `EPOCH_MUTATION_LIMIT` in `level.rs`: `u64::MAX - 2^32`.
    const EPOCH_LIMIT: u64 = u64::MAX - (1 << 32);

    fn standard(id: u64, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn iceberg(id: u64, visible: u64, hidden: u64) -> OrderType<()> {
        OrderType::IcebergOrder {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn reserve(id: u64, visible: u64, hidden: u64, amount: u64) -> OrderType<()> {
        OrderType::ReserveOrder {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(1),
            replenish_amount: NonZeroU64::new(amount),
            auto_replenish: true,
            extra_fields: (),
        }
    }

    /// A named level shape and a taker quantity it can fill in full.
    struct Scenario {
        name: &'static str,
        orders: fn() -> Vec<OrderType<()>>,
        quantity: u64,
    }

    fn scenarios() -> Vec<Scenario> {
        vec![
            Scenario {
                name: "standard",
                orders: || vec![standard(1, 10), standard(2, 10)],
                quantity: 15,
            },
            Scenario {
                name: "iceberg",
                orders: || vec![iceberg(1, 5, 20), standard(2, 10)],
                quantity: 22,
            },
            Scenario {
                name: "reserve",
                orders: || vec![reserve(1, 5, 20, 5), standard(2, 10)],
                quantity: 18,
            },
            Scenario {
                // Three zero-visible reserves at the front: each replenishes
                // (takes a FIFO sequence) before the first trade.
                name: "zero_visible_reserves",
                orders: || {
                    vec![
                        reserve(1, 0, 10, 5),
                        reserve(2, 0, 10, 5),
                        reserve(3, 0, 10, 5),
                        standard(4, 10),
                    ]
                },
                quantity: 25,
            },
            Scenario {
                name: "drains_level",
                orders: || vec![standard(1, 10), iceberg(2, 4, 6)],
                quantity: 20,
            },
        ]
    }

    fn level_with(orders: Vec<OrderType<()>>) -> PriceLevel {
        let level = PriceLevel::new(PRICE);
        for order in orders {
            level.add_order(order).expect("admit");
        }
        level
    }

    fn generator() -> UuidGenerator {
        UuidGenerator::new(
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("namespace"),
        )
    }

    fn take(level: &PriceLevel, quantity: u64, tif: TimeInForce) -> MatchResult {
        take_with(level, quantity, tif, &generator())
    }

    fn take_with(
        level: &PriceLevel,
        quantity: u64,
        tif: TimeInForce,
        generator: &UuidGenerator,
    ) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(TAKER),
            tif,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            generator,
        )
    }

    fn exhausted(counter: ExhaustedCounter) -> PriceLevelError {
        PriceLevelError::counter_exhausted(counter)
    }

    const TIFS: [TimeInForce; 3] = [TimeInForce::Gtc, TimeInForce::Ioc, TimeInForce::Fok];

    #[test]
    fn test_match_requirements_every_shape_equals_real_sweep() {
        for scenario in scenarios() {
            for tif in TIFS {
                let level = level_with((scenario.orders)());
                let requirements = level
                    .match_requirements(scenario.quantity, Id::from_u64(TAKER))
                    .expect("requirements");
                let headroom = level.counter_headroom();
                assert!(headroom.epochs_open());
                requirements.check(&headroom).expect("fits");
                assert!(requirements.fills_completely(&headroom).expect("fits"));
                assert!(!requirements.self_match_rejected());
                assert!(!requirements.stops_at_replenish_overflow());
                assert!(requirements.stop_error().is_none());
                assert_eq!(requirements.incoming_quantity(), scenario.quantity);
                let seq_before = level.test_queue().test_next_seq();
                let (topology_before, mutation_before) = level.test_epochs();

                let result = take(&level, scenario.quantity, tif);

                let label = format!("{} {tif:?}", scenario.name);
                assert!(result.error().is_none(), "{label}: {:?}", result.error());
                assert_eq!(
                    requirements.fillable(),
                    result.executed_quantity().expect("sum").as_u64(),
                    "{label}"
                );
                assert_eq!(requirements.fillable(), scenario.quantity, "{label}");
                assert_eq!(requirements.trades(), result.trades().len(), "{label}");
                assert_eq!(
                    requirements.trade_ids_required().expect("ids"),
                    requirements.trades() as u64,
                    "{label}"
                );
                assert_eq!(
                    requirements.replenishes(),
                    level.test_queue().test_next_seq() - seq_before,
                    "{label}: sequences consumed"
                );
                assert_eq!(requirements.parks(), 0, "{label}");
                // A match bumps the topology epoch at most once (when it
                // drains the level) and never the mutation epoch.
                let (topology_after, mutation_after) = level.test_epochs();
                assert_eq!(mutation_after, mutation_before, "{label}");
                let drained = level.order_count() == 0;
                assert_eq!(
                    topology_after - topology_before,
                    u64::from(drained),
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn test_match_requirements_zero_visible_reserves_replenish_before_first_trade() {
        let level = level_with(vec![
            reserve(1, 0, 10, 5),
            reserve(2, 0, 10, 5),
            reserve(3, 0, 10, 5),
            standard(4, 10),
        ]);
        // Only the standard maker trades, yet three sequences are needed.
        let requirements = level
            .match_requirements(10, Id::from_u64(TAKER))
            .expect("requirements");
        assert_eq!(requirements.trades(), 1);
        assert_eq!(requirements.replenishes(), 3);
    }

    #[test]
    fn test_match_requirements_exact_sequence_headroom_never_exhausted() {
        for scenario in scenarios() {
            for tif in TIFS {
                let level = level_with((scenario.orders)());
                let requirements = level
                    .match_requirements(scenario.quantity, Id::from_u64(TAKER))
                    .expect("requirements");
                level
                    .test_queue()
                    .test_seed_next_seq(u64::MAX - requirements.replenishes());
                let headroom = level.counter_headroom();
                assert_eq!(headroom.queue_sequence(), requirements.replenishes());
                requirements.check(&headroom).expect("exactly sufficient");

                let result = take(&level, scenario.quantity, tif);

                let label = format!("{} {tif:?}", scenario.name);
                assert!(result.error().is_none(), "{label}: {:?}", result.error());
                assert!(result.is_complete(), "{label}");
                assert_eq!(level.test_queue().test_next_seq(), u64::MAX, "{label}");
            }
        }
    }

    #[test]
    fn test_match_requirements_one_sequence_short_refused_like_match() {
        for scenario in scenarios() {
            for tif in TIFS {
                let level = level_with((scenario.orders)());
                let requirements = level
                    .match_requirements(scenario.quantity, Id::from_u64(TAKER))
                    .expect("requirements");
                if requirements.replenishes() == 0 {
                    continue;
                }
                level
                    .test_queue()
                    .test_seed_next_seq(u64::MAX - requirements.replenishes() + 1);
                let label = format!("{} {tif:?}", scenario.name);
                let err = requirements
                    .check(&level.counter_headroom())
                    .expect_err("one short");
                assert_eq!(err, exhausted(ExhaustedCounter::QueueSequence), "{label}");

                let result = take(&level, scenario.quantity, tif);

                assert_eq!(result.error(), Some(&err), "{label}");
                assert!(!result.is_complete(), "{label}");
                if matches!(tif, TimeInForce::Fok) {
                    assert!(result.was_killed(), "{label}");
                    assert!(result.trades().is_empty(), "{label}");
                }
            }
        }
    }

    #[test]
    fn test_match_requirements_epoch_at_limit_refused_like_match() {
        for (topology, mutation, counter) in [
            (EPOCH_LIMIT, 0, ExhaustedCounter::TopologyEpoch),
            (0, EPOCH_LIMIT, ExhaustedCounter::MutationEpoch),
            (EPOCH_LIMIT, EPOCH_LIMIT, ExhaustedCounter::TopologyEpoch),
        ] {
            for tif in TIFS {
                let level = level_with(vec![standard(1, 10)]);
                level.test_seed_epochs(topology, mutation);
                let headroom = level.counter_headroom();
                assert!(!headroom.epochs_open());
                assert_eq!(headroom.closed_epoch(), Some(counter));
                let requirements = level
                    .match_requirements(5, Id::from_u64(TAKER))
                    .expect("requirements");
                let err = requirements.check(&headroom).expect_err("closed epoch");
                assert_eq!(err, exhausted(counter));

                let result = take(&level, 5, tif);

                assert_eq!(result.error(), Some(&err), "{tif:?}");
                assert!(result.trades().is_empty(), "{tif:?}");
            }
        }
    }

    #[test]
    fn test_match_requirements_epoch_below_limit_open_and_matches() {
        for tif in TIFS {
            // Draining the level bumps the topology epoch once, which the
            // limit's reserved headroom absorbs.
            let level = level_with(vec![standard(1, 10)]);
            level.test_seed_epochs(EPOCH_LIMIT - 1, EPOCH_LIMIT - 1);
            let headroom = level.counter_headroom();
            assert!(headroom.epochs_open());
            let requirements = level
                .match_requirements(10, Id::from_u64(TAKER))
                .expect("requirements");
            requirements.check(&headroom).expect("open");

            let result = take(&level, 10, tif);

            assert!(result.error().is_none(), "{tif:?}");
            assert!(result.is_complete(), "{tif:?}");
            assert_eq!(level.test_epochs(), (EPOCH_LIMIT, EPOCH_LIMIT - 1));
        }
    }

    #[test]
    fn test_match_requirements_resting_taker_id_self_match_rejected_like_match() {
        for tif in TIFS {
            // The taker's id rests behind depth the dry run alone would
            // count: `match_order` rejects the whole taker instead.
            let level = level_with(vec![standard(1, 10), standard(TAKER, 5), standard(3, 10)]);
            let requirements = level
                .match_requirements(15, Id::from_u64(TAKER))
                .expect("requirements");
            assert!(requirements.self_match_rejected());
            assert_eq!(requirements.fillable(), 0);
            assert_eq!(requirements.trades(), 0);
            assert_eq!(requirements.replenishes(), 0);
            let headroom = level.counter_headroom();
            requirements.check(&headroom).expect("nothing consumed");
            assert!(!requirements.fills_completely(&headroom).expect("check"));

            let result = take(&level, 15, tif);

            assert!(result.was_rejected(), "{tif:?}");
            assert!(result.trades().is_empty(), "{tif:?}");
            assert!(result.error().is_none(), "{tif:?}");
        }
    }

    #[test]
    fn test_match_requirements_partial_level_does_not_fill_completely() {
        let level = level_with(vec![standard(1, 10)]);
        let requirements = level
            .match_requirements(15, Id::from_u64(TAKER))
            .expect("requirements");
        let headroom = level.counter_headroom();
        requirements.check(&headroom).expect("counters fit");
        assert!(!requirements.fills_completely(&headroom).expect("check"));
        assert_eq!(requirements.fillable(), 10);
    }

    /// Maker 1 trades one unit; the zero-visible iceberg behind it then draws
    /// a tranche that overflows the level's visible counter, so the sweep
    /// stops there after reserving the iceberg's FIFO sequence.
    fn replenish_overflow_level() -> PriceLevel {
        level_with(vec![
            standard(1, 1),
            iceberg(2, 0, u64::MAX / 2),
            standard(3, u64::MAX / 2 + 10),
        ])
    }

    #[test]
    fn test_match_requirements_replenish_overflow_needs_one_more_sequence() {
        // Zero headroom: `check` refuses, and a real non-fill-or-kill sweep
        // stops with `QueueSequence` at the aborting step.
        let level = replenish_overflow_level();
        let requirements = level
            .match_requirements(3, Id::from_u64(TAKER))
            .expect("requirements");
        assert!(requirements.stops_at_replenish_overflow());
        assert_eq!(requirements.replenishes(), 0);
        assert_eq!((requirements.fillable(), requirements.trades()), (1, 1));
        // The aborting zero-visible step would not trade: no extra id.
        assert_eq!(requirements.trade_ids_required().expect("ids"), 1);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let err = requirements
            .check(&level.counter_headroom())
            .expect_err("the aborting step needs a sequence");
        assert_eq!(err, exhausted(ExhaustedCounter::QueueSequence));
        let result = take(&level, 3, TimeInForce::Gtc);
        assert_eq!(result.error(), Some(&err));
        assert_eq!(result.trades().len(), 1);

        // Exactly one sequence: `check` passes and the sweep stops at the
        // abort without an error, having used that sequence.
        let level = replenish_overflow_level();
        level.test_queue().test_seed_next_seq(u64::MAX - 1);
        requirements
            .check(&level.counter_headroom())
            .expect("one sequence suffices");
        let ids = generator();
        let before = ids.remaining();
        let result = take_with(&level, 3, TimeInForce::Gtc, &ids);
        assert!(result.error().is_none(), "{:?}", result.error());
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.remaining_quantity().as_u64(), 2);
        assert_eq!(
            before - ids.remaining(),
            requirements.trade_ids_required().expect("ids")
        );
        assert_eq!(level.test_queue().test_next_seq(), u64::MAX);
    }

    #[test]
    fn test_match_requirements_replenish_overflow_fok_killed_for_depth() {
        // Fill-or-kill is unchanged: the dry run's short fill kills it for
        // depth before any sequence check, with or without headroom.
        for next_seq in [u64::MAX, u64::MAX - 1] {
            let level = replenish_overflow_level();
            level.test_queue().test_seed_next_seq(next_seq);
            let requirements = level
                .match_requirements(3, Id::from_u64(TAKER))
                .expect("requirements");
            let fills = requirements.fills_completely(&level.counter_headroom());
            assert!(!matches!(fills, Ok(true)));
            let result = take(&level, 3, TimeInForce::Fok);
            assert!(result.was_killed());
            assert!(result.error().is_none(), "{:?}", result.error());
            assert_eq!(level.test_queue().test_next_seq(), next_seq);
        }
    }

    #[test]
    fn test_match_requirements_replenish_overflow_trading_step_takes_one_more_id() {
        // The aborting reserve step would trade five units, so it reserves
        // (and skips) a trade id as well as its sequence.
        let level = level_with(vec![
            standard(1, 1),
            reserve(2, 5, u64::MAX / 2, u64::MAX / 2),
            standard(3, u64::MAX / 2 + 10),
        ]);
        let requirements = level
            .match_requirements(10, Id::from_u64(TAKER))
            .expect("requirements");
        assert!(requirements.stops_at_replenish_overflow());
        assert_eq!(requirements.trades(), 1);
        requirements
            .check(&level.counter_headroom())
            .expect("sequences available");
        assert_eq!(requirements.trade_ids_required().expect("ids"), 2);
        let ids = generator();
        let before = ids.remaining();
        let result = take_with(&level, 10, TimeInForce::Ioc, &ids);
        assert!(result.error().is_none(), "{:?}", result.error());
        assert_eq!(result.trades().len(), requirements.trades());
        assert_eq!(
            before - ids.remaining(),
            requirements.trade_ids_required().expect("ids")
        );
    }

    #[test]
    fn test_match_requirements_zero_quantity_closed_epoch_passes() {
        let level = level_with(vec![standard(1, 10)]);
        level.test_seed_epochs(EPOCH_LIMIT, EPOCH_LIMIT);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let requirements = level
            .match_requirements(0, Id::from_u64(TAKER))
            .expect("requirements");
        requirements
            .check(&level.counter_headroom())
            .expect("a zero-quantity match never sweeps");
        let result = take(&level, 0, TimeInForce::Fok);
        assert!(result.error().is_none());
        assert!(result.is_complete());
    }
}
