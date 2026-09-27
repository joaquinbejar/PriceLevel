// benches/latency/stats.rs
//! Percentile computation over raw per-operation nanosecond samples.
//!
//! No `hdrhistogram` (not an approved dependency — see issue #142). This is a
//! plain sort-and-index-lookup over the full sample `Vec<u64>`, computed
//! *after* the timed loop, never inside it.

/// Minimum sample count before p99.99 is reported.
///
/// p99.99 asks "what is the 1-in-10,000 worst sample". With fewer than
/// roughly 10,000 samples that quantile is extrapolated from zero or one
/// observed point, which is not a defensible tail claim (the issue asks to
/// "refuse unsupported tail claims"). This harness requires **at least**
/// `MIN_SAMPLES_FOR_P9999` samples (a 2x margin over the bare 1-in-10,000
/// floor) before it prints a p99.99 number, and even then the value is a
/// **single-run point estimate** — this harness does not repeat runs to
/// check quantile stability, so `BENCH.md` must caveat any p99.99 figure
/// accordingly.
pub const MIN_SAMPLES_FOR_P9999: usize = 20_000;

/// Percentile summary of one scenario's per-operation latency samples.
#[derive(Debug, Clone)]
pub struct Percentiles {
    /// Number of individual operation samples this summary was computed
    /// from.
    pub samples: usize,
    /// 50th percentile, in nanoseconds.
    pub p50_ns: u64,
    /// 99th percentile, in nanoseconds.
    pub p99_ns: u64,
    /// 99.9th percentile, in nanoseconds.
    pub p999_ns: u64,
    /// 99.99th percentile, in nanoseconds — `None` when `samples` is below
    /// [`MIN_SAMPLES_FOR_P9999`]; a caller MUST print "insufficient samples"
    /// rather than inventing a number in that case.
    pub p9999_ns: Option<u64>,
    /// Maximum observed sample, in nanoseconds.
    pub max_ns: u64,
    /// Minimum observed sample, in nanoseconds (useful to sanity-check timer
    /// resolution — see the manifest's `timer_resolution_ns`).
    pub min_ns: u64,
}

/// Nearest-rank index for quantile `q` (`0.0..=1.0`) over `n` sorted samples.
///
/// Uses the common "nearest rank" definition: `ceil(q * n)`, 1-indexed,
/// clamped to `[1, n]`, then converted to a 0-indexed slice position. This is
/// the same definition most HDR-style tools use for a discrete sample set.
fn nearest_rank_index(q: f64, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n_f64 = n as f64;
    let raw = (q * n_f64).ceil();
    // `raw` is finite and >= 0.0 for any q in [0.0, 1.0] and n > 0, so this
    // conversion cannot produce a negative or NaN result; clamp defensively
    // rather than trust that invariant blindly.
    let rank = if raw.is_finite() && raw >= 1.0 {
        raw as usize
    } else {
        1
    };
    rank.clamp(1, n) - 1
}

/// Computes [`Percentiles`] over `samples_ns`, sorting it in place.
///
/// `samples_ns` must contain only samples recorded by [`crate::timing`]
/// (nanosecond durations of exactly one operation each). An empty slice
/// returns all-zero percentiles with `samples == 0`; callers should treat
/// that as "no data", not "operation took zero time".
#[must_use]
pub fn compute(samples_ns: &mut [u64]) -> Percentiles {
    samples_ns.sort_unstable();
    let n = samples_ns.len();
    if n == 0 {
        return Percentiles {
            samples: 0,
            p50_ns: 0,
            p99_ns: 0,
            p999_ns: 0,
            p9999_ns: None,
            max_ns: 0,
            min_ns: 0,
        };
    }

    let p50_ns = samples_ns[nearest_rank_index(0.50, n)];
    let p99_ns = samples_ns[nearest_rank_index(0.99, n)];
    let p999_ns = samples_ns[nearest_rank_index(0.999, n)];
    let p9999_ns = if n >= MIN_SAMPLES_FOR_P9999 {
        Some(samples_ns[nearest_rank_index(0.9999, n)])
    } else {
        None
    };
    let max_ns = samples_ns[n - 1];
    let min_ns = samples_ns[0];

    Percentiles {
        samples: n,
        p50_ns,
        p99_ns,
        p999_ns,
        p9999_ns,
        max_ns,
        min_ns,
    }
}

impl std::fmt::Display for Percentiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p9999 = self
            .p9999_ns
            .map(|v| v.to_string())
            .unwrap_or_else(|| "insufficient samples".to_string());
        write!(
            f,
            "n={:<8} p50={:<8} p99={:<8} p99.9={:<8} p99.99={:<20} max={:<8} min={}",
            self.samples, self.p50_ns, self.p99_ns, self.p999_ns, p9999, self.max_ns, self.min_ns
        )
    }
}

/// Runtime self-check of [`compute`]'s invariants.
///
/// This binary is a `harness = false` bench target (issue #142): `cargo test`
/// never executes `#[test]` functions inside it (there is no libtest runner),
/// so correctness of the statistics core is instead asserted here and called
/// once from `main` at startup — the same "smoke check that panics on
/// violation" convention `benches/concurrent/contention.rs` already uses for
/// its own harness-internal invariants.
///
/// # Panics
///
/// Panics if any invariant below is violated. That is the intended failure
/// mode for a benchmark harness self-check, not a production code path.
pub fn self_check() {
    let mut empty: Vec<u64> = Vec::new();
    let p = compute(&mut empty);
    assert_eq!(
        p.samples, 0,
        "self_check: empty input must report zero samples"
    );
    assert_eq!(
        p.p9999_ns, None,
        "self_check: empty input must not report p99.99"
    );

    let mut below_threshold: Vec<u64> = (1..=1000).collect();
    let p = compute(&mut below_threshold);
    assert_eq!(p.samples, 1000);
    assert_eq!(
        p.p9999_ns, None,
        "self_check: below MIN_SAMPLES_FOR_P9999 must refuse p99.99"
    );

    let mut plenty: Vec<u64> = (1..=50_000).collect();
    let p = compute(&mut plenty);
    assert_eq!(p.samples, 50_000);
    assert!(p.p50_ns <= p.p99_ns, "self_check: p50 must not exceed p99");
    assert!(
        p.p99_ns <= p.p999_ns,
        "self_check: p99 must not exceed p99.9"
    );
    let p9999 = p
        .p9999_ns
        .expect("self_check: 50_000 samples meets MIN_SAMPLES_FOR_P9999");
    assert!(
        p.p999_ns <= p9999,
        "self_check: p99.9 must not exceed p99.99"
    );
    assert!(p9999 <= p.max_ns, "self_check: p99.99 must not exceed max");
    assert_eq!(p.max_ns, 50_000);
    assert_eq!(p.min_ns, 1);

    let mut uniform_100: Vec<u64> = (1..=100).collect();
    let p = compute(&mut uniform_100);
    assert_eq!(
        p.p50_ns, 50,
        "self_check: nearest-rank p50 over 1..=100 must be the 50th value"
    );
}
