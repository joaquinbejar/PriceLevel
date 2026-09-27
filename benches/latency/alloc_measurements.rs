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
use crate::fixtures::{self, EXECUTION_TIMESTAMP_MS, LEVEL_PRICE, TAKER_ID_BASE};
use pricelevel::PriceLevelSnapshotPackage;
use pricelevel::prelude::*;
use std::sync::Arc;

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
        measure_uuid_try_next(config),
        measure_match_sweep_100(config),
        measure_snapshot_capture(config),
        measure_checksum_validate(config),
        measure_restore(config),
    ]
}

/// Measures `add_order` of a fresh order into a level with fixed resting
/// depth. Every harness-owned buffer (`orders`, `handles`) is allocated
/// BEFORE counting starts and dropped AFTER counting stops, so only
/// `add_order`'s own allocations land inside `[before, after)`.
///
/// An earlier version of this function allocated `handles` (a
/// `Vec::with_capacity`, hence one real allocation) AFTER `alloc::enable()`
/// and consumed `orders` with `for order in orders` — since `OrderType<()>`
/// is `Copy`, that consumed the whole `Vec<OrderType<()>>` via
/// `IntoIterator`, and the now-empty `orders` buffer's own deallocation ran
/// at the end of the `for` loop, still inside the counted window, before
/// `after` was read. Both counted the harness's own bookkeeping as if it
/// were part of `add_order`'s cost (issue #142 review finding 3). Iterating
/// `&orders` by reference and copying each element (`OrderType<()>` is
/// `Copy`) avoids consuming `orders` at all during the counted window.
fn measure_add_order(config: &Config) -> AllocReport {
    let reps = config.alloc_reps;
    let level = fixtures::seeded_standard_level(1_000, Side::Buy, 10);
    let orders: Vec<OrderType<()>> = (0..reps as u64)
        .map(|i| fixtures::standard_order(1_000 + i, Side::Buy, 10, TimeInForce::Gtc))
        .collect();
    let mut handles: Vec<Arc<OrderType<()>>> = Vec::with_capacity(reps);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for &order in &orders {
        handles.push(
            level
                .add_order(order)
                .expect("alloc measurement: add_order must succeed for a fresh id"),
        );
    }
    let after = AllocStats::read();
    alloc::disable();
    // Drop the returned handles, the input orders and the level outside the
    // counted window.
    drop(handles);
    drop(orders);
    drop(level);

    AllocReport {
        name: "add_order",
        reps,
        totals: after.since(before),
    }
}

/// Measures `UuidGenerator::try_next` alone (issue #146): the checked
/// reservation plus the counter-to-name encoding and the UUIDv5 hash. The
/// output buffer is pre-sized before counting starts.
fn measure_uuid_try_next(config: &Config) -> AllocReport {
    let reps = config.alloc_reps;
    let generator = fixtures::trade_id_generator();
    let mut ids = Vec::with_capacity(reps);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for _ in 0..reps {
        ids.push(generator.try_next());
    }
    let after = AllocStats::read();
    alloc::disable();

    assert!(
        ids.iter().all(Result::is_ok),
        "alloc measurement (uuid_try_next): a fresh generator must not exhaust"
    );
    drop(ids);

    AllocReport {
        name: "uuid_try_next",
        reps,
        totals: after.since(before),
    }
}

/// Measures one `match_order` sweep that emits `SWEEP_MAKERS` trades (issue
/// #146: "actual matching workloads with one and many trades"). Each
/// repetition sweeps its own freshly seeded level; seeding and teardown run
/// outside the counted window. `reps` is the number of sweeps, so the per-op
/// figure is per SWEEP (divide by `SWEEP_MAKERS` for per-fill).
fn measure_match_sweep_100(config: &Config) -> AllocReport {
    const SWEEP_MAKERS: u64 = 100;
    const QTY: u64 = 10;
    let reps = config.alloc_reps;
    let levels: Vec<PriceLevel> = (0..reps)
        .map(|_| {
            let level = PriceLevel::new(LEVEL_PRICE);
            for i in 0..SWEEP_MAKERS {
                level
                    .add_order(fixtures::standard_order(
                        i,
                        Side::Sell,
                        QTY,
                        TimeInForce::Gtc,
                    ))
                    .expect("alloc measurement: seeding a fresh maker id must succeed");
            }
            level
        })
        .collect();
    let generator = fixtures::trade_id_generator();
    let mut results = Vec::with_capacity(reps);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for (i, level) in levels.iter().enumerate() {
        results.push(level.match_order(
            SWEEP_MAKERS * QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        ));
    }
    let after = AllocStats::read();
    alloc::disable();

    let filled = results
        .iter()
        .filter(|r| {
            r.outcome() == MatchOutcome::Filled && r.trades().len() == SWEEP_MAKERS as usize
        })
        .count();
    assert_eq!(
        filled, reps,
        "alloc measurement (match_sweep_100): every sweep must fill all {SWEEP_MAKERS} makers"
    );

    drop(results);
    drop(levels);

    AllocReport {
        name: "match_sweep_100",
        reps,
        totals: after.since(before),
    }
}

/// Measures a full-fill `match_order` call against a dedicated fresh maker
/// per repetition (same shape as `scenarios::matching::match_full`). Also
/// asserts, after counting is disabled, that every fill was recorded
/// cleanly (issue #142 review finding 1 — see
/// [`fixtures::EXECUTION_TIMESTAMP_MS`]'s docs).
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
    let mut results = Vec::with_capacity(reps);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for i in 0..reps {
        results.push(level.match_order(
            QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        ));
    }
    let after = AllocStats::read();
    alloc::disable();

    let filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::Filled)
        .count();
    assert_eq!(
        filled, reps,
        "alloc measurement (match_full): every call must fully fill against its dedicated maker"
    );
    fixtures::assert_stats_healthy(&level, reps as u64 * QTY, "alloc measurement (match_full)");

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
    let mut snapshots = Vec::with_capacity(reps);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
    for _ in 0..reps {
        snapshots.push(level.snapshot());
    }
    let after = AllocStats::read();
    alloc::disable();

    let succeeded = snapshots.iter().filter(|s| s.is_ok()).count();
    assert_eq!(
        succeeded, reps,
        "alloc measurement (snapshot_capture): every snapshot() call must succeed"
    );
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
    let mut restored = Vec::with_capacity(reps);

    alloc::reset();
    alloc::enable();
    let before = AllocStats::read();
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
