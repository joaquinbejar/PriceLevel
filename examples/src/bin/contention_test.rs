// examples/src/bin/contention_test.rs
//
// Contention patterns on ONE shared price level.
//
// `PriceLevel::match_order` supports a single logical matcher per level: two
// concurrent `match_order` calls on the same level must be serialized by the
// caller. In the read/write test only thread `MATCHER_THREAD_ID` ever calls
// `match_order`; every other thread uses admissions, updates, cancels and
// reads, which the level supports concurrently. The hot spot test does not
// match at all.
//
// This is a smoke demonstration, not a benchmark. "Attempted calls" counts
// every call whatever its outcome; the per-outcome breakdown (successful,
// rejected, missing-order, fills) is printed next to it and is the part that
// says what the level actually did. See "Performance Evidence" in the crate
// docs before quoting any number it prints.

use pricelevel::{
    Hash32, Id, OrderType, OrderUpdate, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce,
    TimestampMs, UuidGenerator, setup_logger,
};
use std::ops::AddAssign;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;
use uuid::Uuid;

// Test parameters
const THREAD_COUNT: usize = 16;
const TEST_DURATION_MS: u64 = 3000; // 3 seconds per test
/// The only thread allowed to call `match_order` on the shared level.
const MATCHER_THREAD_ID: usize = 0;
/// Read/write seed orders use ids `0..READ_WRITE_SEED_ORDER_COUNT`.
const READ_WRITE_SEED_ORDER_COUNT: u64 = 500;
/// Maker thread `t` in the read/write test uses ids
/// `(t + 1) * MAKER_ID_STRIDE + n`: disjoint from the seed range and from
/// every other thread.
const MAKER_ID_STRIDE: u64 = 1 << 32;
/// Taker ids live in a disjoint high range, so a taker id can never equal a
/// resting maker id (a self-fill, impossible for a real order).
const TAKER_ID_BASE: u64 = 1 << 60;

/// Per-thread outcome accounting.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    /// Every public call made, whatever its outcome.
    attempted: u64,
    reads: u64,
    adds_admitted: u64,
    adds_rejected: u64,
    match_calls: u64,
    successful_takers: u64,
    fills: u64,
    cancels_done: u64,
    updates_done: u64,
    /// Cancel / update calls whose target order was not resting.
    missing: u64,
    /// Cancel / update calls that returned an error.
    errors: u64,
}

impl AddAssign for Tally {
    fn add_assign(&mut self, o: Self) {
        self.attempted += o.attempted;
        self.reads += o.reads;
        self.adds_admitted += o.adds_admitted;
        self.adds_rejected += o.adds_rejected;
        self.match_calls += o.match_calls;
        self.successful_takers += o.successful_takers;
        self.fills += o.fills;
        self.cancels_done += o.cancels_done;
        self.updates_done += o.updates_done;
        self.missing += o.missing;
        self.errors += o.errors;
    }
}

impl Tally {
    fn record_update(
        &mut self,
        result: Result<Option<Arc<OrderType<()>>>, pricelevel::PriceLevelError>,
        is_cancel: bool,
    ) {
        match result {
            Ok(Some(_)) if is_cancel => self.cancels_done += 1,
            Ok(Some(_)) => self.updates_done += 1,
            Ok(None) => self.missing += 1,
            Err(_) => self.errors += 1,
        }
    }

    fn log(&self, elapsed: Duration) {
        let secs = elapsed.as_secs_f64();
        info!(
            "Attempted calls (all outcomes): {} in {:?} ({:.2}/s)",
            self.attempted,
            elapsed,
            self.attempted as f64 / secs
        );
        info!(
            "  reads {} | adds admitted {} rejected {} | cancels done {} | updates done {} | missing {} | errors {}",
            self.reads,
            self.adds_admitted,
            self.adds_rejected,
            self.cancels_done,
            self.updates_done,
            self.missing,
            self.errors
        );
        if self.match_calls > 0 {
            info!(
                "  match calls {} | successful takers {} | fills {}",
                self.match_calls, self.successful_takers, self.fills
            );
        }
    }
}

fn main() {
    setup_logger().expect("Failed to initialize logger");
    info!("PriceLevel Contention Pattern Test (smoke demo, not a benchmark)");
    info!("================================================================");

    // Run tests with different contention patterns
    test_hot_spot_contention();
    test_read_write_ratio();
}

