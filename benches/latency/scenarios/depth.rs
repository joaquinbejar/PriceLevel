// benches/latency/scenarios/depth.rs
//! Depth, scaled-quantity and scaled-price sweeps.
//!
//! The 10,000 / 100,000-order depths are opt-in (`PL_LATENCY_LARGE_DEPTHS=1`)
//! per issue #142 ("make the largest depths opt-in via env var if runtime is
//! long") — building and later dropping a 100,000-order level is itself
//! non-trivial wall-clock and memory cost, and that cost is fixture
//! construction, deliberately excluded from every timed sample here.
//!
//! Every `add_order` sweep below adds one order and then, in an untimed
//! teardown, immediately cancels that exact order before the next sample —
//! restoring the level to precisely its declared depth after every single
//! sample, not just at fixture setup. An earlier version of this harness
//! never canceled what it added, so the level grew by one order per warmup
//! and measured sample and the depth a scenario reported (e.g.
//! `depth_sweep_add@100`) no longer matched what `add_order` was actually
//! measured against by the end of the run (issue #142 review finding 2).

use crate::config::Config;
use crate::fixtures::{self, EXECUTION_TIMESTAMP_MS, TAKER_ID_BASE};
use crate::report::ScenarioReport;
use crate::timing::{measure, measure_with_teardown, warmup};
use pricelevel::prelude::*;

/// Depth sweep points always run.
const BASE_DEPTHS: [u64; 2] = [100, 1_000];
/// Depth sweep points that additionally run when `large_depths` is set.
const LARGE_DEPTHS: [u64; 2] = [10_000, 100_000];

/// A depth at or above this threshold uses a reduced sample count (fixture
/// build cost and steady-state memory both scale with depth; this keeps a
/// `PL_LATENCY_LARGE_DEPTHS=1` run bounded).
const LARGE_DEPTH_THRESHOLD: u64 = 10_000;
/// Reduced sample count used at or above [`LARGE_DEPTH_THRESHOLD`].
const LARGE_DEPTH_SAMPLES_CAP: usize = 500;

fn depths_for(config: &Config) -> Vec<u64> {
    let mut depths = BASE_DEPTHS.to_vec();
    if config.large_depths {
        depths.extend_from_slice(&LARGE_DEPTHS);
    }
    depths
}

fn samples_for_depth(config: &Config, depth: u64) -> usize {
    if depth >= LARGE_DEPTH_THRESHOLD {
        config.samples.min(LARGE_DEPTH_SAMPLES_CAP)
    } else {
        config.samples
    }
}

/// Runs every depth / scaled-quantity / scaled-price sweep and returns one
/// report per swept point.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    let mut reports = Vec::new();
    for depth in depths_for(config) {
        reports.push(add_at_depth(config, depth));
    }
    for depth in depths_for(config) {
        reports.push(small_taker_at_depth(config, depth));
    }
    reports.extend(scaled_quantity(config));
    reports.extend(scaled_price(config));
    reports
}

/// Cancels the fixed extra id every `add_at_depth` / `scaled_quantity` /
/// `scaled_price` sample uses, restoring the level to its declared resting
/// count. Shared so every call site restores depth identically.
fn cancel_extra(level: &PriceLevel, extra_id: u64, context: &str) {
    level
        .update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(extra_id),
        })
        .expect(context)
        .expect("teardown cancel must find the order this same iteration just added");
}

