// examples/src/bin/simple.rs
//
// Several threads working one shared price level.
//
// `PriceLevel::match_order` supports a single logical matcher per level: two
// concurrent `match_order` calls on the same level must be serialized by the
// caller. Exactly one thread (`MATCHER_THREAD_ID`) matches here; the others
// add, cancel and resize orders, which the level supports concurrently.
// Cancels and resizes that find no resting order are reported as missing,
// not as successes.

use pricelevel::{
    Hash32, Id, OrderType, OrderUpdate, Price, PriceLevel, PriceLevelError, Quantity, Side,
    TakerKind, TimeInForce, TimestampMs, UnixClock, UuidGenerator, setup_logger,
};
use std::num::NonZeroU64;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;
use uuid::Uuid;

/// The only thread allowed to call `match_order` on the shared level.
const MATCHER_THREAD_ID: usize = 1;
/// Taker ids live in a disjoint high range, so a taker id can never equal a
/// resting maker id (a self-fill, impossible for a real order).
const TAKER_ID_BASE: u64 = 1 << 40;

/// What a worker thread does with the shared level.
#[derive(Debug, Clone, Copy)]
enum Role {
    Add,
    Match,
    Cancel,
    Resize,
}

fn role_for(thread_id: usize) -> Role {
    if thread_id == MATCHER_THREAD_ID {
        return Role::Match;
    }
    match thread_id % 4 {
        0 => Role::Add,
        2 => Role::Cancel,
        // Any other thread that would have matched resizes instead, so the
        // level never sees two concurrent matchers.
        _ => Role::Resize,
    }
}

