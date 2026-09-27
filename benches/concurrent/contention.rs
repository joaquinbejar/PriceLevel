use super::support::{
    WorkerOutcome, WorkerResult, classify_match, create_standard_order, run_timed, wait_for_go,
};
use criterion::{BenchmarkId, Criterion, Throughput};
use pricelevel::{
    Id, OrderUpdate, PriceLevel, PriceLevelError, Quantity, TakerKind, TimeInForce, TimestampMs,
    UuidGenerator,
};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

/// Both contention workloads below run at a fixed 8 threads.
const CONTENTION_THREAD_COUNT: usize = 8;

/// Register benchmarks that test different contention patterns
pub fn register_contention_benchmarks(c: &mut Criterion) {
    // See `register.rs`'s `run_smoke_checks` for why these run
    // unconditionally: they exercise the same id-collision and
    // single-matcher hazards this module was previously not wired into the
    // real `criterion_main!` entry point to catch (issue #141).
    run_contention_smoke_checks();

    let mut group = c.benchmark_group("PriceLevel - Contention Patterns");

    // Both workloads below run `run_timed(CONTENTION_THREAD_COUNT, ...)`:
    // one Criterion "iteration" is one round in which every one of the 8
    // workers performs exactly one operation concurrently (see
    // `support::run_timed`), so the returned Duration covers
    // `CONTENTION_THREAD_COUNT * iters` total operations, not `iters`.
    // Declaring the throughput explicitly makes Criterion report the
    // per-operation rate instead of leaving "1 iteration" ambiguous between
    // "one op" and "one round of 8 ops" (issue #141 review).
    group.throughput(Throughput::Elements(CONTENTION_THREAD_COUNT as u64));

    // Test with different read/write ratios
    for read_ratio in [0, 25, 50, 75, 95].iter() {
        group.bench_with_input(
            BenchmarkId::new("read_write_ratio", read_ratio),
            read_ratio,
            |b, &read_ratio| {
                b.iter_custom(|iters| {
                    let (duration, _outcome) =
                        measure_read_write_contention(CONTENTION_THREAD_COUNT, iters, read_ratio)
                            .unwrap_or_else(|e| panic!("read_write_ratio({read_ratio}): {e}"));
                    duration
                });
            },
        );
    }

    // Test with different access patterns (hot spot vs distributed)
    for hot_spot_percentage in [0, 20, 50, 80, 100].iter() {
        group.bench_with_input(
            BenchmarkId::new("hot_spot_contention", hot_spot_percentage),
            hot_spot_percentage,
            |b, &hot_spot_percentage| {
                b.iter_custom(|iters| {
                    let (duration, _outcome) = measure_hot_spot_contention(
                        CONTENTION_THREAD_COUNT,
                        iters,
                        hot_spot_percentage,
                    )
                    .unwrap_or_else(|e| panic!("hot_spot_contention({hot_spot_percentage}): {e}"));
                    duration
                });
            },
        );
    }

    group.finish();
}

/// Fixed resting-order pool size for [`measure_read_write_contention`],
/// deliberately independent of the requested iteration count.
///
/// `add` and `cancel` both recycle ids within `0..SEED_DEPTH` (the same
/// replace-in-place pattern [`measure_hot_spot_contention`] already uses)
/// instead of each `add` growing the book by one order: growing the book
/// with `iterations` made `snapshot()`'s O(depth) traversal cost — and the
/// resting depth available to the sole matcher — drift with whatever
/// iteration count Criterion's calibration happened to pick, so "time /
/// iters" was not a stable per-operation cost and changed with the
/// warmup/measurement-time settings (issue #141 review). A fixed pool seeded
/// once, outside the timed region, keeps every read and write at a constant
/// cost regardless of `iterations`.
const SEED_DEPTH: u64 = 500;

