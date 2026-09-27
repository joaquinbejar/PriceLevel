// benches/latency/scenarios/snapshot_sizes.rs
//! Snapshot encoding and checksum validation across level sizes (issue #149).
//!
//! Three public operations at 100, 10,000 and 100,000 resting orders:
//!
//! - `snapshot_package`: `PriceLevel::snapshot_package()` (materialize the
//!   snapshot + stream its canonical JSON into SHA-256).
//! - `snapshot_to_json`: `PriceLevel::snapshot_to_json()` (the above, then a
//!   second serialization pass for the package JSON).
//! - `validate`: `PriceLevelSnapshotPackage::validate()` on a decoded package
//!   (re-stream the canonical JSON into SHA-256 and compare).
//!
//! Each runs in two variants, **interleaved sample by sample** (alternating
//! which goes first) so machine load drifts hit both equally:
//!
//! - `stream`: the crate's current path. The orders are serialized borrowed
//!   (`BorrowedOrders`, no reference vector) and the checksum is computed by
//!   `serde_json::to_writer` into a SHA-256 `io::Write` adapter (no payload
//!   buffer).
//! - `legacy`: a bench-local emulation of the pre-#164 path the issue
//!   describes (baseline `aaafd39`): collect a `Vec<&OrderType<()>>` inside
//!   `Serialize`, hash `serde_json::to_vec(snapshot)`, and encode the package
//!   with `serde_json::to_string`. `legacy` validate / package do not build a
//!   `PriceLevelSnapshotPackage` (its fields are private); they produce the
//!   same snapshot + checksum string, so the measured work is the same minus
//!   one move. Before any timing, the legacy bytes and checksums are asserted
//!   byte-identical to the streaming ones.
//!
//! A separate, untimed pass ([`run_alloc`]) reports allocation count, bytes
//! and the peak live bytes (the counting allocator's high-water mark, retained
//! output included) per operation.

use crate::alloc::{self, AllocStats};
use crate::config::Config;
use crate::fixtures;
use crate::report::ScenarioReport;
use pricelevel::PriceLevelSnapshotPackage;
use pricelevel::prelude::*;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use sha2::{Digest, Sha256};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

/// Level sizes for the optimized `bench` profile.
const SIZES: [u64; 3] = [100, 10_000, 100_000];
/// Level sizes for an unoptimized `cargo test --all-targets` smoke run.
const DEBUG_SIZES: [u64; 2] = [100, 1_000];
/// Orders serialized per variant per operation: bounds the sample count at
/// large sizes (`samples = min(PL_LATENCY_SAMPLES, budget / size)`, at least
/// [`MIN_SAMPLES`]).
const SAMPLE_BUDGET_ORDERS: usize = 5_000_000;
/// Floor on the per-size sample count.
const MIN_SAMPLES: usize = 20;
/// Orders serialized per variant per operation in the allocation pass.
const ALLOC_BUDGET_ORDERS: usize = 1_000_000;

fn sizes() -> &'static [u64] {
    if cfg!(debug_assertions) {
        &DEBUG_SIZES
    } else {
        &SIZES
    }
}

fn samples_for(config: &Config, size: u64) -> usize {
    let budget = SAMPLE_BUDGET_ORDERS / usize::try_from(size).unwrap_or(usize::MAX).max(1);
    config.samples.min(budget.max(MIN_SAMPLES))
}

fn alloc_reps_for(config: &Config, size: u64) -> usize {
    let budget = ALLOC_BUDGET_ORDERS / usize::try_from(size).unwrap_or(usize::MAX).max(1);
    config.alloc_reps.min(budget.max(3))
}

// ----------------------------------------------------------------------------
// Legacy (pre-#164) path emulation
// ----------------------------------------------------------------------------

/// Serializes a snapshot the pre-#164 way: the orders are first collected
/// into a `Vec<&OrderType<()>>`. Same struct name, field names and order as
/// `impl Serialize for PriceLevelSnapshot`.
struct LegacySnapshot<'a>(&'a PriceLevelSnapshot);

impl Serialize for LegacySnapshot<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let snapshot = self.0;
        let orders: Vec<&OrderType<()>> = snapshot.orders().iter().map(Arc::as_ref).collect();
        let mut state = serializer.serialize_struct("PriceLevelSnapshot", 6)?;
        state.serialize_field("price", &snapshot.price())?;
        state.serialize_field("visible_quantity", &snapshot.visible_quantity())?;
        state.serialize_field("hidden_quantity", &snapshot.hidden_quantity())?;
        state.serialize_field("order_count", &snapshot.order_count())?;
        state.serialize_field("orders", &orders)?;
        state.serialize_field("statistics", snapshot.statistics())?;
        state.end()
    }
}

