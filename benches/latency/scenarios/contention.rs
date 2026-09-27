// benches/latency/scenarios/contention.rs
//! The one supported contention scenario the issue asks for: one logical
//! matcher thread running `match_order` while `N - 1` other threads
//! concurrently admit, cancel and read on the SAME shared level — the
//! "concurrent mutators + one matcher" contract documented in
//! `src/lib.rs`'s "Concurrency Model" and `doc/architecture.md`'s
//! "Supported execution model".
//!
//! Two variants run: a GTC matcher (never takes the fill-or-kill guard's
//! exclusive side) and an FOK matcher (takes it exclusively per call), so
//! the FOK-guard's blocking effect on matcher-observed latency is visible as
//! a side-by-side comparison against the same writer-thread load, rather
//! than conflated with it (issue #142: "measure FOK blocking effects
//! separately from uncontended service time"). Compare either matcher's
//! contended p50/p99 here against the uncontended `tif_gtc_full_match` /
//! `tif_fok_success` scenarios in `tif.rs` for the "separately from
//! uncontended service time" half of that requirement.
//!
//! This is still a CLOSED-LOOP measurement per thread — see
//! `manifest::COORDINATED_OMISSION_DISCLOSURE`.

use crate::config::Config;
use crate::fixtures;
use crate::report::ScenarioReport;
use pricelevel::prelude::*;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

/// Id range writer threads recycle for their own add / cancel churn.
const CHURN_ID_BASE: u64 = 10_000_000;
/// Fixed size of the writer threads' recycled id pool.
const CHURN_POOL_SIZE: u64 = 2_000;
/// Id range for the matcher's own taker ids.
const TAKER_ID_BASE: u64 = 900_000_000;

/// Runs both contention variants and returns one report each.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    vec![
        run_contention(config, TimeInForce::Gtc, "contention_gtc_matcher"),
        run_contention(config, TimeInForce::Fok, "contention_fok_matcher"),
    ]
}

fn run_contention(config: &Config, matcher_tif: TimeInForce, name: &'static str) -> ScenarioReport {
    let writer_threads = config.contention_threads.saturating_sub(1).max(1);
    let matcher_ops = config.contention_ops;

    // Matcher-target depth: one resting maker per matcher operation, so the
    // matcher never runs dry (each op consumes exactly one).
    let level = fixtures::seeded_standard_level(matcher_ops as u64, Side::Sell, 1);
    // Churn pool, seeded strictly AFTER the matcher-target block above, so
    // every churn order sits behind every matcher-target order in FIFO
    // order and can never be the one the matcher consumes.
    for i in 0..CHURN_POOL_SIZE {
        level
            .add_order(fixtures::standard_order(
                CHURN_ID_BASE + i,
                Side::Sell,
                5,
                TimeInForce::Gtc,
            ))
            .expect("contention: churn pool seeding must succeed");
    }

    let level = Arc::new(level);
    let ready = Arc::new(Barrier::new(writer_threads + 1));
    let go = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));

    let writer_handles: Vec<_> = (0..writer_threads)
        .map(|writer_id| {
            let level = Arc::clone(&level);
            let ready = Arc::clone(&ready);
            let go = Arc::clone(&go);
            let stop = Arc::clone(&stop);
            thread::spawn(move || -> WriterOutcome {
                writer_loop(writer_id, &level, &ready, &go, &stop)
            })
        })
        .collect();

    ready.wait();
    let generator = fixtures::trade_id_generator();
    let mut durations_ns: Vec<u64> = Vec::with_capacity(matcher_ops);
    let mut outcomes: Vec<MatchOutcome> = Vec::with_capacity(matcher_ops);
    go.store(true, Ordering::Release);

    for i in 0..matcher_ops {
        let t0 = Instant::now();
        let result = level.match_order(
            1,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            matcher_tif,
            TakerKind::Standard,
            TimestampMs::new(0),
            &generator,
        );
        let elapsed = t0.elapsed();
        outcomes.push(result.outcome());
        durations_ns.push(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    }
    stop.store(true, Ordering::Relaxed);

    let mut writer_outcome = WriterOutcome::default();
    for handle in writer_handles {
        let outcome = handle
            .join()
            .expect("contention: a writer thread must not panic");
        writer_outcome = writer_outcome.merge(outcome);
    }
    assert_eq!(
        writer_outcome.errors, 0,
        "contention({name}): writer threads must report zero unexpected errors"
    );

    let filled = outcomes
        .iter()
        .filter(|o| **o == MatchOutcome::Filled)
        .count();
    let killed = outcomes.iter().filter(|o| o.was_killed()).count();
    // Every one of the matcher's dedicated single-order targets is
    // guaranteed matchable (qty 1 maker, qty 1 taker), so a GTC matcher must
    // fill every call; an FOK matcher facing an always-feasible 1-for-1 match
    // must also fill every call (a kill would indicate the level lost
    // feasibility unexpectedly under contention, which would itself be a
    // real correctness signal worth failing loudly on).
    assert_eq!(
        filled, matcher_ops,
        "contention({name}): every matcher op targets its own dedicated 1-quantity maker and must \
         fill; {killed} were killed instead"
    );

    ScenarioReport::from_samples(
        name,
        "contention",
        matcher_ops as u64,
        "PriceLevel::match_order — one matcher thread under N-1 concurrent admissions/cancels/reads",
        durations_ns,
        format!(
            "matcher: {filled}/{matcher_ops} Filled; writers: {} completed ({} successful, {} \
             missing, {} rejected) across {writer_threads} threads",
            writer_outcome.completed,
            writer_outcome.successful,
            writer_outcome.missing,
            writer_outcome.rejected,
        ),
    )
}

