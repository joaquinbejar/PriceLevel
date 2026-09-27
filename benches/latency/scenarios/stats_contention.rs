// benches/latency/scenarios/stats_contention.rs
//! Statistics cache-contention scenarios (issue #154).
//!
//! Question under test: does `PriceLevelStatistics`' layout (the producer
//! counters `orders_added` / `orders_removed` packed next to the execution
//! aggregates and the `stats_seq` seqlock word, 96 bytes in one heap
//! allocation) add measurable latency when producers, the matcher and
//! statistics readers share one level?
//!
//! Every case uses the supported schedule of the writer contract (issue #153):
//! ONE matcher thread (the caller's), any number of producer threads doing
//! `update_order(Cancel)` + `add_order`, and any number of reader threads doing
//! statistics reads. No case ever runs two `record_execution` calls at once.
//!
//! # Level-level cases (`statsc_{ok,overflow}_*`)
//!
//! The matcher level is pre-seeded, in FIFO order, with `warmup + ops`
//! dedicated 1-quantity makers (ids `0..`) and then a producer churn pool
//! (`POOL_PER_PRODUCER` orders per configured producer, behind the makers).
//! Each timed `match_order(1)` therefore consumes exactly the maker at the
//! front: maker `i` on call `i`, which is asserted after the loop (the pool is
//! never touched, so producers always find their own orders). Every case
//! builds its levels identically through `PriceLevel::from_snapshot`,
//! whatever its thread topology, so depth and construction path are the same
//! in the control and the contended runs.
//!
//! - `single`: matcher only (control).
//! - `same_producers` / `same_readers` / `same_mixed`: the workers act on the
//!   matcher's own level.
//! - `indep_mixed`: the same workers act on a second, identically-built level
//!   (control for machine load, core placement and memory bandwidth: same
//!   thread count, nothing shared with the matcher's level).
//!
//! The mixed cases also report producer 0's `add_order` latency and reader
//! 0's statistics `Clone` (seqlock multi-field read) latency, recorded only
//! while the matcher's measured loop runs.
//!
//! # Recording outcome is a separate axis
//!
//! `ok` levels start with fresh statistics: every `record_execution`
//! succeeds, asserted after the loop (`!stats_degraded()`, exact
//! `quantity_executed()`). `overflow` levels start with `sum_waiting_time`
//! restored at `u64::MAX`: every record commits `orders_executed`,
//! `quantity_executed` and `value_executed`, overflows on the waiting-time add,
//! rolls all three back and formats its error inside the write section (the
//! longest rejected path), asserted after the loop (`stats_degraded()`, zero
//! `quantity_executed()`, `sum_waiting_time()` still `u64::MAX`). The two are
//! different code paths and are never reported as one number.
//!
//! # Raw statistics-object cases (`statsc_raw_*`)
//!
//! These drive a bare `PriceLevelStatistics` with no level around it, to
//! bound the cost the statistics cache lines alone can add. The matcher
//! thread times a batch of [`RAW_BATCH`] `record_execution` calls per sample
//! (a single call is below the 41.67 ns tick of the Apple timer). Producers
//! call `record_order_added` / `record_order_removed` on the same object (they
//! write different fields from the matcher: pure false sharing) or on a
//! separate object (control). Readers `Clone` the same object (true sharing
//! on `stats_seq` and every field, inherent to the seqlock).
//!
//! Every case is closed-loop; see `manifest::COORDINATED_OMISSION_DISCLOSURE`.
//! Threads are not pinned (macOS exposes no affinity API) and may run on
//! performance or efficiency cores.

use crate::config::Config;
use crate::fixtures::{self, BASE_TIMESTAMP_MS, EXECUTION_TIMESTAMP_MS, TAKER_ID_BASE};
use crate::report::ScenarioReport;
use pricelevel::PriceLevelStatistics;
use pricelevel::prelude::*;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

