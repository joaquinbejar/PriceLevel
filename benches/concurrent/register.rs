use super::support::{
    WorkerOutcome, WorkerResult, classify_match, create_standard_order, run_timed, wait_for_go,
};
use criterion::{BenchmarkId, Criterion};
use pricelevel::{
    Hash32, Id, OrderType, OrderUpdate, PegReferenceType, Price, PriceLevel, PriceLevelError,
    Quantity, Side, TakerKind, TimeInForce, TimestampMs, UuidGenerator,
};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

/// A resting order's quantity in the matching workload below, sized so a
/// single order can satisfy exactly two takers before it is exhausted.
const MATCH_ORDER_QTY: u64 = 10;
/// A taker's quantity in the matching workload below. `MATCH_ORDER_QTY` is
/// exactly twice this, so seeding `iterations` resting orders always
/// supplies exactly twice the demand of `iterations` takers — liquidity
/// never runs dry mid-measurement regardless of how large `iterations`
/// grows (issue #141: the previous fixed 500-order seed degenerated into
/// mostly empty matches once Criterion's requested iteration count passed
/// it).
const MATCH_TAKER_QTY: u64 = 5;

/// Ids `0..MIXED_SEED_COUNT` are pre-populated as resting makers before
/// timing starts in [`measure_concurrent_mixed_operations`]. The cancel
/// branch there intentionally targets this same range (issue #160): only
/// the add branch's *new* maker ids must stay disjoint from it.
const MIXED_SEED_COUNT: u64 = 200;

pub fn register_benchmarks(c: &mut Criterion) {
    // Deterministic smoke checks, run unconditionally every time this
    // function is called — including once per group under `cargo test
    // --all-targets`, which is what originally caught the issue #160 hang.
    // They exercise exact operation accounting far below Criterion's actual
    // sample sizes, so a regression here fails fast instead of only
    // surfacing in a slow `cargo bench` run.
    run_smoke_checks();

    let mut group = c.benchmark_group("PriceLevel - Concurrent Operations");

    // Test with various thread counts
    for thread_count in [2, 4, 8, 16].iter() {
        group.bench_with_input(
            BenchmarkId::new("concurrent_add_standard_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    let (duration, _outcome) = measure_concurrent_operation(
                        thread_count,
                        iters,
                        |price_level, thread_id, i, iterations| {
                            // Disjoint per-thread id block sized by
                            // `iterations`, so any two threads' ranges never
                            // overlap regardless of how many iterations
                            // Criterion requests.
                            let base_id = thread_id as u64 * iterations + i;
                            let order = create_standard_order(base_id, 10000, 100);
                            price_level.add_order(order).map(|_| ())
                        },
                    )
                    .unwrap_or_else(|e| panic!("concurrent_add_standard_orders: {e}"));
                    duration
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_add_mixed_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    let (duration, _outcome) = measure_concurrent_operation(
                        thread_count,
                        iters,
                        |price_level, thread_id, i, iterations| {
                            let base_id = thread_id as u64 * iterations + i;
                            let order = match i % 5 {
                                0 => create_standard_order(base_id, 10000, 100),
                                1 => create_iceberg_order(base_id, 10000, 50, 150),
                                2 => create_post_only_order(base_id, 10000, 100),
                                3 => create_reserve_order(base_id, 10000, 50, 150, 10, true, None),
                                _ => create_pegged_order(base_id, 10000, 100),
                            };
                            price_level.add_order(order).map(|_| ())
                        },
                    )
                    .unwrap_or_else(|e| panic!("concurrent_add_mixed_orders: {e}"));
                    duration
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_match_standard_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    let (duration, _outcome) =
                        measure_concurrent_match_operation(thread_count, iters)
                            .unwrap_or_else(|e| panic!("concurrent_match_standard_orders: {e}"));
                    duration
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_mixed_operations", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    let (duration, _outcome) =
                        measure_concurrent_mixed_operations(thread_count, iters)
                            .unwrap_or_else(|e| panic!("concurrent_mixed_operations: {e}"));
                    duration
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("concurrent_cancel_orders", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter_custom(|iters| {
                    let (duration, _outcome) =
                        measure_concurrent_cancel_operation(thread_count, iters)
                            .unwrap_or_else(|e| panic!("concurrent_cancel_orders: {e}"));
                    duration
                });
            },
        );
    }

    group.finish();
}

