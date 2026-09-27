// benches/latency/scenarios/fok_depth.rs
//! Fill-or-kill feasibility cost versus resting depth (issue #143).
//!
//! The workload is the issue's reproduction: a fresh level per
//! configuration, standard GTC Sell makers at price 100 and timestamp 1,
//! maker quantity 1, taker quantity 1 at timestamp 2, `TakerKind::Standard`,
//! Sequential maker ids `0..depth`, replacement ids `depth + i`, taker id
//! `u64::MAX`, and a trade-id generator with namespace `Uuid::nil()`. The
//! stated depth is maintained by admitting one replacement maker in an
//! untimed teardown after each timed call. Up to 1,000 warmup calls, then up
//! to 5,000 FOK samples or 10,000 GTC samples per configuration (both capped
//! by `PL_LATENCY_WARMUP` / `PL_LATENCY_SAMPLES` for a quick run).
//!
//! Cases, each at depths 1, 100 and 10,000:
//!
//! * `fok_first_maker` / `gtc_first_maker`: the front maker fills the taker.
//! * `fok_rejected`: a taker one unit larger than the level; the dry run
//!   must visit every maker to prove the kill.
//! * `fok_replenish`: iceberg makers (1 visible + 1,000,000 hidden); a qty-2
//!   taker drains the front tranche (a replenishment re-sequenced at the
//!   tail) and one unit of the next maker (depths 100 and 10,000 only).
//!
//! Plus a contention pair at depth 10,000: one matcher thread repeatedly
//! runs the first-maker FOK (or, as the control, GTC) call while a writer
//! thread times its own `add_order` and `update_order(Cancel)` calls, so
//! the time mutators spend blocked behind the fill-or-kill guard's exclusive
//! section is measured directly. Starvation anomalies there (issue #206)
//! are counted, not asserted; see [`writers_during`].

use crate::config::Config;
use crate::report::ScenarioReport;
use crate::timing::{measure, measure_with_teardown, warmup};
use pricelevel::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use uuid::Uuid;

const PRICE: u128 = 100;
const DEPTHS: [u64; 3] = [1, 100, 10_000];
const TAKER: u64 = u64::MAX;
const MAX_WARMUP: usize = 1_000;
const FOK_SAMPLES: usize = 5_000;
const GTC_SAMPLES: usize = 10_000;
const ICEBERG_HIDDEN: u64 = 1_000_000;
const CONTENTION_DEPTH: u64 = 10_000;
/// Writer ids, disjoint from every maker and replacement id.
const WRITER_ID_BASE: u64 = 1_000_000_000_000;

fn standard(id: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        quantity: Quantity::new(1),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn iceberg(id: u64) -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        visible_quantity: Quantity::new(1),
        hidden_quantity: Quantity::new(ICEBERG_HIDDEN),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn level_of(depth: u64, make: fn(u64) -> OrderType<()>) -> PriceLevel {
    let level = PriceLevel::new(PRICE);
    for id in 0..depth {
        level
            .add_order(make(id))
            .expect("fok_depth: seeding must succeed");
    }
    level
}

fn take(
    level: &PriceLevel,
    quantity: u64,
    tif: TimeInForce,
    generator: &UuidGenerator,
) -> MatchResult {
    std::hint::black_box(level.match_order(
        quantity,
        Id::from_u64(TAKER),
        tif,
        TakerKind::Standard,
        TimestampMs::new(2),
        generator,
    ))
}

fn assert_healthy(level: &PriceLevel, context: &str) {
    assert!(
        !level.stats().stats_degraded(),
        "{context}: statistics must not degrade"
    );
}

/// Runs every fill-or-kill depth case and the contention pair.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    let mut reports = Vec::new();
    for depth in DEPTHS {
        reports.push(first_maker(config, depth, TimeInForce::Fok));
        reports.push(first_maker(config, depth, TimeInForce::Gtc));
        reports.push(rejected(config, depth));
        if depth >= 2 {
            reports.push(replenish(config, depth));
        }
    }
    reports.extend(writers_during(config, TimeInForce::Fok));
    reports.extend(writers_during(config, TimeInForce::Gtc));
    reports
}

