// examples/src/bin/multi_threaded_price_level.rs

use pricelevel::{OrderId, OrderType, OrderUpdate, PriceLevel, Side, TimeInForce, setup_logger};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;

fn main() {
    setup_logger();
    info!("Multi-threaded Price Level Example");

    // Number of threads to use
    let thread_count = 8;

    // Create a shared price level at price 10000
    let price_level = Arc::new(PriceLevel::new(10000));

    // Transaction ID generator shared across threads
    let tx_id_generator = Arc::new(AtomicU64::new(1));

    // Synchronization barrier to ensure all threads start at the same time
    let barrier = Arc::new(Barrier::new(thread_count + 1));

    // Pre-populate with some orders
    setup_initial_orders(&price_level);

    // Print initial state
    info!("Initial state:");
    print_price_level_info(&price_level);

    // Spawn worker threads
    let mut handles = Vec::with_capacity(thread_count);

    for thread_id in 0..thread_count {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_tx_id_gen = Arc::clone(&tx_id_generator);

        // Spawn a thread
        let handle = thread::spawn(move || {
            // Each thread will perform a different operation based on its ID
            match thread_id % 4 {
                0 => {
                    // This thread adds orders
                    thread_barrier.wait(); // Wait for all threads to be ready

                    for i in 0..50 {
                        let order_id = thread_id as u64 * 1000 + i;
                        let order = create_order(thread_id, order_id);
                        thread_price_level.add_order(order);

                        // Simulate some work
                        thread::sleep(Duration::from_millis(1));
                    }

                    info!("Thread {} completed: Added 50 orders", thread_id);
                }
                1 => {
                    // This thread matches orders
                    thread_barrier.wait(); // Wait for all threads to be ready

                    for i in 0..20 {
                        let taker_id = OrderId::from_u64(thread_id as u64 * 1000 + i);
                        let match_result = thread_price_level.match_order(
                            5, // Match 5 units each time
                            taker_id,
                            &thread_tx_id_gen,
                        );

                        if i % 5 == 0 {
                            info!(
                                "Thread {} match result: executed={}, remaining={}, complete={}",
                                thread_id,
                                match_result.executed_quantity(),
                                match_result.remaining_quantity,
                                match_result.is_complete
                            );
                        }

                        // Simulate some work
                        thread::sleep(Duration::from_millis(2));
                    }

                    info!(
                        "Thread {} completed: Executed 20 match operations",
                        thread_id
                    );
                }
                2 => {
                    // This thread cancels orders
                    thread_barrier.wait(); // Wait for all threads to be ready

                    for i in 0..30 {
                        // Try to cancel orders created by thread 0
                        let order_id = OrderId::from_u64(i);
                        let result =
                            thread_price_level.update_order(OrderUpdate::Cancel { order_id });

                        if i % 10 == 0 {
                            info!(
                                "Thread {} cancel result for order {}: {:?}",
                                thread_id,
                                order_id,
                                result.is_ok()
                            );
                        }

                        // Simulate some work
                        thread::sleep(Duration::from_millis(3));
                    }

                    info!(
                        "Thread {} completed: Attempted to cancel 30 orders",
                        thread_id
                    );
                }
                _ => {
                    // This thread updates order quantities
                    thread_barrier.wait(); // Wait for all threads to be ready

                    for i in 0..40 {
                        // Try to update orders created by thread 0
                        let order_id = OrderId::from_u64(100 + i);
                        let result = thread_price_level.update_order(OrderUpdate::UpdateQuantity {
                            order_id,
                            new_quantity: 20, // Update to quantity 20
                        });

                        if i % 10 == 0 {
                            info!(
                                "Thread {} update result for order {}: {:?}",
                                thread_id,
                                order_id,
                                result.is_ok()
                            );
                        }

                        // Simulate some work
                        thread::sleep(Duration::from_millis(2));
                    }

                    info!(
                        "Thread {} completed: Attempted to update 40 orders",
                        thread_id
                    );
                }
            }
        });

        handles.push(handle);
    }

    // Start measuring execution time
    let start_time = Instant::now();

    // Release all threads to start working
    barrier.wait();

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    let elapsed = start_time.elapsed();
    info!("All threads completed in {:?}", elapsed);

    // Print final state
    info!("\nFinal state:");
    print_price_level_info(&price_level);

    // Print statistics
    info!("\nStatistics:");
    let stats = price_level.stats();
    info!("Orders added: {}", stats.orders_added());
    info!("Orders removed: {}", stats.orders_removed());
    info!("Orders executed: {}", stats.orders_executed());
    info!("Quantity executed: {}", stats.quantity_executed());
    info!("Value executed: {}", stats.value_executed());

    if let Some(avg_price) = stats.average_execution_price() {
        info!("Average execution price: {:.2}", avg_price);
    }

    if let Some(avg_wait) = stats.average_waiting_time() {
        info!("Average waiting time: {:.2} ms", avg_wait);
    }

    if let Some(time_since) = stats.time_since_last_execution() {
        info!("Time since last execution: {} ms", time_since);
    }
}

// Helper function to set up initial orders
fn setup_initial_orders(price_level: &PriceLevel) {
    // Add 200 standard orders
    for i in 0..200 {
        let order = OrderType::Standard {
            id: OrderId::from_u64(i),
            price: 10000,
            quantity: 10,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
        };
        price_level.add_order(order);
    }

    // Add some iceberg orders
    for i in 200..220 {
        let order = OrderType::IcebergOrder {
            id: OrderId::from_u64(i),
            price: 10000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
        };
        price_level.add_order(order);
    }

    // Add some reserve orders
    for i in 220..240 {
        let order = OrderType::ReserveOrder {
            id: OrderId::from_u64(i),
            price: 10000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: 2,
            replenish_amount: Some(5),
            auto_replenish: true,
        };
        price_level.add_order(order);
    }
}

// Helper function to create different types of orders based on thread ID
fn create_order(thread_id: usize, order_id: u64) -> OrderType {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Create different order types based on the thread ID
    match thread_id % 4 {
        0 => OrderType::Standard {
            id: OrderId::from_u64(order_id),
            price: 10000,
            quantity: 10,
            side: Side::Buy,
            timestamp: current_time,
            time_in_force: TimeInForce::Gtc,
        },
        1 => OrderType::IcebergOrder {
            id: OrderId::from_u64(order_id),
            price: 10000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Buy,
            timestamp: current_time,
            time_in_force: TimeInForce::Gtc,
        },
        2 => OrderType::PostOnly {
            id: OrderId::from_u64(order_id),
            price: 10000,
            quantity: 10,
            side: Side::Buy,
            timestamp: current_time,
            time_in_force: TimeInForce::Gtc,
        },
        _ => OrderType::ReserveOrder {
            id: OrderId::from_u64(order_id),
            price: 10000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Buy,
            timestamp: current_time,
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: 2,
            replenish_amount: Some(5),
            auto_replenish: true,
        },
    }
}

// Helper function to print price level information
fn print_price_level_info(price_level: &PriceLevel) {
    info!("Price: {}", price_level.price());
    info!("Visible quantity: {}", price_level.visible_quantity());
    info!("Hidden quantity: {}", price_level.hidden_quantity());
    info!("Total quantity: {}", price_level.total_quantity());
    info!("Order count: {}", price_level.order_count());
}