/// Measures time for concurrent `add_order` calls sharing one [`PriceLevel`].
///
/// `operation` receives `(price_level, thread_id, i, iterations)` so it can
/// compute a per-thread-disjoint id from the actual requested iteration
/// count rather than a fixed constant.
fn measure_concurrent_operation<F>(
    thread_count: usize,
    iterations: u64,
    operation: F,
) -> Result<(Duration, WorkerOutcome), String>
where
    F: Fn(&PriceLevel, usize, u64, u64) -> Result<(), PriceLevelError> + Send + Sync + 'static,
{
    let price_level = Arc::new(PriceLevel::new(10000));
    let operation = Arc::new(operation);

    run_timed(thread_count, move |thread_id, ready, go| {
        let price_level = Arc::clone(&price_level);
        let operation = Arc::clone(&operation);
        thread::spawn(move || -> WorkerResult {
            wait_for_go(&ready, &go);

            let mut outcome = WorkerOutcome::default();
            for i in 0..iterations {
                operation(&price_level, thread_id, i, iterations)
                    .map_err(|e| format!("thread {thread_id} add failed at iteration {i}: {e}"))?;
                outcome.successful += 1;
                outcome.completed += 1;
            }
            Ok(outcome)
        })
    })
}

/// Measures time for concurrent matching, one independent, fully-seeded
/// [`PriceLevel`] per thread.
///
/// [`PriceLevel::match_order`] documents a single-logical-matcher-per-level
/// contract: concurrent `match_order` calls on the *same* level are the
/// caller's responsibility to serialize. A benchmark measuring per-thread
/// matcher throughput must not paper over that by calling `match_order`
/// from multiple threads on one shared level, so each thread gets its own
/// level here instead.
fn measure_concurrent_match_operation(
    thread_count: usize,
    iterations: u64,
) -> Result<(Duration, WorkerOutcome), String> {
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .map_err(|e| format!("bad transaction-id namespace uuid: {e}"))?;

    // Setup happens entirely outside the timed region: one level per
    // thread, each pre-seeded with `iterations` orders of `MATCH_ORDER_QTY`
    // — exactly twice the `iterations` takers' total demand — so every
    // match fully fills its taker.
    let levels: Vec<Arc<PriceLevel>> = (0..thread_count)
        .map(|_| Arc::new(setup_standard_orders(iterations.max(1), MATCH_ORDER_QTY)))
        .collect();
    let generators: Vec<Arc<UuidGenerator>> = (0..thread_count)
        .map(|_| Arc::new(UuidGenerator::new(namespace)))
        .collect();

    run_timed(thread_count, move |thread_id, ready, go| {
        let level = Arc::clone(&levels[thread_id]);
        let generator = Arc::clone(&generators[thread_id]);
        thread::spawn(move || -> WorkerResult {
            wait_for_go(&ready, &go);

            let mut outcome = WorkerOutcome::default();
            for i in 0..iterations {
                // Taker ids live strictly above this level's own seeded
                // maker range (`0..iterations`), so a taker can never
                // collide with (and self-match against) a resting maker.
                let taker_id = Id::from_u64(iterations + i);
                let result = level.match_order(
                    MATCH_TAKER_QTY,
                    taker_id,
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    TimestampMs::new(1_716_000_000_000),
                    &generator,
                );
                classify_match(&result, &mut outcome);
                outcome.completed += 1;
            }
            Ok(outcome)
        })
    })
}

/// Measures time for concurrent cancellation on one shared, pre-populated
/// [`PriceLevel`].
///
/// Every one of the `thread_count * iterations` cancellation targets is
/// created before timing starts, one per `(thread_id, i)` pair, so every
/// requested iteration actually cancels a real order — there is no cap on
/// how many iterations run (issue #141 removed a 100-op cap that silently
/// under-reported cost for any `iterations > 100`), and no cross-thread id
/// collision at any iteration count (the block width is `iterations`
/// itself, not a fixed constant, closing the same class of bug as #160).
fn measure_concurrent_cancel_operation(
    thread_count: usize,
    iterations: u64,
) -> Result<(Duration, WorkerOutcome), String> {
    let price_level = PriceLevel::new(10000);
    for thread_id in 0..thread_count {
        for i in 0..iterations {
            let order_id = thread_id as u64 * iterations + i;
            let order = create_standard_order(order_id, 10000, 10);
            price_level
                .add_order(order)
                .map_err(|e| format!("cancel fixture setup failed: {e}"))?;
        }
    }
    let price_level = Arc::new(price_level);

    run_timed(thread_count, move |thread_id, ready, go| {
        let price_level = Arc::clone(&price_level);
        thread::spawn(move || -> WorkerResult {
            wait_for_go(&ready, &go);

            let mut outcome = WorkerOutcome::default();
            for i in 0..iterations {
                let order_id = Id::from_u64(thread_id as u64 * iterations + i);
                match price_level.update_order(OrderUpdate::Cancel { order_id }) {
                    Ok(Some(_)) => outcome.successful += 1,
                    Ok(None) => outcome.missing += 1,
                    Err(e) => {
                        return Err(format!(
                            "thread {thread_id} cancel failed at iteration {i}: {e}"
                        ));
                    }
                }
                outcome.completed += 1;
            }
            Ok(outcome)
        })
    })
}