fn first_maker(config: &Config, depth: u64, tif: TimeInForce) -> ScenarioReport {
    let is_fok = matches!(tif, TimeInForce::Fok);
    let samples = config
        .samples
        .min(if is_fok { FOK_SAMPLES } else { GTC_SAMPLES });
    let warm = config.warmup.min(MAX_WARMUP);
    let level = level_of(depth, standard);
    let generator = UuidGenerator::new(Uuid::nil());
    let mut next = depth;
    let mut replace = |level: &PriceLevel| {
        level
            .add_order(standard(next))
            .expect("fok_depth: replacement add must succeed");
        next += 1;
    };

    warmup(warm, |_| {
        let result = take(&level, 1, tif, &generator);
        replace(&level);
        result
    });
    let (durations_ns, results) = measure_with_teardown(
        samples,
        |_| take(&level, 1, tif, &generator),
        |_, _| replace(&level),
    );
    let filled = results
        .iter()
        .filter(|r| r.executed_quantity() == Ok(Quantity::new(1)))
        .count();
    assert_eq!(
        filled, samples,
        "first_maker({depth}): every call executes 1"
    );
    assert_eq!(level.order_count(), depth as usize);
    assert_healthy(&level, "first_maker");
    drop(results);

    let name = if is_fok {
        "fok_first_maker"
    } else {
        "gtc_first_maker"
    };
    ScenarioReport::from_samples(
        format!("{name}@{depth}"),
        "fok_depth",
        depth,
        "PriceLevel::match_order — qty-1 taker, front maker fills (#143)",
        durations_ns,
        format!("{filled}/{samples} executed 1"),
    )
}

fn rejected(config: &Config, depth: u64) -> ScenarioReport {
    let samples = config.samples.min(FOK_SAMPLES);
    let warm = config.warmup.min(MAX_WARMUP);
    let level = level_of(depth, standard);
    let generator = UuidGenerator::new(Uuid::nil());
    warmup(warm, |_| {
        take(&level, depth + 1, TimeInForce::Fok, &generator)
    });
    let (durations_ns, results) = measure(samples, |_| {
        take(&level, depth + 1, TimeInForce::Fok, &generator)
    });
    let killed = results.iter().filter(|r| r.was_killed()).count();
    assert_eq!(killed, samples, "rejected({depth}): every FOK is killed");
    assert_eq!(level.order_count(), depth as usize);
    drop(results);
    ScenarioReport::from_samples(
        format!("fok_rejected@{depth}"),
        "fok_depth",
        depth,
        "PriceLevel::match_order — FOK one unit larger than the level (#143)",
        durations_ns,
        format!("{killed}/{samples} Killed"),
    )
}

fn replenish(config: &Config, depth: u64) -> ScenarioReport {
    let samples = config.samples.min(FOK_SAMPLES);
    let warm = config.warmup.min(MAX_WARMUP);
    let level = level_of(depth, iceberg);
    let generator = UuidGenerator::new(Uuid::nil());
    warmup(warm, |_| take(&level, 2, TimeInForce::Fok, &generator));
    let (durations_ns, results) =
        measure(samples, |_| take(&level, 2, TimeInForce::Fok, &generator));
    let filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::Filled && r.trades().len() == 2)
        .count();
    assert_eq!(
        filled, samples,
        "replenish({depth}): every FOK fills in 2 trades"
    );
    assert_eq!(level.order_count(), depth as usize);
    assert_healthy(&level, "replenish");
    drop(results);
    ScenarioReport::from_samples(
        format!("fok_replenish@{depth}"),
        "fok_depth",
        depth,
        "PriceLevel::match_order — qty-2 FOK over replenishing icebergs (#143)",
        durations_ns,
        format!("{filled}/{samples} Filled (2 trades)"),
    )
}

