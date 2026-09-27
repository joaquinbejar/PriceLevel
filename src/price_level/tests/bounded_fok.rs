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
        DryRun, PriceLevel, count_park, override_lazy_walk_budget, topology_underflow,
    };
    use crate::price_level::order_queue::snapshot_hook::{self, SnapshotHookEvent};
    use crate::utils::alloc::test_seam;
    use crate::utils::{Price, Quantity, TimestampMs};
    use proptest::prelude::*;
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

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1_024, ..ProptestConfig::default() })]

        /// The bounded dry run equals the reference on every field, for the
        /// public `matchable_quantity` too, and the real fill-or-kill verdict
        /// follows the shared prediction.
        #[test]
        fn prop_bounded_dry_run_equals_reference(
            ops in prop::collection::vec(op(), 0..40),
            // Bounded: every trade consumes at least one unit, so the dry run
            // and the real sweep take at most `incoming` trade steps. Huge
            // resting quantities (above) still reach the headroom abort.
            incoming in 0_u64..60,
            self_pick in prop::option::of(any::<usize>()),
            // `None` keeps the production budget (every book here fits in
            // its lazy phase); a small forced budget moves the walk into the
            // bulk continuation at every possible point.
            budget in prop::option::of(0_u64..6),
        ) {
            let _budget = budget.map(override_lazy_walk_budget);
            let (level, ids) = build(&ops);
            let taker = match self_pick {
                Some(pick) if !ids.is_empty() => Id::from_u64(ids[pick % ids.len()]),
                _ => Id::from_u64(FRESH_TAKER),
            };

            let bounded = level.test_dry_run(incoming, taker);
            let reference = reference_dry_run(&level, incoming, taker);
            prop_assert_eq!(&bounded, &reference);
            prop_assert_eq!(
                level.matchable_quantity(incoming, taker),
                reference.as_ref().map(|dry| dry.filled).map_err(Clone::clone)
            );

            // The real fill-or-kill on the same book. A self-match taker is
            // rejected before the dry run; otherwise the verdict is decided by
            // the prediction (every other preflight has headroom here).
            let before = level.visible_quantity();
            let poisoned = level.test_is_poisoned();
            let result = fok(&level, incoming, taker);
            let Ok(dry) = reference else {
                return Err(TestCaseError::fail("reference refused to allocate"));
            };
            let resting_self = ids.iter().any(|id| Id::from_u64(*id) == taker)
                && level.snapshot_by_insertion_seq().is_ok_and(|orders| {
                    orders.iter().any(|o| o.id() == taker)
                });
            if incoming == 0 || poisoned {
                // A poisoned level refuses every match (issue #130).
                prop_assert!(result.trades().is_empty());
            } else if resting_self || result.was_rejected() {
                prop_assert!(result.was_rejected());
                prop_assert!(result.trades().is_empty());
            } else if dry.error.is_none() && dry.filled == incoming {
                prop_assert_eq!(result.outcome(), MatchOutcome::Filled);
                prop_assert_eq!(result.trades().len(), dry.trades);
            } else {
                prop_assert!(result.was_killed());
                prop_assert!(result.trades().is_empty());
                prop_assert_eq!(level.visible_quantity(), before);
            }
        }
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
}