/// Spawn `THREAD_COUNT` workers running `work(thread_id, level, running)`,
/// release them together, stop them after `TEST_DURATION_MS`, and return the
/// merged tally with the elapsed time.
fn run_workers<F>(price_level: &Arc<PriceLevel>, work: F) -> (Tally, Duration)
where
    F: Fn(usize, &PriceLevel, &AtomicBool) -> Tally + Send + Sync + 'static,
{
    let work = Arc::new(work);
    let running = Arc::new(AtomicBool::new(true));
    let barrier = Arc::new(Barrier::new(THREAD_COUNT + 1));

    let mut handles = Vec::with_capacity(THREAD_COUNT);
    for thread_id in 0..THREAD_COUNT {
        let thread_price_level = Arc::clone(price_level);
        let thread_barrier = Arc::clone(&barrier);
        let thread_running = Arc::clone(&running);
        let thread_work = Arc::clone(&work);
        handles.push(thread::spawn(move || {
            thread_barrier.wait();
            thread_work(thread_id, &thread_price_level, &thread_running)
        }));
    }

    let start_time = Instant::now();
    barrier.wait(); // Release all threads
    thread::sleep(Duration::from_millis(TEST_DURATION_MS));
    running.store(false, Ordering::Relaxed);

    let mut total = Tally::default();
    for handle in handles {
        total += handle.join().unwrap();
    }
    (total, start_time.elapsed())
}

// Test how different read/write ratios affect the level
fn test_read_write_ratio() {
    info!("\n[TEST] Read/Write Ratio");
    info!("----------------------");
    info!(
        "Only thread {} calls match_order; the others add, update, cancel and read.",
        MATCHER_THREAD_ID
    );

    let test_cases = [0u64, 25, 50, 75, 95]; // Percentage of read operations
    let mut results = Vec::with_capacity(test_cases.len());

    for read_pct in test_cases {
        info!("\nTesting with {}% read operations...", read_pct);

        // A fresh level per ratio, so ids never collide across phases
        let price_level = Arc::new(PriceLevel::new(10000));
        setup_orders(&price_level, READ_WRITE_SEED_ORDER_COUNT);

        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let tx_id_generator = UuidGenerator::new(namespace);

        let (tally, elapsed) = run_workers(&price_level, move |thread_id, level, running| {
            let mut tally = Tally::default();
            let mut local_counter: u64 = 0;
            let mut next_maker_id = (thread_id as u64 + 1) * MAKER_ID_STRIDE;
            let mut next_taker_id = TAKER_ID_BASE;

            while running.load(Ordering::Relaxed) {
                tally.attempted += 1;
                let is_read = (local_counter % 100) < read_pct;

                if is_read {
                    tally.reads += 1;
                    match local_counter % 3 {
                        0 => {
                            let _snapshot = level.snapshot();
                        }
                        1 => {
                            let _quantity = level.visible_quantity();
                        }
                        _ => {
                            // Two public calls: count both so `attempted` and
                            // `reads` stay per-call, not per-iteration.
                            tally.attempted += 1;
                            tally.reads += 1;
                            let _total = level.total_quantity();
                            let _count = level.order_count();
                        }
                    }
                } else {
                    match local_counter % 3 {
                        0 => {
                            // Add a new order with a thread-unique id
                            let order = create_standard_order(next_maker_id, 10000, 10);
                            next_maker_id += 1;
                            match level.add_order(order) {
                                Ok(_) => tally.adds_admitted += 1,
                                Err(_) => tally.adds_rejected += 1,
                            }
                        }
                        1 if thread_id == MATCHER_THREAD_ID => {
                            // The single logical matcher for this level
                            let taker_id = Id::from_u64(next_taker_id);
                            next_taker_id += 1;
                            let result = level.match_order(
                                5, // Match 5 units
                                taker_id,
                                TimeInForce::Gtc,
                                TakerKind::Standard,
                                TimestampMs::new(now_ms()),
                                &tx_id_generator,
                            );
                            tally.match_calls += 1;
                            if result
                                .executed_quantity()
                                .unwrap_or(Quantity::ZERO)
                                .as_u64()
                                > 0
                            {
                                tally.successful_takers += 1;
                            }
                            tally.fills += result.trades().len() as u64;
                        }
                        1 => {
                            // Non-matcher threads resize a seed order instead
                            let order_id =
                                Id::from_u64(local_counter % READ_WRITE_SEED_ORDER_COUNT);
                            tally.record_update(
                                level.update_order(OrderUpdate::UpdateQuantity {
                                    order_id,
                                    new_quantity: Quantity::new(15),
                                }),
                                false,
                            );
                        }
                        _ => {
                            // Cancel a seed order
                            let order_id =
                                Id::from_u64(local_counter % READ_WRITE_SEED_ORDER_COUNT);
                            tally.record_update(
                                level.update_order(OrderUpdate::Cancel { order_id }),
                                true,
                            );
                        }
                    }
                }

                local_counter += 1;
            }
            tally
        });

        tally.log(elapsed);
        results.push((read_pct, tally, elapsed));
    }

    info!("\nRead/Write Ratio Summary (attempted calls include rejected and missing):");
    info!(
        "Read % | Attempted/s | Adds admitted | Successful takers | Fills | Cancels done | Missing"
    );
    for (pct, t, elapsed) in &results {
        info!(
            "{:>5}% | {:>11.2} | {:>13} | {:>17} | {:>5} | {:>12} | {:>7}",
            pct,
            t.attempted as f64 / elapsed.as_secs_f64(),
            t.adds_admitted,
            t.successful_takers,
            t.fills,
            t.cancels_done,
            t.missing
        );
    }
}