/// First id of the producers' churn pools (disjoint from the matcher makers).
const POOL_ID_BASE: u64 = 20_000_000;
/// Churn orders owned by each producer thread.
const POOL_PER_PRODUCER: u64 = 500;
/// Quantity of each churn order.
const POOL_QUANTITY: u64 = 5;
/// A reader takes one full `PriceLevel::snapshot()` every this many ops.
const SNAPSHOT_EVERY: u64 = 1_024;
/// `record_execution` calls timed per sample in the raw cases.
const RAW_BATCH: usize = 16;

/// Whether the matcher's statistics recording succeeds or is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recording {
    /// Every `record_execution` succeeds.
    Success,
    /// Every `record_execution` overflows `sum_waiting_time` and rolls back.
    OverflowRollback,
}

/// Where the worker threads act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// On the matcher's own level / statistics object.
    Same,
    /// On a second, identically-built level / statistics object.
    Independent,
}

/// Start / measure / stop protocol shared by the matcher and its workers.
struct Control {
    ready: Barrier,
    go: AtomicBool,
    measuring: AtomicBool,
    stop: AtomicBool,
}

impl Control {
    fn new(workers: usize) -> Self {
        Self {
            ready: Barrier::new(workers + 1),
            go: AtomicBool::new(false),
            measuring: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        }
    }

    fn wait_for_go(&self) {
        self.ready.wait();
        while !self.go.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn measuring(&self) -> bool {
        self.measuring.load(Ordering::Relaxed)
    }
}

/// One worker thread's accounting.
#[derive(Debug, Default)]
struct WorkerOutcome {
    completed: u64,
    errors: u64,
    snapshots_ok: u64,
    snapshots_err: u64,
    /// Timed samples (producer 0: `add_order`; reader 0: stats `Clone`).
    samples_ns: Vec<u64>,
    active_secs: f64,
}

/// Runs `workers` threads executing `worker(index, &control)` while the
/// calling thread runs `matcher(&control)`, then stops and joins them.
fn with_workers<W, M, R>(workers: usize, worker: W, matcher: M) -> (R, Vec<WorkerOutcome>)
where
    W: Fn(usize, &Control) -> WorkerOutcome + Sync,
    M: FnOnce(&Control) -> R,
{
    let control = Control::new(workers);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|index| {
                let control = &control;
                let worker = &worker;
                scope.spawn(move || worker(index, control))
            })
            .collect();
        control.ready.wait();
        control.go.store(true, Ordering::Release);
        let result = matcher(&control);
        control.stop.store(true, Ordering::Relaxed);
        let outcomes = handles
            .into_iter()
            .map(|h| h.join().expect("stats_contention: a worker must not panic"))
            .collect();
        (result, outcomes)
    })
}

