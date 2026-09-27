// benches/latency/scenarios/restore_sizes.rs
//! Snapshot restore across level sizes, valid and failing near the end
//! (issue #150).
//!
//! Operations, each at 100, 10,000 and 100,000 resting orders:
//!
//! - `from_snapshot_valid`: `PriceLevel::from_snapshot` on a valid snapshot
//!   (order validation + queue build). The input snapshot is copied with
//!   `try_clone` before the clock starts and the restored level is dropped
//!   after it stops, so only the restore itself is timed.
//! - `from_snapshot_dup_last`: the same, but the LAST order repeats the id of
//!   the one before it (rejected with `DuplicateOrderId`).
//! - `from_snapshot_price_last`: the LAST order sits at another price
//!   (rejected with a topology `InvalidOperation`).
//! - `from_json_valid`: `PriceLevel::from_snapshot_json` end to end (decode +
//!   version / checksum validation + restore) on a valid package.
//! - `from_json_dup_last`: the same on a correctly signed package whose last
//!   order repeats an id: the checksum passes, restore rejects.
//!
//! Only public API is used, so the file builds unchanged against the pre-#150
//! crate: A/B comparisons build this harness on both revisions and run the
//! two binaries alternately (see `BENCH.md`).
//!
//! - `from_snapshot_total_second` / `from_snapshot_total_last`: the visible
//!   sum overflows at the second / last order (aggregate failure, the
//!   highest-ranked rejection).
//!
//! Report names carry the size (`name@size`), so persisted artifact keys
//! are unique across sizes.
//!
//! A separate, untimed pass ([`run_alloc`]) reports allocation count and
//! bytes per operation, and the median and maximum over repetitions of each
//! operation's own peak live bytes (rebased to zero before every operation;
//! see [`count`]).

use crate::alloc::{self, AllocStats};
use crate::config::Config;
use crate::fixtures;
use crate::report::ScenarioReport;
use pricelevel::PriceLevelSnapshotPackage;
use pricelevel::prelude::*;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

/// Level sizes for the optimized `bench` profile.
const SIZES: [u64; 3] = [100, 10_000, 100_000];
/// Level sizes for an unoptimized `cargo test --all-targets` smoke run.
const DEBUG_SIZES: [u64; 2] = [100, 1_000];
/// Orders restored per operation across all samples: bounds the sample count
/// at large sizes (`samples = min(PL_LATENCY_SAMPLES, budget / size)`, at
/// least [`MIN_SAMPLES`]).
const SAMPLE_BUDGET_ORDERS: usize = 5_000_000;
/// Floor on the per-size sample count.
const MIN_SAMPLES: usize = 20;
/// Orders restored per operation in the allocation pass.
const ALLOC_BUDGET_ORDERS: usize = 1_000_000;

fn sizes() -> &'static [u64] {
    if cfg!(debug_assertions) {
        &DEBUG_SIZES
    } else {
        &SIZES
    }
}

fn per_size(total: usize, size: u64) -> usize {
    total / usize::try_from(size).unwrap_or(usize::MAX).max(1)
}

fn samples_for(config: &Config, size: u64) -> usize {
    config
        .samples
        .min(per_size(SAMPLE_BUDGET_ORDERS, size).max(MIN_SAMPLES))
}

fn alloc_reps_for(config: &Config, size: u64) -> usize {
    config
        .alloc_reps
        .min(per_size(ALLOC_BUDGET_ORDERS, size).max(3))
}

/// The inputs for one level size.
struct Inputs {
    valid: PriceLevelSnapshot,
    dup_last: PriceLevelSnapshot,
    price_last: PriceLevelSnapshot,
    /// First order at `u64::MAX`: the visible sum overflows at the second.
    total_second: PriceLevelSnapshot,
    /// Last order at `u64::MAX`: the visible sum overflows at the last.
    total_last: PriceLevelSnapshot,
    valid_json: String,
    dup_last_json: String,
}

/// A copy of `order` with its id replaced by `id`.
fn with_id(order: &OrderType<()>, id: Id) -> OrderType<()> {
    match *order {
        OrderType::Standard {
            price,
            quantity,
            side,
            user_id,
            timestamp,
            time_in_force,
            ..
        } => OrderType::Standard {
            id,
            price,
            quantity,
            side,
            user_id,
            timestamp,
            time_in_force,
            extra_fields: (),
        },
        _ => panic!("restore_sizes: fixtures are Standard orders"),
    }
}

/// A copy of `order` at `price`.
fn with_price(order: &OrderType<()>, price: Price) -> OrderType<()> {
    match *order {
        OrderType::Standard {
            id,
            quantity,
            side,
            user_id,
            timestamp,
            time_in_force,
            ..
        } => OrderType::Standard {
            id,
            price,
            quantity,
            side,
            user_id,
            timestamp,
            time_in_force,
            extra_fields: (),
        },
        _ => panic!("restore_sizes: fixtures are Standard orders"),
    }
}

