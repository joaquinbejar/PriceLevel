use criterion::{BenchmarkId, Criterion, criterion_group};
use pricelevel::{
    OrderId, OrderType, OrderUpdate, PegReferenceType, PriceLevel, Side, TimeInForce,
};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Concurrent Operations");

    // Test with various thread counts
    for thread_count in [2, 4, 8, 16].iter() {
        group.bench_with_input(
            BenchmarkId::new("concurrent_add_standard_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    measure_concurrent_operation(
                        thread_count,
                        iters,
                        |price_level, thread_id, iteration| {
                            // Each thread adds orders with unique IDs
                            let base_id = thread_id as u64 * 1_000_000 + iteration;
                            let order = create_standard_order(base_id, 10000, 100);
                            price_level.add_order(order);
                        },
                    )
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_add_mixed_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    measure_concurrent_operation(
                        thread_count,
                        iters,
                        |price_level, thread_id, iteration| {
                            // Each thread adds a mix of order types with unique IDs
                            let base_id = thread_id as u64 * 1_000_000 + iteration;
                            let order = match iteration % 5 {
                                0 => create_standard_order(base_id, 10000, 100),
                                1 => create_iceberg_order(base_id, 10000, 50, 150),
                                2 => create_post_only_order(base_id, 10000, 100),
                                3 => create_reserve_order(base_id, 10000, 50, 150, 10, true, None),
                                _ => create_pegged_order(base_id, 10000, 100),
                            };
                            price_level.add_order(order);
                        },
                    )
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_match_standard_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    measure_concurrent_match_operation(
                        thread_count,
                        iters,
                        setup_standard_orders(500), // Pre-populate with orders
                        |price_level, thread_id, iteration| {
                            let transaction_id_generator = AtomicU64::new(1);
                            // Use different taker order IDs for each thread/iteration
                            let taker_id = OrderId(thread_id as u64 * 1_000_000 + iteration);
                            price_level.match_order(5, taker_id, &transaction_id_generator);
                        },
                    )
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_mixed_operations", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| measure_concurrent_mixed_operations(thread_count, iters));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_cancel_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    measure_concurrent_cancel_operation(
                        thread_count,
                        iters,
                        |price_level, thread_id, iteration| {
                            // Each thread cancels a different order
                            let order_id = OrderId(thread_id as u64 * 100 + iteration % 100);
                            let _ = price_level.update_order(OrderUpdate::Cancel { order_id });
                        },
                    )
                });
            },
        );
    }

    group.finish();
}

