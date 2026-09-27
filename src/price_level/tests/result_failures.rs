//! Issue #170 / #164 contract: `PriceLevel::match_order` under result-growth
//! and result-recording failures. Every test checks that the returned
//! `MatchResult` reports exactly the committed fills (a FIFO prefix of the
//! unconstrained sweep), carries the typed error, and that the level's
//! counters, topology and statistics agree with its queue.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::{CapacityResource, PriceLevelError};
    use crate::execution::{MatchOutcome, MatchResult, TakerKind};
    use crate::execution::{match_result_seam, trade_list_seam};
    use crate::orders::{Hash32, Id, OrderType, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use uuid::Uuid;

    const PRICE: u128 = 10_000;

    fn standard(id: u64, quantity: u64, side: Side) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side,
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

    fn generator() -> UuidGenerator {
        UuidGenerator::new(
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("namespace"),
        )
    }

    fn take(level: &PriceLevel, quantity: u64, tif: TimeInForce) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(999),
            tif,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator(),
        )
    }

    /// Queue-derived view of the level used to compare states.
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

    /// The live counters agree with the queue they describe.
    fn assert_counters_match_queue(level: &PriceLevel) {
        let s = state(level);
        assert_eq!(s.visible_counter, s.visible.iter().sum::<u64>(), "visible");
        assert_eq!(s.hidden_counter, s.hidden.iter().sum::<u64>(), "hidden");
        assert_eq!(s.order_count, s.ids.len(), "order_count");
    }

    fn executed(result: &MatchResult) -> u64 {
        result.executed_quantity().expect("executed").as_u64()
    }

    fn assert_capacity_error(result: &MatchResult) {
        match result.error() {
            Some(PriceLevelError::CapacityExceeded {
                resource: CapacityResource::Trades,
                ..
            }) => {}
            other => panic!("expected CapacityExceeded(trades), got {other:?}"),
        }
    }

    /// Runs `orders` / `quantity` twice: unconstrained (control) and with the
    /// trade list capped at `limit`. Asserts the #164 contract on the capped
    /// run and returns both results.
    fn capped_vs_control(
        orders: impl Fn() -> Vec<OrderType<()>>,
        quantity: u64,
        limit: usize,
    ) -> (MatchResult, MatchResult, PriceLevel) {
        let control_level = level_with(orders());
        let control = take(&control_level, quantity, TimeInForce::Gtc);
        assert!(control.error().is_none());
        assert!(
            control.trades().len() > limit,
            "scenario must need more than {limit} trades"
        );

        let level = level_with(orders());
        let capped = {
            let _limit = trade_list_seam::limit_trades(limit);
            take(&level, quantity, TimeInForce::Gtc)
        };

        assert_capacity_error(&capped);
        // Committed fills are a FIFO prefix of the unconstrained sweep.
        assert_eq!(capped.trades().len(), limit);
        for (got, want) in capped
            .trades()
            .as_vec()
            .iter()
            .zip(control.trades().as_vec())
        {
            assert_eq!(got.maker_order_id(), want.maker_order_id());
            assert_eq!(got.quantity(), want.quantity());
        }
        // Remaining is the true residual of the committed prefix.
        assert_eq!(
            capped.remaining_quantity().as_u64(),
            quantity - executed(&capped)
        );
        assert!(!capped.is_complete());
        assert_eq!(
            capped.outcome(),
            if limit == 0 {
                MatchOutcome::NotFilled
            } else {
                MatchOutcome::PartiallyFilled
            }
        );
        // Filled ids: exactly the makers the prefix removed from the level.
        for id in capped.filled_order_ids() {
            assert!(
                level
                    .snapshot_by_insertion_seq()
                    .iter()
                    .all(|o| o.id() != *id)
            );
        }
        assert_counters_match_queue(&level);
        let s = state(&level);
        assert_eq!(s.orders_executed, limit, "stats count committed fills only");
        assert_eq!(s.quantity_executed, executed(&capped));

        // The level remains usable: a follow-up unconstrained sweep takes the
        // rest in FIFO order and the combined stream equals the control.
        let rest = take(
            &level,
            capped.remaining_quantity().as_u64(),
            TimeInForce::Gtc,
        );
        assert!(rest.error().is_none());
        let combined: Vec<(Id, Quantity)> = capped
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

        (capped, control, level)
    }

    #[test]
    fn standard_sweep_growth_failure_reports_prefix_and_error() {
        let makers = || (1..=5).map(|id| standard(id, 10, Side::Sell)).collect();
        let (capped, _, _) = capped_vs_control(makers, 50, 2);
        assert_eq!(
            capped.filled_order_ids(),
            &[Id::from_u64(1), Id::from_u64(2)]
        );
    }

    #[test]
    fn growth_failure_before_first_fill_leaves_level_unchanged() {
        let makers = || (1..=3).map(|id| standard(id, 10, Side::Sell)).collect();
        let before = state(&level_with(makers()));
        let (capped, _, _) = capped_vs_control(makers, 30, 0);
        assert!(capped.trades().is_empty());
        assert_eq!(capped.remaining_quantity().as_u64(), 30);
        // Recheck the untouched state on a fresh capped run.
        let level = level_with(makers());
        {
            let _limit = trade_list_seam::limit_trades(0);
            let result = take(&level, 30, TimeInForce::Gtc);
            assert_capacity_error(&result);
        }
        assert_eq!(state(&level), before);
    }

    #[test]
    fn iceberg_sweep_growth_failure_keeps_counters_consistent() {
        let makers = || vec![iceberg(1, 10, 30), standard(2, 10, Side::Sell)];
        let steps = take(&level_with(makers()), 50, TimeInForce::Gtc)
            .trades()
            .len();
        assert!(steps >= 2, "scenario must take several steps");
        for limit in 0..steps {
            capped_vs_control(makers, 50, limit);
        }
    }

    #[test]
    fn reserve_sweep_growth_failure_keeps_counters_consistent() {
        let makers = || vec![reserve(1, 10, 30), standard(2, 10, Side::Sell)];
        let steps = take(&level_with(makers()), 50, TimeInForce::Gtc)
            .trades()
            .len();
        assert!(steps >= 2, "scenario must take several steps");
        for limit in 0..steps {
            capped_vs_control(makers, 50, limit);
        }
    }

    /// #170 comment regression: a failure recording a fill AFTER `match_front`
    /// committed the maker mutation used to `break` before the step's
    /// bookkeeping, leaving `order_count`, the hidden counter and the
    /// single-side topology pinned while the queue had moved on.
    #[test]
    fn post_commit_record_failure_completes_step_bookkeeping() {
        // Buy makers; draining the level must release the Buy topology pin.
        let level = level_with(vec![
            standard(1, 10, Side::Buy),
            OrderType::IcebergOrder {
                id: Id::from_u64(2),
                price: Price::new(PRICE),
                visible_quantity: Quantity::new(10),
                hidden_quantity: Quantity::new(0),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_002),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
        ]);
        let result = {
            let _fail = match_result_seam::fail_add_trade_after(1);
            take(&level, 20, TimeInForce::Gtc)
        };

        // The first fill is reported; the second (committed, unrecordable)
        // stops the sweep with the error set.
        assert!(matches!(
            result.error(),
            Some(PriceLevelError::InvalidOperation { .. })
        ));
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);
        // Remaining is the taker's true residual: both makers were consumed.
        assert_eq!(result.remaining_quantity().as_u64(), 0);

        // Queue drained; every counter agrees with it.
        assert_counters_match_queue(&level);
        let s = state(&level);
        assert_eq!(s.order_count, 0);
        assert_eq!(s.orders_executed, 2, "the committed fill is in the stats");
        assert_eq!(s.quantity_executed, 20);

        // Topology was released: the empty level accepts the other side.
        level
            .add_order(standard(3, 5, Side::Sell))
            .expect("drained level must accept the opposite side");
        assert_counters_match_queue(&level);
    }

    #[test]
    fn fok_growth_failure_is_killed_with_level_unchanged() {
        let makers = || (1..=3).map(|id| standard(id, 10, Side::Sell)).collect();
        for limit in 0..3 {
            let level = level_with(makers());
            let before = state(&level);
            let result = {
                let _limit = trade_list_seam::limit_trades(limit);
                take(&level, 30, TimeInForce::Fok)
            };
            assert_eq!(result.outcome(), MatchOutcome::Killed, "limit {limit}");
            assert_capacity_error(&result);
            assert!(result.trades().is_empty());
            assert!(result.filled_order_ids().is_empty());
            assert_eq!(result.remaining_quantity().as_u64(), 30);
            assert_eq!(state(&level), before, "limit {limit}: level untouched");
        }

        // Exactly enough room: the preflight reservation covers the sweep.
        let level = level_with(makers());
        let result = {
            let _limit = trade_list_seam::limit_trades(3);
            take(&level, 30, TimeInForce::Fok)
        };
        assert!(result.error().is_none());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(result.trades().len(), 3);
        assert_counters_match_queue(&level);
    }

    /// The FOK preflight reserves the EXACT trade count, including the extra
    /// trades a replenishing iceberg emits (more trades than resting orders).
    #[test]
    fn fok_preflight_counts_replenish_trades_exactly() {
        let makers = || vec![iceberg(1, 10, 20), standard(2, 10, Side::Sell)];
        let control_level = level_with(makers());
        let control = take(&control_level, 40, TimeInForce::Fok);
        assert_eq!(control.outcome(), MatchOutcome::Filled);
        let needed = control.trades().len();
        assert!(needed > 2, "replenish must add trades beyond order_count");

        let level = level_with(makers());
        let before = state(&level);
        let killed = {
            let _limit = trade_list_seam::limit_trades(needed - 1);
            take(&level, 40, TimeInForce::Fok)
        };
        assert_eq!(killed.outcome(), MatchOutcome::Killed);
        assert_capacity_error(&killed);
        assert_eq!(state(&level), before);

        let filled = {
            let _limit = trade_list_seam::limit_trades(needed);
            take(&level, 40, TimeInForce::Fok)
        };
        assert!(filled.error().is_none());
        assert_eq!(filled.outcome(), MatchOutcome::Filled);
        assert_eq!(filled.trades().as_vec(), control.trades().as_vec());
        assert_counters_match_queue(&level);
    }

    /// A failed result produced by the engine round-trips with its error.
    #[test]
    fn engine_failed_result_round_trips() {
        let level = level_with((1..=3).map(|id| standard(id, 10, Side::Sell)).collect());
        let result = {
            let _limit = trade_list_seam::limit_trades(1);
            take(&level, 30, TimeInForce::Ioc)
        };
        assert_capacity_error(&result);
        let json = serde_json::to_string(&result).expect("json");
        let decoded: MatchResult = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded.error(), result.error());
        assert_eq!(decoded.trades(), result.trades());
        let bytes =
            bincode::serde::encode_to_vec(&result, bincode::config::standard()).expect("bincode");
        let (decoded, _): (MatchResult, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .expect("bincode decode");
        assert_eq!(decoded.error(), result.error());
        assert_eq!(decoded.remaining_quantity(), result.remaining_quantity());
    }
}