fn elapsed_ns(t0: Instant) -> u64 {
    u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Offset of the statistics object's data pointer inside a 128-byte line.
fn line_offset(stats: &Arc<PriceLevelStatistics>) -> usize {
    Arc::as_ptr(stats) as usize % 128
}

/// Statistics restored with `sum_waiting_time == u64::MAX`, so every later
/// `record_execution` with a positive waiting time overflows and rolls back.
fn saturated_statistics() -> PriceLevelStatistics {
    let mut value = serde_json::to_value(PriceLevelStatistics::new())
        .expect("stats_contention: statistics must serialize");
    value["sum_waiting_time"] = serde_json::json!(u64::MAX);
    serde_json::from_value(value).expect("stats_contention: saturated statistics must decode")
}

fn statistics_for(recording: Recording) -> PriceLevelStatistics {
    match recording {
        Recording::Success => PriceLevelStatistics::new(),
        Recording::OverflowRollback => saturated_statistics(),
    }
}

/// Builds a level holding `makers` 1-quantity makers (ids `0..makers`, FIFO
/// front) followed by `producers * POOL_PER_PRODUCER` churn orders.
fn build_level(recording: Recording, makers: u64, producers: usize) -> PriceLevel {
    let pool = producers as u64 * POOL_PER_PRODUCER;
    let mut orders = Vec::with_capacity(usize::try_from(makers + pool).unwrap_or(0));
    for id in 0..makers {
        orders.push(Arc::new(fixtures::standard_order(
            id,
            Side::Sell,
            1,
            TimeInForce::Gtc,
        )));
    }
    for k in 0..pool {
        orders.push(Arc::new(fixtures::standard_order(
            POOL_ID_BASE + k,
            Side::Sell,
            POOL_QUANTITY,
            TimeInForce::Gtc,
        )));
    }
    let snapshot = PriceLevelSnapshot::with_orders_and_stats(
        Price::new(fixtures::LEVEL_PRICE),
        orders,
        statistics_for(recording),
    )
    .expect("stats_contention: fixture snapshot must build");
    PriceLevel::from_snapshot(snapshot).expect("stats_contention: fixture level must restore")
}

/// Producer loop: cancel then re-add its own churn orders, round-robin.
/// Producer 0 times its `add_order` calls while the matcher is measuring.
fn producer_loop(index: usize, level: &PriceLevel, control: &Control, cap: usize) -> WorkerOutcome {
    let mut out = WorkerOutcome {
        samples_ns: Vec::with_capacity(if index == 0 { cap } else { 0 }),
        ..WorkerOutcome::default()
    };
    control.wait_for_go();
    let start = Instant::now();
    let base = POOL_ID_BASE + index as u64 * POOL_PER_PRODUCER;
    let mut k: u64 = 0;
    while !control.stopped() {
        let id = base + k % POOL_PER_PRODUCER;
        match level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(id),
        }) {
            Ok(Some(_)) => {}
            _ => out.errors += 1,
        }
        let order = fixtures::standard_order(id, Side::Sell, POOL_QUANTITY, TimeInForce::Gtc);
        let record = index == 0 && control.measuring() && out.samples_ns.len() < cap;
        let t0 = Instant::now();
        let result = level.add_order(order);
        let ns = elapsed_ns(t0);
        if result.is_err() {
            out.errors += 1;
        }
        if record {
            out.samples_ns.push(ns);
        }
        out.completed += 2;
        k += 1;
    }
    out.active_secs = start.elapsed().as_secs_f64();
    out
}

/// Reader loop: statistics point reads and seqlock `Clone`s through
/// `PriceLevel::stats()`, plus a full `snapshot()` every `SNAPSHOT_EVERY` ops.
/// Reader 0 times its `Clone` calls while the matcher is measuring.
fn reader_loop(index: usize, level: &PriceLevel, control: &Control, cap: usize) -> WorkerOutcome {
    let mut out = WorkerOutcome {
        samples_ns: Vec::with_capacity(if index == 0 { cap } else { 0 }),
        ..WorkerOutcome::default()
    };
    control.wait_for_go();
    let start = Instant::now();
    let mut k: u64 = 0;
    while !control.stopped() {
        let stats = level.stats();
        if k % SNAPSHOT_EVERY == SNAPSHOT_EVERY - 1 {
            match level.snapshot() {
                Ok(snapshot) => {
                    black_box(&snapshot);
                    out.snapshots_ok += 1;
                }
                Err(_) => out.snapshots_err += 1,
            }
        } else if k.is_multiple_of(2) {
            black_box(stats.orders_added());
            black_box(stats.quantity_executed());
            black_box(stats.stats_degraded());
        } else {
            let record = index == 0 && control.measuring() && out.samples_ns.len() < cap;
            let t0 = Instant::now();
            let copy = (*stats).clone();
            let ns = elapsed_ns(t0);
            black_box(&copy);
            if record {
                out.samples_ns.push(ns);
            }
        }
        out.completed += 1;
        k += 1;
    }
    out.active_secs = start.elapsed().as_secs_f64();
    out
}

/// One level-level case.
struct LevelCase {
    name: &'static str,
    recording: Recording,
    producers: usize,
    readers: usize,
    placement: Placement,
    /// Also report producer 0 / reader 0 latency rows.
    worker_rows: bool,
}

/// Sum of each worker's own `completed / active_secs`.
fn rate(outcomes: &[&WorkerOutcome]) -> f64 {
    outcomes.iter().fold(0.0, |acc, o| {
        acc + o.completed as f64 / o.active_secs.max(f64::EPSILON)
    })
}