/// Measures time for operations with different read/write ratios on one
/// shared [`PriceLevel`].
///
/// Only `thread_id == 0` calls `match_order` (the single-matcher-per-level
/// contract — see `register.rs`'s `measure_concurrent_mixed_operations` doc
/// comment); every other thread performs a genuine concurrency-safe WRITE
/// instead (never a read substitute) so the advertised `read_ratio` matches
/// the real read/write mix — a read substitute here would silently inflate
/// the actual read fraction above the labeled one (issue #141 review).
/// `read_ratio` = percentage of read operations (0-100).
fn measure_read_write_contention(
    thread_count: usize,
    iterations: u64,
    read_ratio: usize,
) -> Result<(Duration, WorkerOutcome), String> {
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .map_err(|e| format!("bad transaction-id namespace uuid: {e}"))?;
    let transaction_id_gen = Arc::new(UuidGenerator::new(namespace));

    let price_level = PriceLevel::new(10000);
    for i in 0..SEED_DEPTH {
        let order = create_standard_order(i, 10000, 10);
        price_level
            .add_order(order)
            .map_err(|e| format!("read/write contention seed setup failed: {e}"))?;
    }
    let price_level = Arc::new(price_level);

    // Only the sole matcher thread uses this block; disjoint from the fixed
    // `0..SEED_DEPTH` pool regardless of `iterations`.
    let taker_base = SEED_DEPTH;

    run_timed(thread_count, move |thread_id, ready, go| {
        let price_level = Arc::clone(&price_level);
        let transaction_id_gen = Arc::clone(&transaction_id_gen);
        thread::spawn(move || -> WorkerResult {
            wait_for_go(&ready, &go);

            let mut outcome = WorkerOutcome::default();
            for i in 0..iterations {
                let is_read = (i as usize % 100) < read_ratio;

                if is_read {
                    if i % 2 == 0 {
                        let _ = price_level.snapshot();
                    } else {
                        let _ = price_level.visible_quantity();
                        let _ = price_level.hidden_quantity();
                        let _ = price_level.order_count();
                    }
                    outcome.successful += 1;
                } else {
                    // Recycle a slot in the fixed pool rather than growing
                    // it; another thread may already be mid-cycle on the
                    // same slot, so a lost `add_order` race is an EXPECTED
                    // duplicate, not a failure.
                    let order_idx = (thread_id as u64 + i) % SEED_DEPTH;

                    match i % 3 {
                        0 => {
                            let order = create_standard_order(order_idx, 10000, 10);
                            match price_level.add_order(order) {
                                Ok(_) => outcome.successful += 1,
                                Err(PriceLevelError::DuplicateOrderId(_)) => {
                                    outcome.rejected += 1;
                                }
                                Err(e) => {
                                    return Err(format!(
                                        "thread {thread_id} add failed at iteration {i}: {e}"
                                    ));
                                }
                            }
                        }
                        1 if thread_id == 0 => {
                            let taker_id = Id::from_u64(taker_base + i);
                            let result = price_level.match_order(
                                2,
                                taker_id,
                                TimeInForce::Gtc,
                                TakerKind::Standard,
                                TimestampMs::new(1_716_000_000_000),
                                &transaction_id_gen,
                            );
                            classify_match(&result, &mut outcome);
                        }
                        // Covers both `i % 3 == 1` on a non-matcher thread
                        // (never calls `match_order`) and `i % 3 == 2`: a
                        // cancel is an equally cheap, genuinely
                        // concurrency-safe write — a read substitute here
                        // would understate the real write fraction below
                        // the labeled `read_ratio` (issue #141 review).
                        _ => match price_level.update_order(OrderUpdate::Cancel {
                            order_id: Id::from_u64(order_idx),
                        }) {
                            Ok(Some(_)) => outcome.successful += 1,
                            Ok(None) => outcome.missing += 1,
                            Err(e) => {
                                return Err(format!(
                                    "thread {thread_id} cancel failed at iteration {i}: {e}"
                                ));
                            }
                        },
                    }
                }
                outcome.completed += 1;
            }
            Ok(outcome)
        })
    })
}