/// Measures time for mixed concurrent operations (add, match, cancel,
/// update) on one shared [`PriceLevel`].
///
/// Only `thread_id == 0` ever calls `match_order`, honoring the
/// single-matcher-per-level contract while every thread still concurrently
/// adds / cancels / updates against the same level (all documented-safe
/// concurrently with a matcher). Every id range below is disjoint at any
/// `iterations` value:
/// - `0..MIXED_SEED_COUNT`: seeded makers, pre-populated before timing.
/// - `MIXED_SEED_COUNT + thread_id * iterations + i`: this thread's new
///   maker ids (the add branch).
/// - `MIXED_SEED_COUNT + thread_count * iterations + i`: taker ids used by
///   the sole matcher thread.
///
/// Issue #160: the previous fixed `thread_id * 1_000_000 + i` new-maker
/// range could collide with the seeded `0..200` range for `thread_id == 0`
/// (`base_id == 0` on the very first add), panicking `add_order`'s
/// `.expect()` and hanging the whole run on the never-reached completion
/// barrier. Scaling every block by the actual `iterations` (instead of a
/// fixed constant) also closes the case where `iterations` exceeds the old
/// fixed stride.
fn measure_concurrent_mixed_operations(
    thread_count: usize,
    iterations: u64,
) -> Result<(Duration, WorkerOutcome), String> {
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .map_err(|e| format!("bad transaction-id namespace uuid: {e}"))?;
    let transaction_id_gen = Arc::new(UuidGenerator::new(namespace));

    let price_level = PriceLevel::new(10000);
    for i in 0..MIXED_SEED_COUNT {
        let order = create_standard_order(i, 10000, 10);
        price_level
            .add_order(order)
            .map_err(|e| format!("mixed-ops seed setup failed: {e}"))?;
    }
    let price_level = Arc::new(price_level);

    let new_maker_base = MIXED_SEED_COUNT;
    let taker_base = MIXED_SEED_COUNT + thread_count as u64 * iterations;

    run_timed(thread_count, move |thread_id, ready, go| {
        let price_level = Arc::clone(&price_level);
        let transaction_id_gen = Arc::clone(&transaction_id_gen);
        thread::spawn(move || -> WorkerResult {
            wait_for_go(&ready, &go);

            let mut outcome = WorkerOutcome::default();
            for i in 0..iterations {
                match i % 4 {
                    0 => {
                        // Add a new maker with a disjoint id.
                        let order_id = new_maker_base + thread_id as u64 * iterations + i;
                        let order = create_standard_order(order_id, 10000, 10);
                        price_level.add_order(order).map_err(|e| {
                            format!("thread {thread_id} add failed at iteration {i}: {e}")
                        })?;
                        outcome.successful += 1;
                    }
                    1 if thread_id == 0 => {
                        // The sole matcher for this shared level.
                        let taker_id = Id::from_u64(taker_base + i);
                        let result = price_level.match_order(
                            5,
                            taker_id,
                            TimeInForce::Gtc,
                            TakerKind::Standard,
                            TimestampMs::new(1_716_000_000_000),
                            &transaction_id_gen,
                        );
                        classify_match(&result, &mut outcome);
                    }
                    1 => {
                        // Non-matcher threads never call `match_order`
                        // concurrently with the designated matcher; a cheap
                        // concurrent-safe read keeps a comparable per-branch
                        // cost without violating the single-matcher
                        // contract.
                        let _ = price_level.visible_quantity();
                        let _ = price_level.hidden_quantity();
                        outcome.successful += 1;
                    }
                    2 => {
                        // Cancel one of the seeded makers. Intentionally
                        // revisits the same `0..MIXED_SEED_COUNT` range
                        // across threads/iterations (issue #160), so
                        // `Ok(None)` (already cancelled) is expected.
                        let order_id = Id::from_u64(i % MIXED_SEED_COUNT);
                        match price_level.update_order(OrderUpdate::Cancel { order_id }) {
                            Ok(Some(_)) => outcome.successful += 1,
                            Ok(None) => outcome.missing += 1,
                            Err(e) => {
                                return Err(format!(
                                    "thread {thread_id} cancel failed at iteration {i}: {e}"
                                ));
                            }
                        }
                    }
                    _ => {
                        // Update the quantity of THIS thread's own add from
                        // this same 4-iteration cycle (`i - 3`, not `i - 1`
                        // — issue #160's original code referenced the
                        // preceding cancel branch's index instead of the
                        // add branch's). `Ok(None)` is still possible if the
                        // sole matcher already consumed this maker.
                        let cycle_start = i - 3;
                        let order_id = Id::from_u64(
                            new_maker_base + thread_id as u64 * iterations + cycle_start,
                        );
                        match price_level.update_order(OrderUpdate::UpdateQuantity {
                            order_id,
                            new_quantity: Quantity::new(20),
                        }) {
                            Ok(Some(_)) => outcome.successful += 1,
                            Ok(None) => outcome.missing += 1,
                            Err(e) => {
                                return Err(format!(
                                    "thread {thread_id} update failed at iteration {i}: {e}"
                                ));
                            }
                        }
                    }
                }
                outcome.completed += 1;
            }
            Ok(outcome)
        })
    })
}