fn run_level_case(config: &Config, case: &LevelCase) -> Vec<ScenarioReport> {
    let warmup = config.warmup;
    let ops = config.stats_ops;
    let makers = (warmup + ops) as u64;
    // Every case seeds the churn pool for the configured producer count, so
    // all cases share one fixture shape whatever their own thread topology.
    let seeded_producers = config.stats_producers;
    let matcher_level = build_level(case.recording, makers, seeded_producers);
    let other_level = build_level(case.recording, makers, seeded_producers);
    let worker_level = match case.placement {
        Placement::Same => &matcher_level,
        Placement::Independent => &other_level,
    };
    let stats_line_offset = line_offset(&matcher_level.stats());
    let generator = fixtures::trade_id_generator();

    let workers = case.producers + case.readers;
    let ((durations_ns, filled, fifo_ok, window_secs), outcomes) = with_workers(
        workers,
        |index, control| {
            if index < case.producers {
                producer_loop(index, worker_level, control, ops)
            } else {
                reader_loop(index - case.producers, worker_level, control, ops)
            }
        },
        |control| {
            let mut durations_ns = Vec::with_capacity(ops);
            let mut filled = 0usize;
            let mut fifo_ok = true;
            let mut window_start = Instant::now();
            for i in 0..warmup + ops {
                if i == warmup {
                    control.measuring.store(true, Ordering::Relaxed);
                    window_start = Instant::now();
                }
                let t0 = Instant::now();
                let result = matcher_level.match_order(
                    1,
                    Id::from_u64(TAKER_ID_BASE + i as u64),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    TimestampMs::new(EXECUTION_TIMESTAMP_MS),
                    &generator,
                );
                let ns = elapsed_ns(t0);
                if i >= warmup {
                    durations_ns.push(ns);
                }
                if result.outcome() == MatchOutcome::Filled {
                    filled += 1;
                }
                let trades = result.trades().as_vec();
                fifo_ok &= trades.len() == 1
                    && trades
                        .first()
                        .is_some_and(|t| t.maker_order_id() == Id::from_u64(i as u64));
            }
            let window_secs = window_start.elapsed().as_secs_f64();
            (durations_ns, filled, fifo_ok, window_secs)
        },
    );

    let context = format!("stats_contention({})", case.name);
    assert_eq!(filled, warmup + ops, "{context}: every match must fill");
    assert!(
        fifo_ok,
        "{context}: call i must consume exactly maker i (FIFO)"
    );
    for (index, o) in outcomes.iter().enumerate() {
        assert_eq!(
            o.errors, 0,
            "{context}: worker {index} saw unexpected errors"
        );
    }
    let stats = matcher_level.stats();
    let recorded = match case.recording {
        Recording::Success => {
            fixtures::assert_stats_healthy(&matcher_level, makers, &context);
            "recording succeeded on every fill"
        }
        Recording::OverflowRollback => {
            assert!(stats.stats_degraded(), "{context}: must be degraded");
            assert_eq!(
                stats.quantity_executed(),
                0,
                "{context}: rollback must undo"
            );
            assert_eq!(stats.orders_executed(), 0, "{context}: rollback must undo");
            assert_eq!(stats.sum_waiting_time(), u64::MAX, "{context}: untouched");
            "recording overflowed and rolled back on every fill"
        }
    };

    let producers: Vec<&WorkerOutcome> = outcomes.iter().take(case.producers).collect();
    let readers: Vec<&WorkerOutcome> = outcomes.iter().skip(case.producers).collect();
    let snapshots_ok: u64 = readers.iter().map(|o| o.snapshots_ok).sum();
    let snapshots_err: u64 = readers.iter().map(|o| o.snapshots_err).sum();
    let placement = match case.placement {
        Placement::Same => "same level",
        Placement::Independent => "independent level",
    };
    let depth = makers + seeded_producers as u64 * POOL_PER_PRODUCER;
    let note = format!(
        "{filled}/{} Filled, FIFO exact; {recorded}; matcher {:.0} ops/s; {} producers ({:.0} ops/s) + {} readers ({:.0} ops/s, snapshots {snapshots_ok} ok / {snapshots_err} retried-out) on {placement}; stats data at line offset {stats_line_offset}/128",
        warmup + ops,
        ops as f64 / window_secs.max(f64::EPSILON),
        case.producers,
        rate(&producers),
        case.readers,
        rate(&readers),
    );

    let mut reports = vec![ScenarioReport::from_samples(
        case.name,
        "stats_contention",
        depth,
        "PriceLevel::match_order (1 unit, GTC, one maker consumed) on the matcher thread",
        durations_ns,
        note,
    )];
    if case.worker_rows {
        let mut outcomes = outcomes;
        if let Some(p0) = outcomes.get_mut(0).filter(|_| case.producers > 0) {
            let samples = std::mem::take(&mut p0.samples_ns);
            reports.push(ScenarioReport::from_samples(
                format!("{}_producer_add", case.name),
                "stats_contention",
                depth,
                "PriceLevel::add_order on producer 0, while the matcher measures",
                samples,
                format!("{placement}; every cancel found and every re-add succeeded"),
            ));
        }
        if let Some(r0) = outcomes
            .get_mut(case.producers)
            .filter(|_| case.readers > 0)
        {
            let samples = std::mem::take(&mut r0.samples_ns);
            reports.push(ScenarioReport::from_samples(
                format!("{}_reader_clone", case.name),
                "stats_contention",
                depth,
                "PriceLevelStatistics::clone (seqlock read) on reader 0, while the matcher measures",
                samples,
                format!("{placement}; {recorded}"),
            ));
        }
    }
    reports
}

