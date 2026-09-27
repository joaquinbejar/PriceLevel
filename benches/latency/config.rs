// benches/latency/config.rs
//! Environment-variable configuration for the latency harness.
//!
//! Every knob has a default that keeps the *default* run fast (seconds, not
//! minutes) and safe on a laptop; the largest depth sweeps (10,000 /
//! 100,000 resting orders) are opt-in via `PL_LATENCY_LARGE_DEPTHS=1`, as the
//! issue requires ("make the largest depths opt-in via env var if runtime is
//! long"). A short validation run sets `PL_LATENCY_SAMPLES` to a small value
//! (see `make bench-latency` and `BENCH.md`).
//!
//! # Debug-build defaults
//!
//! `cargo test --all-targets` builds and RUNS every target with
//! `harness = false`, including this one — with no libtest wrapper, that
//! means it executes this binary's whole `main()`, unoptimized
//! (`debug_assertions` is `true`). That build is only ever exercised as a
//! "does this still compile and run correctly" smoke check (never for
//! reported numbers — `cargo bench` compiles with the optimized `bench`
//! profile, where `debug_assertions` is `false`), so the *unset-env-var*
//! defaults below are an order of magnitude smaller under
//! `cfg!(debug_assertions)`. Any explicit `PL_LATENCY_*` env var still wins
//! in either build.

use std::env;

/// Deterministic default seed for every xorshift-driven workload in this
/// harness. Overridable so a reviewer can rerun with a different seed to
/// probe p99.99 stability (see `stats::MIN_SAMPLES_FOR_P9999`'s docs).
const DEFAULT_SEED: u64 = 0xA5A5_A5A5_A5A5_A5A5;

/// Default per-scenario sample count for the single-threaded isolated /
/// TIF / snapshot scenarios (optimized `bench` profile).
const DEFAULT_SAMPLES: usize = 20_000;
/// Same, but for an unoptimized `cargo test --all-targets` smoke run — see
/// the module docs.
const DEBUG_DEFAULT_SAMPLES: usize = 100;

/// Default number of discarded warmup operations run immediately before the
/// measured loop of each single-threaded scenario, to let branch predictors,
/// caches and (where applicable) allocator arenas reach steady state before
/// the first recorded sample.
const DEFAULT_WARMUP: usize = 2_000;
/// Debug-build default; see the module docs.
const DEBUG_DEFAULT_WARMUP: usize = 20;

/// Default worker thread count for the contention scenario (one of them is
/// the sole matcher; the rest are concurrent admissions/cancels/readers).
const DEFAULT_CONTENTION_THREADS: usize = 4;

/// Default number of matcher-thread operations measured per contention run.
const DEFAULT_CONTENTION_OPS: usize = 5_000;
/// Debug-build default; see the module docs.
const DEBUG_DEFAULT_CONTENTION_OPS: usize = 100;

/// Default number of repetitions averaged per operation in the allocation
/// measurement pass.
const DEFAULT_ALLOC_REPS: usize = 2_000;
/// Debug-build default; see the module docs.
const DEBUG_DEFAULT_ALLOC_REPS: usize = 50;

/// `true` when this binary was compiled without optimizations — i.e. via
/// `cargo test` / `cargo build`, never via `cargo bench`'s `bench` profile.
/// See the module docs.
const fn is_unoptimized_build() -> bool {
    cfg!(debug_assertions)
}

/// Resolved harness configuration, read once from the environment at
/// startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// Per-scenario measured sample count for single-threaded scenarios.
    pub samples: usize,
    /// Discarded warmup iterations before each single-threaded scenario's
    /// measured loop.
    pub warmup: usize,
    /// Deterministic PRNG seed.
    pub seed: u64,
    /// When `true`, also run the 10,000 / 100,000 resting-order depth
    /// sweeps. Off by default (`PL_LATENCY_LARGE_DEPTHS=1` to opt in).
    pub large_depths: bool,
    /// Worker thread count for the contention scenario.
    pub contention_threads: usize,
    /// Matcher-thread operation count for the contention scenario.
    pub contention_ops: usize,
    /// Repetitions per operation in the allocation-measurement pass.
    pub alloc_reps: usize,
    /// Raw `LOGLEVEL` environment value, recorded in the manifest for parity
    /// with the rest of the crate's tooling even though this harness does
    /// not install a `tracing` subscriber.
    pub loglevel: Option<String>,
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_bool(name: &str) -> bool {
    matches!(env::var(name).ok().as_deref(), Some("1") | Some("true"))
}

impl Config {
    /// Reads every knob from the environment, falling back to the documented
    /// defaults above for anything unset or unparsable.
    #[must_use]
    pub fn from_env() -> Self {
        let debug = is_unoptimized_build();
        let default_samples = if debug {
            DEBUG_DEFAULT_SAMPLES
        } else {
            DEFAULT_SAMPLES
        };
        let default_warmup = if debug {
            DEBUG_DEFAULT_WARMUP
        } else {
            DEFAULT_WARMUP
        };
        let default_contention_ops = if debug {
            DEBUG_DEFAULT_CONTENTION_OPS
        } else {
            DEFAULT_CONTENTION_OPS
        };
        let default_alloc_reps = if debug {
            DEBUG_DEFAULT_ALLOC_REPS
        } else {
            DEFAULT_ALLOC_REPS
        };

        Self {
            samples: env_usize("PL_LATENCY_SAMPLES", default_samples),
            warmup: env_usize("PL_LATENCY_WARMUP", default_warmup),
            seed: env_u64("PL_LATENCY_SEED", DEFAULT_SEED),
            large_depths: env_bool("PL_LATENCY_LARGE_DEPTHS"),
            contention_threads: env_usize(
                "PL_LATENCY_CONTENTION_THREADS",
                DEFAULT_CONTENTION_THREADS,
            ),
            contention_ops: env_usize("PL_LATENCY_CONTENTION_OPS", default_contention_ops),
            alloc_reps: env_usize("PL_LATENCY_ALLOC_REPS", default_alloc_reps),
            loglevel: env::var("LOGLEVEL").ok(),
        }
    }
}