/// The package envelope, serialized the pre-#164 way.
struct LegacyPackage<'a> {
    version: u32,
    snapshot: &'a PriceLevelSnapshot,
    checksum: &'a str,
}

impl Serialize for LegacyPackage<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("PriceLevelSnapshotPackage", 3)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("snapshot", &LegacySnapshot(self.snapshot))?;
        state.serialize_field("checksum", self.checksum)?;
        state.end()
    }
}

/// Pre-#164 checksum: hash a fully materialized `serde_json::to_vec` payload.
fn legacy_checksum(snapshot: &PriceLevelSnapshot) -> String {
    use std::fmt::Write as _;
    let payload = serde_json::to_vec(&LegacySnapshot(snapshot))
        .expect("legacy checksum: snapshot must encode");
    let digest = Sha256::digest(&payload);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Pre-#164 `snapshot_package`: snapshot + refreshed aggregates + checksum.
fn legacy_snapshot_package(level: &PriceLevel) -> (PriceLevelSnapshot, String) {
    let mut snapshot = level.snapshot().expect("legacy package: snapshot()");
    snapshot
        .refresh_aggregates()
        .expect("legacy package: refresh_aggregates()");
    let checksum = legacy_checksum(&snapshot);
    (snapshot, checksum)
}

/// Pre-#164 `snapshot_to_json`: legacy package, then `serde_json::to_string`.
fn legacy_snapshot_to_json(level: &PriceLevel, version: u32) -> String {
    let (snapshot, checksum) = legacy_snapshot_package(level);
    serde_json::to_string(&LegacyPackage {
        version,
        snapshot: &snapshot,
        checksum: &checksum,
    })
    .expect("legacy to_json: package must encode")
}

/// Pre-#164 `validate` (the version check is a constant-time slice lookup in
/// both paths and is omitted).
fn legacy_validate(package: &PriceLevelSnapshotPackage) -> bool {
    legacy_checksum(package.snapshot()) == package.checksum()
}

// ----------------------------------------------------------------------------
// Latency
// ----------------------------------------------------------------------------

/// Times `legacy` and `stream` alternately (swapping which runs first each
/// iteration); every output is dropped after its clock read.
fn interleaved<L, N, OL, ON>(
    samples: usize,
    warmup: usize,
    mut legacy: L,
    mut stream: N,
) -> (Vec<u64>, Vec<u64>)
where
    L: FnMut() -> OL,
    N: FnMut() -> ON,
{
    let mut legacy_ns = Vec::with_capacity(samples);
    let mut stream_ns = Vec::with_capacity(samples);
    let nanos = |t0: Instant| u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
    for i in 0..warmup + samples {
        let record = i >= warmup;
        if i % 2 == 0 {
            let t0 = Instant::now();
            let out = legacy();
            let l = nanos(t0);
            drop(black_box(out));
            let t0 = Instant::now();
            let out = stream();
            let s = nanos(t0);
            drop(black_box(out));
            if record {
                legacy_ns.push(l);
                stream_ns.push(s);
            }
        } else {
            let t0 = Instant::now();
            let out = stream();
            let s = nanos(t0);
            drop(black_box(out));
            let t0 = Instant::now();
            let out = legacy();
            let l = nanos(t0);
            drop(black_box(out));
            if record {
                legacy_ns.push(l);
                stream_ns.push(s);
            }
        }
    }
    (legacy_ns, stream_ns)
}

/// Builds the size-`n` fixture and asserts legacy/stream byte equivalence.
fn fixture(size: u64) -> (PriceLevel, PriceLevelSnapshotPackage, String) {
    let level = fixtures::seeded_standard_level(size, Side::Buy, 10);
    let json = level
        .snapshot_to_json()
        .expect("snapshot_sizes: snapshot_to_json must succeed");
    let package = PriceLevelSnapshotPackage::from_json(&json)
        .expect("snapshot_sizes: from_json must succeed for a just-encoded package");
    assert_eq!(
        legacy_snapshot_to_json(&level, package.version()),
        json,
        "snapshot_sizes: legacy and streaming package JSON must be byte-identical"
    );
    assert_eq!(
        legacy_checksum(package.snapshot()),
        package.checksum(),
        "snapshot_sizes: legacy and streaming checksums must be identical"
    );
    (level, package, json)
}

/// Runs every size / operation / variant and returns one report each.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    let mut reports = Vec::new();
    for &size in sizes() {
        let (level, package, json) = fixture(size);
        let version = package.version();
        let samples = samples_for(config, size);
        let warmup = config.warmup.min(samples / 10 + 1);
        let note = format!(
            "{samples} interleaved samples; package JSON {} bytes",
            json.len()
        );

        let (legacy, stream) = interleaved(
            samples,
            warmup,
            || legacy_snapshot_package(&level),
            || {
                level
                    .snapshot_package()
                    .expect("snapshot_sizes: snapshot_package")
            },
        );
        reports.push(ScenarioReport::from_samples(
            "snapshot_package_legacy",
            "snap_sizes",
            size,
            "snapshot + Vec<&Order> + to_vec + SHA-256 (pre-#164 emulation)",
            legacy,
            note.clone(),
        ));
        reports.push(ScenarioReport::from_samples(
            "snapshot_package_stream",
            "snap_sizes",
            size,
            "PriceLevel::snapshot_package()",
            stream,
            note.clone(),
        ));

        let (legacy, stream) = interleaved(
            samples,
            warmup,
            || legacy_snapshot_to_json(&level, version),
            || {
                level
                    .snapshot_to_json()
                    .expect("snapshot_sizes: snapshot_to_json")
            },
        );
        reports.push(ScenarioReport::from_samples(
            "snapshot_to_json_legacy",
            "snap_sizes",
            size,
            "legacy package + serde_json::to_string (pre-#164 emulation)",
            legacy,
            note.clone(),
        ));
        reports.push(ScenarioReport::from_samples(
            "snapshot_to_json_stream",
            "snap_sizes",
            size,
            "PriceLevel::snapshot_to_json()",
            stream,
            note.clone(),
        ));

        let (legacy, stream) = interleaved(
            samples,
            warmup,
            || assert!(legacy_validate(&package), "legacy validate"),
            || package.validate().expect("snapshot_sizes: validate"),
        );
        reports.push(ScenarioReport::from_samples(
            "validate_legacy",
            "snap_sizes",
            size,
            "to_vec(snapshot) + SHA-256 + compare (pre-#164 emulation)",
            legacy,
            note.clone(),
        ));
        reports.push(ScenarioReport::from_samples(
            "validate_stream",
            "snap_sizes",
            size,
            "PriceLevelSnapshotPackage::validate()",
            stream,
            note,
        ));
    }
    reports
}