/// One matcher thread loops the first-maker call at depth 10,000 while this
/// thread times its own admission and cancellation of a writer-owned order.
///
/// # Anomalies are counted, not asserted (issue #206)
///
/// The writer's order `W` normally rests at the tail for the instant between
/// its add and its cancel, and a qty-1 taker never reaches it. But the
/// cancel takes the fill-or-kill guard's shared side, and `std::sync::RwLock`
/// gives it no fairness against a matcher that retakes the exclusive side in
/// a loop: on `origin/main` (a full-depth dry run per FOK) the cancel was
/// observed to wait about 1.5 s, roughly depth × FOK time. Meanwhile the
/// matcher consumes every maker ahead of `W`, `W` becomes the true front and
/// is filled, and the late cancel finds nothing. That is correct FIFO under
/// starvation, not a FIFO violation, so both events are counted and
/// reported in the outcome note (and so in `manifest.json`):
///
/// * `writer-owned consumed`: a matcher call filled a writer order;
/// * `cancel found nothing`: a writer cancel returned `Ok(None)`.
///
/// The public API exposes no insertion sequence, so each writer order
/// records an **admission bracket** instead: how many matcher replacement
/// adds had *completed* before its `add_order` started (those makers are
/// certainly older than `W`) and how many had *started* by the time it
/// returned (every later one is certainly younger). A consumed `W` is then
/// classified against the matcher's own front `m` at that call (the oldest
/// matcher maker still resting, ids being admission-ordered):
///
/// * `proven front`: every matcher maker possibly older than `W` was already
///   consumed, so `W` was the true front;
/// * `proven violation`: a matcher maker certainly older than `W` still
///   rested, so the sweep broke FIFO;
/// * `ambiguous`: `m` falls inside `W`'s bracket (concurrent adds).
///
/// A matcher maker consumed out of id order is counted as a violation too.
/// `PL_LATENCY_STRICT_FIFO=1` turns any anomaly into a hard failure that
/// prints every classified event; it is off by default so an unfair
/// scheduler cannot fail `cargo test --all-targets`.
fn writers_during(config: &Config, matcher_tif: TimeInForce) -> Vec<ScenarioReport> {
    let samples = config.contention_ops;
    let level = Arc::new(level_of(CONTENTION_DEPTH, standard));
    let ready = Arc::new(Barrier::new(2));
    let stop = Arc::new(AtomicBool::new(false));
    let matcher_calls = Arc::new(AtomicU64::new(0));
    // Matcher replacement adds started / completed (issue #206 brackets).
    let adds_started = Arc::new(AtomicU64::new(0));
    let adds_done = Arc::new(AtomicU64::new(0));

    let matcher = {
        let level = Arc::clone(&level);
        let ready = Arc::clone(&ready);
        let stop = Arc::clone(&stop);
        let calls = Arc::clone(&matcher_calls);
        let adds_started = Arc::clone(&adds_started);
        let adds_done = Arc::clone(&adds_done);
        thread::spawn(move || -> MatcherLog {
            let generator = UuidGenerator::new(Uuid::nil());
            let mut next = CONTENTION_DEPTH;
            let mut log = MatcherLog::default();
            ready.wait();
            while !stop.load(Ordering::Relaxed) {
                let result = take(&level, 1, matcher_tif, &generator);
                assert_eq!(
                    result.executed_quantity(),
                    Ok(Quantity::new(1)),
                    "matcher fills 1"
                );
                let maker = result.trades().as_vec().first().map(|t| t.maker_order_id());
                log.record(maker);
                adds_started.fetch_add(1, Ordering::SeqCst);
                level
                    .add_order(standard(next))
                    .expect("matcher replacement add must succeed");
                adds_done.fetch_add(1, Ordering::SeqCst);
                next += 1;
                calls.fetch_add(1, Ordering::Relaxed);
            }
            log
        })
    };

    ready.wait();
    let started = Instant::now();
    let mut add_ns = Vec::with_capacity(samples);
    let mut cancel_ns = Vec::with_capacity(samples);
    let mut brackets = Vec::with_capacity(samples);
    let mut cancel_missing: u64 = 0;
    for i in 0..samples as u64 {
        let id = WRITER_ID_BASE + i;
        let done_before = adds_done.load(Ordering::SeqCst);
        let t0 = Instant::now();
        let added = level.add_order(standard(id));
        let elapsed = t0.elapsed();
        let started_after = adds_started.load(Ordering::SeqCst);
        added.expect("writer add must succeed");
        add_ns.push(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
        brackets.push((done_before, started_after));

        let t0 = Instant::now();
        let cancelled = level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(id),
        });
        let elapsed = t0.elapsed();
        match cancelled {
            Ok(Some(_)) => {}
            Ok(None) => cancel_missing += 1,
            Err(err) => panic!("writer cancel {id} failed: {err}"),
        }
        cancel_ns.push(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    }
    let window = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    let log = matcher.join().expect("matcher thread must not panic");
    let anomalies = log.classify(&brackets, cancel_missing);
    // The matcher admits one replacement per call, including a call that
    // filled a writer order, so each such fill leaves one extra maker.
    assert_eq!(
        level.order_count() as u64,
        CONTENTION_DEPTH + anomalies.writer_consumed,
        "writers_during: resting count must be depth + writer orders the matcher consumed"
    );
    assert_healthy(&level, "writers_during");
    let calls = matcher_calls.load(Ordering::Relaxed);
    let tag = if matches!(matcher_tif, TimeInForce::Fok) {
        "fok"
    } else {
        "gtc"
    };
    if config.strict_fifo && anomalies.any() {
        panic!(
            "PL_LATENCY_STRICT_FIFO: writers_during_{tag}: {anomalies}\nevents (writer id, matcher front id, bracket [done_before, started_after], class):\n{}",
            anomalies.events.join("\n")
        );
    }
    let note = format!(
        "{samples} ok; matcher {calls} calls ({:.0}/s); {anomalies}",
        calls as f64 / window
    );
    vec![
        ScenarioReport::from_samples(
            format!("writer_add_during_{tag}@{CONTENTION_DEPTH}"),
            "fok_depth",
            CONTENTION_DEPTH,
            "PriceLevel::add_order — concurrent with a qty-1 matcher loop (#143)",
            add_ns,
            note.clone(),
        ),
        ScenarioReport::from_samples(
            format!("writer_cancel_during_{tag}@{CONTENTION_DEPTH}"),
            "fok_depth",
            CONTENTION_DEPTH,
            "PriceLevel::update_order(Cancel) — concurrent with a qty-1 matcher loop (#143)",
            cancel_ns,
            note,
        ),
    ]
}