// Test contention when multiple threads target the same "hot" orders. No
// thread matches here: the workload is admissions, cancels and resizes only.
fn test_hot_spot_contention() {
    info!("\n[TEST] Hot Spot Contention");
    info!("---------------------------");

    let test_cases = [0u64, 25, 50, 75, 100]; // Percentage of operations targeting hot spot
    let mut results = Vec::with_capacity(test_cases.len());

    for hot_spot_pct in test_cases {
        info!(
            "\nTesting with {}% operations targeting hot spot...",
            hot_spot_pct
        );

        // Pre-populate with 1000 orders (first 20 are the "hot spot")
        let price_level = Arc::new(PriceLevel::new(10000));
        setup_orders(&price_level, 1000);

        let (tally, elapsed) = run_workers(&price_level, move |thread_id, level, running| {
            let mut tally = Tally::default();
            let mut local_counter: u64 = 0;

            while running.load(Ordering::Relaxed) {
                tally.attempted += 1;
                let target_hot_spot = (local_counter % 100) < hot_spot_pct;
                let id_range = if target_hot_spot { 0..20 } else { 20..1000 };
                let order_idx = (thread_id as u64 + local_counter)
                    % (id_range.end - id_range.start)
                    + id_range.start;

                match local_counter % 3 {
                    0 => {
                        // Re-add an order under a contended id. A duplicate id
                        // is rejected (issue #113); that rejection is counted,
                        // not treated as a successful operation.
                        let order = create_standard_order(order_idx, 10000, 10);
                        match level.add_order(order) {
                            Ok(_) => tally.adds_admitted += 1,
                            Err(_) => tally.adds_rejected += 1,
                        }
                    }
                    1 => {
                        tally.record_update(
                            level.update_order(OrderUpdate::Cancel {
                                order_id: Id::from_u64(order_idx),
                            }),
                            true,
                        );
                    }
                    _ => {
                        tally.record_update(
                            level.update_order(OrderUpdate::UpdateQuantity {
                                order_id: Id::from_u64(order_idx),
                                new_quantity: Quantity::new(15),
                            }),
                            false,
                        );
                    }
                }

                local_counter += 1;
            }
            tally
        });

        tally.log(elapsed);
        results.push((hot_spot_pct, tally, elapsed));
    }

    info!("\nHot Spot Summary (attempted calls include rejected and missing):");
    info!(
        "Hot % | Attempted/s | Adds admitted | Adds rejected | Cancels done | Updates done | Missing"
    );
    for (pct, t, elapsed) in &results {
        info!(
            "{:>4}% | {:>11.2} | {:>13} | {:>13} | {:>12} | {:>12} | {:>7}",
            pct,
            t.attempted as f64 / elapsed.as_secs_f64(),
            t.adds_admitted,
            t.adds_rejected,
            t.cancels_done,
            t.updates_done,
            t.missing
        );
    }
}

// Seed `count` standard orders with ids `0..count`
fn setup_orders(price_level: &PriceLevel, count: u64) {
    for i in 0..count {
        let order = create_standard_order(i, 10000, 10);
        price_level
            .add_order(order)
            .expect("add_order should succeed");
    }
}

// Helper function to create a standard order
fn create_standard_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(now_ms()),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

// Current wall-clock time in milliseconds
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