/// `snapshot` with order `index`'s quantity set to `u64::MAX`. Built through
/// serde because every public constructor refreshes (and so rejects) the
/// aggregates; snapshot deserialization stores them as given.
fn with_quantity_overflow(snapshot: &PriceLevelSnapshot, index: usize) -> PriceLevelSnapshot {
    let mut value = serde_json::to_value(snapshot).expect("restore_sizes: to_value");
    value["orders"][index]["Standard"]["quantity"] = serde_json::Value::from(u64::MAX);
    serde_json::from_value(value).expect("restore_sizes: from_value")
}

fn signed_json(snapshot: &PriceLevelSnapshot) -> String {
    PriceLevelSnapshotPackage::new(snapshot.try_clone().expect("restore_sizes: try_clone"))
        .expect("restore_sizes: package")
        .to_json()
        .expect("restore_sizes: to_json")
}

fn inputs(size: u64) -> Inputs {
    let level = fixtures::seeded_standard_level(size, Side::Buy, 10);
    let valid_json = level
        .snapshot_to_json()
        .expect("restore_sizes: snapshot_to_json");
    let valid = PriceLevelSnapshotPackage::from_json(&valid_json)
        .expect("restore_sizes: from_json")
        .into_snapshot()
        .expect("restore_sizes: into_snapshot");
    let orders: Vec<Arc<OrderType<()>>> = valid.orders().to_vec();
    let n = orders.len();
    let price = valid.price();

    let mut dup = orders.clone();
    let previous_id = dup[n - 2].id();
    dup[n - 1] = Arc::new(with_id(&dup[n - 1], previous_id));
    let dup_last = PriceLevelSnapshot::with_orders(price, dup).expect("restore_sizes: dup");

    let mut wrong = orders;
    wrong[n - 1] = Arc::new(with_price(&wrong[n - 1], Price::new(price.as_u128() + 1)));
    let price_last = PriceLevelSnapshot::with_orders(price, wrong).expect("restore_sizes: price");

    let dup_last_json = signed_json(&dup_last);
    let total_second = with_quantity_overflow(&valid, 0);
    let total_last = with_quantity_overflow(&valid, n - 1);

    // Outcome checks, untimed.
    assert!(PriceLevel::from_snapshot(valid.try_clone().expect("clone")).is_ok());
    assert!(matches!(
        PriceLevel::from_snapshot(dup_last.try_clone().expect("clone")),
        Err(PriceLevelError::DuplicateOrderId(_))
    ));
    assert!(matches!(
        PriceLevel::from_snapshot(price_last.try_clone().expect("clone")),
        Err(PriceLevelError::InvalidOperation { .. })
    ));
    for overflowing in [&total_second, &total_last] {
        assert!(matches!(
            PriceLevel::from_snapshot(overflowing.try_clone().expect("clone")),
            Err(PriceLevelError::InvalidOperation { .. })
        ));
    }
    assert!(PriceLevel::from_snapshot_json(&valid_json).is_ok());
    assert!(matches!(
        PriceLevel::from_snapshot_json(&dup_last_json),
        Err(PriceLevelError::DuplicateOrderId(_))
    ));

    Inputs {
        valid,
        dup_last,
        price_last,
        total_second,
        total_last,
        valid_json,
        dup_last_json,
    }
}

