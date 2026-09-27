// benches/latency/scenarios/snapshot.rs
//! Iteration, snapshot capture, checksum validation and restore scenarios.
//!
//! Each of these has a distinct measured boundary — the issue calls this out
//! explicitly (`snapshot_full_roundtrip` in the pre-existing Criterion bench
//! only times restore, not a full round trip, because JSON encoding happens
//! outside its `b.iter`). This module keeps every boundary separate and
//! names it in `measured_call` rather than bundling them.

use crate::config::Config;
use crate::fixtures;
use crate::report::ScenarioReport;
use crate::timing::measure;
use pricelevel::PriceLevelSnapshotPackage;
use pricelevel::prelude::*;

/// Fixed depth for every snapshot / iteration scenario.
const DEPTH: u64 = 1_000;

/// Runs every snapshot / iteration scenario and returns one report each.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    vec![
        iteration(config),
        snapshot_capture(config),
        checksum_validate(config),
        restore(config),
    ]
}

/// One full traversal of [`PriceLevel::iter_orders`] per sample — the
/// zero-allocation read path (`rules/global_rules.md` "Minimize Copies").
fn iteration(config: &Config) -> ScenarioReport {
    let level = fixtures::seeded_standard_level(DEPTH, Side::Buy, 10);

    for _ in 0..config.warmup {
        let mut total: u64 = 0;
        for order in level.iter_orders() {
            total += order.visible_quantity().as_u64();
        }
        std::hint::black_box(total);
    }

    let (durations_ns, totals) = measure(config.samples, |_| {
        let mut total: u64 = 0;
        let mut count: u64 = 0;
        for order in level.iter_orders() {
            total += order.visible_quantity().as_u64();
            count += 1;
        }
        (total, count)
    });

    let expected_total = DEPTH * 10;
    for &(total, count) in &totals {
        assert_eq!(
            count, DEPTH,
            "iteration: every traversal must visit exactly DEPTH resting orders"
        );
        assert_eq!(
            total, expected_total,
            "iteration: every traversal must sum to DEPTH * quantity_each exactly"
        );
    }

    ScenarioReport::from_samples(
        "iteration",
        "iteration",
        DEPTH,
        "PriceLevel::iter_orders — one full traversal",
        durations_ns,
        format!(
            "{}/{} traversals visited exactly {DEPTH} orders",
            totals.len(),
            config.samples
        ),
    )
}

/// One `PriceLevel::snapshot()` call per sample — materializes the orders
/// and recomputes aggregates (`doc/architecture.md`'s "Data flow" step 4);
/// this is the "capture" boundary, distinct from serialization.
fn snapshot_capture(config: &Config) -> ScenarioReport {
    let level = fixtures::seeded_standard_level(DEPTH, Side::Buy, 10);

    for _ in 0..config.warmup {
        std::hint::black_box(level.snapshot());
    }

    let (durations_ns, snapshots) = measure(config.samples, |_| level.snapshot());

    for snap in &snapshots {
        assert_eq!(
            snap.order_count(),
            DEPTH as usize,
            "snapshot_capture: every snapshot must carry exactly DEPTH orders"
        );
    }

    ScenarioReport::from_samples(
        "snapshot_capture",
        "snapshot",
        DEPTH,
        "PriceLevel::snapshot() — materialize + recompute aggregates",
        durations_ns,
        format!(
            "{}/{} snapshots carried exactly {DEPTH} orders",
            snapshots.len(),
            config.samples
        ),
    )
}

/// One [`PriceLevelSnapshotPackage::validate`] call per sample — SHA-256
/// checksum verification only, given an already-decoded package.
fn checksum_validate(config: &Config) -> ScenarioReport {
    let level = fixtures::seeded_standard_level(DEPTH, Side::Buy, 10);
    let json = level
        .snapshot_to_json()
        .expect("checksum_validate: snapshot_to_json must succeed for a valid level");
    let package = PriceLevelSnapshotPackage::from_json(&json)
        .expect("checksum_validate: from_json must succeed for a just-encoded package");

    for _ in 0..config.warmup {
        package
            .validate()
            .expect("checksum_validate: warmup validate must succeed on an untampered package");
    }

    let (durations_ns, results) = measure(config.samples, |_| package.validate());

    let valid = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        valid, config.samples,
        "checksum_validate: every validate call on an untampered package must succeed"
    );

    ScenarioReport::from_samples(
        "checksum_validate",
        "snapshot",
        DEPTH,
        "PriceLevelSnapshotPackage::validate() — SHA-256 checksum only",
        durations_ns,
        format!("{valid}/{} validated OK", config.samples),
    )
}

/// One `PriceLevel::from_snapshot_json` call per sample — decode, checksum
/// validation, and full level reconstruction from an already-encoded JSON
/// string (encoding happens once, outside every timed call).
fn restore(config: &Config) -> ScenarioReport {
    let level = fixtures::seeded_standard_level(DEPTH, Side::Buy, 10);
    let json = level
        .snapshot_to_json()
        .expect("restore: snapshot_to_json must succeed for a valid level");

    for _ in 0..config.warmup {
        std::hint::black_box(
            PriceLevel::from_snapshot_json(&json)
                .expect("restore: warmup restore must succeed for a valid, untampered snapshot"),
        );
    }

    let (durations_ns, results) =
        measure(config.samples, |_| PriceLevel::from_snapshot_json(&json));

    let mut restored_ok = 0usize;
    for result in &results {
        let restored = result
            .as_ref()
            .expect("restore: every restore of a valid, untampered snapshot must succeed");
        assert_eq!(
            restored.order_count(),
            DEPTH as usize,
            "restore: every restored level must carry exactly DEPTH orders"
        );
        restored_ok += 1;
    }
    assert_eq!(restored_ok, config.samples);

    ScenarioReport::from_samples(
        "restore",
        "snapshot",
        DEPTH,
        "PriceLevel::from_snapshot_json — decode + checksum + reconstruct",
        durations_ns,
        format!(
            "{restored_ok}/{} restored with exactly {DEPTH} orders",
            config.samples
        ),
    )
}
