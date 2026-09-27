//! Issue #163: engine invariant checks are transactional typed failures.
//!
//! * The topology release is checked (`Result<bool, _>`), validated BEFORE
//!   the destructive queue removal it follows, and a post-removal failure
//!   poisons the level instead of being a silent no-op.
//! * `OrderQueue::update_entry_with` validates the decided order id BEFORE the
//!   caller's counter reservation runs, so a rejected decision leaves no
//!   reservation behind.
//! * Update counter deltas are checked `CounterDelta`s; width-sensitive
//!   conversions (sweep capacity, order count, restore packing) are checked.
//!
//! The injected states (a zero count while orders rest, a decision carrying a
//! foreign id) are impossible by construction through the public API; the
//! tests reach them only through `#[cfg(test)]` seams. Externally reachable
//! inputs (absent ids, oversized quantities) are tested through the public
//! API.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::PriceLevelError;
    use crate::execution::{MatchOutcome, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::level::{
        CounterDelta, PriceLevel, UpdatePlan, set_update_decision_hook, sweep_capacity_hint,
    };
    use crate::price_level::order_queue::{OrderQueue, UpdateDecision};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const TAKER: u64 = 9_999;

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

    fn generator() -> UuidGenerator {
        UuidGenerator::new(Uuid::from_u128(163))
    }

    /// Queue contents in consumption (insertion-sequence) order.
    fn fifo(level: &PriceLevel) -> Vec<OrderType<()>> {
        level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| **o)
            .collect()
    }

    fn fifo_ids(level: &PriceLevel) -> Vec<Id> {
        level
            .snapshot_by_insertion_seq()
            .iter()
            .map(|o| o.id())
            .collect()
    }

    fn assert_invalid_operation(err: Option<&PriceLevelError>, needle: &str) {
        match err {
            Some(PriceLevelError::InvalidOperation { message }) => {
                assert!(message.contains(needle), "message: {message}");
            }
            other => panic!("expected InvalidOperation containing {needle:?}, got {other:?}"),
        }
    }

    /// Three admitted sell makers of 5 each, FIFO 1, 2, 3.
    fn three_makers() -> PriceLevel {
        let level = PriceLevel::new(PRICE);
        for id in 1..=3 {
            level.add_order(standard(id, 5)).expect("admit maker");
        }
        level
    }

    // ---------------------------------------------------------------------
    // topology_release_one / release_after_removal
    // ---------------------------------------------------------------------

    #[test]
    fn test_topology_release_one_zero_count_returns_error_word_unchanged() {
        let level = PriceLevel::new(PRICE);
        let err = level.test_topology_release_one().expect_err("underflow");
        assert_invalid_operation(Some(&err), "underflow");
        assert_eq!(level.test_topology_count(), 0);
        assert_eq!(level.order_count(), 0);
        assert!(
            !level.test_is_poisoned(),
            "the bare release does not poison"
        );
    }

    #[test]
    fn test_topology_release_one_success_reports_unpin() {
        let level = PriceLevel::new(PRICE);
        level.test_force_topology(Some(Side::Sell), 2);
        assert_eq!(level.test_topology_release_one(), Ok(false));
        assert_eq!(level.test_topology_count(), 1);
        assert_eq!(level.test_topology_release_one(), Ok(true));
        assert_eq!(level.test_topology_count(), 0);
        // Un-pinned: an opposite-side admission now establishes.
        let mut buy = standard(7, 1);
        if let OrderType::Standard { side, .. } = &mut buy {
            *side = Side::Buy;
        }
        assert!(level.add_order(buy).is_ok());
    }

    #[test]
    fn test_release_after_removal_underflow_poisons_level() {
        let level = three_makers();
        level.test_force_topology(Some(Side::Sell), 0);
        let err = level.test_release_after_removal().expect_err("underflow");
        assert_invalid_operation(Some(&err), "underflow");
        assert!(level.test_is_poisoned(), "post-removal failure poisons");
        // Fail fast afterwards: mutators refuse with a typed error.
        let refused = level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(1),
        });
        assert_invalid_operation(refused.as_ref().err(), "poisoned");
        assert_eq!(fifo_ids(&level).len(), 3, "the refusal removed nothing");
    }

    // ---------------------------------------------------------------------
    // Cancel / price-move removals validate before removing
    // ---------------------------------------------------------------------

    #[test]
    fn test_cancel_with_zero_count_rejected_before_removal() {
        let level = three_makers();
        level.test_force_topology(Some(Side::Sell), 0);
        let before = fifo(&level);
        let visible_before = level.visible_quantity();
        let hidden_before = level.hidden_quantity();
        let removed_before = level.stats().orders_removed();

        let updates = [
            OrderUpdate::Cancel {
                order_id: Id::from_u64(2),
            },
            OrderUpdate::UpdatePrice {
                order_id: Id::from_u64(2),
                new_price: Price::new(PRICE + 1),
            },
            OrderUpdate::UpdatePriceAndQuantity {
                order_id: Id::from_u64(2),
                new_price: Price::new(PRICE + 1),
                new_quantity: Quantity::new(3),
            },
            OrderUpdate::Replace {
                order_id: Id::from_u64(2),
                price: Price::new(PRICE + 1),
                quantity: Quantity::new(3),
                side: Side::Sell,
            },
        ];
        for update in updates {
            let result = level.update_order(update);
            assert_invalid_operation(result.as_ref().err(), "underflow");

            // Nothing mutated: queue, priority, counters, topology, stats.
            assert_eq!(fifo(&level), before, "{update:?}");
            assert_eq!(level.visible_quantity(), visible_before);
            assert_eq!(level.hidden_quantity(), hidden_before);
            assert_eq!(level.test_topology_count(), 0);
            assert_eq!(level.stats().orders_removed(), removed_before);
            assert!(
                !level.test_is_poisoned(),
                "a pre-removal rejection mutated nothing, so it does not poison"
            );
        }

        // An absent id is an ordinary "not found", even with the bad count.
        let absent = level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(404),
        });
        assert_eq!(absent, Ok(None));
    }

    #[test]
    fn test_cancel_success_releases_count_and_unpins_on_drain() {
        let level = three_makers();
        for id in 1..=3 {
            let removed = level
                .update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(id),
                })
                .expect("cancel");
            assert_eq!(removed.map(|o| o.id()), Some(Id::from_u64(id)));
        }
        assert_eq!(level.order_count(), 0);
        assert_eq!(level.visible_quantity(), 0);
        assert!(!level.test_is_poisoned());
        assert_eq!(level.stats().orders_removed(), 3);
    }

    // ---------------------------------------------------------------------
    // Match sweep: validation before removal, #164 prefix contract
    // ---------------------------------------------------------------------

    #[test]
    fn test_sweep_topology_underflow_stops_with_prefix_and_error() {
        for tif in [TimeInForce::Gtc, TimeInForce::Ioc] {
            let level = three_makers();
            // The count claims one resting order while three rest.
            level.test_force_topology(Some(Side::Sell), 1);

            let result = level.match_order(
                15,
                Id::from_u64(TAKER),
                tif,
                TakerKind::Standard,
                TimestampMs::new(1_716_000_000_000),
                &generator(),
            );

            // Committed prefix: maker 1 only; maker 2's removal was rejected
            // before mutation.
            assert_invalid_operation(result.error(), "underflow");
            assert!(result.is_failed(), "{tif:?}");
            assert_eq!(result.trades().len(), 1);
            let trade = &result.trades().as_vec()[0];
            assert_eq!(trade.maker_order_id(), Id::from_u64(1));
            assert_eq!(trade.quantity(), Quantity::new(5));
            assert_eq!(result.remaining_quantity(), Quantity::new(10));
            assert!(!result.is_complete());
            assert_eq!(result.executed_quantity().expect("sum"), Quantity::new(5));
            assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);

            // Counters moved by exactly the committed fill; makers 2 and 3
            // rest untouched in FIFO order.
            assert_eq!(level.visible_quantity(), 10);
            assert_eq!(level.test_topology_count(), 0);
            assert_eq!(fifo(&level), vec![standard(2, 5), standard(3, 5)]);
            assert!(
                !level.test_is_poisoned(),
                "the failure was detected before mutation"
            );
        }
    }

    #[test]
    fn test_sweep_topology_underflow_on_front_maker_emits_no_trade() {
        let level = three_makers();
        level.test_force_topology(Some(Side::Sell), 0);
        let before = fifo(&level);
        let ids = generator();
        let ids_before = ids.remaining();

        let result = level.match_order(
            5,
            Id::from_u64(TAKER),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &ids,
        );

        assert_invalid_operation(result.error(), "underflow");
        assert!(result.trades().is_empty());
        // Check order (#163 before #168): the count check is pure and runs
        // before the trade-id reservation, so no id was consumed.
        assert_eq!(ids.remaining(), ids_before);
        assert_eq!(result.remaining_quantity(), Quantity::new(5));
        assert_eq!(fifo(&level), before);
        assert_eq!(level.visible_quantity(), 15);
    }

    #[test]
    fn test_sweep_partial_fill_does_not_need_a_release() {
        // A partial fill keeps the maker resident, so no count is released and
        // the zero count is never consulted: the fill succeeds.
        let level = three_makers();
        level.test_force_topology(Some(Side::Sell), 0);
        let result = level.match_order(
            3,
            Id::from_u64(TAKER),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &generator(),
        );
        assert!(result.error().is_none());
        assert!(result.is_complete());
        assert_eq!(fifo(&level)[0], standard(1, 2));
    }

    #[test]
    fn test_fok_topology_underflow_killed_before_first_mutation() {
        let level = three_makers();
        level.test_force_topology(Some(Side::Sell), 1);
        let before = fifo(&level);

        // Dry-run parity: the prediction stops where the sweep would.
        assert_eq!(level.matchable_quantity(15, Id::from_u64(TAKER)), 5);

        let result = level.match_order(
            15,
            Id::from_u64(TAKER),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &generator(),
        );

        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_invalid_operation(result.error(), "underflow");
        assert!(result.trades().is_empty());
        assert_eq!(result.remaining_quantity(), Quantity::new(15));
        // Level untouched: queue, priority, counters, topology.
        assert_eq!(fifo(&level), before);
        assert_eq!(level.visible_quantity(), 15);
        assert_eq!(level.test_topology_count(), 1);
        assert!(!level.test_is_poisoned());
    }

    #[test]
    fn test_fok_covered_by_valid_prefix_still_fills() {
        // Count 1 covers exactly one full consume: a FOK of 5 fills.
        let level = three_makers();
        level.test_force_topology(Some(Side::Sell), 1);
        let result = level.match_order(
            5,
            Id::from_u64(TAKER),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &generator(),
        );
        assert!(result.error().is_none());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(fifo_ids(&level), vec![Id::from_u64(2), Id::from_u64(3)]);
    }

    // ---------------------------------------------------------------------
    // update_entry: id validation before reservation
    // ---------------------------------------------------------------------

    #[test]
    fn test_update_with_foreign_id_decision_rejected_without_reservation() {
        let level = PriceLevel::new(PRICE);
        level.add_order(standard(1, 10)).expect("admit 1");
        level.add_order(iceberg(2, 10, 40)).expect("admit 2");
        level.add_order(standard(3, 10)).expect("admit 3");
        let before = fifo(&level);
        let visible_before = level.visible_quantity();
        let hidden_before = level.hidden_quantity();

        // Increase (ReplaceAtTail) and decrease (KeepInPlace) decisions.
        for new_quantity in [25, 4] {
            let _hook = set_update_decision_hook(Box::new(|decided| {
                Arc::new(iceberg(
                    99,
                    decided.visible_quantity().as_u64(),
                    decided.hidden_quantity().as_u64(),
                ))
            }));
            let result = level.update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(2),
                new_quantity: Quantity::new(new_quantity),
            });
            assert_invalid_operation(result.as_ref().err(), "different order id");

            // No reservation left behind, queue and priority untouched.
            assert_eq!(level.visible_quantity(), visible_before, "{new_quantity}");
            assert_eq!(level.hidden_quantity(), hidden_before, "{new_quantity}");
            assert_eq!(fifo(&level), before, "{new_quantity}");
            assert_eq!(level.order_count(), 3);
            assert!(!level.test_is_poisoned());
        }

        // Without the injection the same update commits normally.
        let committed = level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(2),
                new_quantity: Quantity::new(4),
            })
            .expect("update")
            .expect("present");
        assert_eq!(*committed, iceberg(2, 4, 40));
        assert_eq!(level.visible_quantity(), visible_before - 6);
        assert_eq!(level.hidden_quantity(), hidden_before);
    }

    #[test]
    fn test_update_entry_with_id_mismatch_never_calls_reserve() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(standard(1, 10)));
        queue.push(Arc::new(standard(2, 20)));
        queue.push(Arc::new(standard(3, 30)));
        let before: Vec<Id> = queue.to_vec().iter().map(|o| o.id()).collect();

        for demote in [false, true] {
            let mut reserved = false;
            let outcome = queue.update_entry_with(
                Id::from_u64(2),
                |_live| {
                    let foreign = Arc::new(standard(77, 5));
                    let decision = if demote {
                        UpdateDecision::ReplaceAtTail(foreign, queue.try_reserve_seq()?)
                    } else {
                        UpdateDecision::KeepInPlace(foreign)
                    };
                    Ok((decision, ()))
                },
                |()| {
                    reserved = true;
                    Ok(())
                },
            );
            assert!(matches!(
                outcome,
                Some(Err(PriceLevelError::InvalidOperation { .. }))
            ));
            assert!(!reserved, "reserve must not run for a rejected decision");
            let after: Vec<Id> = queue.to_vec().iter().map(|o| o.id()).collect();
            assert_eq!(after, before);
            assert_eq!(
                queue.find(Id::from_u64(2)).map(|o| *o),
                Some(standard(2, 20))
            );
            assert!(queue.find(Id::from_u64(77)).is_none());
            assert!(queue.debug_map_index_consistent());
        }
    }

    #[test]
    fn test_update_entry_with_reserve_error_leaves_queue_untouched() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(standard(1, 10)));
        queue.push(Arc::new(standard(2, 20)));
        let outcome = queue.update_entry_with(
            Id::from_u64(1),
            |_live| {
                Ok((
                    UpdateDecision::ReplaceAtTail(
                        Arc::new(standard(1, 50)),
                        queue.try_reserve_seq()?,
                    ),
                    (),
                ))
            },
            |()| {
                Err(PriceLevelError::InvalidOperation {
                    message: "reservation refused".to_string(),
                })
            },
        );
        assert!(matches!(outcome, Some(Err(_))));
        let ids: Vec<Id> = queue.to_vec().iter().map(|o| o.id()).collect();
        assert_eq!(ids, vec![Id::from_u64(1), Id::from_u64(2)]);
        assert_eq!(
            queue.find(Id::from_u64(1)).map(|o| *o),
            Some(standard(1, 10))
        );
        assert!(queue.debug_map_index_consistent());
    }

    #[test]
    fn test_update_entry_with_absent_id_calls_nothing() {
        let queue = OrderQueue::new();
        let outcome = queue.update_entry_with(
            Id::from_u64(1),
            |_live| -> Result<(UpdateDecision, ()), PriceLevelError> {
                panic!("decide must not run for an absent id")
            },
            |()| panic!("reserve must not run for an absent id"),
        );
        assert!(outcome.is_none());
    }

    // ---------------------------------------------------------------------
    // Checked counter deltas
    // ---------------------------------------------------------------------

    #[test]
    fn test_counter_delta_between_is_exact() {
        assert_eq!(CounterDelta::between(5, 5), CounterDelta::Increase(0));
        assert_eq!(CounterDelta::between(5, 9), CounterDelta::Increase(4));
        assert_eq!(CounterDelta::between(9, 5), CounterDelta::Decrease(4));
        assert_eq!(
            CounterDelta::between(0, u64::MAX),
            CounterDelta::Increase(u64::MAX)
        );
        assert_eq!(
            CounterDelta::between(u64::MAX, 0),
            CounterDelta::Decrease(u64::MAX)
        );
        assert_eq!(
            CounterDelta::Increase(3).inverse(),
            CounterDelta::Decrease(3)
        );
    }

    #[test]
    fn test_counter_delta_apply_is_checked() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert!(!CounterDelta::Increase(2).apply(&counter));
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX - 1);
        assert!(CounterDelta::Increase(1).apply(&counter));
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);

        let counter = AtomicU64::new(1);
        assert!(!CounterDelta::Decrease(2).apply(&counter));
        assert_eq!(counter.load(Ordering::Relaxed), 1);
        assert!(CounterDelta::Decrease(1).apply(&counter));
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_update_plan_second_failure_rolls_back_first() {
        // Visible grows (applied first), hidden overflows: visible rolled back.
        let visible = AtomicU64::new(10);
        let hidden = AtomicU64::new(u64::MAX);
        let mut rollback_failed = false;
        let plan = UpdatePlan::new(1, 6, 1, 2);
        assert!(
            plan.reserve(&visible, &hidden, &mut rollback_failed)
                .is_err()
        );
        assert_eq!(visible.load(Ordering::Relaxed), 10);
        assert_eq!(hidden.load(Ordering::Relaxed), u64::MAX);
        assert!(!rollback_failed);
    }

    #[test]
    fn test_update_plan_increase_is_applied_before_decrease() {
        // Visible shrinks, hidden grows and overflows. Hidden (the increase) is
        // applied first, so its failure leaves visible never touched.
        let visible = AtomicU64::new(10);
        let hidden = AtomicU64::new(u64::MAX);
        let mut rollback_failed = false;
        let plan = UpdatePlan::new(8, 3, 1, 2);
        assert!(
            plan.reserve(&visible, &hidden, &mut rollback_failed)
                .is_err()
        );
        assert_eq!(visible.load(Ordering::Relaxed), 10);
        assert_eq!(hidden.load(Ordering::Relaxed), u64::MAX);
        assert!(!rollback_failed);

        // Success applies both.
        let hidden = AtomicU64::new(0);
        assert!(
            plan.reserve(&visible, &hidden, &mut rollback_failed)
                .is_ok()
        );
        assert_eq!(visible.load(Ordering::Relaxed), 5);
        assert_eq!(hidden.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_update_plan_decrease_underflow_rejected() {
        let visible = AtomicU64::new(2);
        let hidden = AtomicU64::new(0);
        let mut rollback_failed = false;
        let plan = UpdatePlan::new(5, 1, 0, 0);
        assert!(
            plan.reserve(&visible, &hidden, &mut rollback_failed)
                .is_err()
        );
        assert_eq!(visible.load(Ordering::Relaxed), 2);
        assert!(!rollback_failed);
    }

    // ---------------------------------------------------------------------
    // Checked width conversions
    // ---------------------------------------------------------------------

    #[test]
    fn test_sweep_capacity_hint_is_min_of_quantity_and_count() {
        assert_eq!(sweep_capacity_hint(0, 5), 0);
        assert_eq!(sweep_capacity_hint(3, 5), 3);
        assert_eq!(sweep_capacity_hint(10, 5), 5);
        assert_eq!(sweep_capacity_hint(u64::MAX, 7), 7);
        assert_eq!(sweep_capacity_hint(u64::MAX, usize::MAX), usize::MAX);
    }

    /// On a 32-bit target a quantity above `usize::MAX` must not truncate: the
    /// exact minimum is the order count.
    #[cfg(target_pointer_width = "32")]
    #[test]
    fn test_sweep_capacity_hint_quantity_above_usize_is_not_truncated() {
        let above = u64::from(u32::MAX) + 1; // truncating `as usize` would give 0
        assert_eq!(sweep_capacity_hint(above, 7), 7);
        assert_eq!(sweep_capacity_hint(above + 3, 7), 7);
    }

    #[test]
    fn test_try_pack_bounds_the_restored_count() {
        let max = usize::try_from(PriceLevel::TEST_MAX_ORDER_COUNT).expect("fits usize");
        assert!(PriceLevel::test_try_pack(Some(Side::Sell), max).is_ok());
        assert!(PriceLevel::test_try_pack(None, 0).is_ok());
        #[cfg(target_pointer_width = "64")]
        {
            let over = max + 1;
            let err = PriceLevel::test_try_pack(Some(Side::Buy), over).expect_err("over");
            assert_invalid_operation(Some(&err), "exceeds");
        }
    }

    #[test]
    fn test_order_count_reports_max_count_exactly() {
        let level = PriceLevel::new(PRICE);
        level.test_saturate_order_count(Side::Sell);
        assert_eq!(
            level.order_count(),
            usize::try_from(PriceLevel::TEST_MAX_ORDER_COUNT).expect("fits usize")
        );
        // Admission past the cap is a typed rejection with nothing changed.
        let err = level.add_order(standard(1, 5)).expect_err("cap");
        assert_invalid_operation(Some(&err), "count overflow");
        assert_eq!(level.visible_quantity(), 0);
        assert!(fifo(&level).is_empty());
    }

    #[test]
    fn test_snapshot_restore_packs_count_and_side() {
        let level = three_makers();
        let json = level.snapshot_to_json().expect("snapshot");
        let restored = PriceLevel::from_snapshot_json(&json).expect("restore");
        assert_eq!(restored.order_count(), 3);
        assert_eq!(restored.test_topology_count(), 3);
        assert_eq!(fifo(&restored), fifo(&level));
        // The restored side pin is Sell: a buy is rejected.
        let mut buy = standard(9, 1);
        if let OrderType::Standard { side, .. } = &mut buy {
            *side = Side::Buy;
        }
        assert!(restored.add_order(buy).is_err());
    }

    #[test]
    fn test_remove_if_outcomes_absent_refused_removed() {
        use crate::price_level::order_queue::RemoveOutcome;

        let queue = OrderQueue::new();
        queue.push(Arc::new(standard(1, 10)));
        queue.push(Arc::new(standard(2, 20)));

        // Absent: the check never runs.
        let outcome = queue.remove_if(Id::from_u64(9), |_| {
            panic!("check must not run for an absent id")
        });
        assert!(matches!(outcome, RemoveOutcome::Absent));

        // Refused: the check sees the resident order; nothing changes.
        let mut seen = None;
        let outcome = queue.remove_if(Id::from_u64(1), |resident| {
            seen = Some(resident.id());
            false
        });
        assert!(matches!(outcome, RemoveOutcome::Refused));
        assert_eq!(seen, Some(Id::from_u64(1)));
        let ids: Vec<Id> = queue.to_vec().iter().map(|o| o.id()).collect();
        assert_eq!(ids, vec![Id::from_u64(1), Id::from_u64(2)]);
        assert!(queue.debug_map_index_consistent());

        // Removed: map and index both cleaned; FIFO of the rest intact.
        match queue.remove_if(Id::from_u64(1), |_| true) {
            RemoveOutcome::Removed(order) => assert_eq!(*order, standard(1, 10)),
            other => panic!("expected Removed, got {other:?}"),
        }
        let ids: Vec<Id> = queue.to_vec().iter().map(|o| o.id()).collect();
        assert_eq!(ids, vec![Id::from_u64(2)]);
        assert!(queue.debug_map_index_consistent());
        assert!(matches!(
            queue.remove_if(Id::from_u64(1), |_| true),
            RemoveOutcome::Absent
        ));
    }

    #[test]
    fn test_cancel_absent_on_empty_level_is_not_found() {
        // The ordinary empty-level miss: no count error, no poisoning.
        let level = PriceLevel::new(PRICE);
        let outcome = level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(1),
        });
        assert_eq!(outcome, Ok(None));
        assert!(!level.test_is_poisoned());
    }

    // ---------------------------------------------------------------------
    // Review regression (PR #196): the removal count check must not race a
    // concurrent admission of the same id. Public API only.
    // ---------------------------------------------------------------------

    /// Rounds for the admission-vs-removal race. The reviewer's release probe
    /// reproduced 648 spurious errors in 50,000 rounds with the split
    /// (count-then-find) check; the under-lock check must produce none.
    const RACE_ROUNDS: u64 = 20_000;

    /// One Barrier-started round per id on an initially empty level: thread A
    /// admits id `round`, thread B removes the same id (cancel, or a
    /// price-moving update on odd rounds). Whatever the interleaving, the
    /// removal must return `Ok(Some)` (it saw the order) or `Ok(None)` (it
    /// ran first), never an invariant error, and the level's count and
    /// counters must describe its queue after every round.
    #[test]
    fn test_concurrent_admission_and_removal_same_id_never_reports_count_error() {
        use std::sync::Barrier;
        use std::thread;

        let level = Arc::new(PriceLevel::new(PRICE));
        let start = Arc::new(Barrier::new(2));
        let done = Arc::new(Barrier::new(2));
        let checked = Arc::new(Barrier::new(2));

        let admitter = {
            let level = Arc::clone(&level);
            let (start, done, checked) =
                (Arc::clone(&start), Arc::clone(&done), Arc::clone(&checked));
            thread::spawn(move || {
                let mut removed_after_admit = 0u64;
                for round in 1..=RACE_ROUNDS {
                    start.wait();
                    let admitted = level.add_order(standard(round, 7));
                    done.wait();
                    // Both sides of the round have returned: verify and reset.
                    assert!(admitted.is_ok(), "round {round}: admission {admitted:?}");
                    let resting = fifo_ids(&level);
                    match resting.as_slice() {
                        [] => {
                            removed_after_admit += 1;
                            assert_eq!(level.order_count(), 0, "round {round}");
                            assert_eq!(level.visible_quantity(), 0, "round {round}");
                        }
                        [id] => {
                            assert_eq!(*id, Id::from_u64(round));
                            assert_eq!(level.order_count(), 1, "round {round}");
                            assert_eq!(level.visible_quantity(), 7, "round {round}");
                            let cleanup = level.update_order(OrderUpdate::Cancel {
                                order_id: Id::from_u64(round),
                            });
                            assert!(matches!(cleanup, Ok(Some(_))), "round {round}");
                        }
                        other => panic!("round {round}: unexpected queue {other:?}"),
                    }
                    assert_eq!(level.order_count(), 0, "round {round}: reset");
                    assert_eq!(level.hidden_quantity(), 0);
                    checked.wait();
                }
                removed_after_admit
            })
        };

        let remover = {
            let level = Arc::clone(&level);
            let (start, done, checked) =
                (Arc::clone(&start), Arc::clone(&done), Arc::clone(&checked));
            thread::spawn(move || {
                let mut errors = 0u64;
                let mut found = 0u64;
                for round in 1..=RACE_ROUNDS {
                    start.wait();
                    let order_id = Id::from_u64(round);
                    let outcome = if round % 2 == 0 {
                        level.update_order(OrderUpdate::Cancel { order_id })
                    } else {
                        level.update_order(OrderUpdate::UpdatePrice {
                            order_id,
                            new_price: Price::new(PRICE + 1),
                        })
                    };
                    match outcome {
                        Ok(Some(order)) => {
                            assert_eq!(order.id(), order_id);
                            found += 1;
                        }
                        Ok(None) => {}
                        Err(_) => errors += 1,
                    }
                    done.wait();
                    checked.wait();
                }
                (errors, found)
            })
        };

        let removed_after_admit = admitter.join().expect("admitter");
        let (errors, found) = remover.join().expect("remover");
        assert_eq!(
            errors, 0,
            "a removal racing an admission of the same id must never report a count error"
        );
        assert_eq!(
            found, removed_after_admit,
            "every found removal emptied the level"
        );
        assert!(!level.test_is_poisoned());
        assert_eq!(level.order_count(), 0);
        assert_eq!(level.visible_quantity(), 0);
        assert_eq!(level.stats().orders_added(), RACE_ROUNDS as usize);
        assert_eq!(level.stats().orders_removed(), RACE_ROUNDS as usize);
    }
}
