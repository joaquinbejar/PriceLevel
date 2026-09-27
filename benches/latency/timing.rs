// benches/latency/timing.rs
//! The single measurement primitive every scenario in this harness uses:
//! time exactly one operation with [`std::time::Instant`], record the
//! elapsed nanoseconds into a pre-allocated `Vec<u64>`, and hand the
//! operation's own return value back to the caller *after* the clock read so
//! outcome classification never counts against the timed sample (issue
//! #142's "keep fixture construction and result destruction outside the
//! timed window").

use std::time::Instant;

/// Runs `op` exactly `count` times, timing each call in isolation.
///
/// `op` receives the sample index (`0..count`) so it can index into
/// pre-built, per-sample fixtures (e.g. a distinct order id per `add_order`
/// call) without allocating or constructing anything itself — all of that
/// must already have happened before this function is called.
///
/// Returns the raw nanosecond durations (unsorted — callers pass this to
/// `stats::compute`, which sorts in place) and the operation's own return
/// values, in call order, so a caller can verify outcomes after the loop
/// without that verification affecting any recorded duration.
pub fn measure<F, O>(count: usize, mut op: F) -> (Vec<u64>, Vec<O>)
where
    F: FnMut(usize) -> O,
{
    let mut durations_ns = Vec::with_capacity(count);
    let mut outcomes = Vec::with_capacity(count);
    for i in 0..count {
        let t0 = Instant::now();
        let outcome = op(i);
        let elapsed = t0.elapsed();
        // Classification / bookkeeping happens AFTER the clock read above;
        // pushing into a pre-sized Vec does not reallocate, so it adds
        // negligible, unmeasured overhead between iterations rather than
        // inside the timed call.
        outcomes.push(outcome);
        durations_ns.push(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    }
    (durations_ns, outcomes)
}

/// Like [`measure`], but runs an untimed `setup` closure immediately before
/// each timed `op` call.
///
/// Some scenarios need per-sample state that a single upfront batch cannot
/// provide without breaking the "exactly one operation per sample" shape —
/// for example, a partial-fill scenario needs a fresh small resting maker
/// order in place before every timed `match_order` call, or the level would
/// run dry after the first sample. `setup` runs strictly before
/// `Instant::now()` is read, so none of its cost is attributed to the timed
/// sample.
pub fn measure_with_setup<S, F, O>(count: usize, mut setup: S, mut op: F) -> (Vec<u64>, Vec<O>)
where
    S: FnMut(usize),
    F: FnMut(usize) -> O,
{
    let mut durations_ns = Vec::with_capacity(count);
    let mut outcomes = Vec::with_capacity(count);
    for i in 0..count {
        setup(i);
        let t0 = Instant::now();
        let outcome = op(i);
        let elapsed = t0.elapsed();
        outcomes.push(outcome);
        durations_ns.push(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    }
    (durations_ns, outcomes)
}

/// Runs `op` `count` times purely to warm up caches / branch predictors /
/// allocator arenas, discarding every result and every timing. Always call
/// this with fresh fixtures distinct from the measured run's fixtures (e.g.
/// a disjoint id range), so warmup mutations do not change the measured
/// run's starting state.
pub fn warmup<F, O>(count: usize, mut op: F)
where
    F: FnMut(usize) -> O,
{
    for i in 0..count {
        let _ = std::hint::black_box(op(i));
    }
}