// Helper functions to create different types of orders for benchmarking

/// Create an iceberg order for testing
fn create_iceberg_order(id: u64, price: u128, visible: u64, hidden: u64) -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::from_u64(id),
        price: Price::new(price),
        visible_quantity: Quantity::new(visible),
        hidden_quantity: Quantity::new(hidden),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1616823000000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

/// Create a post-only order for testing
fn create_post_only_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
    OrderType::PostOnly {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1616823000000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

/// Create a reserve order for testing
fn create_reserve_order(
    id: u64,
    price: u128,
    visible: u64,
    hidden: u64,
    threshold: u64,
    auto_replenish: bool,
    replenish_amount: Option<u64>,
) -> OrderType<()> {
    OrderType::ReserveOrder {
        id: Id::from_u64(id),
        price: Price::new(price),
        visible_quantity: Quantity::new(visible),
        hidden_quantity: Quantity::new(hidden),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1616823000000),
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: Quantity::new(threshold),
        replenish_amount: replenish_amount.and_then(NonZeroU64::new),
        auto_replenish,
        extra_fields: (),
    }
}

/// Create a pegged order for testing
fn create_pegged_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
    OrderType::PeggedOrder {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1616823000000),
        time_in_force: TimeInForce::Gtc,
        reference_price_offset: -50,
        reference_price_type: PegReferenceType::BestAsk,
        extra_fields: (),
    }
}

/// Set up a price level with standard orders, ids `0..order_count`.
fn setup_standard_orders(order_count: u64, quantity: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);

    for i in 0..order_count {
        let order = OrderType::Standard {
            id: Id::from_u64(i),
            price: Price::new(10000),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level
            .add_order(order)
            .expect("benchmark fixture setup: add_order should succeed on a fresh level");
    }

    price_level
}

// ---------------------------------------------------------------------------
// Smoke checks (issue #141 acceptance: deterministic accounting at 1, 100,
// 101 and a larger iteration count; a failing worker reports an error
// instead of hanging). These run unconditionally from `register_benchmarks`
// so `cargo test --all-targets` exercises them without requiring a
// dedicated test harness for a `harness = false` bench target.
// ---------------------------------------------------------------------------

const SMOKE_ITERATIONS: [u64; 4] = [1, 100, 101, 1000];
const SMOKE_THREAD_COUNT: usize = 3;

fn run_smoke_checks() {
    for &iterations in &SMOKE_ITERATIONS {
        smoke_check_add_orders(iterations);
        smoke_check_match_orders(iterations);
        smoke_check_cancel_orders(iterations);
        smoke_check_mixed_operations(iterations);
    }
    smoke_check_failing_worker_reports_error_without_hanging();
}