/// Isolated `add_order` into a level holding exactly `depth` resting orders
/// on the same side, at every sample — not merely at setup. Each sample
/// adds one order at a fixed id just past the seeded range, then an untimed
/// teardown cancels that exact id before the next sample, so `order_count()`
/// is `depth` both before and after the whole measured loop (issue #142
/// review finding 2).
fn add_at_depth(config: &Config, depth: u64) -> ScenarioReport {
    let samples = samples_for_depth(config, depth);
    let level = fixtures::seeded_standard_level(depth, Side::Buy, 10);
    let extra_id = depth;

    warmup(config.warmup, |_| {
        level
            .add_order(fixtures::standard_order(
                extra_id,
                Side::Buy,
                10,
                TimeInForce::Gtc,
            ))
            .expect("depth sweep add: warmup add_order must succeed");
        cancel_extra(
            &level,
            extra_id,
            "depth sweep add: warmup teardown cancel must succeed",
        );
    });
    assert_eq!(
        level.order_count(),
        depth as usize,
        "add_at_depth({depth}): depth must already be restored before measurement starts"
    );

    let (durations_ns, results) = measure_with_teardown(
        samples,
        |_| {
            level.add_order(fixtures::standard_order(
                extra_id,
                Side::Buy,
                10,
                TimeInForce::Gtc,
            ))
        },
        |_, _outcome| {
            cancel_extra(
                &level,
                extra_id,
                "depth sweep add: teardown cancel must succeed",
            )
        },
    );

    let succeeded = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        succeeded, samples,
        "add_at_depth({depth}): every add must succeed"
    );
    assert_eq!(
        level.order_count(),
        depth as usize,
        "add_at_depth({depth}): depth must be restored to exactly its declared value after \
         every timed sample"
    );

    ScenarioReport::from_samples(
        format!("depth_sweep_add@{depth}"),
        "depth",
        depth,
        "PriceLevel::add_order — swept resting depth, restored after every sample",
        durations_ns,
        format!("{succeeded}/{samples} succeeded"),
    )
}

