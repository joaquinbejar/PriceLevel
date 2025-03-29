// examples/src/bin/hft_simulation.rs

use pricelevel::{
    OrderId, OrderType, OrderUpdate, PegReferenceType, PriceLevel, Side, TimeInForce, setup_logger,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;

// Simulation parameters
const PRICE: u64 = 10000;
const SIMULATION_DURATION_MS: u64 = 5000; // 5 seconds
const MAKER_THREAD_COUNT: usize = 10;
const TAKER_THREAD_COUNT: usize = 10;
const CANCELLER_THREAD_COUNT: usize = 10;
const TOTAL_THREAD_COUNT: usize = MAKER_THREAD_COUNT + TAKER_THREAD_COUNT + CANCELLER_THREAD_COUNT;

fn main() {
    setup_logger();
    info!("High-Frequency Trading Simulation");
    info!("=================================");
    info!("Simulating price level at {}", PRICE);
    info!("Duration: {} ms", SIMULATION_DURATION_MS);
    info!("Maker threads: {}", MAKER_THREAD_COUNT);
    info!("Taker threads: {}", TAKER_THREAD_COUNT);
    info!("Canceller threads: {}", CANCELLER_THREAD_COUNT);
    info!("\n");

    // Create a shared price level
    let price_level = Arc::new(PriceLevel::new(PRICE));

    // Transaction ID generator shared across threads
    let tx_id_generator = Arc::new(AtomicU64::new(1));

    // Counter for orders added
    let orders_added_counter = Arc::new(AtomicU64::new(0));

    // Counter for matches executed
    let matches_executed_counter = Arc::new(AtomicU64::new(0));

    // Counter for cancellations
    let cancellations_counter = Arc::new(AtomicU64::new(0));

    // Flag to signal when to stop the simulation
    let running = Arc::new(AtomicBool::new(true));

    // Synchronization barrier to ensure all threads start at the same time
    let barrier = Arc::new(Barrier::new(TOTAL_THREAD_COUNT + 1)); // +1 for main thread

    // Pre-populate with some orders to ensure there's something to match
    let initial_order_count = 1000;
    info!(
        "Seeding the price level with {} initial orders...",
        initial_order_count
    );
    setup_initial_orders(&price_level, initial_order_count);

    // Print initial state
    info!("Initial state:");
    print_price_level_info(&price_level);

    // Spawn threads
    let mut handles = Vec::with_capacity(TOTAL_THREAD_COUNT);

    // Spawn maker threads (add orders)
    for i in 0..MAKER_THREAD_COUNT {
        let thread_id = i;
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_running = Arc::clone(&running);
        let thread_counter = Arc::clone(&orders_added_counter);

        let handle = thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            let mut local_counter = 0;
            while thread_running.load(Ordering::Relaxed) {
                // Generate a unique order ID
                let order_id = (thread_id as u64) * 1_000_000 + local_counter;

                // Create and add an order
                let order_type = match local_counter % 5 {
                    0 => create_standard_order(order_id),
                    1 => create_iceberg_order(order_id),
                    2 => create_post_only_order(order_id),
                    3 => create_reserve_order(order_id),
                    _ => create_pegged_order(order_id),
                };

                thread_price_level.add_order(order_type);
                local_counter += 1;

                // Update global counter periodically to reduce contention
                if local_counter % 100 == 0 {
                    thread_counter.fetch_add(100, Ordering::Relaxed);
                }

                // Simulate some think time (realistic for HFT is microseconds)
                thread::sleep(Duration::from_micros(50));
            }

            // Update remaining count
            let remainder = local_counter % 100;
            if remainder > 0 {
                thread_counter.fetch_add(remainder, Ordering::Relaxed);
            }

            info!("Maker thread {} added {} orders", thread_id, local_counter);
        });

        handles.push(handle);
    }

    // Spawn taker threads (match orders)
    for i in 0..TAKER_THREAD_COUNT {
        let thread_id = MAKER_THREAD_COUNT + i;
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_running = Arc::clone(&running);
        let thread_tx_id_gen = Arc::clone(&tx_id_generator);
        let thread_counter = Arc::clone(&matches_executed_counter);

        let handle = thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            let mut local_counter = 0;
            while thread_running.load(Ordering::Relaxed) {
                // Generate a unique taker order ID
                let taker_id = OrderId((thread_id as u64) * 1_000_000 + local_counter);

                // Match varying quantities
                let quantity = (local_counter % 5) + 1; // Match 1-5 units
                let result = thread_price_level.match_order(quantity, taker_id, &thread_tx_id_gen);

                // Count successful matches
                if result.executed_quantity() > 0 {
                    local_counter += 1;
                }

                // Update global counter periodically
                if local_counter % 50 == 0 {
                    thread_counter.fetch_add(50, Ordering::Relaxed);
                }

                // Simulate some think time
                thread::sleep(Duration::from_micros(100));
            }

            // Update remaining count
            let remainder = local_counter % 50;
            if remainder > 0 {
                thread_counter.fetch_add(remainder, Ordering::Relaxed);
            }

            info!(
                "Taker thread {} executed {} matches",
                thread_id, local_counter
            );
        });

        handles.push(handle);
    }

    // Spawn canceller threads (cancel orders)
    for i in 0..CANCELLER_THREAD_COUNT {
        let thread_id = MAKER_THREAD_COUNT + TAKER_THREAD_COUNT + i;
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_running = Arc::clone(&running);
        let thread_counter = Arc::clone(&cancellations_counter);

        let handle = thread::spawn(move || {
            // Wait for all threads to be ready
            thread_barrier.wait();

            let mut local_counter = 0;
            while thread_running.load(Ordering::Relaxed) {
                // Try to cancel random orders
                // Use a combination of the thread ID, local counter, and time to generate "random" order IDs
                let time_component = Instant::now().elapsed().as_nanos() as u64 % 1000;
                let order_id = OrderId(time_component + local_counter);

                let result = thread_price_level.update_order(OrderUpdate::Cancel { order_id });

                // Count successful cancellations
                if result.is_ok() && result.unwrap().is_some() {
                    local_counter += 1;
                }

                // Update global counter periodically
                if local_counter % 20 == 0 {
                    thread_counter.fetch_add(20, Ordering::Relaxed);
                }

                // Simulate some think time
                thread::sleep(Duration::from_micros(200));
            }

            // Update remaining count
            let remainder = local_counter % 20;
            if remainder > 0 {
                thread_counter.fetch_add(remainder, Ordering::Relaxed);
            }

            info!(
                "Canceller thread {} executed {} cancellations",
                thread_id, local_counter
            );
        });

        handles.push(handle);
    }

    // Start the simulation timer
    info!("\nStarting simulation for {} ms...", SIMULATION_DURATION_MS);
    let start_time = Instant::now();

    // Release all threads to start working
    barrier.wait();

    // Run the simulation for the specified duration
    thread::sleep(Duration::from_millis(SIMULATION_DURATION_MS));

    // Signal all threads to stop
    running.store(false, Ordering::Relaxed);
    info!("\nStopping simulation...");

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    let elapsed = start_time.elapsed();
    info!("\nSimulation completed in {:?}", elapsed);

    // Print performance statistics
    let orders_added = orders_added_counter.load(Ordering::Relaxed);
    let matches_executed = matches_executed_counter.load(Ordering::Relaxed);
    let cancellations = cancellations_counter.load(Ordering::Relaxed);

    let elapsed_seconds = elapsed.as_secs_f64();
    let total_operations = orders_added + matches_executed + cancellations;

    info!("\nPerformance Statistics:");
    info!("----------------------");
    info!(
        "Orders added: {} ({:.2} per second)",
        orders_added,
        orders_added as f64 / elapsed_seconds
    );
    info!(
        "Matches executed: {} ({:.2} per second)",
        matches_executed,
        matches_executed as f64 / elapsed_seconds
    );
    info!(
        "Cancellations: {} ({:.2} per second)",
        cancellations,
        cancellations as f64 / elapsed_seconds
    );
    info!(
        "Total operations: {} ({:.2} per second)",
        total_operations,
        total_operations as f64 / elapsed_seconds
    );

    // Print final state
    info!("\nFinal state:");
    print_price_level_info(&price_level);

    // Print price level statistics
    info!("\nPrice Level Statistics:");
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
fn setup_initial_orders(price_level: &PriceLevel, count: u64) {
    for i in 0..count {
        // Create different types of orders
        let order = match i % 4 {
            0 => create_standard_order(i),
            1 => create_iceberg_order(i),
            2 => create_post_only_order(i),
            _ => create_reserve_order(i),
        };

        price_level.add_order(order);
    }
}

