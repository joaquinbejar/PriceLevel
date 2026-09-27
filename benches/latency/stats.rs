// benches/latency/stats.rs
//! Percentile computation over raw per-operation nanosecond samples.
//!
//! No `hdrhistogram` (not an approved dependency — see issue #142). This is a
//! plain sort-and-index-lookup over the full sample `Vec<u64>`, computed
//! *after* the timed loop, never inside it.

/// Label appended to every reported p99.99 figure, in every place this
/// harness prints one, so it can never be quoted without its caveat
/// (issue #142 review finding 6).
///
/// A sample count alone does not justify treating p99.99 as a validated
/// tail quantile: the 1-in-10,000 rank is a single order statistic, and
/// nothing here repeats a run to check whether that single point is stable
/// from one run to the next. The review offered two ways to keep this
/// honest: gate the figure behind a sample-count floor and refuse it below
/// that floor, or always compute and report it labelled as exploratory
/// while keeping the raw observations so a reader can judge stability
/// themselves. This harness takes the second path, because the first still
/// implied "enough samples make it valid", which sample count alone cannot
/// establish — and because `persistence.rs` already writes every scenario's
/// raw nanosecond observations to `target/latency/<run-id>/<scenario>.csv`
/// alongside the manifest, so a reader who wants to check stability can
/// rerun this harness and diff the raw files, or resample the same file,
/// instead of trusting one number.
pub const P9999_CAVEAT: &str = "unvalidated exploratory estimate — not confirmed across repeated \
                                 runs; see stats::P9999_CAVEAT and BENCH.md Methodology";

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
    /// 99.99th percentile, in nanoseconds — `None` only when `samples ==
    /// 0`. When `Some`, this is an [`P9999_CAVEAT`]: a single order
    /// statistic from ONE run, not a quantity this harness has validated
    /// for stability across repeated runs. Do not quote it without that
    /// caveat; use the persisted raw observations
    /// (`target/latency/<run-id>/<scenario>.csv`) to check stability
    /// yourself before relying on it.
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
/// that as "no data", not "operation took zero time". `p9999_ns` is `Some`
/// whenever `samples_ns` is non-empty — see its doc for the exploratory
/// caveat that always accompanies it.
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
    let p9999_ns = Some(samples_ns[nearest_rank_index(0.9999, n)]);
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
        match self.p9999_ns {
            Some(v) => write!(
                f,
                "n={:<8} p50={:<8} p99={:<8} p99.9={:<8} p99.99={v} ({}) max={:<8} min={}",
                self.samples,
                self.p50_ns,
                self.p99_ns,
                self.p999_ns,
                P9999_CAVEAT,
                self.max_ns,
                self.min_ns
            ),
            None => write!(
                f,
                "n=0 (no samples) p50={:<8} p99={:<8} p99.9={:<8} p99.99=n/a max={:<8} min={}",
                self.p50_ns, self.p99_ns, self.p999_ns, self.max_ns, self.min_ns
            ),
        }
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

    let mut small: Vec<u64> = (1..=100).collect();
    let p = compute(&mut small);
    assert_eq!(p.samples, 100);
    assert!(
        p.p9999_ns.is_some(),
        "self_check: p99.99 must always be reported (labelled exploratory) for any non-empty input"
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
        .expect("self_check: p99.99 must be present for a non-empty input");
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