/// Measures time for operations with different hot-spot access patterns on
/// one shared [`PriceLevel`].
///
/// The hot (`0..20`) and cold (`20..1000`) id ranges are intentionally
/// shared across every thread — that repeated collision IS the contention
/// under test. `add_order` therefore legitimately loses a race for an id
/// another thread just re-added first; that is classified as `rejected`,
/// not propagated as a worker failure. Only `thread_id == 0` calls
/// `match_order`, for the same single-matcher-per-level reason as
/// `measure_read_write_contention`; every other thread performs a
/// concurrency-safe cancel instead of a read substitute, for the same
/// real-mix reason.
/// `hot_spot_percentage` = percentage of operations targeting the hot range
/// (0-100).
fn measure_hot_spot_contention(
    thread_count: usize,
    iterations: u64,
    hot_spot_percentage: usize,
) -> Result<(Duration, WorkerOutcome), String> {
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"example.com");
    let transaction_id_gen = Arc::new(UuidGenerator::new(namespace));

    const HOT_COUNT: u64 = 20;
    const TOTAL_SEED_COUNT: u64 = 1000;

    let price_level = PriceLevel::new(10000);
    for i in 0..TOTAL_SEED_COUNT {
        let order = create_standard_order(i, 10000, 10);
        price_level
            .add_order(order)
            .map_err(|e| format!("hot-spot contention seed setup failed: {e}"))?;
    }
    let price_level = Arc::new(price_level);

    // Only the sole matcher thread uses this block; sized by `iterations`
    // so it stays disjoint from the seeded `0..TOTAL_SEED_COUNT` range at
    // any iteration count.
    let taker_base = TOTAL_SEED_COUNT;

    run_timed(thread_count, move |thread_id, ready, go| {
        let price_level = Arc::clone(&price_level);
        let transaction_id_gen = Arc::clone(&transaction_id_gen);
        thread::spawn(move || -> WorkerResult {
            wait_for_go(&ready, &go);

            let mut outcome = WorkerOutcome::default();
            for i in 0..iterations {
                let target_hot_spot = (i as usize % 100) < hot_spot_percentage;
                let id_range = if target_hot_spot {
                    0..HOT_COUNT
                } else {
                    HOT_COUNT..TOTAL_SEED_COUNT
                };
                let order_idx =
                    (thread_id as u64 + i) % (id_range.end - id_range.start) + id_range.start;

                match i % 4 {
                    0 => match price_level.update_order(OrderUpdate::Cancel {
                        order_id: Id::from_u64(order_idx),
                    }) {
                        Ok(Some(_)) => outcome.successful += 1,
                        Ok(None) => outcome.missing += 1,
                        Err(e) => {
                            return Err(format!(
                                "thread {thread_id} hot-spot cancel failed at iteration {i}: {e}"
                            ));
                        }
                    },
                    1 => {
                        // Re-add to replace a possibly-cancelled order at
                        // this hot id. Another thread may have already won
                        // the race and re-added it first — an EXPECTED
                        // duplicate, not a failure.
                        let order = create_standard_order(order_idx, 10000, 10);
                        match price_level.add_order(order) {
                            Ok(_) => outcome.successful += 1,
                            Err(PriceLevelError::DuplicateOrderId(_)) => outcome.rejected += 1,
                            Err(e) => {
                                return Err(format!(
                                    "thread {thread_id} hot-spot add failed at iteration {i}: {e}"
                                ));
                            }
                        }
                    }
                    2 => match price_level.update_order(OrderUpdate::UpdateQuantity {
                        order_id: Id::from_u64(order_idx),
                        new_quantity: Quantity::new(15),
                    }) {
                        Ok(Some(_)) => outcome.successful += 1,
                        Ok(None) => outcome.missing += 1,
                        Err(e) => {
                            return Err(format!(
                                "thread {thread_id} hot-spot update failed at iteration {i}: {e}"
                            ));
                        }
                    },
                    _ if thread_id == 0 => {
                        let taker_id = Id::from_u64(taker_base + i);
                        let result = price_level.match_order(
                            1,
                            taker_id,
                            TimeInForce::Gtc,
                            TakerKind::Standard,
                            TimestampMs::new(1_716_000_000_000),
                            &transaction_id_gen,
                        );
                        classify_match(&result, &mut outcome);
                    }
                    _ => {
                        // Non-matcher thread: cancel is an equally cheap,
                        // genuinely concurrency-safe write — a read
                        // substitute here would understate the real write
                        // fraction the same way it would in
                        // `measure_read_write_contention` (issue #141
                        // review).
                        match price_level.update_order(OrderUpdate::Cancel {
                            order_id: Id::from_u64(order_idx),
                        }) {
                            Ok(Some(_)) => outcome.successful += 1,
                            Ok(None) => outcome.missing += 1,
                            Err(e) => {
                                return Err(format!(
                                    "thread {thread_id} hot-spot cancel (non-matcher) failed at \
                                     iteration {i}: {e}"
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

// ---------------------------------------------------------------------------
// Smoke checks — see `register.rs`'s `run_smoke_checks` for the rationale.
// ---------------------------------------------------------------------------

const SMOKE_ITERATIONS: [u64; 4] = [1, 100, 101, 1000];
const SMOKE_THREAD_COUNT: usize = 4;

fn run_contention_smoke_checks() {
    for &iterations in &SMOKE_ITERATIONS {
        smoke_check_read_write_contention(iterations);
        smoke_check_hot_spot_contention(iterations);
    }
}

fn smoke_check_read_write_contention(iterations: u64) {
    let (_duration, outcome) = measure_read_write_contention(SMOKE_THREAD_COUNT, iterations, 50)
        .unwrap_or_else(|e| panic!("smoke_check_read_write_contention({iterations}): {e}"));

    let expected = SMOKE_THREAD_COUNT as u64 * iterations;
    assert_eq!(
        outcome.completed, expected,
        "smoke_check_read_write_contention({iterations}): expected exactly {expected} completed \
         ops — {expected} is thread_count * iterations, the actual op count behind the Duration \
         `run_timed` returns for this single Criterion iteration batch"
    );
    let accounted = outcome.successful
        + outcome.missing
        + outcome.full_match
        + outcome.partial_match
        + outcome.empty_match
        + outcome.rejected;
    assert_eq!(
        accounted, expected,
        "smoke_check_read_write_contention({iterations}): every completed op must land in exactly \
         one outcome bucket — this must keep holding now that both the add and non-matcher write \
         branches recycle the fixed SEED_DEPTH pool and can be legitimately rejected as duplicates"
    );
}

fn smoke_check_hot_spot_contention(iterations: u64) {
    let (_duration, outcome) = measure_hot_spot_contention(SMOKE_THREAD_COUNT, iterations, 50)
        .unwrap_or_else(|e| panic!("smoke_check_hot_spot_contention({iterations}): {e}"));

    let expected = SMOKE_THREAD_COUNT as u64 * iterations;
    assert_eq!(
        outcome.completed, expected,
        "smoke_check_hot_spot_contention({iterations}): expected exactly {expected} completed ops"
    );
    let accounted = outcome.successful
        + outcome.missing
        + outcome.full_match
        + outcome.partial_match
        + outcome.empty_match
        + outcome.rejected;
    assert_eq!(
        accounted, expected,
        "smoke_check_hot_spot_contention({iterations}): every completed op must land in exactly \
         one outcome bucket"
    );
}