/// A small (`qty=1`) taker crossing a level with `depth` resting orders,
/// each sized so the front maker alone can absorb every measured sample
/// without running dry — isolates the cost of matching against a structure
/// with a large number of ENTRIES, not the cost of walking many of them.
/// Matching only ever shrinks the front maker's own quantity, never removes
/// or adds an order, so `order_count()` stays exactly `depth` throughout
/// without any teardown.
fn small_taker_at_depth(config: &Config, depth: u64) -> ScenarioReport {
    let samples = samples_for_depth(config, depth);
    let total_calls = (config.warmup + samples) as u64;
    // Each resting maker can alone absorb every call; the queue nonetheless
    // has `depth` distinct entries backing it (that IS the large-depth cost
    // under test).
    let quantity_each = total_calls + 1;
    let level = fixtures::seeded_standard_level(depth, Side::Sell, quantity_each);
    let generator = fixtures::trade_id_generator();

    warmup(config.warmup, |i| {
        level.match_order(
            1,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    assert_eq!(
        level.order_count(),
        depth as usize,
        "small_taker_at_depth({depth}): matching a front maker's quantity down must never \
         change order_count()"
    );
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure(samples, |i| {
        level.match_order(
            1,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::Filled)
        .count();
    assert_eq!(
        filled, samples,
        "small_taker_at_depth({depth}): every 1-unit taker must fully fill against the front maker"
    );
    assert_eq!(
        level.order_count(),
        depth as usize,
        "small_taker_at_depth({depth}): depth must be unchanged after the measured loop"
    );
    fixtures::assert_stats_healthy(
        &level,
        samples as u64,
        &format!("small_taker_at_depth({depth})"),
    );

    ScenarioReport::from_samples(
        format!("depth_sweep_small_taker@{depth}"),
        "depth",
        depth,
        "PriceLevel::match_order — 1-unit taker, swept resting depth",
        durations_ns,
        format!("{filled}/{samples} Filled"),
    )
}

/// Sweeps the *order's own* quantity magnitude (not the level's depth) for
/// isolated `add_order`, to check whether `u64` quantity magnitude itself
/// affects admission cost (it should not — `Quantity` arithmetic is
/// `O(1)` — but the issue asks for scaled-quantity coverage explicitly).
/// Same add-then-cancel depth restoration as [`add_at_depth`].
fn scaled_quantity(config: &Config) -> Vec<ScenarioReport> {
    const QUANTITIES: [u64; 3] = [1, 10_000, 1_000_000_000];
    const SEED_DEPTH: u64 = 100;
    let extra_id = SEED_DEPTH;
    let mut reports = Vec::new();
    for &qty in &QUANTITIES {
        let level = fixtures::seeded_standard_level(SEED_DEPTH, Side::Buy, qty.max(1));

        warmup(config.warmup, |_| {
            level
                .add_order(fixtures::standard_order(
                    extra_id,
                    Side::Buy,
                    qty,
                    TimeInForce::Gtc,
                ))
                .expect("scaled_quantity: warmup add_order must succeed");
            cancel_extra(
                &level,
                extra_id,
                "scaled_quantity: warmup teardown cancel must succeed",
            );
        });
        assert_eq!(
            level.order_count(),
            SEED_DEPTH as usize,
            "scaled_quantity(qty={qty}): depth must already be restored before measurement starts"
        );

        let (durations_ns, results) = measure_with_teardown(
            config.samples,
            |_| {
                level.add_order(fixtures::standard_order(
                    extra_id,
                    Side::Buy,
                    qty,
                    TimeInForce::Gtc,
                ))
            },
            |_, _outcome| {
                cancel_extra(
                    &level,
                    extra_id,
                    "scaled_quantity: teardown cancel must succeed",
                )
            },
        );

        let succeeded = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            succeeded, config.samples,
            "scaled_quantity(qty={qty}): every add must succeed"
        );
        assert_eq!(
            level.order_count(),
            SEED_DEPTH as usize,
            "scaled_quantity(qty={qty}): depth must be restored after every timed sample"
        );

        reports.push(ScenarioReport::from_samples(
            format!("scaled_quantity@{qty}"),
            "depth",
            SEED_DEPTH,
            "PriceLevel::add_order — swept order quantity magnitude, restored after every sample",
            durations_ns,
            format!("{succeeded}/{} succeeded", config.samples),
        ));
    }
    reports
}

/// Sweeps the LEVEL's own price magnitude (a fresh `PriceLevel` per price
/// point — every resting order at a level must share the level's exact
/// price, see `PriceLevel::add_order`'s admission check) for isolated
/// `add_order`, to check whether `u128` price magnitude affects admission
/// cost. Same add-then-cancel depth restoration as [`add_at_depth`].
fn scaled_price(config: &Config) -> Vec<ScenarioReport> {
    const PRICES: [u128; 3] = [1, 10_000, u64::MAX as u128];
    const SEED_DEPTH: u64 = 100;
    let extra_id = SEED_DEPTH;
    let mut reports = Vec::new();
    for &price in &PRICES {
        let level = PriceLevel::new(price);
        for i in 0..SEED_DEPTH {
            level
                .add_order(fixtures::standard_order_at_price(
                    i,
                    price,
                    Side::Buy,
                    10,
                    TimeInForce::Gtc,
                ))
                .expect("scaled_price: fixture seeding must succeed");
        }

        warmup(config.warmup, |_| {
            level
                .add_order(fixtures::standard_order_at_price(
                    extra_id,
                    price,
                    Side::Buy,
                    10,
                    TimeInForce::Gtc,
                ))
                .expect("scaled_price: warmup add_order must succeed");
            cancel_extra(
                &level,
                extra_id,
                "scaled_price: warmup teardown cancel must succeed",
            );
        });
        assert_eq!(
            level.order_count(),
            SEED_DEPTH as usize,
            "scaled_price(price={price}): depth must already be restored before measurement starts"
        );

        let (durations_ns, results) = measure_with_teardown(
            config.samples,
            |_| {
                level.add_order(fixtures::standard_order_at_price(
                    extra_id,
                    price,
                    Side::Buy,
                    10,
                    TimeInForce::Gtc,
                ))
            },
            |_, _outcome| {
                cancel_extra(
                    &level,
                    extra_id,
                    "scaled_price: teardown cancel must succeed",
                )
            },
        );

        let succeeded = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            succeeded, config.samples,
            "scaled_price(price={price}): every add must succeed"
        );
        assert_eq!(
            level.order_count(),
            SEED_DEPTH as usize,
            "scaled_price(price={price}): depth must be restored after every timed sample"
        );

        reports.push(ScenarioReport::from_samples(
            format!("scaled_price@{price}"),
            "depth",
            SEED_DEPTH,
            "PriceLevel::add_order — swept level price magnitude, restored after every sample",
            durations_ns,
            format!("{succeeded}/{} succeeded", config.samples),
        ));
    }
    reports
}
