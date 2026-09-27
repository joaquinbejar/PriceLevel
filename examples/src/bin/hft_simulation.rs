// examples/src/bin/hft_simulation.rs
//
// A mixed maker / matcher / canceller workload on ONE shared price level.
//
// `PriceLevel::match_order` supports a single logical matcher per level: two
// concurrent `match_order` calls on the same level must be serialized by the
// caller. This simulation therefore runs exactly one matcher thread, next to
// many concurrent makers (`add_order`) and cancellers (`update_order`), which
// the level does support concurrently.
//
// This is a smoke demonstration, not a benchmark. It reports each metric
// separately (attempted calls, successful admissions / cancels, rejected or
// missing-order calls, successful takers, emitted fills) and never blends them
// into a single "operations per second" figure. See "Performance Evidence" in
// the crate docs before quoting any number it prints.

use pricelevel::{
    Hash32, Id, OrderType, OrderUpdate, PegReferenceType, Price, PriceLevel, Quantity, Side,
    TakerKind, TimeInForce, TimestampMs, UuidGenerator, setup_logger,
};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;
use uuid::Uuid;

// Simulation parameters
const PRICE: u128 = 10000;
const SIMULATION_DURATION_MS: u64 = 5000; // 5 seconds
const MAKER_THREAD_COUNT: usize = 10;
/// Exactly one: the level supports one logical matcher at a time.
const MATCHER_THREAD_COUNT: usize = 1;
const CANCELLER_THREAD_COUNT: usize = 10;
const TOTAL_THREAD_COUNT: usize =
    MAKER_THREAD_COUNT + MATCHER_THREAD_COUNT + CANCELLER_THREAD_COUNT;

/// Seed orders use ids `0..INITIAL_ORDER_COUNT`.
const INITIAL_ORDER_COUNT: u64 = 1000;
/// Maker thread `t` uses ids `(t + 1) * MAKER_ID_STRIDE + n`, clear of the seed
/// range and of every other maker thread.
const MAKER_ID_STRIDE: u64 = 1_000_000;
/// Taker ids live in a disjoint high range, so a taker id can never equal a
/// resting maker id (a self-fill, impossible for a real order).
const TAKER_ID_BASE: u64 = 1 << 40;

/// Per-thread maker accounting.
#[derive(Debug, Default, Clone, Copy)]
struct MakerTally {
    attempted: u64,
    admitted: u64,
    rejected: u64,
}

/// Matcher accounting: calls, takers that executed a non-zero quantity, and
/// the fills (trades) they emitted. One taker may emit many fills.
#[derive(Debug, Default, Clone, Copy)]
struct MatcherTally {
    calls: u64,
    successful_takers: u64,
    fills: u64,
    executed_quantity: u64,
}

/// Per-thread canceller accounting. A cancel of an order that was already
/// filled or cancelled is `missing`, not a success.
#[derive(Debug, Default, Clone, Copy)]
struct CancelTally {
    attempted: u64,
    cancelled: u64,
    missing: u64,
    errors: u64,
}

