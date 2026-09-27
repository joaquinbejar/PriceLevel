//! Single-matcher-plus-N-writers contention comparison. Not a Criterion
//! benchmark (Criterion measures per-iteration means on one thread; this is
//! an aggregate multi-thread measurement, closer in spirit to
//! `BENCH.md`'s `contention_{gtc,fok}_matcher` latency scenario, but
//! simplified — see `workloads::run_contention`'s doc comment for exactly
//! how). No hdrhistogram (not an approved dependency): writer p50/p99/p99.9
//! come from `workloads::percentile` over the pooled, sorted per-writer-op
//! nanosecond samples.
//!
//! Run (once per feature):
//! ```sh
//! cargo run --manifest-path benches/compare/Cargo.toml --release --bin contention_compare --features old
//! cargo run --manifest-path benches/compare/Cargo.toml --release --bin contention_compare --features new
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pricelevel_compare::VERSION_LABEL;
use pricelevel_compare::workloads::{self as w, percentile};

fn run_one(depth: u64, fok: bool) {
    let matcher_ops: usize = std::env::var("PL_COMPARE_CONTENTION_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    let writers: usize = std::env::var("PL_COMPARE_CONTENTION_WRITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    let report = w::run_contention(depth, writers, matcher_ops, fok);
    let p50 = percentile(&report.writer_samples_ns, 0.50);
    let p99 = percentile(&report.writer_samples_ns, 0.99);
    let p999 = percentile(&report.writer_samples_ns, 0.999);
    let matcher_ops_per_s = report.matcher_ops as f64 / report.matcher_elapsed.as_secs_f64();

    println!(
        "version={:<8} matcher={:<4} depth={:<6} writers={} matcher_ops={:<6} matcher_elapsed_ms={:<10.3} matcher_ops_per_s={:<12.1} writer_samples={:<8} writer_p50_ns={:<8} writer_p99_ns={:<8} writer_p999_ns={:<8}",
        VERSION_LABEL,
        if fok { "FOK" } else { "GTC" },
        depth,
        writers,
        report.matcher_ops,
        report.matcher_elapsed.as_secs_f64() * 1000.0,
        matcher_ops_per_s,
        report.writer_samples_ns.len(),
        p50,
        p99,
        p999,
    );
}

fn main() {
    for &depth in &[100u64, 10_000] {
        run_one(depth, false);
        run_one(depth, true);
    }
}
