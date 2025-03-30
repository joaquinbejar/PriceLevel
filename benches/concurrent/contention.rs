use criterion::{BenchmarkId, Criterion};
use pricelevel::{OrderId, OrderType, OrderUpdate, PriceLevel, Side, TimeInForce, UuidGenerator};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Register benchmarks that test different contention patterns
#[allow(dead_code)]
#[cfg(not(tarpaulin_include))]
pub fn register_contention_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Contention Patterns");

    // Test with different read/write ratios
    for read_ratio in [0, 25, 50, 75, 95].iter() {
        // Fixed at 8 threads which is a common server core count
        let thread_count = 8;

        group.bench_with_input(
            BenchmarkId::new("read_write_ratio", read_ratio),
            read_ratio,
            |b, &read_ratio| {
                b.iter_custom(|iters| {
                    measure_read_write_contention(thread_count, iters, read_ratio)
                });
            },
        );
    }

    // Test with different access patterns (hot spot vs distributed)
    for hot_spot_percentage in [0, 20, 50, 80, 100].iter() {
        // Fixed at 8 threads
        let thread_count = 8;

        group.bench_with_input(
            BenchmarkId::new("hot_spot_contention", hot_spot_percentage),
            hot_spot_percentage,
            |b, &hot_spot_percentage| {
                b.iter_custom(|iters| {
                    measure_hot_spot_contention(thread_count, iters, hot_spot_percentage)
                });
            },
        );
    }

    group.finish();
}

/// Measures time for operations with different read/write ratios
/// read_ratio = percentage of read operations (0-100)
#[allow(dead_code)]
#[cfg(not(tarpaulin_include))]
fn measure_read_write_contention(
    thread_count: usize,
    iterations: u64,
    read_ratio: usize,
) -> Duration {
    let price_level = Arc::new(PriceLevel::new(10000));
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let transaction_id_gen = Arc::new(UuidGenerator::new(namespace));
    let barrier = Arc::new(Barrier::new(thread_count + 1)); // +1 for main thread

    // Pre-populate with orders to read/match against
    for i in 0..500 {
        let order = create_standard_order(i, 10000, 10);
        price_level.add_order(order);
    }

    let mut handles = Vec::with_capacity(thread_count);

    for thread_id in 0..thread_count {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_transaction_id_gen = Arc::clone(&transaction_id_gen);

        handles.push(thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            for i in 0..iterations {
                // Determine if this is a read or write operation
                let is_read = (i as usize % 100) < read_ratio;

                if is_read {
                    // Read operation: snapshot or query
                    if i % 2 == 0 {
                        // Take a snapshot (read-only operation)
                        let _ = thread_price_level.snapshot();
                    } else {
                        // Query operations (check visible quantity, etc.)
                        let _ = thread_price_level.visible_quantity();
                        let _ = thread_price_level.hidden_quantity();
                        let _ = thread_price_level.order_count();
                    }
                } else {
                    // Write operation
                    let op_type = i % 3;

                    match op_type {
                        0 => {
                            // Add a new order
                            let base_id = thread_id as u64 * 1_000_000 + i;
                            let order = create_standard_order(base_id, 10000, 10);
                            thread_price_level.add_order(order);
                        }
                        1 => {
                            // Match against existing orders
                            let taker_id = OrderId::from_u64(thread_id as u64 * 1_000_000 + i);
                            thread_price_level.match_order(2, taker_id, &thread_transaction_id_gen);
                        }
                        _ => {
                            // Cancel an order
                            let order_id = OrderId::from_u64(i % 500);
                            let _ =
                                thread_price_level.update_order(OrderUpdate::Cancel { order_id });
                        }
                    }
                }
            }

            // Signal completion
            thread_barrier.wait();
        }));
    }

    // Start timing
    barrier.wait();
    let start = Instant::now();

    // Wait for all threads to complete
    barrier.wait();
    let duration = start.elapsed();

    // Join all threads
    for handle in handles {
        let _ = handle.join();
    }

    duration
}

/// Measures time for operations with different hot spot patterns
/// hot_spot_percentage = percentage of operations targeting the same hot spot orders (0-100)
#[allow(dead_code)]
#[cfg(not(tarpaulin_include))]
fn measure_hot_spot_contention(
    thread_count: usize,
    iterations: u64,
    hot_spot_percentage: usize,
) -> Duration {
    let price_level = Arc::new(PriceLevel::new(10000));
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"example.com");
    let transaction_id_gen = Arc::new(UuidGenerator::new(namespace));
    let barrier = Arc::new(Barrier::new(thread_count + 1)); // +1 for main thread

    // Pre-populate with orders
    // First 20 orders are the "hot spot" that may be contended
    for i in 0..20 {
        let order = create_standard_order(i, 10000, 10);
        price_level.add_order(order);
    }

    // Additional 980 orders for the non-hot spot operations
    for i in 20..1000 {
        let order = create_standard_order(i, 10000, 10);
        price_level.add_order(order);
    }

    let mut handles = Vec::with_capacity(thread_count);

    for thread_id in 0..thread_count {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_transaction_id_gen = Arc::clone(&transaction_id_gen);

        handles.push(thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            for i in 0..iterations {
                // Determine if this operation targets the hot spot
                let target_hot_spot = (i as usize % 100) < hot_spot_percentage;

                // Select an order ID range based on hot spot decision
                let id_range = if target_hot_spot {
                    0..20 // Hot spot range
                } else {
                    20..1000 // Non-hot spot range
                };

                // Choose a random order ID within the selected range
                let order_idx =
                    (thread_id as u64 + i) % (id_range.end - id_range.start) + id_range.start;

                // Perform operations
                let op_type = i % 4;
                match op_type {
                    0 => {
                        // Cancel an order
                        let _ = thread_price_level.update_order(OrderUpdate::Cancel {
                            order_id: OrderId::from_u64(order_idx),
                        });
                    }
                    1 => {
                        // Add a new order to replace canceled ones
                        let base_id = order_idx;
                        let order = create_standard_order(base_id, 10000, 10);
                        thread_price_level.add_order(order);
                    }
                    2 => {
                        // Update quantity
                        let _ = thread_price_level.update_order(OrderUpdate::UpdateQuantity {
                            order_id: OrderId::from_u64(order_idx),
                            new_quantity: 15,
                        });
                    }
                    _ => {
                        // Match operations
                        let taker_id = OrderId::from_u64(thread_id as u64 * 1_000_000 + i);
                        thread_price_level.match_order(1, taker_id, &thread_transaction_id_gen);
                    }
                }
            }

            // Signal completion
            thread_barrier.wait();
        }));
    }

    // Start timing
    barrier.wait();
    let start = Instant::now();

    // Wait for all threads to complete
    barrier.wait();
    let duration = start.elapsed();

    // Join all threads
    for handle in handles {
        let _ = handle.join();
    }

    duration
}

/// Create a standard limit order for testing
#[allow(dead_code)]
#[cfg(not(tarpaulin_include))]
fn create_standard_order(id: u64, price: u64, quantity: u64) -> OrderType {
    OrderType::Standard {
        id: OrderId::from_u64(id),
        price,
        quantity,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
    }
}
