#[cfg(test)]
mod tests {
    use crate::price_level::PriceLevelStatistics;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_new() {
        let stats = PriceLevelStatistics::new();
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.last_execution_time(), 0);
        assert!(stats.first_arrival_time() > 0);
        assert_eq!(stats.sum_waiting_time(), 0);
    }

    #[test]
    fn test_record_execution_error_paths() {
        let stats = PriceLevelStatistics::new();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

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

        // Test with no executions
        assert_eq!(stats.time_since_last_execution(), None);

        // Record an execution with an explicit execution time in the past so
        // the wall-clock-based `time_since_last_execution` reports a positive
        // delta deterministically (no sleep needed).
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 1000;
        assert!(stats.record_execution(10, 100, 0, past).is_ok());

        // Should return some non-zero value
        let time_since = stats.time_since_last_execution().unwrap();
        assert!(time_since > 0);
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
        stats.reset();

        // Verify reset worked
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.last_execution_time(), 0);
        assert!(stats.first_arrival_time() > 0);
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
        stats.reset();

        // Verify all statistics are reset
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.last_execution_time(), 0);
        assert!(stats.first_arrival_time() > 0);
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
    }

    // ------------------------------------------------------------------
    // Issue #117 — all-or-nothing statistics + degraded flag
    // ------------------------------------------------------------------

    /// Build statistics pre-loaded with specific aggregate values (no public
    /// counter setter exists, so seed via the `FromStr` surface).
    fn seed_stats(
        orders_executed: usize,
        quantity_executed: u64,
        value_executed: u64,
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
            let stats = seed_stats(0, 0, u64::MAX - 5, 0);
            assert!(stats.record_execution(1, 10, 0, 1_000).is_err());
            assert_eq!(stats.orders_executed(), 0);
            assert_eq!(stats.quantity_executed(), 0, "quantity rolled back");
            assert_eq!(stats.value_executed(), u64::MAX - 5, "value unchanged");
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
        // value multiplication exceeds u64 storage (validation, before mutation).
        {
            let stats = seed_stats(0, 0, 0, 0);
            assert!(stats.record_execution(u64::MAX, 2, 0, 1_000).is_err());
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
            let stats = Arc::new(seed_stats(0, seed, seed, 0));
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
                stats.quantity_executed(),
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
        assert!(degraded.record_execution(u64::MAX, 2, 0, 1_000).is_err()); // value overflow
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
}