fn main() {
    setup_logger().expect("Failed to initialize logger");
    info!("High-Frequency Trading Simulation (smoke demo, not a benchmark)");
    info!("===============================================================");
    info!("Simulating price level at {}", PRICE);
    info!("Duration: {} ms", SIMULATION_DURATION_MS);
    info!("Maker threads: {}", MAKER_THREAD_COUNT);
    info!(
        "Matcher threads: {} (one logical matcher per level)",
        MATCHER_THREAD_COUNT
    );
    info!("Canceller threads: {}", CANCELLER_THREAD_COUNT);
    info!("\n");

    // Create a shared price level
    let price_level = Arc::new(PriceLevel::new(PRICE));

    // Trade id generator used by the single matcher
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let tx_id_generator = UuidGenerator::new(namespace);

    // Flag to signal when to stop the simulation
    let running = Arc::new(AtomicBool::new(true));

    // Synchronization barrier to ensure all threads start at the same time
    let barrier = Arc::new(Barrier::new(TOTAL_THREAD_COUNT + 1)); // +1 for main thread

    // Pre-populate with some orders to ensure there's something to match
    info!(
        "Seeding the price level with {} initial orders...",
        INITIAL_ORDER_COUNT
    );
    setup_initial_orders(&price_level, INITIAL_ORDER_COUNT);

    // Print initial state
    info!("Initial state:");
    print_price_level_info(&price_level);

    // Spawn maker threads (add orders)
    let mut maker_handles = Vec::with_capacity(MAKER_THREAD_COUNT);
    for thread_id in 0..MAKER_THREAD_COUNT {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_running = Arc::clone(&running);
        maker_handles.push(thread::spawn(move || {
            thread_barrier.wait();

            let mut tally = MakerTally::default();
            while thread_running.load(Ordering::Relaxed) {
                let order_id = (thread_id as u64 + 1) * MAKER_ID_STRIDE + tally.attempted;
                let order_type = match tally.attempted % 5 {
                    0 => create_standard_order(order_id),
                    1 => create_iceberg_order(order_id),
                    2 => create_post_only_order(order_id),
                    3 => create_reserve_order(order_id),
                    _ => create_pegged_order(order_id),
                };

                tally.attempted += 1;
                match thread_price_level.add_order(order_type) {
                    Ok(_) => tally.admitted += 1,
                    Err(_) => tally.rejected += 1,
                }

                // Simulate some think time
                thread::sleep(Duration::from_micros(50));
            }

            info!(
                "Maker thread {}: attempted {}, admitted {}, rejected {}",
                thread_id, tally.attempted, tally.admitted, tally.rejected
            );
            tally
        }));
    }

    // Spawn the single matcher thread. It owns the trade id generator: there is
    // no second matcher to share it with.
    let matcher_handle = {
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_running = Arc::clone(&running);
        thread::spawn(move || {
            thread_barrier.wait();

            let mut tally = MatcherTally::default();
            while thread_running.load(Ordering::Relaxed) {
                let taker_id = Id::from_u64(TAKER_ID_BASE + tally.calls);
                let quantity = (tally.calls % 5) + 1; // Match 1-5 units
                tally.calls += 1;

                let result = thread_price_level.match_order(
                    quantity,
                    taker_id,
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    TimestampMs::new(get_current_timestamp()),
                    &tx_id_generator,
                );

                let executed = result
                    .executed_quantity()
                    .unwrap_or(Quantity::ZERO)
                    .as_u64();
                if executed > 0 {
                    tally.successful_takers += 1;
                    tally.executed_quantity += executed;
                }
                tally.fills += result.trades().len() as u64;

                // Simulate some think time
                thread::sleep(Duration::from_micros(100));
            }

            info!(
                "Matcher thread: {} calls, {} successful takers, {} fills, {} units executed",
                tally.calls, tally.successful_takers, tally.fills, tally.executed_quantity
            );
            tally
        })
    };

    // Spawn canceller threads. Canceller `i` walks the id sequence of maker
    // thread `i % MAKER_THREAD_COUNT`, so it targets orders that really were
    // submitted; ids already filled, already cancelled or not yet admitted are
    // counted as `missing`.
    let mut canceller_handles = Vec::with_capacity(CANCELLER_THREAD_COUNT);
    for i in 0..CANCELLER_THREAD_COUNT {
        let target_maker = (i % MAKER_THREAD_COUNT) as u64;
        let thread_price_level = Arc::clone(&price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_running = Arc::clone(&running);
        canceller_handles.push(thread::spawn(move || {
            thread_barrier.wait();

            let mut tally = CancelTally::default();
            while thread_running.load(Ordering::Relaxed) {
                let order_id = Id::from_u64((target_maker + 1) * MAKER_ID_STRIDE + tally.attempted);
                tally.attempted += 1;

                match thread_price_level.update_order(OrderUpdate::Cancel { order_id }) {
                    Ok(Some(_)) => tally.cancelled += 1,
                    Ok(None) => tally.missing += 1,
                    Err(_) => tally.errors += 1,
                }

                // Simulate some think time
                thread::sleep(Duration::from_micros(200));
            }

            info!(
                "Canceller thread {}: attempted {}, cancelled {}, missing {}, errors {}",
                i, tally.attempted, tally.cancelled, tally.missing, tally.errors
            );
            tally
        }));
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

    // Wait for all threads to complete and aggregate their tallies
    let mut makers = MakerTally::default();
    for handle in maker_handles {
        let t = handle.join().unwrap();
        makers.attempted += t.attempted;
        makers.admitted += t.admitted;
        makers.rejected += t.rejected;
    }
    let matcher = matcher_handle.join().unwrap();
    let mut cancels = CancelTally::default();
    for handle in canceller_handles {
        let t = handle.join().unwrap();
        cancels.attempted += t.attempted;
        cancels.cancelled += t.cancelled;
        cancels.missing += t.missing;
        cancels.errors += t.errors;
    }

    let elapsed = start_time.elapsed();
    info!("\nSimulation completed in {:?}", elapsed);
    let secs = elapsed.as_secs_f64();
    let rate = |n: u64| n as f64 / secs;

    // Each metric is reported on its own; they are different quantities and
    // are deliberately not summed into one total.
    info!("\nOperation accounting (wall-clock rates, smoke run only):");
    info!("--------------------------------------------------------");
    info!(
        "add_order: {} attempted, {} admitted ({:.2}/s), {} rejected",
        makers.attempted,
        makers.admitted,
        rate(makers.admitted),
        makers.rejected
    );
    info!(
        "match_order: {} calls, {} successful takers ({:.2}/s), {} fills emitted ({:.2}/s)",
        matcher.calls,
        matcher.successful_takers,
        rate(matcher.successful_takers),
        matcher.fills,
        rate(matcher.fills)
    );
    info!(
        "cancel: {} attempted, {} cancelled ({:.2}/s), {} missing, {} errors",
        cancels.attempted,
        cancels.cancelled,
        rate(cancels.cancelled),
        cancels.missing,
        cancels.errors
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

        price_level
            .add_order(order)
            .expect("add_order should succeed");
    }
}

// Helper function to create a standard order
fn create_standard_order(id: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        quantity: Quantity::new(10),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(get_current_timestamp()),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

// Helper function to create an iceberg order
fn create_iceberg_order(id: u64) -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        visible_quantity: Quantity::new(5),
        hidden_quantity: Quantity::new(15),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(get_current_timestamp()),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

// Helper function to create a post-only order
fn create_post_only_order(id: u64) -> OrderType<()> {
    OrderType::PostOnly {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        quantity: Quantity::new(8),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(get_current_timestamp()),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

// Helper function to create a reserve order
fn create_reserve_order(id: u64) -> OrderType<()> {
    OrderType::ReserveOrder {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        visible_quantity: Quantity::new(5),
        hidden_quantity: Quantity::new(15),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(get_current_timestamp()),
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: Quantity::new(2),
        replenish_amount: NonZeroU64::new(5),
        auto_replenish: true,
        extra_fields: (),
    }
}

// Helper function to create a pegged order
fn create_pegged_order(id: u64) -> OrderType<()> {
    OrderType::PeggedOrder {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        quantity: Quantity::new(10),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(get_current_timestamp()),
        time_in_force: TimeInForce::Gtc,
        reference_price_offset: -50,
        reference_price_type: PegReferenceType::BestAsk,
        extra_fields: (),
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
    info!("Total quantity: {:?}", price_level.total_quantity());
    info!("Order count: {}", price_level.order_count());
    info!("Statistics: {}", price_level.stats());
}
