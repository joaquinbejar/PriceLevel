//! Issue #164: engine and snapshot collection growth is fallible and happens
//! before state mutation.
//!
//! Every test injects a reservation refusal through the `cfg(test)`-only
//! `utils::alloc::test_seam` (never real memory exhaustion) and checks the
//! typed `CapacityExceeded` error together with the preserved state: the
//! queue (ids, per-order quantities, FIFO order), the level counters, caller
//! buffers, and the `MatchResult` contract (committed prefix + remaining).

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::{CapacityResource, PriceLevelError};
    use crate::execution::{MatchOutcome, MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, Side, TimeInForce};
    use crate::price_level::level::{
        PriceLevel, count_park, set_pre_fok_lock_hook, set_sweep_start_hook,
    };
    use crate::price_level::order_queue::{
        FrontAction, FrontOutcome, OrderQueue, ParkedSeqs, UpdateDecision,
        disable_park_inline_slot, snapshot_hook,
    };
    use crate::price_level::{PriceLevelData, PriceLevelSnapshot, PriceLevelSnapshotPackage};
    use crate::utils::alloc::test_seam;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::str::FromStr;
    use std::sync::Arc;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const TAKER: u64 = 999;

    fn standard(id: u64, quantity: u64) -> OrderType<()> {
        standard_at(id, quantity, 1_616_823_000_000 + id)
    }

    fn standard_at(id: u64, quantity: u64, timestamp: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(timestamp),
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

    fn sample_level() -> PriceLevel {
        level_with(vec![standard(1, 5), iceberg(2, 3, 9), standard(3, 7)])
    }

    fn generator() -> UuidGenerator {
        UuidGenerator::new(
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("namespace"),
        )
    }

    fn take(level: &PriceLevel, quantity: u64, tif: TimeInForce) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(TAKER),
            tif,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator(),
        )
    }

    /// Queue-derived view of the level (read with no injection armed).
    #[derive(Debug, PartialEq)]
    struct LevelState {
        ids: Vec<Id>,
        visible: Vec<u64>,
        hidden: Vec<u64>,
        visible_counter: u64,
        hidden_counter: u64,
        order_count: usize,
    }

    fn state(level: &PriceLevel) -> LevelState {
        let orders = level.snapshot_by_insertion_seq().expect("materialize");
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
        }
    }

    fn assert_counters_match_queue(level: &PriceLevel) {
        let s = state(level);
        assert_eq!(s.visible_counter, s.visible.iter().sum::<u64>(), "visible");
        assert_eq!(s.hidden_counter, s.hidden.iter().sum::<u64>(), "hidden");
        assert_eq!(s.order_count, s.ids.len(), "order_count");
    }

    fn assert_capacity(err: &PriceLevelError, want: CapacityResource) {
        match err {
            PriceLevelError::CapacityExceeded { resource, .. } if *resource == want => {}
            other => panic!("expected CapacityExceeded({want:?}), got {other:?}"),
        }
    }

    fn ids(orders: &[Arc<OrderType<()>>]) -> Vec<Id> {
        orders.iter().map(|o| o.id()).collect()
    }

    // ------------------------------------------------------------------
    // Queue materializations
    // ------------------------------------------------------------------

    #[test]
    fn materializers_report_typed_error_and_leave_level_unchanged() {
        let level = sample_level();
        let before = state(&level);
        {
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
            assert_capacity(
                &level.snapshot_orders().expect_err("refused"),
                CapacityResource::OrderSnapshot,
            );
            assert_capacity(
                &level.snapshot_by_insertion_seq().expect_err("refused"),
                CapacityResource::OrderSnapshot,
            );
            assert_capacity(
                &PriceLevelData::try_from(&level).expect_err("refused"),
                CapacityResource::OrderSnapshot,
            );
            assert_capacity(
                &level
                    .matchable_quantity(10, Id::from_u64(TAKER))
                    .expect_err("refused"),
                CapacityResource::OrderSnapshot,
            );
            assert_capacity(
                &level.snapshot().expect_err("refused"),
                CapacityResource::OrderSnapshot,
            );
            assert!(test_seam::injected() >= 5);
        }
        assert_eq!(state(&level), before);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn a_refused_output_reservation_leaves_the_caller_buffer_untouched() {
        let level = sample_level();
        let other = level_with(vec![standard(40, 1), standard(41, 2)]);
        let mut out = other.snapshot_by_insertion_seq().expect("materialize");
        out.shrink_to_fit();
        let before_ids = ids(&out);
        let before_capacity = out.capacity();

        // Refuse the internal pairs buffer.
        {
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
            let err = level.snapshot_by_seq_into(&mut out).expect_err("refused");
            assert_capacity(&err, CapacityResource::OrderSnapshot);
        }
        assert_eq!(ids(&out), before_ids);
        assert_eq!(out.capacity(), before_capacity);

        // Let the pairs buffer through and refuse only `out`'s own growth.
        {
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 1);
            let err = level.snapshot_by_seq_into(&mut out).expect_err("refused");
            assert_capacity(&err, CapacityResource::OrderSnapshot);
            assert_eq!(test_seam::injected(), 1);
        }
        assert_eq!(ids(&out), before_ids, "out must not be cleared on error");
        assert_eq!(out.capacity(), before_capacity);

        // Unarmed: the call succeeds and yields the sweep order.
        level.snapshot_by_seq_into(&mut out).expect("materialize");
        assert_eq!(
            out.iter().map(|o| o.id()).collect::<Vec<_>>(),
            vec![Id::from_u64(1), Id::from_u64(2), Id::from_u64(3)]
        );
    }

    #[test]
    fn unstable_sorts_stay_deterministic_on_equal_timestamps() {
        // Every order shares one timestamp, so `(timestamp, seq)` and `seq`
        // are the only tiebreaks: both views must be insertion order.
        let orders: Vec<_> = (1..=64).map(|id| standard_at(id, 1, 1_000)).collect();
        let level = level_with(orders);
        let want: Vec<Id> = (1..=64).map(Id::from_u64).collect();
        for _ in 0..8 {
            assert_eq!(ids(&level.snapshot_orders().expect("materialize")), want);
            assert_eq!(
                ids(&level.snapshot_by_insertion_seq().expect("materialize")),
                want
            );
        }
    }

    #[test]
    fn queue_views_and_adapters_are_fallible() {
        let queue = OrderQueue::new();
        for id in 1..=3 {
            queue.try_push(Arc::new(standard(id, 2))).expect("push");
        }
        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        assert_capacity(
            &queue.snapshot_vec().expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
        assert_capacity(
            &queue.to_vec().expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
        assert!(serde_json::to_string(&queue).is_err());
        // `Display` never reports `fmt::Error`; it writes a marker that the
        // parser rejects, so a failed rendering is not an empty queue.
        let text = queue.to_string();
        assert!(text.starts_with("OrderQueue:orders=!"), "{text}");
        assert!(format!("{queue:?}").contains("<unavailable"));
        drop(_fail);
        assert!(OrderQueue::from_str(&text).is_err());
        assert_eq!(queue.len(), 3);

        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        let converted: Result<Vec<Arc<OrderType<()>>>, _> = queue.try_into();
        assert_capacity(
            &converted.expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
    }

    #[test]
    fn level_display_and_serialize_are_fallible_without_fmt_errors() {
        let level = sample_level();
        let (text, json) = {
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
            assert!(format!("{level:?}").contains("<unavailable"));
            (level.to_string(), serde_json::to_string(&level))
        };
        assert!(text.contains(";orders=!Capacity exceeded"), "{text}");
        assert!(PriceLevel::from_str(&text).is_err());
        let err = json.expect_err("refused");
        assert!(err.to_string().contains("order snapshot"), "{err}");

        // Unarmed: `Serialize for PriceLevel` is byte-identical to the
        // `PriceLevelData` it describes, and the text round-trips.
        let direct = serde_json::to_string(&level).expect("serialize");
        let data = PriceLevelData::try_from(&level).expect("materialize");
        assert_eq!(
            direct,
            serde_json::to_string(&data).expect("serialize data")
        );
        let back: PriceLevel = serde_json::from_str(&direct).expect("deserialize");
        assert_eq!(state(&back), state(&level));
        let parsed = PriceLevel::from_str(&level.to_string()).expect("parse");
        assert_eq!(parsed.order_count(), level.order_count());
    }

    #[test]
    fn decoding_price_level_data_orders_is_fallible() {
        let level = sample_level();
        let json = serde_json::to_string(&level).expect("serialize");
        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        let err = serde_json::from_str::<PriceLevelData>(&json).expect_err("refused");
        assert!(err.to_string().contains("order snapshot"), "{err}");
    }

    // ------------------------------------------------------------------
    // Matching
    // ------------------------------------------------------------------

    #[test]
    fn fill_or_kill_dry_run_refusal_kills_with_the_level_untouched() {
        let level = sample_level();
        let before = state(&level);
        let result = {
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
            take(&level, 10, TimeInForce::Fok)
        };
        assert!(result.was_killed());
        assert!(result.trades().is_empty());
        assert_eq!(result.remaining_quantity().as_u64(), 10);
        assert_capacity(
            result.error().expect("error"),
            CapacityResource::OrderSnapshot,
        );
        assert_eq!(state(&level), before);

        // Unarmed, the same taker fills: behaviour is otherwise unchanged.
        let filled = take(&level, 10, TimeInForce::Fok);
        assert_eq!(filled.outcome(), MatchOutcome::Filled);
        assert!(filled.error().is_none());
        assert_counters_match_queue(&level);
    }

    #[test]
    fn gtc_sweeps_do_not_materialize_or_park() {
        // A plain sweep takes no order snapshot and parks nothing: arming
        // both resources changes nothing.
        let control = sample_level();
        let control_result = take(&control, 20, TimeInForce::Gtc);
        for resource in [
            CapacityResource::SweepScratch,
            CapacityResource::OrderSnapshot,
        ] {
            let level = sample_level();
            let result = {
                let _armed = test_seam::fail_after(resource, 0);
                let r = take(&level, 20, TimeInForce::Gtc);
                assert_eq!(test_seam::injected(), 0, "{resource:?}");
                r
            };
            assert!(result.error().is_none());
            assert_eq!(result.trades().len(), control_result.trades().len());
            assert_eq!(state(&level), state(&control));
        }
    }

    /// Admits an order sharing the taker id right after the pre-sweep
    /// self-match check, so the sweep reaches it and parks it.
    fn admit_taker_id_at_sweep_start(level: &Arc<PriceLevel>, quantity: u64) -> impl Drop {
        let level = Arc::clone(level);
        set_sweep_start_hook(Box::new(move || {
            level
                .add_order(standard(TAKER, quantity))
                .expect("admit taker id");
        }))
    }

    #[test]
    fn a_refused_park_after_a_fill_keeps_the_committed_prefix() {
        let level = Arc::new(level_with(vec![standard(1, 5)]));
        let _hook = admit_taker_id_at_sweep_start(&level, 4);
        // The taker-id order is admitted behind maker 1 at sweep start. The
        // inline slot is disabled so the single park reaches the spill set's
        // (refused) reservation.
        let result = {
            let _spill = disable_park_inline_slot();
            let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
            take(&level, 20, TimeInForce::Gtc)
        };
        // Maker 1 filled (5); the taker-id order could not be parked, so the
        // sweep stopped there with the error and the true remainder.
        assert_eq!(result.trades().len(), 1);
        assert_eq!(
            result.trades().as_vec()[0].maker_order_id(),
            Id::from_u64(1)
        );
        assert_eq!(result.remaining_quantity().as_u64(), 15);
        assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);
        assert_capacity(
            result.error().expect("error"),
            CapacityResource::SweepScratch,
        );
        // The parked maker is untouched; counters agree with the queue.
        let s = state(&level);
        assert_eq!(s.ids, vec![Id::from_u64(TAKER)]);
        assert_eq!(s.visible, vec![4]);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn a_refused_park_before_any_fill_reports_no_trades() {
        let level = Arc::new(PriceLevel::new(PRICE));
        let admit = Arc::clone(&level);
        let _hook = set_sweep_start_hook(Box::new(move || {
            admit.add_order(standard(TAKER, 4)).expect("admit taker id");
            admit.add_order(standard(7, 6)).expect("admit maker");
        }));
        let result = {
            let _spill = disable_park_inline_slot();
            let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
            take(&level, 3, TimeInForce::Ioc)
        };
        assert!(result.trades().is_empty());
        assert_eq!(result.remaining_quantity().as_u64(), 3);
        assert_capacity(
            result.error().expect("error"),
            CapacityResource::SweepScratch,
        );
        let s = state(&level);
        assert_eq!(s.ids, vec![Id::from_u64(TAKER), Id::from_u64(7)]);
        assert_eq!(s.visible, vec![4, 6]);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn an_accepted_park_still_skips_the_taker_id_maker() {
        // Control for the two tests above: without an injected refusal the
        // park succeeds and the sweep trades past the skipped maker.
        let level = Arc::new(PriceLevel::new(PRICE));
        let admit = Arc::clone(&level);
        let _hook = set_sweep_start_hook(Box::new(move || {
            admit.add_order(standard(TAKER, 4)).expect("admit taker id");
            admit.add_order(standard(7, 6)).expect("admit maker");
        }));
        let result = take(&level, 3, TimeInForce::Ioc);
        assert!(result.error().is_none());
        assert_eq!(result.trades().len(), 1);
        assert_eq!(
            result.trades().as_vec()[0].maker_order_id(),
            Id::from_u64(7)
        );
        assert_counters_match_queue(&level);
    }

    #[test]
    fn a_single_live_park_never_allocates() {
        // Item 7 of the #164 review: the only park that fires today (one
        // order sharing the taker id) uses the inline slot, so even a refusing
        // allocator cannot stop the sweep there.
        let level = Arc::new(level_with(vec![standard(1, 5)]));
        let admit = Arc::clone(&level);
        let _hook = set_sweep_start_hook(Box::new(move || {
            admit.add_order(standard(TAKER, 4)).expect("admit taker id");
            admit.add_order(standard(7, 6)).expect("admit maker");
        }));
        let result = {
            let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
            let r = take(&level, 20, TimeInForce::Ioc);
            assert_eq!(test_seam::injected(), 0, "no reservation attempted");
            r
        };
        assert!(result.error().is_none());
        let makers: Vec<Id> = result
            .trades()
            .as_vec()
            .iter()
            .map(|t| t.maker_order_id())
            .collect();
        assert_eq!(makers, vec![Id::from_u64(1), Id::from_u64(7)]);
        assert_eq!(state(&level).ids, vec![Id::from_u64(TAKER)]);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn match_front_park_refusal_is_a_no_op_and_carries_the_original_error() {
        let queue = OrderQueue::new();
        queue.try_push(Arc::new(standard(1, 5))).expect("push");
        queue.try_push(Arc::new(standard(2, 6))).expect("push");
        let mut set_aside = ParkedSeqs::new();
        let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);

        // First park: inline slot, no reservation.
        let first = queue.match_front(&mut set_aside, |_, _| (FrontAction::SetAside, ()));
        assert!(matches!(first, FrontOutcome::Matched { .. }));
        assert_eq!(set_aside.len(), 1);
        assert_eq!(test_seam::injected(), 0);

        // Second live park: the spill set must grow and is refused. The
        // queue's own typed error is carried, not a fabricated one.
        let second = queue.match_front(&mut set_aside, |_, _| (FrontAction::SetAside, ()));
        match second {
            FrontOutcome::ParkRefused { error, .. } => assert_eq!(
                error,
                PriceLevelError::CapacityExceeded {
                    resource: CapacityResource::SweepScratch,
                    additional: 1,
                }
            ),
            other => panic!("expected ParkRefused, got {other:?}"),
        }
        assert_eq!(test_seam::injected(), 1);
        assert_eq!(set_aside.len(), 1, "the refused sequence is not parked");
        let orders = queue.to_vec().expect("materialize");
        assert_eq!(ids(&orders), vec![Id::from_u64(1), Id::from_u64(2)]);
        assert_eq!(orders[0].visible_quantity().as_u64(), 5);
        assert_eq!(orders[1].visible_quantity().as_u64(), 6);
        drop(_fail);

        // A pre-reserved set never reaches the (refused) reservation.
        let mut reserved = ParkedSeqs::new();
        reserved.try_reserve(2).expect("reserve");
        let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
        for _ in 0..2 {
            let outcome = queue.match_front(&mut reserved, |_, _| (FrontAction::SetAside, ()));
            assert!(matches!(outcome, FrontOutcome::Matched { .. }));
        }
        assert_eq!(reserved.len(), 2);
        assert!(!reserved.is_empty());
        assert_eq!(test_seam::injected(), 0);
    }

    #[test]
    fn park_set_reservation_accounts_for_the_inline_slot() {
        let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
        let mut set = ParkedSeqs::new();
        // Zero or one predicted park needs no allocation.
        set.try_reserve(0).expect("zero");
        set.try_reserve(1).expect("inline covers one");
        assert_eq!(test_seam::injected(), 0);
        // Two need a spill slot: refused, set unchanged.
        match set.try_reserve(2) {
            Err(PriceLevelError::CapacityExceeded {
                resource: CapacityResource::SweepScratch,
                additional: 1,
            }) => {}
            other => panic!("expected a SweepScratch refusal, got {other:?}"),
        }
        assert!(set.is_empty());
    }

    #[test]
    fn dry_run_park_count_overflow_is_a_typed_error() {
        // Review item 5: an overflowing park count no longer breaks silently
        // (which under-reported the fill); the dry run records this error.
        assert_eq!(count_park(0), Ok(1));
        assert_eq!(
            count_park(usize::MAX),
            Err(PriceLevelError::CapacityExceeded {
                resource: CapacityResource::SweepScratch,
                additional: 1,
            })
        );
    }

    /// Records `(level, message)` of every event dispatched on this thread.
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Message(String);
            impl tracing::field::Visit for Message {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut message = Message(String::new());
            event.record(&mut message);
            self.0
                .lock()
                .expect("capture lock")
                .push((*event.metadata().level(), message.0));
        }
    }

    /// Runs `attempt` under a capturing subscriber until it emits an `ERROR`
    /// event containing `want`, and returns that attempt's value.
    ///
    /// `tracing` caches callsite interest process-wide, so a concurrent test
    /// thread that (re)registers dispatchers can transiently hide an event
    /// from this thread's scoped subscriber (the same reason
    /// `caller_boundaries.rs` retries). Each attempt must therefore build its
    /// own fresh state.
    fn with_error_event<R>(want: &str, mut attempt: impl FnMut() -> R) -> R {
        use tracing_subscriber::layer::SubscriberExt;

        for _ in 0..1_000 {
            let captured = Captured::default();
            let subscriber = tracing_subscriber::registry().with(captured.clone());
            let value = tracing::subscriber::with_default(subscriber, || {
                tracing::callsite::rebuild_interest_cache();
                attempt()
            });
            let seen = captured
                .0
                .lock()
                .expect("capture lock")
                .iter()
                .any(|(lvl, msg)| *lvl == tracing::Level::ERROR && msg.contains(want));
            if seen {
                return value;
            }
        }
        panic!("no ERROR event containing {want:?} was observed");
    }

    #[test]
    fn fill_or_kill_dry_run_refusal_is_logged_at_error() {
        let result = with_error_event("dry-run working snapshot could not be reserved", || {
            let level = sample_level();
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
            take(&level, 10, TimeInForce::Fok)
        });
        assert!(result.was_killed());
    }

    // ------------------------------------------------------------------
    // Snapshot, restore, serialization
    // ------------------------------------------------------------------

    #[test]
    fn snapshot_capacity_failure_is_returned_without_recollecting() {
        let level = sample_level();
        let attempts = std::rc::Rc::new(std::cell::Cell::new(0_u32));
        let counter = std::rc::Rc::clone(&attempts);
        let _hook = snapshot_hook::install(move |event| {
            if matches!(event, snapshot_hook::SnapshotHookEvent::AttemptStart) {
                counter.set(counter.get() + 1);
            }
        });
        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        assert_capacity(
            &level.snapshot().expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
        assert_capacity(
            &level.snapshot_to_json().expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
        assert_eq!(attempts.get(), 2, "one attempt per call, no recollection");
    }

    #[test]
    fn restore_duplicate_set_refusal_is_typed() {
        let snapshot = sample_level().snapshot().expect("snapshot");
        let _fail = test_seam::fail_after(CapacityResource::RestoreScratch, 0);
        match PriceLevel::from_snapshot(snapshot) {
            Err(PriceLevelError::CapacityExceeded {
                resource: CapacityResource::RestoreScratch,
                additional: 3,
            }) => {}
            other => panic!("expected RestoreScratch refusal, got {other:?}"),
        }
    }

    #[test]
    fn package_json_and_checksum_growth_is_fallible() {
        let level = sample_level();
        let package = level.snapshot_package().expect("package");

        {
            let _fail = test_seam::fail_after(CapacityResource::SerializationBuffer, 0);
            assert_capacity(
                &package.to_json().expect_err("refused"),
                CapacityResource::SerializationBuffer,
            );
            // Checksum hex buffer (the digest itself streams, unbuffered).
            let snapshot = level.snapshot().expect("snapshot");
            assert_capacity(
                &PriceLevelSnapshotPackage::new(snapshot).expect_err("refused"),
                CapacityResource::SerializationBuffer,
            );
            assert_capacity(
                &package.validate().expect_err("refused"),
                CapacityResource::SerializationBuffer,
            );
        }

        // A refusal part-way through the output (after some growth).
        {
            let _fail = test_seam::fail_after(CapacityResource::SerializationBuffer, 2);
            assert_capacity(
                &package.to_json().expect_err("refused"),
                CapacityResource::SerializationBuffer,
            );
        }

        // Unarmed: the round-trip and the checksum are unchanged.
        let json = package.to_json().expect("json");
        let restored = PriceLevel::from_snapshot_json(&json).expect("restore");
        assert_eq!(state(&restored), state(&level));
        assert_eq!(
            restored.snapshot_package().expect("package").checksum(),
            package.checksum()
        );
    }

    #[test]
    fn package_decode_growth_is_fallible_and_legacy_payloads_still_restore() {
        let level = sample_level();
        let json = level.snapshot_to_json().expect("json");
        {
            let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
            match PriceLevelSnapshotPackage::from_json(&json) {
                Err(PriceLevelError::DeserializationError { message }) => {
                    assert!(message.contains("order snapshot"), "{message}");
                }
                other => panic!("expected a decode refusal, got {other:?}"),
            }
        }
        {
            let _fail = test_seam::fail_after(CapacityResource::SerializationBuffer, 0);
            // The checksum string is copied out of the borrowed input.
            assert!(PriceLevelSnapshotPackage::from_json(&json).is_err());
        }
        // A payload that omits the orders field still decodes (legacy /
        // hand-built fixtures): the default allocates nothing.
        let empty: PriceLevelSnapshot = serde_json::from_str(
            r#"{"price":1,"visible_quantity":0,"hidden_quantity":0,"order_count":0}"#,
        )
        .expect("decode");
        assert!(empty.orders().is_empty());
    }

    #[test]
    fn snapshot_and_package_try_clone() {
        let level = sample_level();
        let package = level.snapshot_package().expect("package");
        let snapshot = package.snapshot();

        let copy = snapshot.try_clone().expect("try_clone");
        assert_eq!(ids(copy.orders()), ids(snapshot.orders()));
        assert_eq!(copy.visible_quantity(), snapshot.visible_quantity());
        let package_copy = package.try_clone().expect("try_clone");
        assert_eq!(package_copy.checksum(), package.checksum());
        package_copy.validate().expect("valid copy");

        let _fail = test_seam::fail_after(CapacityResource::OrderSnapshot, 0);
        assert_capacity(
            &snapshot.try_clone().expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
        assert_capacity(
            &package.try_clone().expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
        assert_capacity(
            &PriceLevel::try_from(snapshot).expect_err("refused"),
            CapacityResource::OrderSnapshot,
        );
    }

    #[test]
    fn text_parser_growth_reports_the_fixed_text_resource() {
        let level = sample_level();
        let text = level.to_string();
        let _fail = test_seam::fail_after(CapacityResource::Text, 0);
        assert_capacity(
            &PriceLevel::from_str(&text).expect_err("refused"),
            CapacityResource::Text,
        );
    }

    // ------------------------------------------------------------------
    // PR #199 review: a fill-or-kill dry run can predict a park, and a stale
    // inline park key frees itself.
    // ------------------------------------------------------------------

    /// Admits a maker sharing the taker id, then maker 3, between the
    /// self-match lookup and the fill-or-kill exclusive guard, on the matcher
    /// thread (the window a concurrent mutator can use). The queue becomes
    /// `[1, TAKER, 3]`, so the dry run must park the taker-id maker to reach
    /// maker 3.
    fn admit_taker_id_before_fok_lock(level: &Arc<PriceLevel>) -> impl Drop + use<> {
        let admit = Arc::clone(level);
        set_pre_fok_lock_hook(Box::new(move || {
            admit.add_order(standard(TAKER, 4)).expect("admit taker id");
            admit.add_order(standard(3, 7)).expect("admit maker");
        }))
    }

    #[test]
    fn fill_or_kill_park_set_refusal_kills_before_any_mutation_and_logs() {
        let (result, level) = with_error_event("park set could not be reserved", || {
            let level = Arc::new(level_with(vec![standard(1, 5)]));
            let _hook = admit_taker_id_before_fok_lock(&level);
            // The inline slot is disabled so the one predicted park needs a
            // spill reservation, which is refused on the matcher thread.
            let _spill = disable_park_inline_slot();
            let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
            let r = take(&level, 10, TimeInForce::Fok);
            assert_eq!(test_seam::injected(), 1, "the park set was reserved");
            (r, level)
        });

        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert!(result.trades().is_empty());
        assert!(result.filled_order_ids().is_empty());
        assert_eq!(result.remaining_quantity().as_u64(), 10);
        assert_capacity(
            result.error().expect("error"),
            CapacityResource::SweepScratch,
        );
        // Only the admissions happened: every order rests unchanged.
        let s = state(&level);
        assert_eq!(
            s.ids,
            vec![Id::from_u64(1), Id::from_u64(TAKER), Id::from_u64(3)]
        );
        assert_eq!(s.visible, vec![5, 4, 7]);
        assert_counters_match_queue(&level);
    }

    #[test]
    fn fill_or_kill_with_one_predicted_park_uses_the_inline_slot() {
        let level = Arc::new(level_with(vec![standard(1, 5)]));
        let _hook = admit_taker_id_before_fok_lock(&level);
        let result = {
            let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
            let r = take(&level, 10, TimeInForce::Fok);
            assert_eq!(test_seam::injected(), 0, "no spill reservation");
            r
        };
        assert!(result.error().is_none());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        let makers: Vec<Id> = result
            .trades()
            .as_vec()
            .iter()
            .map(|t| t.maker_order_id())
            .collect();
        assert_eq!(makers, vec![Id::from_u64(1), Id::from_u64(3)]);
        let s = state(&level);
        assert_eq!(s.ids, vec![Id::from_u64(TAKER), Id::from_u64(3)]);
        assert_eq!(s.visible, vec![4, 2]);
        assert_counters_match_queue(&level);
    }

    /// One sweep step the way `match_order` drives it for taker `TAKER`:
    /// park the maker sharing the taker id, fully consume any other.
    fn self_skip_step(queue: &OrderQueue, set: &mut ParkedSeqs) -> FrontOutcome<bool> {
        queue.match_front(set, |_seq, order| {
            if order.id() == Id::from_u64(TAKER) {
                (FrontAction::SetAside, false)
            } else {
                (FrontAction::Remove, true)
            }
        })
    }

    fn assert_parked_without_spill(outcome: &FrontOutcome<bool>, set: &ParkedSeqs) {
        assert!(
            matches!(outcome, FrontOutcome::Matched { result: false }),
            "{outcome:?}"
        );
        assert_eq!(set.len(), 1, "one live park, held inline");
        assert_eq!(test_seam::injected(), 0, "no spill reservation");
    }

    #[test]
    fn a_stale_inline_park_frees_itself_after_cancel_and_readmit() {
        let queue = OrderQueue::new();
        queue.try_push(Arc::new(standard(TAKER, 4))).expect("push");
        let mut set = ParkedSeqs::new();
        let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);

        let first = self_skip_step(&queue, &mut set);
        assert_parked_without_spill(&first, &set);
        let parked = set.inline_seq().expect("inline park");

        // Cancel the parked maker, then readmit the same id: it rests under a
        // fresh sequence and its old index key is gone, so the scan never
        // revisits the parked key.
        assert!(queue.remove(Id::from_u64(TAKER)).is_some());
        queue
            .try_push(Arc::new(standard(TAKER, 4)))
            .expect("readmit");

        let second = self_skip_step(&queue, &mut set);
        assert_parked_without_spill(&second, &set);
        assert_ne!(set.inline_seq(), Some(parked), "dead key replaced");
        assert!(queue.debug_map_index_consistent());
    }

    #[test]
    fn a_stale_inline_park_frees_itself_after_a_tail_demotion() {
        let queue = OrderQueue::new();
        queue.try_push(Arc::new(standard(TAKER, 4))).expect("push");
        let mut set = ParkedSeqs::new();
        let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);

        let first = self_skip_step(&queue, &mut set);
        assert_parked_without_spill(&first, &set);
        let parked = set.inline_seq().expect("inline park");

        // A quantity increase moves the maker to a fresh tail sequence and
        // re-keys the index away from the parked key.
        let committed = queue.update_entry(Id::from_u64(TAKER), |_live| {
            Ok(UpdateDecision::ReplaceAtTail(
                Arc::new(standard(TAKER, 9)),
                queue.try_reserve_seq()?,
            ))
        });
        assert!(matches!(committed, Some(Ok(_))));

        let second = self_skip_step(&queue, &mut set);
        assert_parked_without_spill(&second, &set);
        assert_ne!(set.inline_seq(), Some(parked), "dead key replaced");
        assert!(queue.debug_map_index_consistent());
    }

    #[test]
    fn a_live_inline_park_is_kept_and_a_second_live_park_spills() {
        // The self-clear must not drop a key that is still live: with the
        // inline key live, a second live park spills (and is refused here).
        let queue = OrderQueue::new();
        queue.try_push(Arc::new(standard(1, 5))).expect("push");
        queue.try_push(Arc::new(standard(2, 6))).expect("push");
        let mut set = ParkedSeqs::new();
        let _fail = test_seam::fail_after(CapacityResource::SweepScratch, 0);
        let first = queue.match_front(&mut set, |_, _| (FrontAction::SetAside, ()));
        assert!(matches!(first, FrontOutcome::Matched { .. }));
        let parked = set.inline_seq().expect("inline park");
        let second = queue.match_front(&mut set, |_, _| (FrontAction::SetAside, ()));
        assert!(matches!(second, FrontOutcome::ParkRefused { .. }));
        assert_eq!(set.inline_seq(), Some(parked), "live key kept");
        assert_eq!(test_seam::injected(), 1);
    }
}