fn smoke_check_add_orders(iterations: u64) {
    let (_duration, outcome) = measure_concurrent_operation(
        SMOKE_THREAD_COUNT,
        iterations,
        |price_level, thread_id, i, iters| {
            let base_id = thread_id as u64 * iters + i;
            price_level
                .add_order(create_standard_order(base_id, 10000, 100))
                .map(|_| ())
        },
    )
    .unwrap_or_else(|e| panic!("smoke_check_add_orders({iterations}): {e}"));

    let expected = SMOKE_THREAD_COUNT as u64 * iterations;
    assert_eq!(
        outcome.completed, expected,
        "smoke_check_add_orders({iterations}): expected exactly {expected} completed adds"
    );
    assert_eq!(
        outcome.successful, expected,
        "smoke_check_add_orders({iterations}): every disjoint-id add must succeed"
    );
}

fn smoke_check_match_orders(iterations: u64) {
    let (_duration, outcome) = measure_concurrent_match_operation(SMOKE_THREAD_COUNT, iterations)
        .unwrap_or_else(|e| panic!("smoke_check_match_orders({iterations}): {e}"));

    let expected = SMOKE_THREAD_COUNT as u64 * iterations;
    assert_eq!(
        outcome.completed, expected,
        "smoke_check_match_orders({iterations}): expected exactly {expected} completed matches"
    );
    assert_eq!(
        outcome.full_match, expected,
        "smoke_check_match_orders({iterations}): 2x-headroom liquidity must fully fill every taker"
    );
    assert_eq!(outcome.partial_match, 0);
    assert_eq!(outcome.empty_match, 0);
    assert_eq!(outcome.rejected, 0);
}

fn smoke_check_cancel_orders(iterations: u64) {
    let (_duration, outcome) = measure_concurrent_cancel_operation(SMOKE_THREAD_COUNT, iterations)
        .unwrap_or_else(|e| panic!("smoke_check_cancel_orders({iterations}): {e}"));

    let expected = SMOKE_THREAD_COUNT as u64 * iterations;
    assert_eq!(
        outcome.completed, expected,
        "smoke_check_cancel_orders({iterations}): expected exactly {expected} completed cancels \
         (issue #141: no 100-op cap)"
    );
    assert_eq!(
        outcome.successful, expected,
        "smoke_check_cancel_orders({iterations}): every pre-seeded, disjoint-id cancel must succeed"
    );
    assert_eq!(outcome.missing, 0);
}

fn smoke_check_mixed_operations(iterations: u64) {
    let (_duration, outcome) = measure_concurrent_mixed_operations(SMOKE_THREAD_COUNT, iterations)
        .unwrap_or_else(|e| panic!("smoke_check_mixed_operations({iterations}): {e}"));

    let expected = SMOKE_THREAD_COUNT as u64 * iterations;
    assert_eq!(
        outcome.completed, expected,
        "smoke_check_mixed_operations({iterations}): expected exactly {expected} completed ops \
         (issue #160: a duplicate-id panic must never truncate a worker's run)"
    );
    // The matcher thread can race a same-cycle update onto an id it just
    // consumed, so which bucket a given op lands in is not fully
    // deterministic — but every completed op must land in exactly one.
    let accounted = outcome.successful
        + outcome.missing
        + outcome.full_match
        + outcome.partial_match
        + outcome.empty_match
        + outcome.rejected;
    assert_eq!(
        accounted, expected,
        "smoke_check_mixed_operations({iterations}): every completed op must land in exactly one \
         outcome bucket"
    );
}

/// Proves a worker's reported failure surfaces as an `Err` from `run_timed`
/// rather than hanging the coordinator — the failure-propagation half of
/// issue #141 / #160. If this hung, the enclosing `cargo test --all-targets`
/// / `cargo bench` invocation would never return, so the process completing
/// at all is itself part of the proof.
fn smoke_check_failing_worker_reports_error_without_hanging() {
    let thread_count = 4;
    let failing_thread = 2usize;

    let result = run_timed(thread_count, move |thread_id, ready, go| {
        thread::spawn(move || -> WorkerResult {
            wait_for_go(&ready, &go);
            if thread_id == failing_thread {
                return Err(format!(
                    "thread {thread_id}: intentional smoke-check failure"
                ));
            }
            Ok(WorkerOutcome {
                completed: 1,
                successful: 1,
                ..WorkerOutcome::default()
            })
        })
    });

    match result {
        Err(message) => assert!(
            message.contains("intentional smoke-check failure"),
            "unexpected failure message: {message}"
        ),
        Ok(_) => panic!("expected the intentionally failing worker to be reported as an error"),
    }
}
