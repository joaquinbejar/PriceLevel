// examples/src/bin/contention_test.rs

use pricelevel::{
    OrderId, OrderType, OrderUpdate, PriceLevel, Side, TimeInForce, UuidGenerator, setup_logger,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;
use uuid::Uuid;

// Test parameters
const THREAD_COUNT: usize = 16;
const TEST_DURATION_MS: u64 = 3000; // 3 seconds per test

fn main() {
    setup_logger();
    info!("PriceLevel Contention Pattern Test");
    info!("=================================");

    // Run tests with different contention patterns
    test_hot_spot_contention();
    test_read_write_ratio();
}

// Test how different read/write ratios affect performance
fn test_read_write_ratio() {
    info!("\n[TEST] Read/Write Ratio");
    info!("----------------------");

    let test_cases = [0, 25, 50, 75, 95]; // Percentage of read operations

    // Results table for read/write test
    let mut results = HashMap::new();

    for read_percentage in &test_cases {
        info!("\nTesting with {}% read operations...", read_percentage);

        // Create a shared price level at price 10000
        let price_level = Arc::new(PriceLevel::new(10000));

        // Transaction ID generator for match operations
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let tx_id_generator = Arc::new(UuidGenerator::new(namespace));

        // Pre-populate with orders
        setup_orders_for_read_write_test(&price_level);

        // Counter for operations performed by each thread
        let operation_counters = Arc::new(Mutex::new(vec![0; THREAD_COUNT]));

        // Flag to signal when to stop the test
        let running = Arc::new(AtomicBool::new(true));

        // Barrier for synchronized start
        let barrier = Arc::new(Barrier::new(THREAD_COUNT + 1));

        // Spawn worker threads
        let mut handles = Vec::with_capacity(THREAD_COUNT);

        for thread_id in 0..THREAD_COUNT {
            let thread_price_level = Arc::clone(&price_level);
            let thread_barrier = Arc::clone(&barrier);
            let thread_running = Arc::clone(&running);
            let thread_tx_id_gen = Arc::clone(&tx_id_generator);
            let thread_counters = Arc::clone(&operation_counters);
            let read_pct = *read_percentage;

            let handle = thread::spawn(move || {
                // Wait for synchronized start
                thread_barrier.wait();

                let mut local_counter = 0;

                while thread_running.load(Ordering::Relaxed) {
                    // Determine if this is a read or write operation
                    let is_read = (local_counter % 100) < read_pct;

                    if is_read {
                        // Read operation
                        match local_counter % 3 {
                            0 => {
                                // Take a snapshot
                                let _snapshot = thread_price_level.snapshot();
                            }
                            1 => {
                                // Read visible quantity
                                let _quantity = thread_price_level.visible_quantity();
                            }
                            _ => {
                                // Read total quantity and order count
                                let _total = thread_price_level.total_quantity();
                                let _count = thread_price_level.order_count();
                            }
                        }
                    } else {
                        // Write operation
                        match local_counter % 3 {
                            0 => {
                                // Add a new order
                                let order_id = thread_id as u64 * 10000 + local_counter;
                                let order = create_standard_order(order_id, 10000, 10);
                                thread_price_level.add_order(order);
                            }
                            1 => {
                                // Match order
                                let taker_id =
                                    OrderId::from_u64(thread_id as u64 * 10000 + local_counter);
                                thread_price_level.match_order(
                                    5, // Match 5 units
                                    taker_id,
                                    &thread_tx_id_gen,
                                );
                            }
                            _ => {
                                // Cancel/update order
                                let order_id = OrderId::from_u64(local_counter % 500);
                                let _ = thread_price_level
                                    .update_order(OrderUpdate::Cancel { order_id });
                            }
                        }
                    }

                    local_counter += 1;
                }

                // Update the operation counter
                if let Ok(mut counters) = thread_counters.lock() {
                    counters[thread_id] = local_counter;
                }
            });

            handles.push(handle);
        }

        // Start the test
        let start_time = Instant::now();
        barrier.wait(); // Release all threads

        // Run for test duration
        thread::sleep(Duration::from_millis(TEST_DURATION_MS));

        // Stop all threads
        running.store(false, Ordering::Relaxed);

        // Wait for all threads to finish
        for handle in handles {
            handle.join().unwrap();
        }

        let elapsed = start_time.elapsed();

        // Calculate total operations
        let total_ops: usize = if let Ok(counters) = operation_counters.lock() {
            counters.iter().map(|&count| count as usize).sum()
        } else {
            0
        };

        let ops_per_second = total_ops as f64 / elapsed.as_secs_f64();

        info!("Completed {} operations in {:?}", total_ops, elapsed);
        info!("Throughput: {:.2} operations/second", ops_per_second);

        // Store result
        results.insert(*read_percentage, ops_per_second);
    }

    // Print summary
    info!("\nRead/Write Ratio Results:");
    info!("--------------------------");
    info!("Read %     |  Operations/second");
    info!("---------------------------");

    for pct in &test_cases {
        if let Some(ops) = results.get(pct) {
            info!("{}%        |  {:.2}", pct, ops);
        }
    }
}

// Helper function to set up orders for read/write test
fn setup_orders_for_read_write_test(price_level: &PriceLevel) {
    // Add 500 orders
    for i in 0..500 {
        let order = create_standard_order(i, 10000, 10);
        price_level.add_order(order);
    }
}

// Helper function to create a standard order
fn create_standard_order(id: u64, price: u64, quantity: u64) -> OrderType<()> {
    OrderType::Standard {
        id: OrderId::from_u64(id),
        price,
        quantity,
        side: Side::Buy,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

// Helper function to set up orders for hot spot test
fn setup_orders_for_hot_spot_test(price_level: &PriceLevel) {
    // Add 1000 orders (first 20 are hot spot)
    for i in 0..1000 {
        let order = create_standard_order(i, 10000, 10);
        price_level.add_order(order);
    }
}

// Test contention when multiple threads target the same "hot" orders
fn test_hot_spot_contention() {
    info!("\n[TEST] Hot Spot Contention");
    info!("---------------------------");

    let test_cases = [0, 25, 50, 75, 100]; // Percentage of operations targeting hot spot

    // Results table for hot spot test
    let mut results = HashMap::new();

    for hot_spot_percentage in &test_cases {
        info!(
            "\nTesting with {}% operations targeting hot spot...",
            hot_spot_percentage
        );

        // Create a shared price level at price 10000
        let price_level = Arc::new(PriceLevel::new(10000));

        // Pre-populate with 1000 orders (first 20 are "hot spot")
        setup_orders_for_hot_spot_test(&price_level);

        // Counter for operations performed by each thread
        let operation_counters = Arc::new(Mutex::new(vec![0; THREAD_COUNT]));

        // Flag to signal when to stop the test
        let running = Arc::new(AtomicBool::new(true));

        // Barrier for synchronized start
        let barrier = Arc::new(Barrier::new(THREAD_COUNT + 1));

        // Spawn worker threads
        let mut handles = Vec::with_capacity(THREAD_COUNT);

        for thread_id in 0..THREAD_COUNT {
            let thread_price_level = Arc::clone(&price_level);
            let thread_barrier = Arc::clone(&barrier);
            let thread_running = Arc::clone(&running);
            let thread_counters = Arc::clone(&operation_counters);
            let hot_spot_pct = *hot_spot_percentage;

            let handle = thread::spawn(move || {
                // Wait for synchronized start
                thread_barrier.wait();

                let mut local_counter = 0;

                while thread_running.load(Ordering::Relaxed) {
                    // Determine if this operation targets the hot spot
                    let target_hot_spot = (local_counter % 100) < hot_spot_pct;

                    // Select an order ID range based on hot spot decision
                    let id_range = if target_hot_spot {
                        // Hot spot range (first 20 orders)
                        0..20
                    } else {
                        // Non-hot spot range (remaining orders)
                        20..1000
                    };

                    // Choose an order ID within the selected range
                    let order_idx = (thread_id as u64 + local_counter)
                        % (id_range.end - id_range.start)
                        + id_range.start;

                    // Perform an operation based on iteration
                    match local_counter % 3 {
                        0 => {
                            // Add a new order with same ID (this will likely fail, but creates contention)
                            let order = create_standard_order(order_idx, 10000, 10);
                            thread_price_level.add_order(order);
                        }
                        1 => {
                            // Cancel an order
                            let _result = thread_price_level.update_order(OrderUpdate::Cancel {
                                order_id: OrderId::from_u64(order_idx),
                            });
                        }
                        _ => {
                            // Update an order
                            let _result =
                                thread_price_level.update_order(OrderUpdate::UpdateQuantity {
                                    order_id: OrderId::from_u64(order_idx),
                                    new_quantity: 15,
                                });
                        }
                    }

                    local_counter += 1;
                }

                // Update the operation counter
                if let Ok(mut counters) = thread_counters.lock() {
                    counters[thread_id] = local_counter;
                }
            });

            handles.push(handle);
        }

        // Start the test
        let start_time = Instant::now();
        barrier.wait(); // Release all threads

        // Run for test duration
        thread::sleep(Duration::from_millis(TEST_DURATION_MS));

        // Stop all threads
        running.store(false, Ordering::Relaxed);

        // Wait for all threads to finish
        for handle in handles {
            handle.join().unwrap();
        }

        let elapsed = start_time.elapsed();

        // Calculate total operations
        let total_ops: usize = if let Ok(counters) = operation_counters.lock() {
            counters.iter().map(|&count| count as usize).sum()
        } else {
            0
        };

        let ops_per_second = total_ops as f64 / elapsed.as_secs_f64();

        info!("Completed {} operations in {:?}", total_ops, elapsed);
        info!("Throughput: {:.2} operations/second", ops_per_second);

        // Store result
        results.insert(*hot_spot_percentage, ops_per_second);
    }

    // Print summary
    info!("\nHot Spot Contention Results:");
    info!("-----------------------------");
    info!("Hot Spot %  |  Operations/second");
    info!("---------------------------");

    for pct in &test_cases {
        if let Some(ops) = results.get(pct) {
            info!("{}%        |  {:.2}", pct, ops);
        }
    }
}
