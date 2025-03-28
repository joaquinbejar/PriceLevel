#[cfg(test)]
mod tests {
    use crate::price_level::PriceLevelStatistics;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn test_new() {
        let stats = PriceLevelStatistics::new();
        assert_eq!(stats.orders_added(), 0);
        assert_eq!(stats.orders_removed(), 0);
        assert_eq!(stats.orders_executed(), 0);
        assert_eq!(stats.quantity_executed(), 0);
        assert_eq!(stats.value_executed(), 0);
        assert_eq!(stats.last_execution_time.load(Ordering::Relaxed), 0);
        assert!(stats.first_arrival_time.load(Ordering::Relaxed) > 0);
        assert_eq!(stats.sum_waiting_time.load(Ordering::Relaxed), 0);
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

        // Test recording executed orders
        stats.record_execution(10, 100, 0); // qty=10, price=100, no timestamp
        assert_eq!(stats.orders_executed(), 1);
        assert_eq!(stats.quantity_executed(), 10);
        assert_eq!(stats.value_executed(), 1000); // 10 * 100
        assert!(stats.last_execution_time.load(Ordering::Relaxed) > 0);

        // Test with timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 1000; // 1 second ago

        // Sleep to ensure waiting time is measurable
        thread::sleep(Duration::from_millis(10));

        stats.record_execution(5, 200, timestamp);
        assert_eq!(stats.orders_executed(), 2);
        assert_eq!(stats.quantity_executed(), 15); // 10 + 5
        assert_eq!(stats.value_executed(), 2000); // 1000 + (5 * 200)
        assert!(stats.sum_waiting_time.load(Ordering::Relaxed) >= 1000); // At least 1 second waiting time
    }

    #[test]
    fn test_average_execution_price() {
        let stats = PriceLevelStatistics::new();

        // Test with no executions
        assert_eq!(stats.average_execution_price(), None);

        // Test with executions
        stats.record_execution(10, 100, 0); // Total value: 1000
        stats.record_execution(20, 150, 0); // Total value: 3000 + 1000 = 4000

        // Average price should be 4000 / 30 = 133.33...
        let avg_price = stats.average_execution_price().unwrap();
        assert!((avg_price - 133.33).abs() < 0.01);
    }

    #[test]
    fn test_average_waiting_time() {
        let stats = PriceLevelStatistics::new();

        // Test with no executions
        assert_eq!(stats.average_waiting_time(), None);

        // Test with executions
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        stats.record_execution(10, 100, now - 1000); // 1 second ago
        stats.record_execution(20, 150, now - 3000); // 3 seconds ago

        // Total waiting time: 1000 + 3000 = 4000ms, average = 2000ms
        let avg_wait = stats.average_waiting_time().unwrap();
        assert!((1900.0..=2100.0).contains(&avg_wait));
    }

    #[test]
    fn test_time_since_last_execution() {
        let stats = PriceLevelStatistics::new();

        // Test with no executions
        assert_eq!(stats.time_since_last_execution(), None);

        // Record an execution
        stats.record_execution(10, 100, 0);

        // Sleep a bit to ensure time passes
        thread::sleep(Duration::from_millis(10));

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
        stats.record_execution(10, 100, 0);

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
        assert_eq!(stats.last_execution_time.load(Ordering::Relaxed), 0);
        assert!(stats.first_arrival_time.load(Ordering::Relaxed) > 0);
        assert_eq!(stats.sum_waiting_time.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_display() {
        let stats = PriceLevelStatistics::new();

        // Add some data
        stats.record_order_added();
        stats.record_order_removed();
        stats.record_execution(10, 100, 0);

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
        assert_eq!(
            stats.last_execution_time.load(Ordering::Relaxed),
            1616823000000
        );
        assert_eq!(
            stats.first_arrival_time.load(Ordering::Relaxed),
            1616823000001
        );
        assert_eq!(stats.sum_waiting_time.load(Ordering::Relaxed), 1000);
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
        stats.record_execution(10, 100, 0);

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
        let stats = PriceLevelStatistics::new();

        // Use precise timestamps to avoid timing issues
        let current_time: u64 = 1616823000000;
        stats
            .last_execution_time
            .store(current_time, Ordering::Relaxed);
        stats
            .first_arrival_time
            .store(current_time + 1, Ordering::Relaxed);

        // Add some data
        stats.record_order_added();
        stats.record_order_added();
        stats.record_order_removed();

        // Manual record to have predictable values
        stats.orders_executed.store(2, Ordering::Relaxed);
        stats.quantity_executed.store(15, Ordering::Relaxed);
        stats.value_executed.store(2000, Ordering::Relaxed);
        stats.sum_waiting_time.store(1000, Ordering::Relaxed);

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
        assert_eq!(
            parsed.last_execution_time.load(Ordering::Relaxed),
            stats.last_execution_time.load(Ordering::Relaxed)
        );
        assert_eq!(
            parsed.first_arrival_time.load(Ordering::Relaxed),
            stats.first_arrival_time.load(Ordering::Relaxed)
        );
        assert_eq!(
            parsed.sum_waiting_time.load(Ordering::Relaxed),
            stats.sum_waiting_time.load(Ordering::Relaxed)
        );
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
                    stats_clone.record_execution(1, 100, 0);
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
}
