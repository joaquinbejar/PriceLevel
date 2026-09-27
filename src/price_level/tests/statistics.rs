#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::price_level::PriceLevelStatistics;
    use crate::utils::{TimestampMs, UnixClock};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::thread;

    /// Test clock returning a fixed reading.
    struct FixedClock(u64);

    impl UnixClock for FixedClock {
        fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
            Ok(TimestampMs::new(self.0))
        }
    }

    /// Test clock that always fails, as a pre-epoch reading would.
    struct FailingClock;

    impl UnixClock for FailingClock {
        fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
            TimestampMs::try_from_system_time(
                std::time::UNIX_EPOCH
                    .checked_sub(std::time::Duration::from_millis(1))
                    .unwrap(),
            )
        }
    }

    const NOW: u64 = 1_716_000_000_000;

    #[test]
    fn test_new() {
        let stats = PriceLevelStatistics::new();
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.last_execution_time(), 0);
        // Deterministic and clock-free (issue #171): unstamped start time.
        assert_eq!(stats.first_arrival_time(), 0);
        assert_eq!(stats.sum_waiting_time(), 0);
    }

    #[test]
    fn test_new_at_and_try_new_stamp_start_time() {
        let stats = PriceLevelStatistics::new_at(TimestampMs::new(NOW));
        assert_eq!(stats.first_arrival_time(), NOW);

        let stats = PriceLevelStatistics::try_new(&FixedClock(NOW + 5)).unwrap();
        assert_eq!(stats.first_arrival_time(), NOW + 5);
        assert_eq!(stats.orders_added(), 0);

        let clock: &dyn UnixClock = &FixedClock(NOW);
        assert_eq!(
            PriceLevelStatistics::try_new(clock)
                .unwrap()
                .first_arrival_time(),
            NOW
        );
    }

    #[test]
    fn test_try_new_propagates_clock_failure() {
        let err = PriceLevelStatistics::try_new(&FailingClock).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
    }

    #[test]
    fn test_record_execution_error_paths() {
        let stats = PriceLevelStatistics::new();

        let now = NOW;

        // Future timestamps (maker arrived after execution) should return an
        // explicit error.
        assert!(stats.record_execution(1, 100, now + 1_000, now).is_err());

        // Multiplication overflow should return an explicit error.
        assert!(stats.record_execution(u64::MAX, u128::MAX, 0, now).is_err());
    }

    #[test]
    fn test_default() {
        let stats = PriceLevelStatistics::default();
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        // Deterministic: identical to `new()`, no clock read (issue #171).
        assert_eq!(stats.first_arrival_time(), 0);
        assert_eq!(stats.to_string(), PriceLevelStatistics::new().to_string());
    }

    #[test]
    fn test_record_operations() {
        let stats = PriceLevelStatistics::new();

        // Test recording added orders
        for _ in 0..5 {
            stats.record_order_added();
        }
        assert_eq!(stats.orders_added(), 5);

        // Test recording removed orders
        for _ in 0..3 {
            stats.record_order_removed();
        }
        assert_eq!(stats.orders_removed(), 3);

        // Deterministic execution timestamp threaded in by the caller.
        let execution_time: u64 = 1_716_000_000_000;

        // Test recording executed orders
        assert!(stats.record_execution(10, 100, 0, execution_time).is_ok()); // qty=10, price=100, no timestamp
        assert_eq!(stats.orders_executed(), 1);
        assert_eq!(stats.quantity_executed(), 10);
        assert_eq!(stats.value_executed(), 1000); // 10 * 100
        assert!(stats.last_execution_time() > 0);

        // Test with a maker timestamp 1 second before the execution time.
        let timestamp = execution_time - 1000; // 1 second ago

        assert!(
            stats
                .record_execution(5, 200, timestamp, execution_time)
                .is_ok()
        );
        assert_eq!(stats.orders_executed(), 2);
        assert_eq!(stats.quantity_executed(), 15); // 10 + 5
        assert_eq!(stats.value_executed(), 2000); // 1000 + (5 * 200)
        assert!(stats.sum_waiting_time() >= 1000); // At least 1 second waiting time
    }

    #[test]
    fn test_average_execution_price() {
        let stats = PriceLevelStatistics::new();

        // Test with no executions
        assert_eq!(stats.average_execution_price(), None);

        // Test with executions
        let execution_time: u64 = 1_716_000_000_000;
        assert!(stats.record_execution(10, 100, 0, execution_time).is_ok()); // Total value: 1000
        assert!(stats.record_execution(20, 150, 0, execution_time).is_ok()); // Total value: 3000 + 1000 = 4000

        // Average price should be 4000 / 30 = 133.33...
        let avg_price = stats.average_execution_price().unwrap();
        assert!((avg_price - 133.33).abs() < 0.01);
    }

    #[test]
    fn test_average_waiting_time() {
        let stats = PriceLevelStatistics::new();

        // Test with no executions
        assert_eq!(stats.average_waiting_time(), None);

        // Test with executions against a fixed execution time.
        let now: u64 = 1_716_000_000_000;

        assert!(stats.record_execution(10, 100, now - 1000, now).is_ok()); // 1 second ago
        assert!(stats.record_execution(20, 150, now - 3000, now).is_ok()); // 3 seconds ago

        // Total waiting time: 1000 + 3000 = 4000ms, average = 2000ms
        let avg_wait = stats.average_waiting_time().unwrap();
        assert!((1900.0..=2100.0).contains(&avg_wait));
    }

    #[test]
    fn test_time_since_last_execution() {
        let stats = PriceLevelStatistics::new();

        // No execution: Ok(None), and the clock is not even consulted.
        assert_eq!(
            stats.time_since_last_execution(&FailingClock).unwrap(),
            None
        );
        assert_eq!(
            stats
                .time_since_last_execution_at(TimestampMs::new(NOW))
                .unwrap(),
            None
        );

        assert!(stats.record_execution(10, 100, 0, NOW - 1000).is_ok());

        assert_eq!(
            stats.time_since_last_execution(&FixedClock(NOW)).unwrap(),
            Some(1000)
        );
        assert_eq!(
            stats
                .time_since_last_execution_at(TimestampMs::new(NOW - 1000))
                .unwrap(),
            Some(0)
        );
    }

    #[test]
    fn test_time_since_last_execution_distinguishes_clock_failure() {
        let stats = PriceLevelStatistics::new();
        assert!(stats.record_execution(10, 100, 0, NOW).is_ok());

        // Clock failure is a typed error, not `None`.
        let err = stats.time_since_last_execution(&FailingClock).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));

        // A clock behind the last execution is a typed error, not `None`.
        let err = stats
            .time_since_last_execution_at(TimestampMs::new(NOW - 1))
            .unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
        let err = stats
            .time_since_last_execution(&FixedClock(NOW - 1))
            .unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
    }

    /// Clock that, when sampled, lets a concurrent matcher thread record a fill
    /// and waits for it to finish before returning its reading: a fill lands
    /// exactly between the reader's `last_execution_time` load and its clock
    /// sample, deterministically (barriers, no sleeps).
    struct InterleavingClock {
        now: u64,
        fill_may_start: std::sync::Barrier,
        fill_done: std::sync::Barrier,
    }

    impl UnixClock for InterleavingClock {
        fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
            self.fill_may_start.wait();
            self.fill_done.wait();
            Ok(TimestampMs::new(self.now))
        }
    }

    #[test]
    fn test_time_since_last_execution_fill_between_load_and_clock_sample() {
        // Review regression (#171): last = 100, clock samples 150, then a
        // concurrent fill at 200 lands before the difference is computed. The
        // clock is healthy, so the result must be measured from the execution
        // observed before sampling (50 ms), not an InvalidOperation.
        let stats = PriceLevelStatistics::new();
        stats.record_execution(1, 1, 0, 100).unwrap();
        let clock = InterleavingClock {
            now: 150,
            fill_may_start: std::sync::Barrier::new(2),
            fill_done: std::sync::Barrier::new(2),
        };

        let elapsed = thread::scope(|scope| {
            scope.spawn(|| {
                clock.fill_may_start.wait();
                stats.record_execution(1, 1, 0, 200).unwrap();
                clock.fill_done.wait();
            });
            stats.time_since_last_execution(&clock)
        });

        assert_eq!(elapsed.unwrap(), Some(50));
        // The concurrent fill was recorded.
        assert_eq!(stats.last_execution_time(), 200);
    }

    #[test]
    fn test_failed_reset_leaves_statistics_unchanged() {
        let stats = PriceLevelStatistics::new_at(TimestampMs::new(NOW - 10_000));
        stats.record_order_added();
        stats.record_order_added();
        stats.record_order_removed();
        assert!(stats.record_execution(10, 100, NOW - 5_000, NOW).is_ok());
        stats.mark_degraded();
        let before = stats.to_string();
        assert!(before.contains("stats_degraded=true"));

        let err = stats.reset(&FailingClock).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));

        // Counters, timestamps and the degraded flag are untouched.
        assert_eq!(stats.to_string(), before);
        assert_eq!(stats.orders_added(), 2);
        assert_eq!(stats.orders_removed(), 1);
        assert_eq!(stats.orders_executed(), 1);
        assert_eq!(stats.quantity_executed(), 10);
        assert_eq!(stats.value_executed(), 1000);
        assert_eq!(stats.last_execution_time(), NOW);
        assert_eq!(stats.first_arrival_time(), NOW - 10_000);
        assert_eq!(stats.sum_waiting_time(), 5_000);
        assert!(stats.stats_degraded());

        // The write section closed cleanly: a subsequent reset succeeds.
        assert!(stats.reset(&FixedClock(NOW + 1)).is_ok());
        assert_eq!(stats.first_arrival_time(), NOW + 1);
        assert!(!stats.stats_degraded());
    }

    #[test]
    fn test_reset_at_is_clock_free() {
        let stats = PriceLevelStatistics::new();
        stats.record_order_added();
        stats.reset_at(TimestampMs::new(42));
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.first_arrival_time(), 42);
    }

    #[test]
    fn test_reset() {
        let stats = PriceLevelStatistics::new();

        // Add some data
        stats.record_order_added();
        stats.record_order_removed();
        assert!(
            stats
                .record_execution(10, 100, 0, 1_716_000_000_000)
                .is_ok()
        );

        // Verify data was recorded
        assert_eq!(stats.orders_added(), 1);
        assert_eq!(stats.orders_removed(), 1);
        assert_eq!(stats.orders_executed(), 1);

        // Reset stats
        stats.reset(&FixedClock(NOW)).unwrap();

        // Verify reset worked
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.last_execution_time(), 0);
        assert_eq!(stats.first_arrival_time(), NOW);
        assert_eq!(stats.sum_waiting_time(), 0);
    }

    #[test]
    fn test_display() {
        let stats = PriceLevelStatistics::new();

        // Add some data
        stats.record_order_added();
        stats.record_order_removed();
        assert!(
            stats
                .record_execution(10, 100, 0, 1_716_000_000_000)
                .is_ok()
        );

        // Get display string
        let display_str = stats.to_string();

        // Verify format
        assert!(display_str.starts_with("PriceLevelStatistics:"));
        assert!(display_str.contains("orders_added=1"));
        assert!(display_str.contains("orders_removed=1"));
        assert!(display_str.contains("orders_executed=1"));
        assert!(display_str.contains("quantity_executed=10"));
        assert!(display_str.contains("value_executed=1000"));
    }

    #[test]
    fn test_from_str() {
        // Create sample string representation
        let input = "PriceLevelStatistics:orders_added=5;orders_removed=3;orders_executed=2;quantity_executed=15;value_executed=2000;last_execution_time=1616823000000;first_arrival_time=1616823000001;sum_waiting_time=1000";

        // Parse from string
        let stats = PriceLevelStatistics::from_str(input).unwrap();

        // Verify values
        assert_eq!(stats.orders_added(), 5);
        assert_eq!(stats.orders_removed(), 3);
        assert_eq!(stats.orders_executed(), 2);
        assert_eq!(stats.quantity_executed(), 15);
        assert_eq!(stats.value_executed(), 2000);
        assert_eq!(stats.last_execution_time(), 1616823000000);
        assert_eq!(stats.first_arrival_time(), 1616823000001);
        assert_eq!(stats.sum_waiting_time(), 1000);
    }

    #[test]
    fn test_from_str_invalid_format() {
        let input = "InvalidFormat";
        assert!(PriceLevelStatistics::from_str(input).is_err());
    }

    #[test]
    fn test_from_str_missing_field() {
        // Missing sum_waiting_time
        let input = "PriceLevelStatistics:orders_added=5;orders_removed=3;orders_executed=2;quantity_executed=15;value_executed=2000;last_execution_time=1616823000000;first_arrival_time=1616823000001";
        assert!(PriceLevelStatistics::from_str(input).is_err());
    }

    #[test]
    fn test_from_str_invalid_field_value() {
        // Invalid orders_added (not a number)
        let input = "PriceLevelStatistics:orders_added=invalid;orders_removed=3;orders_executed=2;quantity_executed=15;value_executed=2000;last_execution_time=1616823000000;first_arrival_time=1616823000001;sum_waiting_time=1000";
        assert!(PriceLevelStatistics::from_str(input).is_err());
    }

    #[test]
    fn test_serialize_deserialize_json() {
        let stats = PriceLevelStatistics::new();

        // Add some data
        stats.record_order_added();
        stats.record_order_removed();
        assert!(
            stats
                .record_execution(10, 100, 0, 1_716_000_000_000)
                .is_ok()
        );

        // Serialize to JSON
        let json = serde_json::to_string(&stats).unwrap();

        // Verify JSON format
        assert!(json.contains("\"orders_added\":1"));
        assert!(json.contains("\"orders_removed\":1"));
        assert!(json.contains("\"orders_executed\":1"));
        assert!(json.contains("\"quantity_executed\":10"));
        assert!(json.contains("\"value_executed\":1000"));

        // Deserialize from JSON
        let deserialized: PriceLevelStatistics = serde_json::from_str(&json).unwrap();

        // Verify values
        assert_eq!(deserialized.orders_added(), 1);
        assert_eq!(deserialized.orders_removed(), 1);
        assert_eq!(deserialized.orders_executed(), 1);
        assert_eq!(deserialized.quantity_executed(), 10);
        assert_eq!(deserialized.value_executed(), 1000);
    }

    #[test]
    fn test_round_trip_display_parse() {
        // Build a fully-populated statistics value through the public `FromStr`
        // path (the counters are private and have no public mutator), with
        // predictable, precise values in every field to avoid timing issues.
        let current_time: u64 = 1616823000000;
        let input = format!(
            "PriceLevelStatistics:orders_added=2;orders_removed=1;orders_executed=2;quantity_executed=15;value_executed=2000;last_execution_time={};first_arrival_time={};sum_waiting_time=1000",
            current_time,
            current_time + 1
        );
        let stats = PriceLevelStatistics::from_str(&input).unwrap();

        // Convert to string
        let string_representation = stats.to_string();

        // Parse back
        let parsed = PriceLevelStatistics::from_str(&string_representation).unwrap();

        // Verify values match
        assert_eq!(parsed.orders_added(), stats.orders_added());
        assert_eq!(parsed.orders_removed(), stats.orders_removed());
        assert_eq!(parsed.orders_executed(), stats.orders_executed());
        assert_eq!(parsed.quantity_executed(), stats.quantity_executed());
        assert_eq!(parsed.value_executed(), stats.value_executed());
        assert_eq!(parsed.last_execution_time(), stats.last_execution_time());
        assert_eq!(parsed.first_arrival_time(), stats.first_arrival_time());
        assert_eq!(parsed.sum_waiting_time(), stats.sum_waiting_time());
    }

    #[test]
    fn test_thread_safety() {
        let stats = PriceLevelStatistics::new();
        let stats_arc = Arc::new(stats);

        let mut handles = vec![];

        // Spawn 10 threads to concurrently update stats
        for _ in 0..10 {
            let stats_clone = Arc::clone(&stats_arc);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    stats_clone.record_order_added();
                    stats_clone.record_order_removed();
                    if let Err(error) = stats_clone.record_execution(1, 100, 0, 1_716_000_000_000) {
                        panic!("record_execution failed in thread: {error}");
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify final counts
        assert_eq!(stats_arc.orders_added(), 1000); // 10 threads * 100 calls
        assert_eq!(stats_arc.orders_removed(), 1000);
        assert_eq!(stats_arc.orders_executed(), 1000);
        assert_eq!(stats_arc.quantity_executed(), 1000);
        assert_eq!(stats_arc.value_executed(), 100000); // 1000 * 100
    }

    #[test]
    fn test_statistics_reset_and_verify() {
        let stats = PriceLevelStatistics::new();

        // Add some data
        stats.record_order_added();
        stats.record_order_added();
        stats.record_order_removed();
        assert!(
            stats
                .record_execution(10, 100, 0, 1_716_000_000_000)
                .is_ok()
        );

        // Verify stats were recorded
        assert_eq!(stats.orders_added(), 2);
        assert_eq!(stats.orders_removed(), 1);
        assert_eq!(stats.orders_executed(), 1);

        // Reset stats
        stats.reset(&FixedClock(NOW)).unwrap();

        // Verify all statistics are reset
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.last_execution_time(), 0);
        assert_eq!(stats.first_arrival_time(), NOW);
        assert_eq!(stats.sum_waiting_time(), 0);
    }

    #[test]
    fn test_statistics_serialize_deserialize_fields() {
        // Populate every field with a distinct value through the public
        // `FromStr` path (the counters are private and have no public mutator).
        let input = "PriceLevelStatistics:orders_added=1;orders_removed=2;orders_executed=3;quantity_executed=4;value_executed=5;last_execution_time=6;first_arrival_time=7;sum_waiting_time=8";
        let stats = PriceLevelStatistics::from_str(input).unwrap();

        // Serialize to JSON
        let serialized = serde_json::to_string(&stats).unwrap();

        // Should contain all the field values
        assert!(serialized.contains("\"orders_added\":1"));
        assert!(serialized.contains("\"orders_removed\":2"));
        assert!(serialized.contains("\"orders_executed\":3"));
        assert!(serialized.contains("\"quantity_executed\":4"));
        assert!(serialized.contains("\"value_executed\":5"));
        assert!(serialized.contains("\"last_execution_time\":6"));
        assert!(serialized.contains("\"first_arrival_time\":7"));
        assert!(serialized.contains("\"sum_waiting_time\":8"));

        // Deserialize back
        let deserialized: PriceLevelStatistics = serde_json::from_str(&serialized).unwrap();

        // Verify all fields are deserialized correctly
        assert_eq!(deserialized.orders_added(), 1);
        assert_eq!(deserialized.orders_removed(), 2);
        assert_eq!(deserialized.orders_executed(), 3);
        assert_eq!(deserialized.quantity_executed(), 4);
        assert_eq!(deserialized.value_executed(), 5);
        assert_eq!(deserialized.last_execution_time(), 6);
        assert_eq!(deserialized.first_arrival_time(), 7);
        assert_eq!(deserialized.sum_waiting_time(), 8);
    }

    #[test]
    fn test_statistics_visitor_missing_fields() {
        // Test with a partial JSON
        let json = r#"{
        "orders_added": 1,
        "orders_removed": 2,
        "orders_executed": 3
    }"#;

        // Should still deserialize correctly with default values for missing fields
        let deserialized: PriceLevelStatistics = serde_json::from_str(json).unwrap();

        assert_eq!(deserialized.orders_added(), 1);
        assert_eq!(deserialized.orders_removed(), 2);
        assert_eq!(deserialized.orders_executed(), 3);
        assert_eq!(deserialized.quantity_executed(), 0);
        assert_eq!(deserialized.value_executed(), 0);
        // Missing stats_degraded defaults to false.
        assert!(!deserialized.stats_degraded());
        // Missing first_arrival_time decodes as unstamped, deterministically:
        // no clock is read during deserialization (issue #171).
        assert_eq!(deserialized.first_arrival_time(), 0);
        let again: PriceLevelStatistics = serde_json::from_str(json).unwrap();
        assert_eq!(again.to_string(), deserialized.to_string());
    }

    // ------------------------------------------------------------------
    // Issue #117 — all-or-nothing statistics + degraded flag
    // ------------------------------------------------------------------

    /// Build statistics pre-loaded with specific aggregate values (no public
    /// counter setter exists, so seed via the `FromStr` surface).
    fn seed_stats(
        orders_executed: usize,
        quantity_executed: u64,
        value_executed: u128,
        sum_waiting_time: u64,
    ) -> PriceLevelStatistics {
        let text = format!(
            "PriceLevelStatistics:orders_added=0;orders_removed=0;orders_executed={orders_executed};\
             quantity_executed={quantity_executed};value_executed={value_executed};\
             last_execution_time=0;first_arrival_time=0;sum_waiting_time={sum_waiting_time};\
             stats_degraded=false"
        );
        PriceLevelStatistics::from_str(&text).expect("seed stats must parse")
    }

    #[test]
    fn test_record_execution_all_or_nothing_on_each_overflow() {
        // quantity_executed overflow: nothing advances, degraded set.
        {
            let stats = seed_stats(0, u64::MAX - 5, 0, 0);
            assert!(stats.record_execution(10, 1, 0, 1_000).is_err());
            assert_eq!(stats.orders_executed(), 0, "orders_executed rolled back");
            assert_eq!(
                stats.quantity_executed(),
                u64::MAX - 5,
                "quantity unchanged"
            );
            assert_eq!(stats.value_executed(), 0, "value unchanged");
            assert!(stats.stats_degraded(), "degraded flag set");
        }
        // value_executed overflow: orders + quantity rolled back.
        {
            let stats = seed_stats(0, 0, u128::MAX - 5, 0);
            assert!(stats.record_execution(1, 10, 0, 1_000).is_err());
            assert_eq!(stats.orders_executed(), 0);
            assert_eq!(stats.quantity_executed(), 0, "quantity rolled back");
            assert_eq!(stats.value_executed(), u128::MAX - 5, "value unchanged");
            assert!(stats.stats_degraded());
        }
        // sum_waiting_time overflow: orders + quantity + value rolled back.
        {
            let stats = seed_stats(0, 0, 0, u64::MAX - 5);
            assert!(stats.record_execution(1, 1, 1, 100).is_err());
            assert_eq!(stats.orders_executed(), 0);
            assert_eq!(stats.quantity_executed(), 0, "quantity rolled back");
            assert_eq!(stats.value_executed(), 0, "value rolled back");
            assert_eq!(stats.sum_waiting_time(), u64::MAX - 5, "sum unchanged");
            assert!(stats.stats_degraded());
        }
        // value multiplication overflows u128 (validation, before mutation).
        {
            let stats = seed_stats(0, 0, 0, 0);
            assert!(
                stats
                    .record_execution(u64::MAX, u128::MAX, 0, 1_000)
                    .is_err()
            );
            assert_eq!(stats.orders_executed(), 0);
            assert_eq!(stats.quantity_executed(), 0);
            assert_eq!(stats.value_executed(), 0);
            assert!(stats.stats_degraded());
        }
        // Maker timestamp in the future of execution (validation).
        {
            let stats = seed_stats(0, 0, 0, 0);
            assert!(stats.record_execution(1, 1, 200, 100).is_err());
            assert_eq!(stats.orders_executed(), 0);
            assert!(stats.stats_degraded());
        }
    }

    #[test]
    fn test_record_execution_success_leaves_flag_clear() {
        let stats = PriceLevelStatistics::new();
        assert!(stats.record_execution(10, 5, 0, 1_000).is_ok());
        assert_eq!(stats.orders_executed(), 1);
        assert_eq!(stats.quantity_executed(), 10);
        assert_eq!(stats.value_executed(), 50);
        assert!(
            !stats.stats_degraded(),
            "a successful record must not degrade"
        );
    }

    #[test]
    fn test_concurrent_record_execution_cross_aggregate_consistency() {
        use std::sync::Barrier;

        // Seed quantity_executed AND value_executed with exactly K*Q of headroom
        // below u64::MAX (SYMMETRIC headroom, price = 1, so each record adds Q to
        // both). With N > K concurrent records, exactly K fit and the rest fail
        // at the FIRST additive counter (quantity_executed) — so in this
        // symmetric config a loser never advances any counter and no
        // cross-counter rollback is exercised; it is a clean reject. (Under
        // ASYMMETRIC headroom a loser could pass quantity and fail at value, and
        // the exact-fit count would be `<= K` rather than `== K`; the rollback
        // path is covered by the deterministic per-aggregate test above.)
        // Invariant here: orders_executed == K, quantity_executed ==
        // value_executed (each success advances both in lockstep), the surplus
        // over the seed equals orders_executed * Q, and the degraded flag is set.
        const N: usize = 8;
        const K: u64 = 3;
        const Q: u64 = 1_000;
        let seed = u64::MAX - K * Q;

        for _ in 0..50 {
            let stats = Arc::new(seed_stats(0, seed, u128::from(seed), 0));
            // Barrier so all N records race from the same instant.
            let barrier = Arc::new(Barrier::new(N));
            let mut handles = Vec::with_capacity(N);
            for _ in 0..N {
                let stats = Arc::clone(&stats);
                let barrier = Arc::clone(&barrier);
                handles.push(thread::spawn(move || {
                    barrier.wait();
                    stats.record_execution(Q, 1, 0, 1_000).is_ok()
                }));
            }
            let successes: u64 = handles
                .into_iter()
                .map(|h| u64::from(h.join().expect("thread panicked")))
                .sum();

            assert_eq!(successes, K, "exactly K records must fit the headroom");
            assert_eq!(stats.orders_executed() as u64, K);
            assert_eq!(
                u128::from(stats.quantity_executed()),
                stats.value_executed(),
                "quantity and value advance in lockstep (each success adds to both)"
            );
            assert_eq!(
                stats.quantity_executed() - seed,
                K * Q,
                "the surplus over the seed equals orders_executed * Q"
            );
            assert!(
                stats.stats_degraded(),
                "some records overflowed -> degraded"
            );
        }
    }

    #[test]
    fn test_serialize_omits_degraded_flag_when_false() {
        // A non-degraded statistics serializes in the pre-#117 8-field form
        // (the flag is skipped when false), so a v2 snapshot payload persisted
        // before the flag existed re-serializes byte-identically and its SHA-256
        // checksum still validates.
        let stats = PriceLevelStatistics::new();
        assert!(!stats.stats_degraded());
        let json = serde_json::to_string(&stats).expect("serialize");
        assert!(
            !json.contains("stats_degraded"),
            "a non-degraded statistics must omit the flag (old-v2 checksum compat): {json}"
        );
        // It still round-trips: a missing flag decodes to false.
        let back: PriceLevelStatistics = serde_json::from_str(&json).expect("deserialize");
        assert!(!back.stats_degraded());

        // A degraded statistics DOES emit the field, and it round-trips.
        let degraded = seed_stats(0, 0, 0, 0);
        assert!(
            degraded
                .record_execution(u64::MAX, u128::MAX, 0, 1_000)
                .is_err()
        ); // value overflow
        assert!(degraded.stats_degraded());
        let degraded_json = serde_json::to_string(&degraded).expect("serialize degraded");
        assert!(
            degraded_json.contains("\"stats_degraded\":true"),
            "a degraded statistics must emit the flag: {degraded_json}"
        );
        let degraded_back: PriceLevelStatistics =
            serde_json::from_str(&degraded_json).expect("deserialize degraded");
        assert!(degraded_back.stats_degraded(), "the flag must round-trip");
    }

    #[test]
    fn test_orders_executed_overflow_seeded_via_fromstr_is_all_or_nothing() {
        // Issue #129: `orders_executed` can be seeded to `usize::MAX` through
        // FromStr / serde. A later record's checked `+1` must reject the overflow
        // all-or-nothing — no counter wraps, and no OTHER aggregate advances.
        let seeded = format!(
            "PriceLevelStatistics:orders_added=0;orders_removed=0;orders_executed={};quantity_executed=0;value_executed=0;last_execution_time=0;first_arrival_time=0;sum_waiting_time=0;stats_degraded=false",
            usize::MAX
        );
        let stats = PriceLevelStatistics::from_str(&seeded).expect("parse MAX orders_executed");
        assert_eq!(stats.orders_executed(), usize::MAX);

        assert!(
            stats.record_execution(5, 100, 0, 1_000).is_err(),
            "the checked orders_executed overflow must be rejected"
        );
        // No wrap, and nothing else moved (all-or-nothing).
        assert_eq!(stats.orders_executed(), usize::MAX, "no wrap to 0");
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.sum_waiting_time(), 0);
        assert!(
            stats.stats_degraded(),
            "the dropped execution is observable"
        );
    }

    #[test]
    fn test_mark_degraded_transitions_once() {
        // Issue #129: only the FIRST drop transitions the sticky flag
        // (compare_exchange), so `PriceLevel::match_order` logs one WARN per
        // degraded episode rather than one per dropped execution.
        let stats = PriceLevelStatistics::new();
        assert!(!stats.stats_degraded());
        assert!(
            stats.mark_degraded(),
            "first drop transitions false -> true"
        );
        assert!(stats.stats_degraded());
        assert!(!stats.mark_degraded(), "second drop is silent");
        assert!(!stats.mark_degraded(), "still silent");
        // A reset re-arms the transition.
        stats.reset_at(TimestampMs::new(NOW));
        assert!(!stats.stats_degraded());
        assert!(stats.mark_degraded(), "post-reset drop transitions again");
    }

    #[test]
    fn test_last_execution_time_is_monotonic() {
        // Issue #129: `fetch_max` — recording an OLDER execution after a newer
        // one never moves `last_execution_time` backwards.
        let stats = PriceLevelStatistics::new();
        stats.record_execution(1, 100, 0, 5_000).expect("newer");
        assert_eq!(stats.last_execution_time(), 5_000);
        stats.record_execution(1, 100, 0, 2_000).expect("older");
        assert_eq!(
            stats.last_execution_time(),
            5_000,
            "an older record must not move the last-execution time back"
        );
        stats.record_execution(1, 100, 0, 9_000).expect("newest");
        assert_eq!(stats.last_execution_time(), 9_000);
    }

    #[test]
    fn test_last_execution_time_max_wins_under_barrier() {
        // Issue #129: two records with distinct timestamps racing — the max wins
        // regardless of order (`fetch_max` is atomic, independent of the seqlock).
        use std::sync::{Arc as StdArc, Barrier};

        const HI: u64 = 9_000;
        const LO: u64 = 3_000;
        for _ in 0..500 {
            let stats = StdArc::new(PriceLevelStatistics::new());
            let barrier = StdArc::new(Barrier::new(2));
            let hi = {
                let stats = StdArc::clone(&stats);
                let barrier = StdArc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let _ = stats.record_execution(1, 100, 0, HI);
                })
            };
            let lo = {
                let stats = StdArc::clone(&stats);
                let barrier = StdArc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let _ = stats.record_execution(1, 100, 0, LO);
                })
            };
            hi.join().expect("hi");
            lo.join().expect("lo");
            assert_eq!(
                stats.last_execution_time(),
                HI,
                "the max timestamp must win"
            );
        }
    }

    #[test]
    fn test_clone_is_consistent_under_concurrent_record() {
        // Issue #129 (F5): a `Clone` (which backs the checksummed snapshot) must
        // capture a COHERENT set under a concurrent single writer. Each record
        // adds qty=1 / value=100 / orders+1 inside the seqlock, so a consistent
        // copy always has `value == 100 * quantity` AND `orders == quantity`. A
        // pre-#129 torn clone could mix `orders=k` with `quantity=k-1`.
        use std::sync::{Arc as StdArc, Barrier};

        const N: u64 = 3_000;
        let stats = StdArc::new(PriceLevelStatistics::new());
        let barrier = StdArc::new(Barrier::new(2));

        let writer = {
            let stats = StdArc::clone(&stats);
            let barrier = StdArc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..N {
                    stats.record_execution(1, 100, 0, 1_000).expect("record");
                }
            })
        };
        let reader = {
            let stats = StdArc::clone(&stats);
            let barrier = StdArc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..50_000 {
                    let copy = (*stats).clone();
                    let q = copy.quantity_executed();
                    assert_eq!(
                        copy.value_executed(),
                        u128::from(q) * 100,
                        "clone must not tear value vs quantity"
                    );
                    assert_eq!(
                        copy.orders_executed() as u64,
                        q,
                        "clone must not tear orders vs quantity"
                    );
                }
            })
        };

        writer.join().expect("writer");
        reader.join().expect("reader");

        assert_eq!(stats.orders_executed() as u64, N);
        assert_eq!(stats.quantity_executed(), N);
        assert_eq!(stats.value_executed(), u128::from(N) * 100);
    }

    // ------------------------------------------------------------------
    // Issue #140 — `value_executed` is a `u128` accumulator
    // ------------------------------------------------------------------

    /// Fixed-point scale applied to BOTH price and quantity in the issue's
    /// reproduction, so every `quantity * price` product carries `SCALE^2`.
    const SCALE: u64 = 100_000_000;

    #[test]
    fn test_record_execution_single_value_above_u64_max_is_recorded() {
        // Issue #140 case A: 1.0 @ 49_995.0 with both operands scaled by 1e8.
        // The product (~5e20) exceeds `u64::MAX` but not `u128`.
        let stats = PriceLevelStatistics::new();
        let price = 49_995 * u128::from(SCALE);
        let expected = u128::from(SCALE) * price;
        assert!(expected > u128::from(u64::MAX));

        assert!(stats.record_execution(SCALE, price, 0, 1_000).is_ok());
        assert_eq!(stats.value_executed(), expected);
        assert_eq!(stats.orders_executed(), 1);
        assert_eq!(stats.quantity_executed(), SCALE);
        assert!(!stats.stats_degraded());
    }

    #[test]
    fn test_record_execution_running_total_crosses_u64_max_is_recorded() {
        // Issue #140 case B: each 1.0 @ 1.0 execution fits in `u64`, but the
        // running total crossed `u64::MAX` after 1845 of them before the fix.
        const ROUNDS: u64 = 3_000;
        let stats = PriceLevelStatistics::new();
        let price = u128::from(SCALE);
        for _ in 0..ROUNDS {
            assert!(stats.record_execution(SCALE, price, 0, 1_000).is_ok());
        }
        let expected = u128::from(ROUNDS) * u128::from(SCALE) * price;
        assert!(expected > u128::from(u64::MAX));
        assert_eq!(stats.value_executed(), expected);
        assert_eq!(stats.orders_executed() as u64, ROUNDS);
        assert!(!stats.stats_degraded());
        assert_eq!(stats.average_execution_price(), Some(SCALE as f64));
    }

    #[test]
    fn test_record_execution_value_multiplication_overflow_is_all_or_nothing() {
        // `quantity * price` itself overflows `u128`: rejected before any
        // counter moves, and the drop is observable.
        let stats = seed_stats(4, 40, 1_000, 7);
        let err = stats.record_execution(2, u128::MAX, 0, 1_000);
        assert!(matches!(
            err,
            Err(crate::errors::PriceLevelError::InvalidOperation { .. })
        ));
        assert_eq!(stats.orders_executed(), 4);
        assert_eq!(stats.quantity_executed(), 40);
        assert_eq!(stats.value_executed(), 1_000);
        assert_eq!(stats.sum_waiting_time(), 7);
        assert_eq!(stats.last_execution_time(), 0);
        assert!(stats.stats_degraded());
    }

    #[test]
    fn test_record_execution_value_accumulator_overflow_is_all_or_nothing() {
        // The product fits in `u128`, but adding it to the running total does
        // not: orders + quantity (already committed) are rolled back.
        let seed = u128::MAX - 5;
        let stats = seed_stats(4, 40, seed, 7);
        let err = stats.record_execution(1, 6, 0, 1_000);
        assert!(matches!(
            err,
            Err(crate::errors::PriceLevelError::InvalidOperation { .. })
        ));
        assert_eq!(stats.orders_executed(), 4, "orders rolled back");
        assert_eq!(stats.quantity_executed(), 40, "quantity rolled back");
        assert_eq!(stats.value_executed(), seed, "value unchanged");
        assert_eq!(stats.sum_waiting_time(), 7);
        assert_eq!(stats.last_execution_time(), 0);
        assert!(stats.stats_degraded());

        // Exactly filling the accumulator is not an overflow.
        let stats = seed_stats(0, 0, seed, 0);
        assert!(stats.record_execution(1, 5, 0, 1_000).is_ok());
        assert_eq!(stats.value_executed(), u128::MAX);
        assert!(!stats.stats_degraded());
    }

    #[test]
    fn test_record_execution_rolls_back_value_above_u64_max_on_waiting_time_overflow() {
        // `value_executed` successfully takes a contribution above `u64::MAX`,
        // then the later `sum_waiting_time` add overflows: the whole
        // contribution, including the wide value, must be subtracted back.
        let value_seed = u128::from(u64::MAX) + 17;
        let stats = seed_stats(2, 20, value_seed, u64::MAX - 5);
        let price = 49_995 * u128::from(SCALE);
        assert!(u128::from(SCALE) * price > u128::from(u64::MAX));

        // Maker at t=1, execution at t=1_000: waiting time 999 overflows.
        let err = stats.record_execution(SCALE, price, 1, 1_000);
        assert!(matches!(
            err,
            Err(crate::errors::PriceLevelError::InvalidOperation { .. })
        ));
        assert_eq!(stats.orders_executed(), 2, "orders rolled back");
        assert_eq!(stats.quantity_executed(), 20, "quantity rolled back");
        assert_eq!(stats.value_executed(), value_seed, "wide value rolled back");
        assert_eq!(stats.sum_waiting_time(), u64::MAX - 5, "sum unchanged");
        assert_eq!(
            stats.last_execution_time(),
            0,
            "last execution not advanced"
        );
        assert!(stats.stats_degraded());
    }

    #[test]
    fn test_value_executed_above_u64_max_round_trips_display_fromstr() {
        let value = u128::from(u64::MAX) * 3 + 11;
        let stats = seed_stats(1, 2, value, 3);
        let text = stats.to_string();
        assert!(text.contains(&format!("value_executed={value}")), "{text}");
        let parsed = PriceLevelStatistics::from_str(&text).expect("parse wide value");
        assert_eq!(parsed.value_executed(), value);
        assert_eq!(parsed.to_string(), text);
    }

    #[test]
    fn test_value_executed_above_u64_max_round_trips_serde_json() {
        let value = u128::from(u64::MAX) * 3 + 11;
        let stats = seed_stats(1, 2, value, 3);
        let json = serde_json::to_string(&stats).expect("serialize");
        assert!(
            json.contains(&format!("\"value_executed\":{value}")),
            "the wide value is a plain JSON integer: {json}"
        );
        let back: PriceLevelStatistics = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.value_executed(), value);
        assert_eq!(serde_json::to_string(&back).expect("reserialize"), json);
    }

    #[test]
    fn test_value_executed_legacy_u64_encodings_still_parse() {
        // Text and JSON written before issue #140 carry a `u64` value; both
        // decode unchanged into the `u128` accumulator.
        let text = format!(
            "PriceLevelStatistics:orders_added=0;orders_removed=0;orders_executed=1;\
             quantity_executed=1;value_executed={};last_execution_time=0;\
             first_arrival_time=0;sum_waiting_time=0",
            u64::MAX
        );
        let parsed = PriceLevelStatistics::from_str(&text).expect("legacy text");
        assert_eq!(parsed.value_executed(), u128::from(u64::MAX));

        let json = format!(
            "{{\"orders_added\":0,\"orders_removed\":0,\"orders_executed\":1,\
             \"quantity_executed\":1,\"value_executed\":{},\"last_execution_time\":0,\
             \"first_arrival_time\":0,\"sum_waiting_time\":0}}",
            u64::MAX
        );
        let back: PriceLevelStatistics = serde_json::from_str(&json).expect("legacy json");
        assert_eq!(back.value_executed(), u128::from(u64::MAX));
        assert_eq!(
            serde_json::to_string(&back).expect("reserialize"),
            json,
            "a legacy payload re-serializes byte-identically (checksum compat)"
        );
    }

    #[test]
    fn test_value_executed_rejects_out_of_range_text() {
        let text = format!(
            "PriceLevelStatistics:orders_added=0;orders_removed=0;orders_executed=0;\
             quantity_executed=0;value_executed={}0;last_execution_time=0;\
             first_arrival_time=0;sum_waiting_time=0",
            u128::MAX
        );
        assert!(matches!(
            PriceLevelStatistics::from_str(&text),
            Err(crate::errors::PriceLevelError::InvalidFieldValue { .. })
        ));
    }

    #[test]
    fn test_concurrent_record_execution_crosses_u64_max_exactly() {
        // Concurrent writers push `value_executed` across `u64::MAX`: every
        // contribution lands exactly once (no lost CAS update, no truncation at
        // the 64-bit boundary) and nothing degrades.
        use std::sync::Barrier;

        const THREADS: usize = 8;
        const PER_THREAD: u64 = 2_000;
        let price = u128::from(SCALE);
        let per_record = u128::from(SCALE) * price;
        let seed = u128::from(u64::MAX) - 5 * per_record;

        let stats = Arc::new(seed_stats(0, 0, seed, 0));
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let stats = Arc::clone(&stats);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..PER_THREAD {
                        stats
                            .record_execution(SCALE, price, 0, 1_000)
                            .expect("record");
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread panicked");
        }

        let records = THREADS as u64 * PER_THREAD;
        let expected = seed + u128::from(records) * per_record;
        assert!(expected > u128::from(u64::MAX));
        assert_eq!(stats.value_executed(), expected);
        assert_eq!(stats.orders_executed() as u64, records);
        assert_eq!(stats.quantity_executed(), records * SCALE);
        assert!(!stats.stats_degraded());
    }
}
