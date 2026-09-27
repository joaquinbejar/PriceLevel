//! Issue #155: bound on repeated front scans of parked makers.
//!
//! `OrderQueue::match_front` restarts its front scan from the lowest sequence
//! on every step and skips the sequences the current sweep has parked. With a
//! live prefix of `S` parked makers followed by `K` fills that would cost
//! about `S * K` extra visits. These tests pin down why `S <= 1` for every
//! state reachable through the public API, so the cost is at most one extra
//! visit per step:
//!
//! - the no-progress `SetAside` guard can never fire: for a positive taker
//!   remainder `match_against` always consumes, draws hidden, or removes the
//!   maker, for every `OrderType` variant and every field value;
//! - the self-trade skip parks only a maker whose id equals the taker's, and
//!   the id-keyed storage holds at most one resting order per id;
//! - every other `SetAside` reason (`Failed`, `IdsExhausted`,
//!   `SequenceExhausted`, `Abort`) stops the sweep, so nothing is rescanned.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::PriceLevelError;
    use crate::execution::TakerKind;
    use crate::orders::{Hash32, Id, OrderType, PegReferenceType, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::price_level::order_queue::{
        FrontAction, FrontOutcome, OrderQueue, UpdateDecision, test_take_front_scan_visits,
    };
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::collections::HashSet;
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;

    fn standard(id: u64, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    /// Every variant with every small field combination, including the
    /// degenerate zero-visible / zero-hidden / zero-threshold shapes an update
    /// or snapshot restore could leave behind.
    fn all_shapes() -> Vec<OrderType<()>> {
        let id = Id::from_u64(1);
        let price = Price::new(PRICE);
        let side = Side::Sell;
        let user_id = Hash32::zero();
        let timestamp = TimestampMs::new(1);
        let time_in_force = TimeInForce::Gtc;
        let small = [0u64, 1, 2, 3, 7];
        let mut shapes = Vec::new();
        for &q in &small {
            let quantity = Quantity::new(q);
            shapes.push(OrderType::Standard {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: (),
            });
            shapes.push(OrderType::PostOnly {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: (),
            });
            shapes.push(OrderType::TrailingStop {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                trail_amount: Quantity::new(1),
                last_reference_price: price,
                extra_fields: (),
            });
            shapes.push(OrderType::PeggedOrder {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                reference_price_offset: 0,
                reference_price_type: PegReferenceType::BestBid,
                extra_fields: (),
            });
            shapes.push(OrderType::MarketToLimit {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: (),
            });
            for &h in &small {
                shapes.push(OrderType::IcebergOrder {
                    id,
                    price,
                    visible_quantity: quantity,
                    hidden_quantity: Quantity::new(h),
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    extra_fields: (),
                });
                for &threshold in &small {
                    for amount in [None, NonZeroU64::new(1), NonZeroU64::new(5)] {
                        for auto_replenish in [false, true] {
                            shapes.push(OrderType::ReserveOrder {
                                id,
                                price,
                                visible_quantity: quantity,
                                hidden_quantity: Quantity::new(h),
                                side,
                                user_id,
                                timestamp,
                                time_in_force,
                                replenish_threshold: Quantity::new(threshold),
                                replenish_amount: amount,
                                auto_replenish,
                                extra_fields: (),
                            });
                        }
                    }
                }
            }
        }
        shapes
    }

    #[test]
    fn no_progress_guard_is_unreachable_for_every_order_shape() {
        // The sweep parks a maker as "no progress" only when `match_against`
        // returns `consumed == 0 && hidden_reduced == 0 && new_remaining ==
        // remaining && residual.is_some()`. The loop runs only while
        // `remaining > 0`. For such a taker every variant either consumes
        // (`min(visible, remaining) > 0`), draws a positive hidden tranche, or
        // returns no residual (a zero-quantity maker is removed, not parked).
        let shapes = all_shapes();
        // 5 quantities x (5 single-tranche variants + 5 hidden x (1 iceberg
        // + 5 thresholds x 3 amounts x 2 auto flags)).
        assert_eq!(shapes.len(), 800, "grid covers every variant");
        for maker in &shapes {
            for remaining in 1u64..=9 {
                let step: Result<_, PriceLevelError> = maker.match_against(remaining);
                let Ok((consumed, residual, hidden_reduced, new_remaining)) = step else {
                    // A typed arithmetic failure stops the sweep; it never parks.
                    continue;
                };
                let no_progress = consumed == 0
                    && hidden_reduced == 0
                    && new_remaining == remaining
                    && residual.is_some();
                assert!(
                    !no_progress,
                    "no-progress shape reachable: {maker:?} against {remaining}"
                );
            }
        }
    }

    #[test]
    fn at_most_one_resting_order_can_share_the_taker_id() {
        // The self-trade skip parks a maker whose id equals the taker's. The
        // storage is keyed by id and admission is insert-if-absent, so a
        // second order with that id is rejected: at most one live entry can
        // ever be parked by the skip.
        let level = PriceLevel::new(PRICE);
        let first = level.add_order(standard(7, 10));
        assert!(first.is_ok());
        let second = level.add_order(standard(7, 20));
        assert!(matches!(second, Err(PriceLevelError::DuplicateOrderId(_))));
        assert_eq!(level.order_count(), 1);
    }

    /// Drive `match_front` the way the sweep does for a GTC taker `taker`:
    /// park a maker sharing the taker id, fully consume every other one.
    /// Returns the number of fills.
    fn sweep_with_self_skip(queue: &OrderQueue, taker: Id, max_fills: usize) -> usize {
        let mut set_aside = HashSet::new();
        let mut fills = 0usize;
        while fills < max_fills {
            let outcome = queue.match_front(&mut set_aside, |_seq, order| {
                if order.id() == taker {
                    (FrontAction::SetAside, false)
                } else {
                    (FrontAction::Remove, true)
                }
            });
            match outcome {
                FrontOutcome::Empty => break,
                FrontOutcome::Matched { result: true } => fills += 1,
                FrontOutcome::Matched { result: false } => {}
            }
        }
        fills
    }

    #[test]
    fn sweep_without_parked_makers_visits_one_entry_per_fill() {
        // Baseline through the public API: K fills visit exactly K entries.
        const K: u64 = 64;
        let level = PriceLevel::new(PRICE);
        for id in 1..=K {
            assert!(level.add_order(standard(id, 1)).is_ok());
        }
        let generator = UuidGenerator::new(Uuid::nil());
        let _ = test_take_front_scan_visits();
        let result = level.match_order(
            K,
            Id::from_u64(10_000),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(2),
            &generator,
        );
        assert!(result.is_complete());
        assert_eq!(result.trades().len() as u64, K);
        assert_eq!(test_take_front_scan_visits(), K);
    }

    #[test]
    fn a_parked_self_trade_maker_costs_at_most_one_extra_visit_per_step() {
        // The reachable case: the taker's own order is admitted after the
        // pre-sweep self-match probe and lands at the front of the remaining
        // queue. It is modelled directly on the queue because the probe makes
        // the interleaving timing-dependent through `match_order`; the state
        // itself (one resting order carrying the taker id) is admissible.
        const K: u64 = 64;
        let taker = Id::from_u64(10_000);
        let queue = OrderQueue::new();
        assert!(queue.try_push(Arc::new(standard(10_000, 5))).is_ok());
        for id in 1..=K {
            assert!(queue.try_push(Arc::new(standard(id, 1))).is_ok());
        }
        let _ = test_take_front_scan_visits();
        let fills = sweep_with_self_skip(&queue, taker, usize::MAX);
        assert_eq!(fills as u64, K);
        // Step 1 visits the parked maker only; each of the K fills visits it
        // plus the filled maker; the closing `Empty` scan visits it once more.
        assert_eq!(test_take_front_scan_visits(), 2 * K + 2);
        assert_eq!(queue.len(), 1, "the parked maker rests untouched");
    }

    #[test]
    fn a_re_sequenced_parked_maker_never_accumulates_a_prefix() {
        // A concurrent resize that demotes the parked maker moves it to the
        // tail under a fresh sequence and drops the old index key, so the sweep
        // meets it (and parks it) again later instead of holding two parked
        // entries. The live parked prefix stays at most one.
        const K: u64 = 32;
        let taker = Id::from_u64(10_000);
        let queue = OrderQueue::new();
        assert!(queue.try_push(Arc::new(standard(10_000, 5))).is_ok());
        for id in 1..=K {
            assert!(queue.try_push(Arc::new(standard(id, 1))).is_ok());
        }
        let _ = test_take_front_scan_visits();
        // Park it, fill half, demote it (as a concurrent increase would), fill
        // the rest.
        assert_eq!(sweep_with_self_skip(&queue, taker, (K / 2) as usize), 16);
        let demoted = queue.update_entry(taker, |_live| {
            Ok(UpdateDecision::ReplaceAtTail(
                Arc::new(standard(10_000, 6)),
                queue.try_reserve_seq()?,
            ))
        });
        assert!(matches!(demoted, Some(Ok(_))));
        assert!(queue.debug_map_index_consistent());
        let first_half = test_take_front_scan_visits();
        assert_eq!(first_half, 1 + 2 * (K / 2));
        // A fresh sweep scratch set: the demoted maker is now at the tail.
        let fills = sweep_with_self_skip(&queue, taker, usize::MAX);
        assert_eq!(fills as u64, K / 2);
        // One visit per fill, then the tail maker is parked, then `Empty`.
        assert_eq!(test_take_front_scan_visits(), K / 2 + 2);
    }
}
