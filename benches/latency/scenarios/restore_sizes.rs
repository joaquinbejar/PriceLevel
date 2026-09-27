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
//! A separate, untimed pass ([`run_alloc`]) reports allocation count, bytes
//! and the peak live bytes (the counting allocator's high-water mark over one
//! operation, including the restored level while it is alive) per operation.

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
    assert!(PriceLevel::from_snapshot_json(&valid_json).is_ok());
    assert!(matches!(
        PriceLevel::from_snapshot_json(&dup_last_json),
        Err(PriceLevelError::DuplicateOrderId(_))
    ));

    Inputs {
        valid,
        dup_last,
        price_last,
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

/// Runs every size / operation and returns one report each.
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
        let cases: [(&str, &'static str, Vec<u64>); 5] = [
            (
                "from_snapshot_valid",
                "PriceLevel::from_snapshot (valid)",
                time_from_snapshot(&inputs.valid, samples, warmup),
            ),
            (
                "from_snapshot_dup_last",
                "PriceLevel::from_snapshot (duplicate id at the last order)",
                time_from_snapshot(&inputs.dup_last, samples, warmup),
            ),
            (
                "from_snapshot_price_last",
                "PriceLevel::from_snapshot (wrong price at the last order)",
                time_from_snapshot(&inputs.price_last, samples, warmup),
            ),
            (
                "from_json_valid",
                "PriceLevel::from_snapshot_json (valid)",
                time_from_json(&inputs.valid_json, samples, warmup),
            ),
            (
                "from_json_dup_last",
                "PriceLevel::from_snapshot_json (signed, duplicate id last)",
                time_from_json(&inputs.dup_last_json, samples, warmup),
            ),
        ];
        for (name, call, durations) in cases {
            reports.push(ScenarioReport::from_samples(
                name,
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

/// Counts `reps` runs of `op` (each output dropped inside the window, so the
/// peak is the single-operation high-water mark). `setup` runs with counting
/// disabled.
fn count<S, I, F, O>(reps: usize, mut setup: S, mut op: F) -> (AllocStats, i64)
where
    S: FnMut() -> I,
    F: FnMut(I) -> O,
{
    let mut inputs: Vec<I> = (0..reps).map(|_| setup()).collect();
    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for input in inputs.drain(..) {
        drop(black_box(op(input)));
    }
    let after = AllocStats::read();
    alloc::disable();
    (after.since(before), alloc::peak_live_bytes())
}

/// Runs the allocation pass and returns one printable line per size /
/// operation.
#[must_use]
pub fn run_alloc(config: &Config) -> Vec<String> {
    let mut lines = Vec::new();
    for &size in sizes() {
        let inputs = inputs(size);
        let reps = alloc_reps_for(config, size);
        let mut push = |name: &str, (stats, peak): (AllocStats, i64)| {
            let reps_f = reps as f64;
            lines.push(format!(
                "{name:<26} n={size:<7} reps={reps:<5} alloc_count/op={:<10.2} \
                 alloc_bytes/op={:<12.0} peak_live_bytes={}",
                stats.alloc_count as f64 / reps_f,
                stats.alloc_bytes as f64 / reps_f,
                peak,
            ));
        };
        let clone = |s: &PriceLevelSnapshot| s.try_clone().expect("try_clone");
        push(
            "from_snapshot_valid",
            count(reps, || clone(&inputs.valid), PriceLevel::from_snapshot),
        );
        push(
            "from_snapshot_dup_last",
            count(reps, || clone(&inputs.dup_last), PriceLevel::from_snapshot),
        );
        push(
            "from_snapshot_price_last",
            count(
                reps,
                || clone(&inputs.price_last),
                PriceLevel::from_snapshot,
            ),
        );
        push(
            "from_json_valid",
            count(
                reps,
                || (),
                |()| PriceLevel::from_snapshot_json(&inputs.valid_json),
            ),
        );
        push(
            "from_json_dup_last",
            count(
                reps,
                || (),
                |()| PriceLevel::from_snapshot_json(&inputs.dup_last_json),
            ),
        );
    }
    lines
}