/// Worker roles in the raw statistics-object cases.
#[derive(Debug, Clone, Copy)]
enum RawWorkers {
    None,
    Producers,
    Readers,
}

fn run_raw_case(
    config: &Config,
    name: &'static str,
    recording: Recording,
    role: RawWorkers,
    placement: Placement,
) -> ScenarioReport {
    let warmup = config.warmup;
    let ops = config.stats_ops;
    let matcher_stats = Arc::new(statistics_for(recording));
    let other_stats = Arc::new(PriceLevelStatistics::new());
    let worker_stats = match placement {
        Placement::Same => &matcher_stats,
        Placement::Independent => &other_stats,
    };
    let workers = match role {
        RawWorkers::None => 0,
        RawWorkers::Producers => config.stats_producers,
        RawWorkers::Readers => config.stats_readers,
    };
    let order_ts = BASE_TIMESTAMP_MS;

    let ((durations_ns, ok, err), outcomes) = with_workers(
        workers,
        |_, control| {
            let mut out = WorkerOutcome::default();
            control.wait_for_go();
            let start = Instant::now();
            while !control.stopped() {
                match role {
                    RawWorkers::Producers => {
                        let added = worker_stats.record_order_added();
                        let removed = worker_stats.record_order_removed();
                        if added.is_err() || removed.is_err() {
                            out.errors += 1;
                        }
                        out.completed += 2;
                    }
                    RawWorkers::Readers => {
                        black_box((**worker_stats).clone());
                        out.completed += 1;
                    }
                    RawWorkers::None => break,
                }
            }
            out.active_secs = start.elapsed().as_secs_f64();
            out
        },
        |_| {
            let mut durations_ns = Vec::with_capacity(ops);
            let (mut ok, mut err) = (0u64, 0u64);
            for i in 0..warmup + ops {
                let t0 = Instant::now();
                for _ in 0..RAW_BATCH {
                    match black_box(&matcher_stats).record_execution(
                        1,
                        fixtures::LEVEL_PRICE,
                        order_ts,
                        EXECUTION_TIMESTAMP_MS,
                    ) {
                        Ok(()) => ok += 1,
                        Err(e) => {
                            black_box(e);
                            err += 1;
                        }
                    }
                }
                let ns = elapsed_ns(t0);
                if i >= warmup {
                    durations_ns.push(ns);
                }
            }
            (durations_ns, ok, err)
        },
    );

    let total = ((warmup + ops) * RAW_BATCH) as u64;
    for o in &outcomes {
        assert_eq!(o.errors, 0, "stats_contention({name}): worker errors");
    }
    let recorded = match recording {
        Recording::Success => {
            assert_eq!((ok, err), (total, 0), "stats_contention({name})");
            assert_eq!(matcher_stats.quantity_executed(), total);
            assert!(!matcher_stats.stats_degraded());
            "every record succeeded"
        }
        Recording::OverflowRollback => {
            assert_eq!((ok, err), (0, total), "stats_contention({name})");
            assert_eq!(matcher_stats.quantity_executed(), 0);
            assert!(matcher_stats.stats_degraded());
            "every record overflowed and rolled back"
        }
    };
    let placement = match placement {
        Placement::Same => "same object",
        Placement::Independent => "independent object",
    };
    let role_name = match role {
        RawWorkers::None => "workers",
        RawWorkers::Producers => "producers (record_order_added + record_order_removed)",
        RawWorkers::Readers => "readers (Clone)",
    };
    let worker_refs: Vec<&WorkerOutcome> = outcomes.iter().collect();
    ScenarioReport::from_samples(
        name,
        "stats_contention",
        0,
        "batch of 16 PriceLevelStatistics::record_execution calls (one sample = one batch)",
        durations_ns,
        format!(
            "{recorded}; {workers} {role_name} on {placement} ({:.0} ops/s); stats data at line offset {}/128",
            rate(&worker_refs),
            line_offset(&matcher_stats),
        ),
    )
}