// ----------------------------------------------------------------------------
// Allocations (untimed)
// ----------------------------------------------------------------------------

/// Counts `reps` runs of `op` (each output dropped inside the window, so the
/// peak is the single-operation high-water mark).
fn count<F, O>(reps: usize, mut op: F) -> (AllocStats, i64)
where
    F: FnMut() -> O,
{
    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for _ in 0..reps {
        drop(black_box(op()));
    }
    let after = AllocStats::read();
    alloc::disable();
    (after.since(before), alloc::peak_live_bytes())
}

/// Runs the allocation pass and returns one printable line per size /
/// operation / variant.
#[must_use]
pub fn run_alloc(config: &Config) -> Vec<String> {
    let mut lines = Vec::new();
    for &size in sizes() {
        let (level, package, _json) = fixture(size);
        let version = package.version();
        let reps = alloc_reps_for(config, size);
        let mut push = |name: &str, (stats, peak): (AllocStats, i64)| {
            let reps_f = reps as f64;
            lines.push(format!(
                "{name:<24} n={size:<7} reps={reps:<5} alloc_count/op={:<10.2} \
                 alloc_bytes/op={:<12.0} peak_live_bytes={}",
                stats.alloc_count as f64 / reps_f,
                stats.alloc_bytes as f64 / reps_f,
                peak,
            ));
        };
        push(
            "snapshot_package_legacy",
            count(reps, || legacy_snapshot_package(&level)),
        );
        push(
            "snapshot_package_stream",
            count(reps, || level.snapshot_package().expect("snapshot_package")),
        );
        push(
            "snapshot_to_json_legacy",
            count(reps, || legacy_snapshot_to_json(&level, version)),
        );
        push(
            "snapshot_to_json_stream",
            count(reps, || level.snapshot_to_json().expect("snapshot_to_json")),
        );
        push(
            "validate_legacy",
            count(reps, || assert!(legacy_validate(&package))),
        );
        push(
            "validate_stream",
            count(reps, || package.validate().expect("validate")),
        );
    }
    lines
}
