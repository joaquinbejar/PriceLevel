// benches/latency/scenarios/depth.rs
//! Depth, scaled-quantity and scaled-price sweeps.
//!
//! The 10,000 / 100,000-order depths are opt-in (`PL_LATENCY_LARGE_DEPTHS=1`)
//! per issue #142 ("make the largest depths opt-in via env var if runtime is
//! long") — building and later dropping a 100,000-order level is itself
//! non-trivial wall-clock and memory cost, and that cost is fixture
//! construction, deliberately excluded from every timed sample here.

use crate::config::Config;
use crate::fixtures::{self, TAKER_ID_BASE};
use crate::report::ScenarioReport;
use crate::timing::{measure, warmup};
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

/// Isolated `add_order` into a level already holding `depth` resting orders
/// on the same side — same measured boundary as
/// `scenarios::isolated::add_gtc`, swept across depth.
fn add_at_depth(config: &Config, depth: u64) -> ScenarioReport {
    let samples = samples_for_depth(config, depth);
    let level = fixtures::seeded_standard_level(depth, Side::Buy, 10);

    let mut orders: Vec<Option<OrderType<()>>> = (0..(config.warmup + samples) as u64)
        .map(|i| {
            Some(fixtures::standard_order(
                depth + i,
                Side::Buy,
                10,
                TimeInForce::Gtc,
            ))
        })
        .collect();

    warmup(config.warmup, |i| {
        level
            .add_order(orders[i].take().expect("warmup order slot must be present"))
            .expect("depth sweep add: warmup add_order must succeed")
    });

    let (durations_ns, results) = measure(samples, |i| {
        let order = orders[config.warmup + i]
            .take()
            .expect("measured order slot must be present");
        level.add_order(order)
    });

    let succeeded = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        succeeded, samples,
        "add_at_depth({depth}): every add must succeed"
    );

    ScenarioReport::from_samples(
        format!("depth_sweep_add@{depth}"),
        "depth",
        depth,
        "PriceLevel::add_order — swept resting depth",
        durations_ns,
        format!("{succeeded}/{samples} succeeded"),
    )
}

/// A small (`qty=1`) taker crossing a level with `depth` resting orders,
/// each sized so the front maker alone can absorb every measured sample
/// without running dry — isolates the cost of matching against a structure
/// with a large number of ENTRIES, not the cost of walking many of them.
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
            TimestampMs::new(0),
            &generator,
        )
    });

    let (durations_ns, results) = measure(samples, |i| {
        level.match_order(
            1,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(0),
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
fn scaled_quantity(config: &Config) -> Vec<ScenarioReport> {
    const QUANTITIES: [u64; 3] = [1, 10_000, 1_000_000_000];
    let mut reports = Vec::new();
    for &qty in &QUANTITIES {
        let level = fixtures::seeded_standard_level(100, Side::Buy, qty.max(1));
        let mut orders: Vec<Option<OrderType<()>>> = (0..(config.warmup + config.samples) as u64)
            .map(|i| {
                Some(fixtures::standard_order(
                    100 + i,
                    Side::Buy,
                    qty,
                    TimeInForce::Gtc,
                ))
            })
            .collect();

        warmup(config.warmup, |i| {
            level
                .add_order(orders[i].take().expect("warmup order slot must be present"))
                .expect("scaled_quantity: warmup add_order must succeed")
        });

        let (durations_ns, results) = measure(config.samples, |i| {
            let order = orders[config.warmup + i]
                .take()
                .expect("measured order slot must be present");
            level.add_order(order)
        });

        let succeeded = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            succeeded, config.samples,
            "scaled_quantity(qty={qty}): every add must succeed"
        );

        reports.push(ScenarioReport::from_samples(
            format!("scaled_quantity@{qty}"),
            "depth",
            100,
            "PriceLevel::add_order — swept order quantity magnitude",
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
/// cost.
fn scaled_price(config: &Config) -> Vec<ScenarioReport> {
    const PRICES: [u128; 3] = [1, 10_000, u64::MAX as u128];
    let mut reports = Vec::new();
    for &price in &PRICES {
        let level = PriceLevel::new(price);
        for i in 0..100u64 {
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

        let mut orders: Vec<Option<OrderType<()>>> = (0..(config.warmup + config.samples) as u64)
            .map(|i| {
                Some(fixtures::standard_order_at_price(
                    100 + i,
                    price,
                    Side::Buy,
                    10,
                    TimeInForce::Gtc,
                ))
            })
            .collect();

        warmup(config.warmup, |i| {
            level
                .add_order(orders[i].take().expect("warmup order slot must be present"))
                .expect("scaled_price: warmup add_order must succeed")
        });

        let (durations_ns, results) = measure(config.samples, |i| {
            let order = orders[config.warmup + i]
                .take()
                .expect("measured order slot must be present");
            level.add_order(order)
        });

        let succeeded = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            succeeded, config.samples,
            "scaled_price(price={price}): every add must succeed"
        );

        reports.push(ScenarioReport::from_samples(
            format!("scaled_price@{price}"),
            "depth",
            100,
            "PriceLevel::add_order — swept level price magnitude",
            durations_ns,
            format!("{succeeded}/{} succeeded", config.samples),
        ));
    }
    reports
}
