//! Issue #165: monotonic internal counters refuse to wrap.
//!
//! Every fixture seeds a counter just below its limit through a `cfg(test)`
//! seam instead of iterating toward it, then checks that the operation needing
//! a fresh value returns the typed `CounterExhausted` error and that the queue,
//! the level counters and the statistics are exactly as before (or, for a
//! committed mutation whose advisory statistic is exhausted, that the mutation
//! stands and the statistics are marked degraded). Plain atomic wrap does not
//! panic in any profile, so these tests assert the protocol, not a panic.

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): raw arithmetic is
// permitted inside `mod tests` per the Testing section of
// `rules/global_rules.md`. Production code outside this module keeps the
// full deny list.
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::{CapacityResource, ExhaustedCounter, PriceLevelError};
    use crate::execution::{MatchOutcome, MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::price_level::order_queue::OrderQueue;
    use crate::price_level::{PriceLevelSnapshotPackage, PriceLevelStatistics};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::str::FromStr;
    use std::sync::Arc;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    /// Mirror of the private `EPOCH_MUTATION_LIMIT` in `level.rs`.
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

    fn take(level: &PriceLevel, quantity: u64, tif: TimeInForce, kind: TakerKind) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(999),
            tif,
            kind,
            TimestampMs::new(1_700_000_000_000),
            &generator(),
        )
    }

    /// Everything observable about a level that an exhausted operation must
    /// leave untouched.
    #[derive(Debug, PartialEq)]
    struct LevelState {
        orders: Vec<(Id, u64, u64, Option<u64>)>,
        visible_counter: u64,
        hidden_counter: u64,
        order_count: usize,
        next_seq: u64,
        epochs: (u64, u64),
        stats: String,
    }

    fn state(level: &PriceLevel) -> LevelState {
        let queue = level.test_queue();
        LevelState {
            orders: level
                .snapshot_by_insertion_seq()
                .expect("materialize")
                .iter()
                .map(|o| {
                    (
                        o.id(),
                        o.visible_quantity().as_u64(),
                        o.hidden_quantity().as_u64(),
                        queue.test_seq_of(o.id()),
                    )
                })
                .collect(),
            visible_counter: level.visible_quantity(),
            hidden_counter: level.hidden_quantity(),
            order_count: level.order_count(),
            next_seq: queue.test_next_seq(),
            epochs: level.test_epochs(),
            stats: level.stats().to_string(),
        }
    }

    fn assert_counters_match_queue(level: &PriceLevel) {
        let s = state(level);
        assert_eq!(
            s.visible_counter,
            s.orders.iter().map(|o| o.1).sum::<u64>(),
            "visible"
        );
        assert_eq!(
            s.hidden_counter,
            s.orders.iter().map(|o| o.2).sum::<u64>(),
            "hidden"
        );
        assert_eq!(s.order_count, s.orders.len(), "order_count");
        assert!(level.test_queue().debug_map_index_consistent());
    }

    fn exhausted(counter: ExhaustedCounter) -> PriceLevelError {
        PriceLevelError::CounterExhausted { counter }
    }

    // ------------------------------------------------------------------
    // Statistics: orders_added / orders_removed
    // ------------------------------------------------------------------

    fn stats_at_max() -> PriceLevelStatistics {
        let text = format!(
            "PriceLevelStatistics:orders_added={max};orders_removed={max};orders_executed=0;quantity_executed=0;value_executed=0;last_execution_time=0;first_arrival_time=0;sum_waiting_time=0",
            max = usize::MAX
        );
        PriceLevelStatistics::from_str(&text).expect("parse")
    }

    #[test]
    fn order_event_counters_at_max_refuse_and_preserve() {
        let stats = stats_at_max();
        assert!(!stats.stats_degraded());

        assert_eq!(
            stats.record_order_added(),
            Err(exhausted(ExhaustedCounter::OrdersAdded))
        );
        assert_eq!(stats.orders_added(), usize::MAX, "no wrap to zero");
        assert!(stats.stats_degraded());

        assert_eq!(
            stats.record_order_removed(),
            Err(exhausted(ExhaustedCounter::OrdersRemoved))
        );
        assert_eq!(stats.orders_removed(), usize::MAX, "no wrap to zero");

        // Repeated refusals keep the counter where it is.
        assert!(stats.record_order_added().is_err());
        assert_eq!(stats.orders_added(), usize::MAX);
        // Untouched aggregates.
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
    }

    /// Concurrent drops at `usize::MAX`: exactly one caller is told it
    /// transitioned the degraded flag, so the engine logs exactly once.
    #[test]
    fn concurrent_order_event_drops_report_exactly_one_transition() {
        const THREADS: usize = 8;
        for _ in 0..50 {
            let stats = Arc::new(PriceLevelStatistics::new());
            stats.test_seed_order_events(usize::MAX, usize::MAX);
            let barrier = Arc::new(std::sync::Barrier::new(THREADS));
            let handles: Vec<_> = (0..THREADS)
                .map(|i| {
                    let stats = Arc::clone(&stats);
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        let outcome = if i % 2 == 0 {
                            stats.record_order_added_reporting()
                        } else {
                            stats.record_order_removed_reporting()
                        };
                        match outcome {
                            Ok(()) => panic!("counter at MAX must refuse"),
                            Err(drop) => drop.degraded_now,
                        }
                    })
                })
                .collect();
            let transitions = handles
                .into_iter()
                .map(|h| h.join().expect("thread"))
                .filter(|degraded_now| *degraded_now)
                .count();
            assert_eq!(transitions, 1);
            assert!(stats.stats_degraded());
            assert_eq!(stats.orders_added(), usize::MAX);
            assert_eq!(stats.orders_removed(), usize::MAX);
        }
    }

    #[test]
    fn order_event_counters_reach_max_then_refuse() {
        let stats = PriceLevelStatistics::new();
        stats.test_seed_order_events(usize::MAX - 1, usize::MAX - 1);
        assert_eq!(stats.record_order_added(), Ok(()));
        assert_eq!(stats.record_order_removed(), Ok(()));
        assert_eq!(stats.orders_added(), usize::MAX);
        assert_eq!(stats.orders_removed(), usize::MAX);
        assert!(!stats.stats_degraded(), "success does not degrade");
        assert!(stats.record_order_added().is_err());
        assert!(stats.stats_degraded());
    }

    #[test]
    fn level_admission_and_cancel_stand_when_order_event_stats_exhausted() {
        let level = PriceLevel::new(PRICE);
        level.stats().test_seed_order_events(usize::MAX, usize::MAX);

        let admitted = level.add_order(standard(1, 10)).expect("admission stands");
        assert_eq!(admitted.id(), Id::from_u64(1));
        assert_eq!(level.order_count(), 1);
        assert_eq!(level.visible_quantity(), 10);
        assert_eq!(level.stats().orders_added(), usize::MAX);
        assert!(level.stats().stats_degraded());

        let cancelled = level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(1),
            })
            .expect("cancel stands");
        assert!(cancelled.is_some());
        assert_eq!(level.order_count(), 0);
        assert_eq!(level.visible_quantity(), 0);
        assert_eq!(level.stats().orders_removed(), usize::MAX);
        assert_counters_match_queue(&level);

        // The degraded flag round-trips through a checksummed snapshot.
        let json = level.snapshot_to_json().expect("snapshot");
        let restored = PriceLevel::from_snapshot_json(&json).expect("restore");
        assert!(restored.stats().stats_degraded());
        assert_eq!(restored.stats().orders_added(), usize::MAX);
    }

    // ------------------------------------------------------------------
    // Statistics seqlock sequence
    // ------------------------------------------------------------------

    #[test]
    fn stats_sequence_refuses_entry_without_exit_headroom() {
        let stats = PriceLevelStatistics::new_at(TimestampMs::new(7));
        stats.record_execution(3, 100, 0, 50).expect("record");
        let before = stats.to_string();

        // `u64::MAX - 1` is even and above the entry limit (`u64::MAX - 2`).
        stats.test_seed_stats_seq(u64::MAX - 1);
        assert_eq!(
            stats.record_execution(4, 100, 0, 60),
            Err(exhausted(ExhaustedCounter::StatisticsSequence))
        );
        assert_eq!(stats.test_stats_seq(), u64::MAX - 1, "sequence untouched");
        assert!(stats.stats_degraded(), "dropped execution is visible");
        assert_eq!(stats.orders_executed(), 1);
        assert_eq!(stats.quantity_executed(), 3);
        assert_eq!(stats.last_execution_time(), 50);
        // Multi-field readers still terminate (sequence is even).
        let clone = stats.clone();
        assert_eq!(clone.orders_executed(), 1);
        assert_ne!(stats.to_string(), before, "only the degraded flag moved");
        assert!(stats.to_string().ends_with("stats_degraded=true"));

        // Reset is refused with nothing changed (flag stays set).
        let snapshot = stats.to_string();
        assert_eq!(
            stats.reset_at(TimestampMs::new(99)),
            Err(exhausted(ExhaustedCounter::StatisticsSequence))
        );
        assert_eq!(stats.to_string(), snapshot);
        assert_eq!(stats.test_stats_seq(), u64::MAX - 1);

        // A rebuilt copy starts a fresh sequence and records again.
        let rebuilt = stats.clone();
        assert_eq!(rebuilt.test_stats_seq(), 0);
        assert_eq!(rebuilt.record_execution(1, 100, 0, 70), Ok(()));
    }

    #[test]
    fn stats_sequence_last_section_exits_in_range() {
        let stats = PriceLevelStatistics::new();
        // Largest even value from which a section may still open.
        stats.test_seed_stats_seq(u64::MAX - 3);
        assert_eq!(stats.record_execution(2, 100, 0, 10), Ok(()));
        assert_eq!(stats.test_stats_seq(), u64::MAX - 1, "exit landed even");
        assert!(!stats.stats_degraded());
        assert_eq!(stats.clone().orders_executed(), 1);

        // The next section cannot open.
        assert!(stats.record_execution(2, 100, 0, 10).is_err());
        assert_eq!(stats.orders_executed(), 1);

        // A failing record (validation error) inside the last section also
        // exits in range: seed again and fail on a future maker timestamp.
        let stats = PriceLevelStatistics::new();
        stats.test_seed_stats_seq(u64::MAX - 3);
        assert!(stats.record_execution(2, 100, 20, 10).is_err());
        assert_eq!(stats.test_stats_seq(), u64::MAX - 1);

        // Reset uses the same last section.
        let stats = PriceLevelStatistics::new();
        stats.test_seed_stats_seq(u64::MAX - 3);
        assert_eq!(stats.reset_at(TimestampMs::new(5)), Ok(()));
        assert_eq!(stats.test_stats_seq(), u64::MAX - 1);
        assert_eq!(stats.first_arrival_time(), 5);
    }

    #[test]
    fn match_commits_trades_when_stats_sequence_exhausted() {
        let level = level_with(vec![standard(1, 5), standard(2, 5)]);
        level.stats().test_seed_stats_seq(u64::MAX - 1);
        let result = take(&level, 10, TimeInForce::Gtc, TakerKind::Standard);
        // The match itself is unaffected (#117): trades committed, no error.
        assert!(result.error().is_none());
        assert_eq!(result.trades().len(), 2);
        assert!(level.stats().stats_degraded());
        assert_eq!(level.stats().orders_executed(), 0);
        assert_counters_match_queue(&level);
    }

    // ------------------------------------------------------------------
    // FIFO sequence: insertion
    // ------------------------------------------------------------------

    #[test]
    fn queue_insertion_at_last_sequence_then_refuses() {
        let queue = OrderQueue::new();
        queue.test_seed_next_seq(u64::MAX - 1);

        queue
            .try_push(Arc::new(standard(1, 10)))
            .expect("last sequence");
        assert_eq!(queue.test_seq_of(Id::from_u64(1)), Some(u64::MAX - 1));
        assert_eq!(queue.test_next_seq(), u64::MAX);

        assert_eq!(
            queue.try_push(Arc::new(standard(2, 10))),
            Err(exhausted(ExhaustedCounter::QueueSequence))
        );
        assert_eq!(queue.test_next_seq(), u64::MAX, "never wraps to 0");
        assert_eq!(queue.len(), 1);
        assert!(queue.find(Id::from_u64(2)).is_none());
        assert!(queue.debug_map_index_consistent());

        // Identity is decided first: a duplicate still reports the duplicate.
        assert!(matches!(
            queue.try_push(Arc::new(standard(1, 99))),
            Err(PriceLevelError::DuplicateOrderId(_))
        ));
        assert_eq!(
            queue.find(Id::from_u64(1)).map(|o| o.visible_quantity()),
            Some(Quantity::new(10))
        );
    }

    #[test]
    fn level_admission_refused_on_exhausted_sequence_level_unchanged() {
        let level = level_with(vec![standard(1, 10)]);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let before = state(&level);

        assert_eq!(
            level.add_order(standard(2, 5)).map(|o| o.id()),
            Err(exhausted(ExhaustedCounter::QueueSequence))
        );
        assert_eq!(state(&level), before);
        assert_counters_match_queue(&level);

        // A duplicate id takes precedence over the exhausted sequence.
        assert!(matches!(
            level.add_order(standard(1, 5)),
            Err(PriceLevelError::DuplicateOrderId(_))
        ));
        assert_eq!(state(&level), before);

        // The resting maker still trades.
        let result = take(&level, 10, TimeInForce::Gtc, TakerKind::Standard);
        assert!(result.error().is_none());
        assert_eq!(result.trades().len(), 1);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn empty_level_admission_refused_leaves_side_unpinned() {
        let level = PriceLevel::new(PRICE);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let before = state(&level);
        assert!(level.add_order(standard(1, 5)).is_err());
        assert_eq!(state(&level), before, "no pin, no epoch bump");
        // The opposite side is still admissible once a sequence exists.
        level.test_queue().test_seed_next_seq(u64::MAX - 1);
        let mut buy = standard(2, 5);
        if let OrderType::Standard { side, .. } = &mut buy {
            *side = Side::Buy;
        }
        level.add_order(buy).expect("either side after refusal");
    }

    // ------------------------------------------------------------------
    // FIFO sequence: replenishment inside the sweep
    // ------------------------------------------------------------------

    #[test]
    fn replenish_on_exhausted_sequence_stops_sweep_with_committed_prefix() {
        let level = level_with(vec![standard(1, 5), iceberg(2, 10, 100), standard(3, 7)]);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let iceberg_seq = level.test_queue().test_seq_of(Id::from_u64(2));

        let result = take(&level, 15, TimeInForce::Gtc, TakerKind::Standard);

        assert_eq!(
            result.error(),
            Some(&exhausted(ExhaustedCounter::QueueSequence))
        );
        // Only the FIFO prefix before the iceberg committed.
        assert_eq!(result.trades().len(), 1);
        assert_eq!(
            result.trades().as_vec()[0].maker_order_id(),
            Id::from_u64(1)
        );
        assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);
        assert_eq!(result.remaining_quantity().as_u64(), 10);
        assert!(!result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);

        // The iceberg is byte-identical at its original sequence; the younger
        // maker behind it did not trade.
        let iceberg_now = level.test_queue().find(Id::from_u64(2)).expect("resting");
        assert_eq!(iceberg_now.visible_quantity().as_u64(), 10);
        assert_eq!(iceberg_now.hidden_quantity().as_u64(), 100);
        assert_eq!(level.test_queue().test_seq_of(Id::from_u64(2)), iceberg_seq);
        assert!(level.test_queue().find(Id::from_u64(3)).is_some());
        assert_eq!(level.test_queue().test_next_seq(), u64::MAX);
        assert_eq!(level.visible_quantity(), 17);
        assert_eq!(level.hidden_quantity(), 100);
        assert_eq!(level.stats().orders_executed(), 1);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn replenish_takes_last_sequence() {
        let level = level_with(vec![standard(1, 5), iceberg(2, 10, 100), standard(3, 7)]);
        level.test_queue().test_seed_next_seq(u64::MAX - 1);

        let result = take(&level, 15, TimeInForce::Gtc, TakerKind::Standard);
        assert!(result.error().is_none());
        assert_eq!(result.trades().len(), 2);
        assert!(result.is_complete());
        // Re-sequenced at the tail with the last value; behind maker 3.
        assert_eq!(
            level.test_queue().test_seq_of(Id::from_u64(2)),
            Some(u64::MAX - 1)
        );
        let ids: Vec<Id> = level
            .snapshot_by_insertion_seq()
            .expect("materialize")
            .iter()
            .map(|o| o.id())
            .collect();
        assert_eq!(ids, vec![Id::from_u64(3), Id::from_u64(2)]);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn fill_or_kill_needing_a_replenish_is_killed_before_mutation() {
        let level = level_with(vec![standard(1, 5), iceberg(2, 10, 100), standard(3, 7)]);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let before = state(&level);

        let result = take(&level, 15, TimeInForce::Fok, TakerKind::Standard);
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_eq!(
            result.error(),
            Some(&exhausted(ExhaustedCounter::QueueSequence))
        );
        assert!(result.trades().is_empty());
        assert_eq!(result.remaining_quantity().as_u64(), 15);
        assert_eq!(state(&level), before, "level untouched");

        // A fill-or-kill that needs no replenish still fills.
        let result = take(&level, 5, TimeInForce::Fok, TakerKind::Standard);
        assert!(result.error().is_none());
        assert!(result.is_complete());
        assert_counters_match_queue(&level);
    }

    // ------------------------------------------------------------------
    // FIFO sequence: resize demotion
    // ------------------------------------------------------------------

    #[test]
    fn resize_demotion_on_exhausted_sequence_is_refused_in_place() {
        let level = level_with(vec![standard(1, 10), standard(2, 20)]);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let before = state(&level);

        let err = level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(1),
                new_quantity: Quantity::new(50),
            })
            .expect_err("increase needs a fresh sequence");
        assert_eq!(err, exhausted(ExhaustedCounter::QueueSequence));
        assert_eq!(state(&level), before, "maker keeps size and place");

        // A decrease keeps its sequence and needs none.
        let updated = level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(1),
                new_quantity: Quantity::new(4),
            })
            .expect("decrease")
            .expect("present");
        assert_eq!(updated.visible_quantity().as_u64(), 4);
        assert_eq!(level.visible_quantity(), 24);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn resize_demotion_takes_last_sequence() {
        let level = level_with(vec![standard(1, 10), standard(2, 20)]);
        level.test_queue().test_seed_next_seq(u64::MAX - 1);
        level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(1),
                new_quantity: Quantity::new(50),
            })
            .expect("increase")
            .expect("present");
        assert_eq!(
            level.test_queue().test_seq_of(Id::from_u64(1)),
            Some(u64::MAX - 1)
        );
        assert_eq!(level.visible_quantity(), 70);
        assert_counters_match_queue(&level);
    }

    // ------------------------------------------------------------------
    // Epochs
    // ------------------------------------------------------------------

    #[test]
    fn epochs_without_headroom_refuse_every_mutation_before_it_starts() {
        for (topology, mutation, counter) in [
            (EPOCH_LIMIT, 0, ExhaustedCounter::TopologyEpoch),
            (0, EPOCH_LIMIT, ExhaustedCounter::MutationEpoch),
        ] {
            let level = level_with(vec![standard(1, 10), standard(2, 20)]);
            level.test_seed_epochs(topology, mutation);
            let before = state(&level);

            assert_eq!(
                level.add_order(standard(3, 5)).map(|o| o.id()),
                Err(exhausted(counter))
            );
            assert_eq!(
                level.update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(1)
                }),
                Err(exhausted(counter))
            );
            assert_eq!(state(&level), before);

            if counter == ExhaustedCounter::TopologyEpoch {
                // A sweep may drain the level, so it is refused up front.
                let result = take(&level, 30, TimeInForce::Gtc, TakerKind::Standard);
                assert_eq!(result.error(), Some(&exhausted(counter)));
                assert!(result.trades().is_empty());
                assert_eq!(result.remaining_quantity().as_u64(), 30);

                let result = take(&level, 30, TimeInForce::Fok, TakerKind::Standard);
                assert_eq!(result.outcome(), MatchOutcome::Killed);
                assert_eq!(result.error(), Some(&exhausted(counter)));
                assert_eq!(state(&level), before);
            }
        }
    }

    #[test]
    fn epochs_just_below_limit_accept_and_advance() {
        let level = level_with(vec![standard(1, 10)]);
        level.test_seed_epochs(EPOCH_LIMIT - 1, EPOCH_LIMIT - 1);
        level.add_order(standard(2, 5)).expect("admit");
        assert_eq!(level.test_epochs(), (EPOCH_LIMIT - 1, EPOCH_LIMIT));
        // Now the mutation epoch is at the limit: further mutations refused.
        assert!(level.add_order(standard(3, 5)).is_err());
    }

    #[test]
    fn epoch_bump_stops_at_sentinel_and_readers_stay_sound() {
        let level = level_with(vec![standard(1, 10)]);
        level.test_seed_epochs(u64::MAX - 1, u64::MAX - 1);
        level.test_bump_epochs();
        assert_eq!(level.test_epochs(), (u64::MAX, u64::MAX));
        level.test_bump_epochs();
        assert_eq!(level.test_epochs(), (u64::MAX, u64::MAX), "never wraps");

        // Post-only cannot linearize its depth scan: rejected with the error.
        let result = take(&level, 5, TimeInForce::Gtc, TakerKind::PostOnly);
        assert_eq!(result.outcome(), MatchOutcome::Rejected);
        assert_eq!(
            result.error(),
            Some(&exhausted(ExhaustedCounter::MutationEpoch))
        );
        assert!(result.trades().is_empty());

        // Snapshot falls back to its structural single-side check and still
        // round-trips through the checksummed package.
        let snapshot = level.snapshot().expect("single-side snapshot");
        let package = PriceLevelSnapshotPackage::new(snapshot).expect("package");
        let restored = PriceLevel::from_snapshot_package(package).expect("restore");
        assert_eq!(restored.test_epochs(), (0, 0), "restore rebuilds epochs");
        assert_eq!(restored.visible_quantity(), 10);
        restored
            .add_order(standard(2, 1))
            .expect("fresh epochs admit");
    }

    // ------------------------------------------------------------------
    // Step pre-mutation check order with #168 (trade ids) and #169
    // (match_against errors): match_against error, then trade id, then FIFO
    // sequence, then visible headroom.
    // ------------------------------------------------------------------

    /// A generator with exactly `left` trade ids still issuable (restored
    /// through its public serde form, as in the #168 tests).
    fn generator_with(left: u64) -> UuidGenerator {
        let counter = UuidGenerator::EXHAUSTED - left;
        let json = format!(
            r#"{{"namespace":"6ba7b810-9dad-11d1-80b4-00c04fd430c8","counter":{counter}}}"#
        );
        let generator: UuidGenerator = serde_json::from_str(&json).expect("generator");
        assert_eq!(generator.remaining(), left);
        generator
    }

    fn take_with(
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

    /// The #169 reserve whose partial fill overflows `match_against`.
    fn overflowing_reserve(id: u64) -> OrderType<()> {
        OrderType::ReserveOrder {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(u64::MAX),
            hidden_quantity: Quantity::new(u64::MAX),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(u64::MAX),
            replenish_amount: std::num::NonZeroU64::new(u64::MAX),
            auto_replenish: true,
            extra_fields: (),
        }
    }

    fn id_exhausted(result: &MatchResult) -> bool {
        matches!(
            result.error(),
            Some(PriceLevelError::CapacityExceeded {
                resource: CapacityResource::IdSequence,
                ..
            })
        )
    }

    #[test]
    fn trade_id_exhaustion_is_reported_before_sequence_exhaustion() {
        // Maker 1 fills with the last id; the replenishing iceberg then has
        // neither a trade id nor a FIFO sequence: the id check comes first.
        let level = level_with(vec![standard(1, 5), iceberg(2, 10, 100), standard(3, 7)]);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let iceberg_seq = level.test_queue().test_seq_of(Id::from_u64(2));
        let generator = generator_with(1);

        let result = take_with(&level, 15, TimeInForce::Gtc, &generator);
        assert!(id_exhausted(&result), "got {:?}", result.error());
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.remaining_quantity().as_u64(), 10);
        assert_eq!(level.test_queue().test_seq_of(Id::from_u64(2)), iceberg_seq);
        assert_eq!(level.test_queue().test_next_seq(), u64::MAX);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn sequence_exhaustion_with_ids_available_skips_the_reserved_id() {
        let level = level_with(vec![standard(1, 5), iceberg(2, 10, 100)]);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let generator = generator_with(5);

        let result = take_with(&level, 15, TimeInForce::Gtc, &generator);
        assert_eq!(
            result.error(),
            Some(&exhausted(ExhaustedCounter::QueueSequence))
        );
        assert_eq!(result.trades().len(), 1);
        // One id for maker 1's trade, one reserved for the stopped step and
        // skipped (never reissued).
        assert_eq!(generator.remaining(), 3);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn match_against_error_is_reported_before_sequence_and_id_exhaustion() {
        let level = PriceLevel::new(PRICE);
        level.add_order(standard(1, 5)).expect("admit");
        level
            .test_rest_unadmitted(overflowing_reserve(2))
            .expect("rest failing maker");
        level.test_queue().test_seed_next_seq(u64::MAX);
        let visible = level.visible_quantity();

        // One id, for maker 1 only; the failing maker needs neither an id nor
        // a sequence because `match_against` fails first.
        let generator = generator_with(1);
        let result = take_with(&level, 10, TimeInForce::Gtc, &generator);
        assert!(
            matches!(
                result.error(),
                Some(PriceLevelError::InvalidOperation { .. })
            ),
            "got {:?}",
            result.error()
        );
        assert_eq!(result.trades().len(), 1);
        assert_eq!(generator.remaining(), 0);
        assert_eq!(level.visible_quantity(), visible - 5);
    }

    #[test]
    fn fill_or_kill_preflight_orders_error_then_sequence_then_ids() {
        // A dry-run match_against error wins over sequence exhaustion.
        let level = PriceLevel::new(PRICE);
        level.add_order(standard(1, 5)).expect("admit");
        level
            .test_rest_unadmitted(overflowing_reserve(2))
            .expect("rest failing maker");
        level.test_queue().test_seed_next_seq(u64::MAX);
        let result = take_with(&level, 10, TimeInForce::Fok, &generator_with(0));
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert!(matches!(
            result.error(),
            Some(PriceLevelError::InvalidOperation { .. })
        ));

        // Sequence headroom is checked before the trade-id block.
        let level = level_with(vec![standard(1, 5), iceberg(2, 10, 100), standard(3, 7)]);
        level.test_queue().test_seed_next_seq(u64::MAX);
        let before = state(&level);
        let result = take_with(&level, 15, TimeInForce::Fok, &generator_with(0));
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_eq!(
            result.error(),
            Some(&exhausted(ExhaustedCounter::QueueSequence))
        );
        assert_eq!(state(&level), before);

        // With one sequence left the replenishment fits; the id block (2
        // trades) is then the binding limit.
        level.test_queue().test_seed_next_seq(u64::MAX - 1);
        let before = state(&level);
        let result = take_with(&level, 15, TimeInForce::Fok, &generator_with(1));
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert!(id_exhausted(&result), "got {:?}", result.error());
        assert_eq!(state(&level), before);

        // Enough of both: fills completely, using the last sequence.
        let result = take_with(&level, 15, TimeInForce::Fok, &generator_with(2));
        assert!(result.error().is_none());
        assert!(result.is_complete());
        assert_eq!(
            level.test_queue().test_seq_of(Id::from_u64(2)),
            Some(u64::MAX - 1)
        );
        assert_counters_match_queue(&level);
    }
}