/// Runs every statistics-contention case (issue #154).
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    let p = config.stats_producers;
    let r = config.stats_readers;
    let level_cases = [
        LevelCase {
            name: "statsc_ok_single",
            recording: Recording::Success,
            producers: 0,
            readers: 0,
            placement: Placement::Same,
            worker_rows: false,
        },
        LevelCase {
            name: "statsc_ok_same_producers",
            recording: Recording::Success,
            producers: p,
            readers: 0,
            placement: Placement::Same,
            worker_rows: false,
        },
        LevelCase {
            name: "statsc_ok_same_readers",
            recording: Recording::Success,
            producers: 0,
            readers: r,
            placement: Placement::Same,
            worker_rows: false,
        },
        LevelCase {
            name: "statsc_ok_same_mixed",
            recording: Recording::Success,
            producers: p,
            readers: r,
            placement: Placement::Same,
            worker_rows: true,
        },
        LevelCase {
            name: "statsc_ok_indep_mixed",
            recording: Recording::Success,
            producers: p,
            readers: r,
            placement: Placement::Independent,
            worker_rows: true,
        },
        LevelCase {
            name: "statsc_overflow_single",
            recording: Recording::OverflowRollback,
            producers: 0,
            readers: 0,
            placement: Placement::Same,
            worker_rows: false,
        },
        LevelCase {
            name: "statsc_overflow_same_mixed",
            recording: Recording::OverflowRollback,
            producers: p,
            readers: r,
            placement: Placement::Same,
            worker_rows: true,
        },
        LevelCase {
            name: "statsc_overflow_indep_mixed",
            recording: Recording::OverflowRollback,
            producers: p,
            readers: r,
            placement: Placement::Independent,
            worker_rows: true,
        },
    ];

    let mut reports = Vec::new();
    for case in &level_cases {
        reports.extend(run_level_case(config, case));
    }
    let raw_cases = [
        (
            "statsc_raw_ok_single",
            Recording::Success,
            RawWorkers::None,
            Placement::Same,
        ),
        (
            "statsc_raw_ok_same_producers",
            Recording::Success,
            RawWorkers::Producers,
            Placement::Same,
        ),
        (
            "statsc_raw_ok_indep_producers",
            Recording::Success,
            RawWorkers::Producers,
            Placement::Independent,
        ),
        (
            "statsc_raw_ok_same_readers",
            Recording::Success,
            RawWorkers::Readers,
            Placement::Same,
        ),
        (
            "statsc_raw_overflow_single",
            Recording::OverflowRollback,
            RawWorkers::None,
            Placement::Same,
        ),
        (
            "statsc_raw_overflow_same_producers",
            Recording::OverflowRollback,
            RawWorkers::Producers,
            Placement::Same,
        ),
    ];
    for (name, recording, role, placement) in raw_cases {
        reports.push(run_raw_case(config, name, recording, role, placement));
    }
    reports
}
