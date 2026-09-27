//! Issue #168 / #164 contract: `PriceLevel::match_order` with a trade-id
//! generator that runs out of sequence values. A non-fill-or-kill sweep stops
//! BEFORE committing the step whose id cannot be reserved and reports the
//! committed FIFO prefix with the typed error; a fill-or-kill taker reserves
//! every id up front and is killed with the level untouched when it cannot.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::{CapacityResource, PriceLevelError};
    use crate::execution::{MatchOutcome, MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const NAMESPACE: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";

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

    fn reserve(id: u64, visible: u64, hidden: u64) -> OrderType<()> {
        OrderType::ReserveOrder {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(0),
            replenish_amount: None,
            auto_replenish: true,
            extra_fields: (),
        }
    }

    fn level_with(orders: Vec<OrderType<()>>) -> PriceLevel {
        let level = PriceLevel::new(PRICE);
        for order in orders {
            level.add_order(order).expect("add_order");
        }
        level
    }

    fn fresh_generator() -> UuidGenerator {
        UuidGenerator::new(Uuid::parse_str(NAMESPACE).expect("namespace"))
    }

    /// A generator (restored through its public serde form) with exactly
    /// `left` sequence values still issuable.
    fn generator_with(left: u64) -> UuidGenerator {
        let counter = UuidGenerator::EXHAUSTED - left;
        let json = format!(r#"{{"namespace":"{NAMESPACE}","counter":{counter}}}"#);
        let generator: UuidGenerator = serde_json::from_str(&json).expect("generator");
        assert_eq!(generator.remaining(), left);
        generator
    }

    fn expected_id(sequence: u64) -> Id {
        let namespace = Uuid::parse_str(NAMESPACE).expect("namespace");
        Id::from_uuid(Uuid::new_v5(&namespace, sequence.to_string().as_bytes()))
    }

    fn take(
        level: &PriceLevel,
        quantity: u64,
        tif: TimeInForce,
        generator: &UuidGenerator,
    ) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(999),
            tif,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            generator,
        )
    }

    #[derive(Debug, PartialEq)]
    struct LevelState {
        ids: Vec<Id>,
        visible: Vec<u64>,
        hidden: Vec<u64>,
        visible_counter: u64,
        hidden_counter: u64,
        order_count: usize,
        orders_executed: usize,
        quantity_executed: u64,
    }

    fn state(level: &PriceLevel) -> LevelState {
        let orders = level.snapshot_by_insertion_seq();
        LevelState {
            ids: orders.iter().map(|o| o.id()).collect(),
            visible: orders
                .iter()
                .map(|o| o.visible_quantity().as_u64())
                .collect(),
            hidden: orders
                .iter()
                .map(|o| o.hidden_quantity().as_u64())
                .collect(),
            visible_counter: level.visible_quantity(),
            hidden_counter: level.hidden_quantity(),
            order_count: level.order_count(),
            orders_executed: level.stats().orders_executed(),
            quantity_executed: level.stats().quantity_executed(),
        }
    }

    fn assert_counters_match_queue(level: &PriceLevel) {
        let s = state(level);
        assert_eq!(s.visible_counter, s.visible.iter().sum::<u64>(), "visible");
        assert_eq!(s.hidden_counter, s.hidden.iter().sum::<u64>(), "hidden");
        assert_eq!(s.order_count, s.ids.len(), "order_count");
    }

    fn executed(result: &MatchResult) -> u64 {
        result.executed_quantity().expect("executed").as_u64()
    }

    fn assert_id_exhausted(result: &MatchResult, additional: usize) {
        assert_eq!(
            result.error(),
            Some(&PriceLevelError::CapacityExceeded {
                resource: CapacityResource::IdSequence,
                additional,
            })
        );
    }

    /// Runs `orders` / `quantity` unconstrained (control) and with only `left`
    /// trade ids available; asserts the #164 contract on the constrained run.
    fn exhausted_vs_control(
        orders: impl Fn() -> Vec<OrderType<()>>,
        quantity: u64,
        left: u64,
    ) -> (MatchResult, PriceLevel) {
        let control_level = level_with(orders());
        let control = take(
            &control_level,
            quantity,
            TimeInForce::Gtc,
            &fresh_generator(),
        );
        assert!(control.error().is_none());
        let left_usize = usize::try_from(left).expect("left");
        assert!(control.trades().len() > left_usize, "scenario too small");

        let level = level_with(orders());
        let generator = generator_with(left);
        let result = take(&level, quantity, TimeInForce::Gtc, &generator);

        assert_id_exhausted(&result, 1);
        assert!(generator.is_exhausted());
        assert_eq!(result.trades().len(), left_usize);
        // Committed fills are a FIFO prefix of the unconstrained sweep, and
        // they carry the generator's final ids in order.
        for (index, (got, want)) in result
            .trades()
            .as_vec()
            .iter()
            .zip(control.trades().as_vec())
            .enumerate()
        {
            assert_eq!(got.maker_order_id(), want.maker_order_id());
            assert_eq!(got.quantity(), want.quantity());
            let sequence = UuidGenerator::EXHAUSTED - left + u64::try_from(index).expect("index");
            assert_eq!(got.trade_id(), expected_id(sequence));
        }
        assert_eq!(
            result.remaining_quantity().as_u64(),
            quantity - executed(&result)
        );
        assert_eq!(
            result.outcome(),
            if left == 0 {
                MatchOutcome::NotFilled
            } else {
                MatchOutcome::PartiallyFilled
            }
        );
        let resting = level.snapshot_by_insertion_seq();
        for id in result.filled_order_ids() {
            assert!(resting.iter().all(|o| o.id() != *id));
        }
        assert_counters_match_queue(&level);
        let s = state(&level);
        assert_eq!(s.orders_executed, left_usize, "stats: committed fills only");
        assert_eq!(s.quantity_executed, executed(&result));

        // Subsequent calls with the exhausted generator fail cleanly: no
        // trade, full remainder, the level byte-identical.
        let before = state(&level);
        let again = take(&level, quantity, TimeInForce::Ioc, &generator);
        assert_id_exhausted(&again, 1);
        assert!(again.trades().is_empty());
        assert!(again.filled_order_ids().is_empty());
        assert_eq!(again.remaining_quantity().as_u64(), quantity);
        assert_eq!(again.outcome(), MatchOutcome::NotFilled);
        assert_eq!(state(&level), before);

        // With a fresh generator the rest fills in FIFO order and the combined
        // stream equals the control.
        let rest = take(
            &level,
            result.remaining_quantity().as_u64(),
            TimeInForce::Gtc,
            &fresh_generator(),
        );
        assert!(rest.error().is_none());
        let combined: Vec<(Id, Quantity)> = result
            .trades()
            .as_vec()
            .iter()
            .chain(rest.trades().as_vec())
            .map(|t| (t.maker_order_id(), t.quantity()))
            .collect();
        let want: Vec<(Id, Quantity)> = control
            .trades()
            .as_vec()
            .iter()
            .map(|t| (t.maker_order_id(), t.quantity()))
            .collect();
        assert_eq!(combined, want);
        assert_eq!(state(&level).ids, state(&control_level).ids);
        assert_counters_match_queue(&level);

        (result, level)
    }

    #[test]
    fn test_match_order_trade_ids_follow_generator_sequence_without_gaps() {
        let level = level_with((1..=4).map(|id| standard(id, 10)).collect());
        let generator = fresh_generator();
        let first = take(&level, 20, TimeInForce::Gtc, &generator);
        let second = take(&level, 100, TimeInForce::Ioc, &generator);
        let ids: Vec<Id> = first
            .trades()
            .as_vec()
            .iter()
            .chain(second.trades().as_vec())
            .map(|t| t.trade_id())
            .collect();
        // Exactly one sequence value per trade, even though the second sweep
        // ran past the drained queue.
        assert_eq!(ids, (0..4).map(expected_id).collect::<Vec<_>>());
        assert_eq!(generator.remaining(), UuidGenerator::EXHAUSTED - 4);
    }

    #[test]
    fn test_match_order_id_exhaustion_after_prior_fills_reports_prefix() {
        let makers = || (1..=5).map(|id| standard(id, 10)).collect();
        let (result, _) = exhausted_vs_control(makers, 50, 2);
        assert_eq!(
            result.filled_order_ids(),
            &[Id::from_u64(1), Id::from_u64(2)]
        );
        assert_eq!(result.remaining_quantity().as_u64(), 30);
    }

    #[test]
    fn test_match_order_id_exhaustion_at_first_fill_leaves_level_unchanged() {
        let makers = || (1..=3).map(|id| standard(id, 10)).collect::<Vec<_>>();
        let before = state(&level_with(makers()));
        let (result, _) = exhausted_vs_control(makers, 30, 0);
        assert!(result.trades().is_empty());
        assert_eq!(result.remaining_quantity().as_u64(), 30);

        let level = level_with(makers());
        let result = take(&level, 30, TimeInForce::Gtc, &generator_with(0));
        assert_id_exhausted(&result, 1);
        assert_eq!(state(&level), before);
    }

    #[test]
    fn test_match_order_id_exhaustion_before_partial_fill_leaves_next_maker_intact() {
        // One id: the first maker is consumed; the second would be partially
        // filled, but the sweep stops before touching it.
        let makers = || vec![standard(1, 10), standard(2, 10), standard(3, 10)];
        let (result, level) = exhausted_vs_control(makers, 25, 1);
        assert_eq!(result.remaining_quantity().as_u64(), 15);
        assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);
        // `exhausted_vs_control` drained the rest with a fresh generator; the
        // third maker keeps its 5-unit residual at the front.
        assert_eq!(state(&level).ids, vec![Id::from_u64(3)]);
        assert_eq!(state(&level).visible, vec![5]);
    }

    #[test]
    fn test_match_order_id_exhaustion_iceberg_replenish_keeps_counters_consistent() {
        let makers = || vec![iceberg(1, 10, 30), standard(2, 10)];
        let steps = take(
            &level_with(makers()),
            50,
            TimeInForce::Gtc,
            &fresh_generator(),
        )
        .trades()
        .len();
        assert!(steps >= 3, "scenario must replenish");
        for left in 0..u64::try_from(steps).expect("steps") {
            exhausted_vs_control(makers, 50, left);
        }
    }

    #[test]
    fn test_match_order_id_exhaustion_reserve_replenish_keeps_counters_consistent() {
        let makers = || vec![reserve(1, 10, 30), standard(2, 10)];
        let steps = take(
            &level_with(makers()),
            50,
            TimeInForce::Gtc,
            &fresh_generator(),
        )
        .trades()
        .len();
        assert!(steps >= 3, "scenario must replenish");
        for left in 0..u64::try_from(steps).expect("steps") {
            exhausted_vs_control(makers, 50, left);
        }
    }

    #[test]
    fn test_match_order_fok_insufficient_ids_killed_with_level_unchanged() {
        let makers = || (1..=3).map(|id| standard(id, 10)).collect();
        for left in 0..3 {
            let level = level_with(makers());
            let before = state(&level);
            let generator = generator_with(left);
            let result = take(&level, 30, TimeInForce::Fok, &generator);
            assert_eq!(result.outcome(), MatchOutcome::Killed, "left {left}");
            assert_id_exhausted(&result, 3);
            assert!(result.trades().is_empty());
            assert!(result.filled_order_ids().is_empty());
            assert_eq!(result.remaining_quantity().as_u64(), 30);
            assert_eq!(state(&level), before, "left {left}: level untouched");
            // All or nothing: the failed preflight reserved no id.
            assert_eq!(generator.remaining(), left);
        }

        // Exactly enough ids: fills in full and uses the final three.
        let level = level_with(makers());
        let generator = generator_with(3);
        let result = take(&level, 30, TimeInForce::Fok, &generator);
        assert!(result.error().is_none());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        let ids: Vec<Id> = result
            .trades()
            .as_vec()
            .iter()
            .map(|t| t.trade_id())
            .collect();
        let want: Vec<Id> = (UuidGenerator::EXHAUSTED - 3..UuidGenerator::EXHAUSTED)
            .map(expected_id)
            .collect();
        assert_eq!(ids, want);
        assert!(generator.is_exhausted());
        assert_counters_match_queue(&level);
    }

    /// The FOK id preflight covers the extra trades of a replenishing iceberg
    /// (more trades than resting orders) exactly.
    #[test]
    fn test_match_order_fok_id_preflight_counts_replenish_trades_exactly() {
        let makers = || vec![iceberg(1, 10, 20), standard(2, 10)];
        let control = take(
            &level_with(makers()),
            40,
            TimeInForce::Fok,
            &fresh_generator(),
        );
        assert_eq!(control.outcome(), MatchOutcome::Filled);
        let needed = u64::try_from(control.trades().len()).expect("needed");
        assert!(needed > 2, "replenish must add trades beyond order_count");

        let level = level_with(makers());
        let before = state(&level);
        let short = generator_with(needed - 1);
        let killed = take(&level, 40, TimeInForce::Fok, &short);
        assert_eq!(killed.outcome(), MatchOutcome::Killed);
        assert_id_exhausted(&killed, usize::try_from(needed).expect("needed"));
        assert_eq!(state(&level), before);
        assert_eq!(short.remaining(), needed - 1);

        let exact = generator_with(needed);
        let filled = take(&level, 40, TimeInForce::Fok, &exact);
        assert!(filled.error().is_none());
        assert_eq!(filled.outcome(), MatchOutcome::Filled);
        assert_eq!(
            filled.trades().len(),
            usize::try_from(needed).expect("needed")
        );
        assert!(exact.is_exhausted());
        assert_counters_match_queue(&level);
    }

    #[test]
    fn test_match_order_id_exhaustion_result_round_trips() {
        let level = level_with((1..=3).map(|id| standard(id, 10)).collect());
        let result = take(&level, 30, TimeInForce::Gtc, &generator_with(1));
        assert_id_exhausted(&result, 1);
        let json = serde_json::to_string(&result).expect("serialize");
        let back: MatchResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.error(), result.error());
        assert_eq!(back.trades(), result.trades());
        assert_eq!(back.remaining_quantity(), result.remaining_quantity());
        let bytes =
            bincode::serde::encode_to_vec(&result, bincode::config::standard()).expect("bincode");
        let (decoded, _): (MatchResult, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .expect("bincode decode");
        assert_eq!(decoded.error(), result.error());
    }
}
