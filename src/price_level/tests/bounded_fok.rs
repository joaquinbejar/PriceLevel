//! Issue #143: the fill-or-kill dry run walks only the prefix the sweep
//! consumes.
//!
//! The bounded dry run replaced one that materialized and sorted the whole
//! queue before simulating. That former implementation is retained here, and
//! only here, as the reference model: a property test drives random books
//! (standard, iceberg and reserve makers; resizes that demote; cancels; GTC
//! sweeps that leave partial fronts and re-sequenced tranches; self-match
//! taker ids; quantities large enough to hit the visible-counter headroom
//! abort) and requires the two dry runs to agree exactly on every field,
//! including the stop error. It also checks the fill-or-kill verdict of the
//! real `match_order` against the prediction.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::{CapacityResource, PriceLevelError};
    use crate::execution::{MatchOutcome, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::level::{
        DryRun, PriceLevel, count_park, override_lazy_walk_budget, test_take_tail_revisits,
        topology_underflow,
    };
    use crate::price_level::order_queue::snapshot_hook::{self, SnapshotHookEvent};
    use crate::price_level::order_queue::test_take_bulk_switches;
    use crate::utils::alloc::test_seam;
    use crate::utils::{Price, Quantity, TimestampMs};
    use proptest::prelude::*;
    use proptest::test_runner::TestRunner;
    use std::cell::Cell;
    use std::num::NonZeroU64;
    use std::rc::Rc;
    use std::sync::Arc;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const EXEC_TS: u64 = 4_000_000_000_000;
    const FRESH_TAKER: u64 = u64::MAX;

    /// The pre-#143 dry run, verbatim in behaviour: materialize the queue in
    /// insertion sequence, then simulate the sweep over a `VecDeque`.
    fn reference_dry_run(
        level: &PriceLevel,
        incoming_quantity: u64,
        taker_id: Id,
    ) -> Result<DryRun, PriceLevelError> {
        let mut dry = DryRun {
            filled: 0,
            trades: 0,
            replenishes: 0,
            parks: 0,
            error: None,
        };
        if incoming_quantity == 0 {
            return Ok(dry);
        }
        let mut pending: std::collections::VecDeque<Arc<OrderType<()>>> =
            level.snapshot_by_insertion_seq()?.into();
        let mut parks: usize = 0;
        let mut remaining = incoming_quantity;
        let mut filled: u64 = 0;
        let mut trades: usize = 0;
        let mut replenishes: u64 = 0;
        let mut projected_visible = level.visible_quantity();
        let mut projected_count = level.test_topology_count();

        while remaining > 0 {
            let Some(order) = pending.pop_front() else {
                break;
            };
            if order.id() == taker_id {
                match count_park(parks) {
                    Ok(count) => parks = count,
                    Err(err) => {
                        dry.error = Some(err);
                        break;
                    }
                }
                dry.parks = parks;
                continue;
            }
            let (consumed, updated_order, hidden_reduced, new_remaining) =
                match order.match_against(remaining) {
                    Ok(step) => step,
                    Err(err) => {
                        dry.error = Some(err);
                        break;
                    }
                };
            if consumed == 0
                && hidden_reduced == 0
                && new_remaining == remaining
                && updated_order.is_some()
            {
                match count_park(parks) {
                    Ok(count) => parks = count,
                    Err(err) => {
                        dry.error = Some(err);
                        break;
                    }
                }
                dry.parks = parks;
                continue;
            }
            if updated_order.is_none() {
                match projected_count.checked_sub(1) {
                    Some(next) => projected_count = next,
                    None => {
                        dry.error = Some(topology_underflow(level.price()));
                        break;
                    }
                }
            }
            if hidden_reduced > 0 {
                match projected_visible
                    .checked_sub(consumed)
                    .and_then(|v| v.checked_add(hidden_reduced))
                {
                    Some(next) => projected_visible = next,
                    None => break,
                }
            } else {
                let Some(next) = projected_visible.checked_sub(consumed) else {
                    break;
                };
                projected_visible = next;
            }
            filled = match filled.checked_add(consumed) {
                Some(total) => total,
                None => break,
            };
            if consumed > 0 {
                trades = match trades.checked_add(1) {
                    Some(count) => count,
                    None => break,
                };
            }
            if hidden_reduced > 0 && updated_order.is_some() {
                replenishes = match replenishes.checked_add(1) {
                    Some(count) => count,
                    None => break,
                };
            }
            dry.filled = filled;
            dry.trades = trades;
            dry.replenishes = replenishes;
            remaining = new_remaining;

            if let Some(updated) = updated_order {
                if hidden_reduced > 0 {
                    pending.push_back(Arc::new(updated));
                } else {
                    pending.push_front(Arc::new(updated));
                }
            }
        }
        Ok(dry)
    }

    // ------------------------------------------------------------------
    // Fixtures
    // ------------------------------------------------------------------

    #[derive(Debug, Clone)]
    enum Kind {
        Standard,
        Iceberg,
        Reserve {
            threshold: u64,
            amount: Option<u64>,
            auto: bool,
        },
    }

    fn order(id: u64, kind: &Kind, visible: u64, hidden: u64, ts: u64) -> OrderType<()> {
        let price = Price::new(PRICE);
        let timestamp = TimestampMs::new(ts);
        match kind {
            Kind::Standard => OrderType::Standard {
                id: Id::from_u64(id),
                price,
                quantity: Quantity::new(visible),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp,
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
            Kind::Iceberg => OrderType::IcebergOrder {
                id: Id::from_u64(id),
                price,
                visible_quantity: Quantity::new(visible),
                hidden_quantity: Quantity::new(hidden),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp,
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
            Kind::Reserve {
                threshold,
                amount,
                auto,
            } => OrderType::ReserveOrder {
                id: Id::from_u64(id),
                price,
                visible_quantity: Quantity::new(visible),
                hidden_quantity: Quantity::new(hidden),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp,
                time_in_force: TimeInForce::Gtc,
                replenish_threshold: Quantity::new(*threshold),
                replenish_amount: amount.and_then(NonZeroU64::new),
                auto_replenish: *auto,
                extra_fields: (),
            },
        }
    }

    fn generator() -> UuidGenerator {
        UuidGenerator::new(Uuid::nil())
    }

    #[derive(Debug, Clone)]
    enum Op {
        Add {
            kind: Kind,
            visible: u64,
            hidden: u64,
            ts: u64,
        },
        Resize {
            pick: usize,
            quantity: u64,
        },
        Cancel {
            pick: usize,
        },
        Gtc {
            quantity: u64,
        },
        /// Rests a reserve whose `match_against` fails with a typed
        /// arithmetic error (issue #169), bypassing admission as the #169
        /// tests do; the counters then no longer describe it, which also
        /// reaches the count-projection stop (issue #163).
        RestFailing,
    }

    /// Mostly small quantities, occasionally near the `u64` edge so the
    /// visible-counter headroom abort is reachable.
    fn quantity() -> impl Strategy<Value = u64> {
        prop_oneof![
            8 => 0_u64..12,
            1 => Just(u64::MAX / 2),
            1 => (u64::MAX / 3)..=(u64::MAX / 3 + 4),
        ]
    }

    fn kind() -> impl Strategy<Value = Kind> {
        prop_oneof![
            3 => Just(Kind::Standard),
            2 => Just(Kind::Iceberg),
            2 => (0_u64..6, prop::option::of(0_u64..6), any::<bool>()).prop_map(
                |(threshold, amount, auto)| Kind::Reserve { threshold, amount, auto }
            ),
        ]
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            6 => (kind(), quantity(), quantity(), 0_u64..1_000).prop_map(
                |(kind, visible, hidden, ts)| Op::Add { kind, visible, hidden, ts }
            ),
            1 => (any::<usize>(), quantity())
                .prop_map(|(pick, quantity)| Op::Resize { pick, quantity }),
            1 => any::<usize>().prop_map(|pick| Op::Cancel { pick }),
            1 => (1_u64..8).prop_map(|quantity| Op::Gtc { quantity }),
            1 => Just(Op::RestFailing),
        ]
    }

    /// Replays `ops` onto a fresh level; returns it and every id admitted.
    /// Deterministic, so two calls build identical levels.
    fn build(ops: &[Op]) -> (PriceLevel, Vec<u64>) {
        let level = PriceLevel::new(PRICE);
        let generator = generator();
        let mut ids = Vec::new();
        for (step, op) in ops.iter().enumerate() {
            let step = step as u64;
            match op {
                Op::Add {
                    kind,
                    visible,
                    hidden,
                    ts,
                } => {
                    let id = step + 1;
                    let ts = 1_616_823_000_000 + ts;
                    if level
                        .add_order(order(id, kind, *visible, *hidden, ts))
                        .is_ok()
                    {
                        ids.push(id);
                    }
                }
                Op::Resize { pick, quantity } if !ids.is_empty() => {
                    let id = ids[pick % ids.len()];
                    let _ = level.update_order(OrderUpdate::UpdateQuantity {
                        order_id: Id::from_u64(id),
                        new_quantity: Quantity::new(*quantity),
                    });
                }
                Op::Cancel { pick } if !ids.is_empty() => {
                    let id = ids[pick % ids.len()];
                    let _ = level.update_order(OrderUpdate::Cancel {
                        order_id: Id::from_u64(id),
                    });
                }
                Op::RestFailing => {
                    let id = step + 1;
                    let failing = OrderType::ReserveOrder {
                        id: Id::from_u64(id),
                        price: Price::new(PRICE),
                        visible_quantity: Quantity::new(u64::MAX),
                        hidden_quantity: Quantity::new(u64::MAX),
                        side: Side::Sell,
                        user_id: Hash32::zero(),
                        timestamp: TimestampMs::new(1_616_823_000_000),
                        time_in_force: TimeInForce::Gtc,
                        replenish_threshold: Quantity::new(u64::MAX),
                        replenish_amount: NonZeroU64::new(u64::MAX),
                        auto_replenish: true,
                        extra_fields: (),
                    };
                    if level.test_rest_unadmitted(failing).is_ok() {
                        ids.push(id);
                    }
                }
                Op::Gtc { quantity } => {
                    let _ = level.match_order(
                        *quantity,
                        Id::from_u64(1_000_000 + step),
                        TimeInForce::Gtc,
                        TakerKind::Standard,
                        TimestampMs::new(EXEC_TS),
                        &generator,
                    );
                }
                _ => {}
            }
        }
        (level, ids)
    }

    fn fok(level: &PriceLevel, quantity: u64, taker: Id) -> crate::execution::MatchResult {
        level.match_order(
            quantity,
            taker,
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(EXEC_TS),
            &generator(),
        )
    }

    /// Queue-derived view used to prove a killed fill-or-kill left the level
    /// untouched: ids in insertion order with their visible / hidden
    /// quantities, plus the level counters.
    #[derive(Debug, PartialEq)]
    struct LevelView {
        orders: Vec<(Id, u64, u64)>,
        visible: u64,
        hidden: u64,
        count: usize,
    }

    fn view(level: &PriceLevel) -> LevelView {
        LevelView {
            orders: level
                .snapshot_by_insertion_seq()
                .expect("materialize")
                .iter()
                .map(|o| {
                    (
                        o.id(),
                        o.visible_quantity().as_u64(),
                        o.hidden_quantity().as_u64(),
                    )
                })
                .collect(),
            visible: level.visible_quantity(),
            hidden: level.hidden_quantity(),
            count: level.order_count(),
        }
    }

    /// Paths the property must reach, summed over all cases.
    #[derive(Debug, Default)]
    struct Coverage {
        filled: u64,
        killed: u64,
        rejected: u64,
        stop_errors: u64,
        parks: u64,
        replenishes: u64,
        tail_revisits: u64,
        bulk_switches: u64,
    }

    fn strategy() -> impl Strategy<Value = (Vec<Op>, u64, Option<usize>, Option<u64>)> {
        (
            prop::collection::vec(op(), 0..40),
            // Bounded: every trade consumes at least one unit, so the dry run
            // and the real sweep take at most `incoming` trade steps. Huge
            // resting quantities still reach the headroom abort.
            0_u64..60,
            prop::option::of(any::<usize>()),
            // `None` keeps the production budget, `max(8, count / 64)` = 8
            // for these books, which up to 40 operations often exceed; a
            // small forced budget moves the switch to the bulk continuation
            // to every position.
            prop::option::of(0_u64..6),
        )
    }

    /// One case: the bounded dry run equals the reference on every field,
    /// the public `matchable_quantity` returns its fill, and the real
    /// fill-or-kill on the same book follows the prediction.
    fn check_case(
        (ops, incoming, self_pick, budget): (Vec<Op>, u64, Option<usize>, Option<u64>),
        coverage: &mut Coverage,
    ) -> Result<(), TestCaseError> {
        let _budget = budget.map(override_lazy_walk_budget);
        let (level, ids) = build(&ops);
        let taker = match self_pick {
            Some(pick) if !ids.is_empty() => Id::from_u64(ids[pick % ids.len()]),
            _ => Id::from_u64(FRESH_TAKER),
        };

        test_take_tail_revisits();
        test_take_bulk_switches();
        let bounded = level.test_dry_run(incoming, taker);
        coverage.tail_revisits += test_take_tail_revisits();
        coverage.bulk_switches += test_take_bulk_switches();
        let reference = reference_dry_run(&level, incoming, taker);
        prop_assert_eq!(&bounded, &reference);
        prop_assert_eq!(
            level.matchable_quantity(incoming, taker),
            reference
                .as_ref()
                .map(|dry| dry.filled)
                .map_err(Clone::clone)
        );
        let Ok(dry) = reference else {
            return Err(TestCaseError::fail("reference refused to allocate"));
        };
        coverage.parks += dry.parks as u64;
        coverage.replenishes += dry.replenishes;
        coverage.stop_errors += u64::from(dry.error.is_some());

        let resting_self = level
            .snapshot_by_insertion_seq()
            .is_ok_and(|orders| orders.iter().any(|o| o.id() == taker));
        // Parks are not observable on the real sweep, but single-threaded
        // they only come from a resting self-match maker, which
        // `match_order` rejects before the sweep: a taker that does not rest
        // here must predict none.
        if !resting_self {
            prop_assert_eq!(dry.parks, 0);
        }

        let before = view(&level);
        let seq_before = level.test_queue().test_next_seq();
        let poisoned = level.test_is_poisoned();
        let result = fok(&level, incoming, taker);
        let seq_used = level.test_queue().test_next_seq() - seq_before;

        if incoming == 0 || poisoned {
            // A poisoned level refuses every match (issue #130).
            prop_assert!(result.trades().is_empty());
            prop_assert_eq!(view(&level), before);
        } else if resting_self {
            prop_assert!(result.was_rejected());
            prop_assert!(result.trades().is_empty());
            prop_assert_eq!(view(&level), before);
            coverage.rejected += 1;
        } else if dry.error.is_none() && dry.filled == incoming {
            prop_assert_eq!(result.outcome(), MatchOutcome::Filled);
            prop_assert_eq!(result.trades().len(), dry.trades);
            prop_assert_eq!(result.executed_quantity(), Ok(Quantity::new(dry.filled)));
            // Every replenishment that keeps its maker resident reserves one
            // fresh FIFO sequence; nothing else does on this thread.
            prop_assert_eq!(seq_used, dry.replenishes);
            coverage.filled += 1;
        } else {
            prop_assert!(result.was_killed());
            prop_assert!(result.trades().is_empty());
            prop_assert_eq!(result.error().is_some(), dry.error.is_some());
            // Untouched: order count, visible and hidden counters, and every
            // resting order's quantities in queue order.
            prop_assert_eq!(view(&level), before);
            prop_assert_eq!(seq_used, 0);
            coverage.killed += 1;
        }
        Ok(())
    }

    #[test]
    fn prop_bounded_dry_run_equals_reference() {
        let mut runner = TestRunner::new(ProptestConfig {
            cases: 1_024,
            ..ProptestConfig::default()
        });

        let coverage_cell = std::cell::RefCell::new(Coverage::default());
        let outcome = runner.run(&strategy(), |case| {
            check_case(case, &mut coverage_cell.borrow_mut())
        });
        if let Err(err) = outcome {
            panic!("{err}");
        }
        let coverage = coverage_cell.into_inner();
        // The property is only as strong as the paths it reaches.
        assert!(coverage.filled > 0, "{coverage:?}");
        assert!(coverage.killed > 0, "{coverage:?}");
        assert!(coverage.rejected > 0, "{coverage:?}");
        assert!(coverage.stop_errors > 0, "{coverage:?}");
        assert!(coverage.parks > 0, "{coverage:?}");
        assert!(coverage.replenishes > 0, "{coverage:?}");
        assert!(coverage.tail_revisits > 0, "{coverage:?}");
        assert!(coverage.bulk_switches > 0, "{coverage:?}");
    }

    // ------------------------------------------------------------------
    // Bounded work
    // ------------------------------------------------------------------

    fn standard_level(depth: u64) -> PriceLevel {
        let level = PriceLevel::new(PRICE);
        for id in 0..depth {
            level
                .add_order(order(id, &Kind::Standard, 1, 0, 1))
                .expect("add");
        }
        level
    }

    #[test]
    fn small_fok_does_not_materialize_the_level() {
        // The former dry run collected every order (one `Collected` hook
        // event per order) before simulating. The bounded walk collects
        // nothing and allocates no working buffer.
        let level = standard_level(10_000);
        let collected = Rc::new(Cell::new(0_u64));
        let seen = Rc::clone(&collected);
        let _hook = snapshot_hook::install(move |event| {
            if matches!(event, SnapshotHookEvent::Collected(_)) {
                seen.set(seen.get() + 1);
            }
        });
        // Any working-buffer reservation would be refused and kill the taker.
        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        let dry = level
            .test_dry_run(1, Id::from_u64(FRESH_TAKER))
            .expect("bounded dry run");
        assert_eq!(dry.filled, 1);
        assert_eq!(dry.trades, 1);
        let result = fok(&level, 1, Id::from_u64(FRESH_TAKER));
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(collected.get(), 0, "no queue materialization");
        assert_eq!(test_seam::injected(), 0, "no working buffer reserved");
        assert_eq!(level.order_count(), 9_999);
    }

    #[test]
    fn replenished_tranche_is_revisited_behind_the_current_queue() {
        // iceberg(1): 2 visible + 4 hidden; standard(2): 3. A 7-unit taker
        // takes 2 (iceberg, replenished to the tail), 3 (standard), then 2
        // from the re-queued tranche (replenished again): three trades, two
        // replenishes.
        let level = PriceLevel::new(PRICE);
        level
            .add_order(order(1, &Kind::Iceberg, 2, 4, 1))
            .expect("add");
        level
            .add_order(order(2, &Kind::Standard, 3, 0, 2))
            .expect("add");
        let taker = Id::from_u64(FRESH_TAKER);
        let dry = level.test_dry_run(7, taker).expect("dry run");
        assert_eq!(dry, reference_dry_run(&level, 7, taker).expect("reference"));
        assert_eq!((dry.filled, dry.trades, dry.replenishes), (7, 3, 2));

        // The buffered tranche is the one growth site: refusing it is the
        // typed error, and the fill-or-kill is killed with the level intact.
        {
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
            assert!(matches!(
                level.test_dry_run(7, taker),
                Err(PriceLevelError::CapacityExceeded {
                    resource: CapacityResource::OrderSnapshot,
                    additional: 1,
                })
            ));
            let killed = fok(&level, 7, taker);
            assert!(killed.was_killed());
            assert!(killed.error().is_some());
        }
        assert_eq!(level.visible_quantity(), 5);
        let filled = fok(&level, 7, taker);
        assert_eq!(filled.outcome(), MatchOutcome::Filled);
        assert_eq!(filled.trades().len(), 3);
    }

    #[test]
    fn replenish_that_exhausts_the_taker_buffers_nothing() {
        // The 2-unit taker ends on the iceberg's replenish: the tranche would
        // never be revisited, so it is not buffered (no reservation).
        let level = PriceLevel::new(PRICE);
        level
            .add_order(order(1, &Kind::Iceberg, 2, 4, 1))
            .expect("add");
        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        let dry = level
            .test_dry_run(2, Id::from_u64(FRESH_TAKER))
            .expect("dry run");
        assert_eq!((dry.filled, dry.trades, dry.replenishes), (2, 1, 1));
        assert_eq!(test_seam::injected(), 0);
    }

    #[test]
    fn headroom_abort_and_self_park_match_the_reference() {
        // A zero-visible iceberg draws its whole hidden tranche while the
        // visible counter is already near `u64::MAX`: the sweep would abort
        // at that maker, and the bounded dry run stops at the same point.
        let level = PriceLevel::new(PRICE);
        level
            .add_order(order(1, &Kind::Standard, 1, 0, 1))
            .expect("add");
        level
            .add_order(order(2, &Kind::Iceberg, 0, u64::MAX / 2, 2))
            .expect("add");
        level
            .add_order(order(3, &Kind::Standard, u64::MAX / 2 + 10, 0, 3))
            .expect("add");
        level
            .add_order(order(4, &Kind::Standard, 5, 0, 4))
            .expect("add");
        for (quantity, taker) in [(3, FRESH_TAKER), (3, 1), (3, 2), (u64::MAX, FRESH_TAKER)] {
            let taker = Id::from_u64(taker);
            assert_eq!(
                level.test_dry_run(quantity, taker),
                reference_dry_run(&level, quantity, taker)
            );
        }
        // Fresh taker: one unit from maker 1, then the abort at maker 2.
        let dry = level
            .test_dry_run(3, Id::from_u64(FRESH_TAKER))
            .expect("dry run");
        assert_eq!((dry.filled, dry.trades, dry.parks), (1, 1, 0));
        // Taker 2 shares the iceberg's id: it is parked, not drawn, so the
        // fill continues into maker 3.
        let dry = level.test_dry_run(3, Id::from_u64(2)).expect("dry run");
        assert_eq!((dry.filled, dry.trades, dry.parks), (3, 2, 1));
        let killed = fok(&level, 3, Id::from_u64(FRESH_TAKER));
        assert!(killed.was_killed());
        assert!(killed.trades().is_empty());
    }

    #[test]
    fn a_walk_past_the_budget_continues_in_bulk_and_matches_the_reference() {
        // 300 makers: the production budget is `max(8, 300 / 64)` = 8. A
        // FOK filled within 8 makers stays lazy (no collection); a larger
        // one crosses into the bulk continuation, which collects only the
        // makers after the lazy prefix; a rejected one collects the same
        // remainder.
        let level = standard_level(300);
        let taker = Id::from_u64(FRESH_TAKER);
        let collected = Rc::new(Cell::new(0_u64));
        let seen = Rc::clone(&collected);
        let _hook = snapshot_hook::install(move |event| {
            if matches!(event, SnapshotHookEvent::Collected(_)) {
                seen.set(seen.get() + 1);
            }
        });
        for (quantity, lazy_only) in [(1, true), (8, true), (9, false), (200, false), (301, false)]
        {
            collected.set(0);
            let bounded = level.test_dry_run(quantity, taker);
            assert_eq!(collected.get(), if lazy_only { 0 } else { 300 - 8 });
            collected.set(0);
            assert_eq!(bounded, reference_dry_run(&level, quantity, taker));
        }

        // The bulk continuation is fallible like the former snapshot.
        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        assert!(level.test_dry_run(8, taker).is_ok());
        assert!(matches!(
            level.test_dry_run(9, taker),
            Err(PriceLevelError::CapacityExceeded {
                resource: CapacityResource::OrderSnapshot,
                ..
            })
        ));
        let killed = fok(&level, 9, taker);
        assert!(killed.was_killed());
        assert_eq!(level.order_count(), 300);
    }

    #[test]
    fn the_budget_scales_with_depth() {
        // `max(8, count / 64)`: 10,000 orders give a 156-maker lazy phase.
        let level = standard_level(10_000);
        let taker = Id::from_u64(FRESH_TAKER);
        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        assert_eq!(level.test_dry_run(156, taker).map(|d| d.filled), Ok(156));
        assert!(level.test_dry_run(157, taker).is_err());
    }

    // ------------------------------------------------------------------
    // Unguarded walk: no double count under re-sequencing (review item 1)
    // ------------------------------------------------------------------

    /// Runs `mutation` on a helper thread and waits for it: a snapshot hook
    /// may fire while the walking thread holds a shard read lock.
    fn run_concurrently(
        level: &Arc<PriceLevel>,
        mutation: impl FnOnce(&PriceLevel) + Send + 'static,
    ) {
        let level = Arc::clone(level);
        std::thread::spawn(move || mutation(&level))
            .join()
            .expect("concurrent mutation thread panicked");
    }

    /// Demoting resize: a visible increase re-sequences the maker at the
    /// tail, inserting its new index key before removing the old one.
    fn demote(level: &PriceLevel, id: u64, quantity: u64) {
        level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(id),
                new_quantity: Quantity::new(quantity),
            })
            .expect("resize")
            .expect("resting");
    }

    /// Two 5-unit makers in two distinct order-storage shards (so a hook
    /// running under one shard's read lock can resize the other), as
    /// `(first in map walk order, second)`.
    fn two_shard_level() -> (Arc<PriceLevel>, u64, u64) {
        for first in 1..4_096_u64 {
            let level = PriceLevel::new(PRICE);
            level
                .add_order(order(first, &Kind::Standard, 5, 0, 1))
                .expect("add");
            for second in first + 1..first + 64 {
                level
                    .add_order(order(second, &Kind::Standard, 5, 0, 2))
                    .expect("add");
                let runs = level.test_shard_runs();
                if runs.len() == 2 {
                    let head = runs[0][0];
                    let (a, b) = if head == Id::from_u64(first) {
                        (first, second)
                    } else {
                        (second, first)
                    };
                    return (Arc::new(level), a, b);
                }
                level
                    .update_order(OrderUpdate::Cancel {
                        order_id: Id::from_u64(second),
                    })
                    .expect("cancel")
                    .expect("resting");
            }
        }
        panic!("could not place two makers in distinct shards");
    }

    #[test]
    fn the_lazy_walk_would_double_count_a_maker_resequenced_mid_walk() {
        // Why the lazy phase needs the fill-or-kill guard: makers A (5) and
        // B (5); after the walk yields A, A is resized to 7, which moves it
        // to a new tail key. The index walk then meets A again at that key.
        let level = Arc::new(PriceLevel::new(PRICE));
        level
            .add_order(order(1, &Kind::Standard, 5, 0, 1))
            .expect("add");
        level
            .add_order(order(2, &Kind::Standard, 5, 0, 2))
            .expect("add");
        let fired = Rc::new(Cell::new(false));
        let _hook = snapshot_hook::install({
            let (fired, level) = (Rc::clone(&fired), Arc::clone(&level));
            move |event| {
                if event == SnapshotHookEvent::LazyYield(Id::from_u64(1)) && !fired.get() {
                    fired.set(true);
                    run_concurrently(&level, |l| demote(l, 1, 7));
                }
            }
        });
        let mut walk = level.test_queue().seq_walk(64);
        let mut total = 0;
        while let Some(order) = walk.try_next().expect("walk") {
            total += order.visible_quantity().as_u64();
        }
        assert!(fired.get());
        assert_eq!(total, 17, "A counted at 5 and again at 7");
    }

    #[test]
    fn unguarded_matchable_quantity_never_double_counts_a_resequenced_maker() {
        // The same demotion lands while the public, unguarded
        // `matchable_quantity` walks the level: after the first maker is
        // collected the other is resized 5 -> 7 (re-sequenced at the tail).
        // Each maker is collected once, so the estimate is at most the larger
        // committed total (10 before, 12 after), never 17.
        for resize_first_collected in [false, true] {
            let (level, a, b) = two_shard_level();
            let target = if resize_first_collected { a } else { b };
            let fired = Rc::new(Cell::new(false));
            let yielded = Rc::new(Cell::new(0_u32));
            let _hook = snapshot_hook::install({
                let (fired, yielded, level) =
                    (Rc::clone(&fired), Rc::clone(&yielded), Arc::clone(&level));
                move |event| match event {
                    SnapshotHookEvent::Collected(id) if id == Id::from_u64(a) && !fired.get() => {
                        fired.set(true);
                        // `a`'s shard is read-locked here; `target` may be `a`
                        // itself, so resize from a helper thread only once the
                        // lock can be taken: `b` directly, `a` after release.
                        if target == b {
                            run_concurrently(&level, move |l| demote(l, b, 7));
                        }
                    }
                    SnapshotHookEvent::Collected(id)
                        if id == Id::from_u64(b) && target == a && fired.get() =>
                    {
                        run_concurrently(&level, move |l| demote(l, a, 7));
                    }
                    SnapshotHookEvent::LazyYield(_) => yielded.set(yielded.get() + 1),
                    _ => {}
                }
            });
            let estimate = level
                .matchable_quantity(100, Id::from_u64(FRESH_TAKER))
                .expect("estimate");
            assert!(fired.get());
            assert_eq!(yielded.get(), 0, "the unguarded walk skips the lazy phase");
            // Each maker is collected once, so the estimate never exceeds the
            // larger committed total. It may undercount (stale): the replay
            // projects the visible counter read before the walk, and a maker
            // collected at its resized quantity can exceed that projection,
            // which stops the replay conservatively.
            assert!(
                estimate <= 12,
                "estimate {estimate} exceeds every committed total: double count"
            );
            assert_eq!(level.visible_quantity(), 12);
        }
    }
}