fn nanos(t0: Instant) -> u64 {
    u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Times `from_snapshot` on a fresh copy of `snapshot` per sample; the copy
/// and the drop of the result are outside the clock.
fn time_from_snapshot(snapshot: &PriceLevelSnapshot, samples: usize, warmup: usize) -> Vec<u64> {
    let mut out = Vec::with_capacity(samples);
    for i in 0..warmup + samples {
        let input = snapshot.try_clone().expect("restore_sizes: try_clone");
        let t0 = Instant::now();
        let result = PriceLevel::from_snapshot(input);
        let ns = nanos(t0);
        drop(black_box(result));
        if i >= warmup {
            out.push(ns);
        }
    }
    out
}

/// Times `from_snapshot_json` per sample; the drop is outside the clock.
fn time_from_json(json: &str, samples: usize, warmup: usize) -> Vec<u64> {
    let mut out = Vec::with_capacity(samples);
    for i in 0..warmup + samples {
        let t0 = Instant::now();
        let result = PriceLevel::from_snapshot_json(black_box(json));
        let ns = nanos(t0);
        drop(black_box(result));
        if i >= warmup {
            out.push(ns);
        }
    }
    out
}

/// One restore operation: `from_snapshot` on a snapshot copy, or
/// `from_snapshot_json` on a string.
enum Op<'a> {
    Snapshot(&'a PriceLevelSnapshot),
    Json(&'a str),
}

/// Every measured case for one size: (name, measured call, operation).
fn cases(inputs: &Inputs) -> [(&'static str, &'static str, Op<'_>); 7] {
    [
        (
            "from_snapshot_valid",
            "PriceLevel::from_snapshot (valid)",
            Op::Snapshot(&inputs.valid),
        ),
        (
            "from_snapshot_dup_last",
            "PriceLevel::from_snapshot (duplicate id at the last order)",
            Op::Snapshot(&inputs.dup_last),
        ),
        (
            "from_snapshot_price_last",
            "PriceLevel::from_snapshot (wrong price at the last order)",
            Op::Snapshot(&inputs.price_last),
        ),
        (
            "from_snapshot_total_second",
            "PriceLevel::from_snapshot (visible sum overflows at the second order)",
            Op::Snapshot(&inputs.total_second),
        ),
        (
            "from_snapshot_total_last",
            "PriceLevel::from_snapshot (visible sum overflows at the last order)",
            Op::Snapshot(&inputs.total_last),
        ),
        (
            "from_json_valid",
            "PriceLevel::from_snapshot_json (valid)",
            Op::Json(&inputs.valid_json),
        ),
        (
            "from_json_dup_last",
            "PriceLevel::from_snapshot_json (signed, duplicate id last)",
            Op::Json(&inputs.dup_last_json),
        ),
    ]
}

/// Runs every size / operation and returns one report each. Report names
/// carry the size (`name@size`) so every persisted artifact key is unique.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    let mut reports = Vec::new();
    for &size in sizes() {
        let inputs = inputs(size);
        let samples = samples_for(config, size);
        let warmup = config.warmup.min(samples / 10 + 1);
        let note = format!(
            "{samples} samples; package JSON {} bytes",
            inputs.valid_json.len()
        );
        for (name, call, op) in cases(&inputs) {
            let durations = match op {
                Op::Snapshot(snapshot) => time_from_snapshot(snapshot, samples, warmup),
                Op::Json(json) => time_from_json(json, samples, warmup),
            };
            reports.push(ScenarioReport::from_samples(
                format!("{name}@{size}"),
                "restore_sizes",
                size,
                call,
                durations,
                note.clone(),
            ));
        }
    }
    reports
}

/// Allocation totals over `reps` operations plus the per-operation peaks.
struct AllocResult {
    stats: AllocStats,
    /// Per-repetition high-water mark of live counted bytes.
    peaks: Vec<i64>,
}

/// Counts `reps` runs of `op`. `setup` runs with counting disabled, before
/// the window. The live-byte baseline is rebased to zero immediately before
/// each operation, so each repetition's peak is that one operation's own
/// high-water mark: the bytes it allocated beyond what already existed (the
/// pre-existing input is excluded; input buffers the operation frees lower
/// the live count), including its output while alive.
fn count<S, I, F, O>(reps: usize, mut setup: S, mut op: F) -> AllocResult
where
    S: FnMut() -> I,
    F: FnMut(I) -> O,
{
    let mut inputs: Vec<I> = (0..reps).map(|_| setup()).collect();
    let mut peaks = Vec::with_capacity(reps);
    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for input in inputs.drain(..) {
        alloc::rebase_live();
        drop(black_box(op(input)));
        peaks.push(alloc::peak_live_bytes());
    }
    let after = AllocStats::read();
    alloc::disable();
    AllocResult {
        stats: after.since(before),
        peaks,
    }
}

/// Runs the allocation pass and returns one printable line per size /
/// operation: mean allocations and bytes per operation, and the median and
/// maximum of the per-operation peaks.
#[must_use]
pub fn run_alloc(config: &Config) -> Vec<String> {
    let mut lines = Vec::new();
    for &size in sizes() {
        let inputs = inputs(size);
        let reps = alloc_reps_for(config, size);
        for (name, _call, op) in cases(&inputs) {
            let mut result = match op {
                Op::Snapshot(snapshot) => count(
                    reps,
                    || snapshot.try_clone().expect("try_clone"),
                    PriceLevel::from_snapshot,
                ),
                Op::Json(json) => count(reps, || (), |()| PriceLevel::from_snapshot_json(json)),
            };
            result.peaks.sort_unstable();
            let median = result
                .peaks
                .get(result.peaks.len() / 2)
                .copied()
                .unwrap_or(0);
            let max = result.peaks.last().copied().unwrap_or(0);
            let reps_f = reps as f64;
            lines.push(format!(
                "{name:<28} n={size:<7} reps={reps:<5} alloc_count/op={:<10.2} \
                 alloc_bytes/op={:<12.0} peak_live_bytes/op median={median} max={max}",
                result.stats.alloc_count as f64 / reps_f,
                result.stats.alloc_bytes as f64 / reps_f,
            ));
        }
    }
    lines
}