fn main() {
    setup_logger().expect("Failed to initialize logger");
    info!("Multi-threaded Price Level Example");

    // Number of threads to use
    let thread_count = 8;

    // Create a shared price level at price 10000
    let price_level = Arc::new(PriceLevel::new(10000));

    // Trade id generator, used only by the matcher thread
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let tx_id_generator = Arc::new(UuidGenerator::new(namespace));

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
            thread_barrier.wait(); // Wait for all threads to be ready

            match role_for(thread_id) {
                Role::Add => {
                    let (mut admitted, mut rejected) = (0u64, 0u64);
                    for i in 0..50 {
                        // Offset past the ids seeded by setup_initial_orders
                        // (0..240): admission rejects a duplicate id (issue
                        // #113) instead of silently overwriting.
                        let order_id = 10_000 + thread_id as u64 * 1000 + i;
                        let order = create_order(thread_id, order_id);
                        match thread_price_level.add_order(order) {
                            Ok(_) => admitted += 1,
                            Err(_) => rejected += 1,
                        }

                        // Simulate some work
                        thread::sleep(Duration::from_millis(1));
                    }

                    info!(
                        "Thread {} (add): 50 attempted, {} admitted, {} rejected",
                        thread_id, admitted, rejected
                    );
                }
                Role::Match => {
                    let (mut successful_takers, mut fills) = (0u64, 0usize);
                    for i in 0..20 {
                        let taker_id = Id::from_u64(TAKER_ID_BASE + i);
                        let match_result = thread_price_level.match_order(
                            5, // Match 5 units each time
                            taker_id,
                            TimeInForce::Gtc,
                            TakerKind::Standard,
                            TimestampMs::new(now_ms()),
                            &thread_tx_id_gen,
                        );

                        let executed = match_result.executed_quantity().unwrap_or(Quantity::ZERO);
                        if executed.as_u64() > 0 {
                            successful_takers += 1;
                        }
                        fills += match_result.trades().len();

                        if i % 5 == 0 {
                            info!(
                                "Thread {} match result: executed={}, remaining={}, complete={}",
                                thread_id,
                                executed,
                                match_result.remaining_quantity(),
                                match_result.is_complete()
                            );
                        }

                        // Simulate some work
                        thread::sleep(Duration::from_millis(2));
                    }

                    info!(
                        "Thread {} (match): 20 calls, {} successful takers, {} fills",
                        thread_id, successful_takers, fills
                    );
                }
                Role::Cancel => {
                    let (mut cancelled, mut missing, mut errors) = (0u64, 0u64, 0u64);
                    for i in 0..30 {
                        // Try to cancel seeded orders; another canceller or
                        // the matcher may already have removed them.
                        let order_id = Id::from_u64(i);
                        match thread_price_level.update_order(OrderUpdate::Cancel { order_id }) {
                            Ok(Some(_)) => cancelled += 1,
                            Ok(None) => missing += 1,
                            Err(_) => errors += 1,
                        }

                        // Simulate some work
                        thread::sleep(Duration::from_millis(3));
                    }

                    info!(
                        "Thread {} (cancel): 30 attempted, {} cancelled, {} missing, {} errors",
                        thread_id, cancelled, missing, errors
                    );
                }
                Role::Resize => {
                    let (mut resized, mut missing, mut errors) = (0u64, 0u64, 0u64);
                    for i in 0..40 {
                        // Try to resize seeded orders
                        let order_id = Id::from_u64(100 + i);
                        match thread_price_level.update_order(OrderUpdate::UpdateQuantity {
                            order_id,
                            new_quantity: Quantity::new(20), // Update to quantity 20
                        }) {
                            Ok(Some(_)) => resized += 1,
                            Ok(None) => missing += 1,
                            Err(_) => errors += 1,
                        }

                        // Simulate some work
                        thread::sleep(Duration::from_millis(2));
                    }

                    info!(
                        "Thread {} (resize): 40 attempted, {} resized, {} missing, {} errors",
                        thread_id, resized, missing, errors
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

    match stats.time_since_last_execution(&WallClock) {
        Ok(Some(time_since)) => info!("Time since last execution: {} ms", time_since),
        Ok(None) => info!("No execution recorded yet"),
        Err(error) => info!("Time since last execution unavailable: {}", error),
    }
}

/// Example wall clock. The crate reads no clock itself: it converts the
/// `SystemTime` the application reads with checked arithmetic, so a pre-epoch
/// or unrepresentable reading is a typed error rather than a silent `0`.
struct WallClock;

impl UnixClock for WallClock {
    fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
        TimestampMs::try_from_system_time(std::time::SystemTime::now())
    }
}

// Helper function to set up initial orders
fn setup_initial_orders(price_level: &PriceLevel) {
    // Add 200 standard orders
    for i in 0..200 {
        let order = OrderType::Standard {
            id: Id::from_u64(i),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");
    }

    // Add some iceberg orders
    for i in 200..220 {
        let order = OrderType::IcebergOrder {
            id: Id::from_u64(i),
            price: Price::new(10000),
            visible_quantity: Quantity::new(5),
            hidden_quantity: Quantity::new(15),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");
    }

    // Add some reserve orders
    for i in 220..240 {
        let order = OrderType::ReserveOrder {
            id: Id::from_u64(i),
            price: Price::new(10000),
            visible_quantity: Quantity::new(5),
            hidden_quantity: Quantity::new(15),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000 + i),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(2),
            replenish_amount: NonZeroU64::new(5),
            auto_replenish: true,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("add_order should succeed");
    }
}

// Helper function to create different types of orders based on thread ID
fn create_order(thread_id: usize, order_id: u64) -> OrderType<()> {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Create different order types based on the thread ID
    match thread_id % 4 {
        0 => OrderType::Standard {
            id: Id::from_u64(order_id),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(current_time),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        },
        1 => OrderType::IcebergOrder {
            id: Id::from_u64(order_id),
            price: Price::new(10000),
            visible_quantity: Quantity::new(5),
            hidden_quantity: Quantity::new(15),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(current_time),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        },
        2 => OrderType::PostOnly {
            id: Id::from_u64(order_id),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(current_time),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        },
        _ => OrderType::ReserveOrder {
            id: Id::from_u64(order_id),
            price: Price::new(10000),
            visible_quantity: Quantity::new(5),
            hidden_quantity: Quantity::new(15),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(current_time),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(2),
            replenish_amount: NonZeroU64::new(5),
            auto_replenish: true,
            extra_fields: (),
        },
    }
}

// Helper function to print price level information
fn print_price_level_info(price_level: &PriceLevel) {
    info!("Price: {}", price_level.price());
    info!("Visible quantity: {}", price_level.visible_quantity());
    info!("Hidden quantity: {}", price_level.hidden_quantity());
    info!("Total quantity: {:?}", price_level.total_quantity());
    info!("Order count: {}", price_level.order_count());
}

// Current wall-clock time in milliseconds
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
