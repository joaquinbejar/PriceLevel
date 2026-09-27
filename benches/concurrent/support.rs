// benches/concurrent/support.rs
//! Shared coordination and outcome-accounting helpers for the concurrent
//! benchmark workloads (`register.rs`, `contention.rs`).
//!
//! Centralizing the worker-coordination primitive keeps the start-of-timing
//! guarantee and the failure-propagation behavior identical — and
//! independently auditable — across every workload (issue #141, #160).

use pricelevel::{
    Hash32, Id, MatchOutcome, MatchResult, OrderType, Price, Quantity, Side, TimeInForce,
    TimestampMs,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

/// Per-worker outcome counters for a concurrent workload.
///
/// Every field is a plain `u64` count over a worker's own iterations.
/// Workers accumulate these locally (no cross-thread synchronization, so
/// counting never adds contention to the timed region); [`run_timed`] sums
/// them across workers, outside the timed region, so callers can assert
/// exact accounting.
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkerOutcome {
    /// Iterations this worker actually ran to completion. Always equals the
    /// requested iteration count on a successful run; a worker that returns
    /// `Err` stops incrementing this at the failing iteration.
    pub completed: u64,
    /// Operations that mutated the level (or performed a read) successfully,
    /// including any operation this workload does not classify more
    /// specifically below.
    pub successful: u64,
    /// `update_order` calls that found no matching id (`Ok(None)`) — an
    /// expected outcome under contention (another thread already cancelled
    /// or consumed the target), never a failure.
    pub missing: u64,
    /// `match_order` calls that fully filled the taker
    /// ([`MatchOutcome::Filled`]).
    pub full_match: u64,
    /// `match_order` calls that partially filled the taker
    /// ([`MatchOutcome::PartiallyFilled`]).
    pub partial_match: u64,
    /// `match_order` calls that filled nothing because the level had no
    /// matchable depth left ([`MatchOutcome::NotFilled`]).
    pub empty_match: u64,
    /// Operations rejected by a documented business rule: a killed /
    /// rejected match ([`MatchOutcome::Killed`] / [`MatchOutcome::Rejected`])
    /// or an `add_order` that lost a legitimate id race
    /// (`PriceLevelError::DuplicateOrderId`) under intentional contention.
    pub rejected: u64,
}

impl WorkerOutcome {
    /// Folds another worker's counters into this one.
    #[must_use]
    pub fn merge(mut self, other: Self) -> Self {
        self.completed += other.completed;
        self.successful += other.successful;
        self.missing += other.missing;
        self.full_match += other.full_match;
        self.partial_match += other.partial_match;
        self.empty_match += other.empty_match;
        self.rejected += other.rejected;
        self
    }
}

/// Classifies a [`MatchResult`] into the matching buckets of a
/// [`WorkerOutcome`], using [`MatchResult::outcome`] rather than inferring
/// the case from `trades` / `remaining_quantity` (a kill, a rejection and an
/// empty level all leave zero trades and the full amount remaining, so only
/// the explicit [`MatchOutcome`] tells them apart).
pub fn classify_match(result: &MatchResult, outcome: &mut WorkerOutcome) {
    match result.outcome() {
        MatchOutcome::Filled => outcome.full_match += 1,
        MatchOutcome::PartiallyFilled => outcome.partial_match += 1,
        MatchOutcome::NotFilled => outcome.empty_match += 1,
        MatchOutcome::Killed | MatchOutcome::Rejected => outcome.rejected += 1,
    }
}

/// A worker's fallible per-iteration body returns `Err(message)` on an
/// unexpected failure instead of panicking; see [`run_timed`].
pub type WorkerResult = Result<WorkerOutcome, String>;

/// Blocks the calling worker until every other worker has also reached this
/// call (`ready.wait()`), then spins on `go` until the coordinator
/// (`run_timed`) has captured its start timestamp and released the run.
///
/// The `Acquire` load here pairs with the `Release` store in `run_timed`:
/// once a worker observes `go == true`, the timestamp capture that preceded
/// the store is guaranteed to have already happened. No operation the
/// caller runs after this call can therefore execute before timing started.
pub fn wait_for_go(ready: &Barrier, go: &AtomicBool) {
    ready.wait();
    while !go.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
}

/// Spawns `thread_count` workers via `spawn_worker`, releases them all as
/// close to simultaneously as the start-of-timing protocol in
/// [`wait_for_go`] allows, joins every one, and returns the wall-clock
/// duration of the run together with the summed [`WorkerOutcome`].
///
/// # Start-of-timing correctness
///
/// A single [`Barrier`] shared between the coordinator and the workers
/// cannot by itself guarantee that timing starts before a worker's first
/// operation: `Barrier::wait` releases every party once the last one
/// arrives, but nothing orders *which* released thread resumes execution
/// first — a woken worker can run operations before the coordinator's own
/// `wait` call even returns. This function instead uses a `ready` barrier
/// (workers signal they have reached [`wait_for_go`]'s spin loop) followed
/// by capturing `Instant::now()` and only then flipping the `go` flag; see
/// [`wait_for_go`] for the acquire/release pairing that makes this safe.
///
/// # Failure propagation
///
/// A worker never panics on an operation failure — its closure returns
/// `Err(message)` instead. There is deliberately no second barrier wait
/// after that: `thread::JoinHandle::join` is what the coordinator waits on,
/// so a worker that returns early (successfully or with an error) can never
/// leave another party blocked. This function joins every handle, treats an
/// actual thread panic the same as a reported `Err`, and returns the FIRST
/// failure found instead of the measured duration — so a failing worker is
/// reported, not hung (issue #141, #160).
pub fn run_timed<S>(
    thread_count: usize,
    spawn_worker: S,
) -> Result<(Duration, WorkerOutcome), String>
where
    S: Fn(usize, Arc<Barrier>, Arc<AtomicBool>) -> thread::JoinHandle<WorkerResult>,
{
    let ready = Arc::new(Barrier::new(thread_count + 1));
    let go = Arc::new(AtomicBool::new(false));

    let handles: Vec<_> = (0..thread_count)
        .map(|thread_id| spawn_worker(thread_id, Arc::clone(&ready), Arc::clone(&go)))
        .collect();

    // Every worker has reached `wait_for_go`'s spin loop once this returns.
    ready.wait();
    let start = Instant::now();
    // Release: pairs with the Acquire load in `wait_for_go`. No worker can
    // observe `true` before `start` above was captured.
    go.store(true, Ordering::Release);

    let mut outcome = WorkerOutcome::default();
    let mut first_error: Option<String> = None;
    for handle in handles {
        match handle.join() {
            Ok(Ok(worker_outcome)) => outcome = outcome.merge(worker_outcome),
            Ok(Err(message)) => {
                first_error.get_or_insert(message);
            }
            Err(panic_payload) => {
                first_error.get_or_insert(describe_panic(panic_payload));
            }
        }
    }
    let duration = start.elapsed();

    match first_error {
        Some(message) => Err(message),
        None => Ok((duration, outcome)),
    }
}

/// Renders a caught worker panic payload into a readable message.
fn describe_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("worker panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("worker panicked: {message}")
    } else {
        "worker panicked with a non-string payload".to_string()
    }
}

/// Creates a standard limit order for benchmark fixtures.
pub fn create_standard_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_616_823_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}