// Helper function to create a standard order
fn create_standard_order(id: u64) -> OrderType {
    OrderType::Standard {
        id: OrderId::from_u64(id),
        price: PRICE,
        quantity: 10,
        side: Side::Buy,
        timestamp: get_current_timestamp(),
        time_in_force: TimeInForce::Gtc,
    }
}

// Helper function to create an iceberg order
fn create_iceberg_order(id: u64) -> OrderType {
    OrderType::IcebergOrder {
        id: OrderId::from_u64(id),
        price: PRICE,
        visible_quantity: 5,
        hidden_quantity: 15,
        side: Side::Buy,
        timestamp: get_current_timestamp(),
        time_in_force: TimeInForce::Gtc,
    }
}

// Helper function to create a post-only order
fn create_post_only_order(id: u64) -> OrderType {
    OrderType::PostOnly {
        id: OrderId::from_u64(id),
        price: PRICE,
        quantity: 8,
        side: Side::Buy,
        timestamp: get_current_timestamp(),
        time_in_force: TimeInForce::Gtc,
    }
}

// Helper function to create a reserve order
fn create_reserve_order(id: u64) -> OrderType {
    OrderType::ReserveOrder {
        id: OrderId::from_u64(id),
        price: PRICE,
        visible_quantity: 5,
        hidden_quantity: 15,
        side: Side::Buy,
        timestamp: get_current_timestamp(),
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: 2,
        replenish_amount: Some(5),
        auto_replenish: true,
    }
}

// Helper function to create a pegged order
fn create_pegged_order(id: u64) -> OrderType {
    OrderType::PeggedOrder {
        id: OrderId::from_u64(id),
        price: PRICE,
        quantity: 10,
        side: Side::Buy,
        timestamp: get_current_timestamp(),
        time_in_force: TimeInForce::Gtc,
        reference_price_offset: -50,
        reference_price_type: PegReferenceType::BestAsk,
    }
}

// Helper function to get current timestamp in milliseconds
fn get_current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// Helper function to print price level information
fn print_price_level_info(price_level: &PriceLevel) {
    info!("Price: {}", price_level.price());
    info!("Visible quantity: {}", price_level.visible_quantity());
    info!("Hidden quantity: {}", price_level.hidden_quantity());
    info!("Total quantity: {}", price_level.total_quantity());
    info!("Order count: {}", price_level.order_count());
    info!("Statistics: {}", price_level.stats());
}