/// One writer/reader thread's loop: cycles cancel / add / read over a fixed
/// recycled id pool disjoint from the matcher's targets, until `stop` is
/// set. Never calls `match_order` — matching stays single-threaded on the
/// caller's own thread, per the one-logical-matcher-per-level contract.
fn writer_loop(
    writer_id: usize,
    level: &PriceLevel,
    ready: &Barrier,
    go: &AtomicBool,
    stop: &AtomicBool,
) -> WriterOutcome {
    ready.wait();
    while !go.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }

    let mut outcome = WriterOutcome::default();
    let mut i: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        let slot = CHURN_ID_BASE + (writer_id as u64 + i) % CHURN_POOL_SIZE;
        match i % 3 {
            0 => match level.update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(slot),
            }) {
                Ok(Some(_)) => outcome.successful += 1,
                Ok(None) => outcome.missing += 1,
                Err(e) => {
                    log_unexpected(&format!("contention writer cancel error: {e}"));
                    outcome.errors += 1;
                }
            },
            1 => {
                let order = fixtures::standard_order(slot, Side::Sell, 5, TimeInForce::Gtc);
                match level.add_order(order) {
                    Ok(_) => outcome.successful += 1,
                    Err(PriceLevelError::DuplicateOrderId(_)) => outcome.rejected += 1,
                    Err(e) => {
                        log_unexpected(&format!("contention writer add error: {e}"));
                        outcome.errors += 1;
                    }
                }
            }
            _ => {
                let _ = level.visible_quantity();
                let _ = level.order_count();
                outcome.successful += 1;
            }
        }
        outcome.completed += 1;
        i += 1;
    }
    outcome
}

#[derive(Debug, Default, Clone, Copy)]
struct WriterOutcome {
    completed: u64,
    successful: u64,
    missing: u64,
    rejected: u64,
    errors: u64,
}

impl WriterOutcome {
    fn merge(mut self, other: Self) -> Self {
        self.completed += other.completed;
        self.successful += other.successful;
        self.missing += other.missing;
        self.rejected += other.rejected;
        self.errors += other.errors;
        self
    }
}

/// This harness has no `tracing` subscriber installed (it never installs a
/// global one — `rules/global_rules.md`'s "Library code must NOT install a
/// global subscriber on a normal call path" applies just as well to a bench
/// binary's own diagnostics); an unexpected writer error is instead recorded
/// in [`WriterOutcome::errors`] and asserted to be zero. This helper exists
/// only so an unexpected error's message is visible on stderr for the rare
/// case that assertion fires, without reaching for `println!` (forbidden by
/// the same rules) or standing up a subscriber for a one-line diagnostic.
fn log_unexpected(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}
