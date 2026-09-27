// benches/latency/alloc_measurements.rs
//! Dedicated allocation-count / byte pass, run entirely separately from any
//! latency loop (issue #142: "Measure allocation count/bytes per operation
//! separately from latency runs so instrumentation overhead is explicit").
//!
//! Every measurement here follows the same shape: build the fixture with
//! counting disabled, reset the counters, enable counting, run
//! `config.alloc_reps` repetitions of exactly one operation, disable
//! counting, then report the per-operation average. This pass is NOT timed
//! with `Instant` at all — it only reads the allocator's own counters, so it
//! carries no latency claim whatsoever.

use crate::alloc::{self, AllocStats};
use crate::config::Config;
use crate::fixtures::{self, LEVEL_PRICE, TAKER_ID_BASE};
use pricelevel::PriceLevelSnapshotPackage;
use pricelevel::prelude::*;

/// One operation's allocation-measurement result.
#[derive(Debug, Clone)]
pub struct AllocReport {
    /// Scenario name, matching the latency scenario it corresponds to where
    /// one exists.
    pub name: &'static str,
    /// Repetitions the totals below were divided by.
    pub reps: usize,
    /// Total allocation counters accumulated over `reps` repetitions.
    pub totals: AllocStats,
}

impl AllocReport {
    fn alloc_count_per_op(&self) -> f64 {
        self.totals.alloc_count as f64 / self.reps as f64
    }

    fn alloc_bytes_per_op(&self) -> f64 {
        self.totals.alloc_bytes as f64 / self.reps as f64
    }
}

impl std::fmt::Display for AllocReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:<20} reps={:<6} alloc_count/op={:<8.2} alloc_bytes/op={:<10.2} \
             dealloc_count_total={:<8} dealloc_bytes_total={}",
            self.name,
            self.reps,
            self.alloc_count_per_op(),
            self.alloc_bytes_per_op(),
            self.totals.dealloc_count,
            self.totals.dealloc_bytes,
        )
    }
}

/// Runs every allocation measurement and returns one report each.
#[must_use]
pub fn run_all(config: &Config) -> Vec<AllocReport> {
    vec![
        measure_add_order(config),
        measure_match_full(config),
        measure_snapshot_capture(config),
        measure_checksum_validate(config),
        measure_restore(config),
    ]
}

/// Measures `add_order` of a fresh order into a level with fixed resting
/// depth. Result / fixture destruction happens after counting is disabled,
/// so it is excluded from the totals below.
fn measure_add_order(config: &Config) -> AllocReport {
    let reps = config.alloc_reps;
    let level = fixtures::seeded_standard_level(1_000, Side::Buy, 10);
    let orders: Vec<OrderType<()>> = (0..reps as u64)
        .map(|i| fixtures::standard_order(1_000 + i, Side::Buy, 10, TimeInForce::Gtc))
        .collect();

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    let mut handles = Vec::with_capacity(reps);
    for order in orders {
        handles.push(
            level
                .add_order(order)
                .expect("alloc measurement: add_order must succeed for a fresh id"),
        );
    }
    let after = AllocStats::read();
    alloc::disable();
    // Drop the returned handles and the level outside the counted window.
    drop(handles);
    drop(level);

    AllocReport {
        name: "add_order",
        reps,
        totals: after.since(before),
    }
}

/// Measures a full-fill `match_order` call against a dedicated fresh maker
/// per repetition (same shape as `scenarios::matching::match_full`).
fn measure_match_full(config: &Config) -> AllocReport {
    const QTY: u64 = 10;
    let reps = config.alloc_reps;
    let level = PriceLevel::new(LEVEL_PRICE);
    for i in 0..reps as u64 {
        level
            .add_order(fixtures::standard_order(
                i,
                Side::Sell,
                QTY,
                TimeInForce::Gtc,
            ))
            .expect("alloc measurement: seeding a fresh maker id must succeed");
    }
    let generator = fixtures::trade_id_generator();

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    let mut results = Vec::with_capacity(reps);
    for i in 0..reps {
        results.push(level.match_order(
            QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(0),
            &generator,
        ));
    }
    let after = AllocStats::read();
    alloc::disable();
    drop(results);
    drop(level);

    AllocReport {
        name: "match_full",
        reps,
        totals: after.since(before),
    }
}

/// Measures `PriceLevel::snapshot()` on a fixed-depth level.
fn measure_snapshot_capture(config: &Config) -> AllocReport {
    let reps = config.alloc_reps;
    let level = fixtures::seeded_standard_level(1_000, Side::Buy, 10);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    let mut snapshots = Vec::with_capacity(reps);
    for _ in 0..reps {
        snapshots.push(level.snapshot());
    }
    let after = AllocStats::read();
    alloc::disable();
    drop(snapshots);
    drop(level);

    AllocReport {
        name: "snapshot_capture",
        reps,
        totals: after.since(before),
    }
}

/// Measures [`PriceLevelSnapshotPackage::validate`] on an already-decoded
/// package (encoding happens before counting starts).
fn measure_checksum_validate(config: &Config) -> AllocReport {
    let reps = config.alloc_reps;
    let level = fixtures::seeded_standard_level(1_000, Side::Buy, 10);
    let json = level
        .snapshot_to_json()
        .expect("alloc measurement: snapshot_to_json must succeed");
    let package = PriceLevelSnapshotPackage::from_json(&json)
        .expect("alloc measurement: from_json must succeed for a just-encoded package");
    drop(level);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for _ in 0..reps {
        package
            .validate()
            .expect("alloc measurement: validate must succeed on an untampered package");
    }
    let after = AllocStats::read();
    alloc::disable();
    drop(package);

    AllocReport {
        name: "checksum_validate",
        reps,
        totals: after.since(before),
    }
}

/// Measures `PriceLevel::from_snapshot_json` on an already-encoded JSON
/// string.
fn measure_restore(config: &Config) -> AllocReport {
    let reps = config.alloc_reps;
    let level = fixtures::seeded_standard_level(1_000, Side::Buy, 10);
    let json = level
        .snapshot_to_json()
        .expect("alloc measurement: snapshot_to_json must succeed");
    drop(level);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    let mut restored = Vec::with_capacity(reps);
    for _ in 0..reps {
        restored.push(
            PriceLevel::from_snapshot_json(&json)
                .expect("alloc measurement: restore must succeed for a valid snapshot"),
        );
    }
    let after = AllocStats::read();
    alloc::disable();
    drop(restored);

    AllocReport {
        name: "restore",
        reps,
        totals: after.since(before),
    }
}