/// What the matcher consumed, in call order (issue #206).
/// Recovers the `u64` a fixture id was built from: `Id::from_u64` stores it
/// big-endian in the leading eight UUID bytes with the rest zero.
fn fixture_u64(id: Id) -> Option<u64> {
    match id {
        Id::Sequential(value) => Some(value),
        Id::Uuid(uuid) => {
            let (head, tail) = uuid.as_bytes().split_at(8);
            if tail.iter().any(|b| *b != 0) {
                return None;
            }
            head.try_into().ok().map(u64::from_be_bytes)
        }
        _ => None,
    }
}

#[derive(Default)]
struct MatcherLog {
    /// Oldest matcher-owned maker id not yet consumed (ids are
    /// admission-ordered: seeds `0..depth`, then replacements).
    front: u64,
    /// `(writer index, matcher front id at that call)` per consumed writer
    /// order.
    writer_fills: Vec<(u64, u64)>,
    /// `(expected front id, consumed matcher id)` per out-of-order fill.
    out_of_order: Vec<(u64, u64)>,
}

impl MatcherLog {
    fn record(&mut self, maker: Option<Id>) {
        let Some(id) = maker.and_then(fixture_u64) else {
            panic!("matcher trade must carry a fixture maker id: {maker:?}");
        };
        if id >= WRITER_ID_BASE {
            self.writer_fills.push((id - WRITER_ID_BASE, self.front));
        } else if id == self.front {
            self.front += 1;
        } else {
            self.out_of_order.push((self.front, id));
        }
    }

    fn classify(&self, brackets: &[(u64, u64)], cancel_missing: u64) -> Anomalies {
        let mut out = Anomalies {
            writer_consumed: self.writer_fills.len() as u64,
            cancel_missing,
            out_of_order: self.out_of_order.len() as u64,
            ..Anomalies::default()
        };
        for &(index, front) in &self.writer_fills {
            let (done_before, started_after) = brackets[index as usize];
            // Matcher makers older than W for certain: seeds and the first
            // `done_before` replacements. Possibly older: up to
            // `started_after` replacements.
            let certainly_older_end = CONTENTION_DEPTH + done_before;
            let possibly_older_end = CONTENTION_DEPTH + started_after;
            let class = if front >= possibly_older_end {
                out.proven_front += 1;
                "proven front"
            } else if front < certainly_older_end {
                out.proven_violation += 1;
                "proven violation"
            } else {
                out.ambiguous += 1;
                "ambiguous"
            };
            out.events.push(format!(
                "W {} front {front} bracket [{done_before}, {started_after}] {class}",
                WRITER_ID_BASE + index
            ));
        }
        for &(expected, got) in &self.out_of_order {
            out.events.push(format!(
                "matcher expected {expected} consumed {got} out of order"
            ));
        }
        out
    }
}

#[derive(Default)]
struct Anomalies {
    writer_consumed: u64,
    cancel_missing: u64,
    out_of_order: u64,
    proven_front: u64,
    proven_violation: u64,
    ambiguous: u64,
    events: Vec<String>,
}

impl Anomalies {
    fn any(&self) -> bool {
        self.writer_consumed > 0 || self.cancel_missing > 0 || self.out_of_order > 0
    }
}

impl std::fmt::Display for Anomalies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "writer-owned consumed {} (proven front {}, proven violation {}, ambiguous {}); \
             cancel found nothing {}; matcher out of order {}",
            self.writer_consumed,
            self.proven_front,
            self.proven_violation,
            self.ambiguous,
            self.cancel_missing,
            self.out_of_order
        )
    }
}