/// Measures time for concurrent operations on a price level
fn measure_concurrent_operation<F>(thread_count: usize, iterations: u64, operation: F) -> Duration
where
    F: Fn(&Arc<PriceLevel>, usize, u64) + Send + Sync + 'static,
{
    let price_level = Arc::new(PriceLevel::new(10000));
    let operation = Arc::new(operation);
    let barrier = Arc::new(Barrier::new(thread_count + 1)); // +1 for main thread

    let mut handles = Vec::with_capacity(thread_count);

    for thread_id in 0..thread_count {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_operation = Arc::clone(&operation);

        handles.push(thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            for i in 0..iterations {
                thread_operation(&thread_price_level, thread_id, i);
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

/// Measures time for concurrent match operations on a pre-populated price level
fn measure_concurrent_match_operation<F>(
    thread_count: usize,
    iterations: u64,
    initial_price_level: PriceLevel,
    operation: F,
) -> Duration
where
    F: Fn(&Arc<PriceLevel>, usize, u64) + Send + Sync + 'static,
{
    // Create an Arc wrapping the pre-populated price level
    let price_level = Arc::new(initial_price_level);
    let operation = Arc::new(operation);
    let barrier = Arc::new(Barrier::new(thread_count + 1)); // +1 for main thread

    let mut handles = Vec::with_capacity(thread_count);

    for thread_id in 0..thread_count {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_operation = Arc::clone(&operation);

        handles.push(thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            for i in 0..iterations {
                thread_operation(&thread_price_level, thread_id, i);
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

/// Measures time for concurrent cancellation operations on a pre-populated price level
fn measure_concurrent_cancel_operation<F>(
    thread_count: usize,
    iterations: u64,
    operation: F,
) -> Duration
where
    F: Fn(&Arc<PriceLevel>, usize, u64) + Send + Sync + 'static,
{
    // Create a price level with orders to cancel
    let initial_price_level = PriceLevel::new(10000);

    // Add orders that will be cancelled
    // Each thread gets 100 orders with IDs that don't overlap
    for thread_id in 0..thread_count {
        for i in 0..100 {
            let order_id = thread_id as u64 * 100 + i;
            let order = create_standard_order(order_id, 10000, 10);
            initial_price_level.add_order(order);
        }
    }

    // Wrap in Arc and proceed with concurrent operation measurement
    let price_level = Arc::new(initial_price_level);
    let operation = Arc::new(operation);
    let barrier = Arc::new(Barrier::new(thread_count + 1)); // +1 for main thread

    let mut handles = Vec::with_capacity(thread_count);

    for thread_id in 0..thread_count {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_operation = Arc::clone(&operation);

        handles.push(thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            // Limit iterations to 100 to match the number of orders per thread
            let actual_iterations = std::cmp::min(iterations, 100);
            for i in 0..actual_iterations {
                thread_operation(&thread_price_level, thread_id, i);
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

/// Measures time for mixed concurrent operations (add, match, cancel) on a price level
fn measure_concurrent_mixed_operations(thread_count: usize, iterations: u64) -> Duration {
    let price_level = Arc::new(PriceLevel::new(10000));
    let transaction_id_gen = Arc::new(AtomicU64::new(1));
    let barrier = Arc::new(Barrier::new(thread_count + 1)); // +1 for main thread

    // Pre-populate with some orders
    for i in 0..200 {
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
                // Determine operation based on iteration
                match i % 4 {
                    0 => {
                        // Add a new order
                        let base_id = thread_id as u64 * 1_000_000 + i;
                        let order = create_standard_order(base_id, 10000, 10);
                        thread_price_level.add_order(order);
                    }
                    1 => {
                        // Match against existing orders
                        let taker_id = OrderId(thread_id as u64 * 1_000_000 + i);
                        thread_price_level.match_order(5, taker_id, &thread_transaction_id_gen);
                    }
                    2 => {
                        // Cancel one of the initial orders
                        let order_id = OrderId(i % 200);
                        let _ = thread_price_level.update_order(OrderUpdate::Cancel { order_id });
                    }
                    _ => {
                        // Update quantity
                        let base_id = thread_id as u64 * 1_000_000 + (i - 1);
                        let _ = thread_price_level.update_order(OrderUpdate::UpdateQuantity {
                            order_id: OrderId(base_id),
                            new_quantity: 20,
                        });
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

// Helper functions to create different types of orders for benchmarking

/// Create a standard limit order for testing
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

/// Create an iceberg order for testing
fn create_iceberg_order(id: u64, price: u64, visible: u64, hidden: u64) -> OrderType {
    OrderType::IcebergOrder {
        id: OrderId::from_u64(id),
        price,
        visible_quantity: visible,
        hidden_quantity: hidden,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
    }
}

/// Create a post-only order for testing
fn create_post_only_order(id: u64, price: u64, quantity: u64) -> OrderType {
    OrderType::PostOnly {
        id: OrderId::from_u64(id),
        price,
        quantity,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
    }
}

/// Create a reserve order for testing
fn create_reserve_order(
    id: u64,
    price: u64,
    visible: u64,
    hidden: u64,
    threshold: u64,
    auto_replenish: bool,
    replenish_amount: Option<u64>,
) -> OrderType {
    OrderType::ReserveOrder {
        id: OrderId::from_u64(id),
        price,
        visible_quantity: visible,
        hidden_quantity: hidden,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: threshold,
        replenish_amount,
        auto_replenish,
    }
}

/// Create a pegged order for testing
fn create_pegged_order(id: u64, price: u64, quantity: u64) -> OrderType {
    OrderType::PeggedOrder {
        id: OrderId::from_u64(id),
        price,
        quantity,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
        reference_price_offset: -50,
        reference_price_type: PegReferenceType::BestAsk,
    }
}

/// Set up a price level with standard orders
fn setup_standard_orders(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);

    for i in 0..order_count {
        let order = OrderType::Standard {
            id: OrderId(i),
            price: 10000,
            quantity: 10,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
        };
        price_level.add_order(order);
    }

    price_level
}

criterion_group!(concurrent_benches, register_benchmarks);
