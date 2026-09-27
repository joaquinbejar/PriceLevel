// benches/latency/main.rs
//! Isolated operation and tail-latency benchmark harness (issue #142).
//!
//! A `harness = false` bench target (`[[bench]] name = "latency"` in
//! `Cargo.toml`) so it runs as a plain binary under `cargo bench --bench
//! latency`, entirely separate from the Criterion suite wired through
//! `benches/mod.rs` — it does not slow that suite down, and Criterion's own
//! statistical-convergence loop is not designed to hand back individual
//! per-operation samples, which is the entire point of this harness.
//!
//! No `hdrhistogram` (not an approved dependency for this crate — see
//! `rules/global_rules.md`'s dependency list and `.claude/skills/bench-hdr`,
//! whose template this harness intentionally does NOT use for that reason).
//! Percentiles are computed by sorting the raw sample `Vec<u64>` in
//! `stats.rs`.
//!
//! Run with `make bench-latency`, or directly with `cargo bench --bench
//! latency`. Every knob is an environment variable (see `config.rs`) —
//! `PL_LATENCY_SAMPLES=200 PL_LATENCY_CONTENTION_OPS=200 make bench-latency`
//! is the short validation run `BENCH.md` documents.
//!
//! See `manifest::COORDINATED_OMISSION_DISCLOSURE` for what these numbers
//! are — and are not — evidence of.

mod alloc;
mod alloc_measurements;
mod config;
mod fixtures;
mod manifest;
mod persistence;
mod report;
mod scenarios;
mod stats;
mod timing;

use config::Config;
use manifest::{COORDINATED_OMISSION_DISCLOSURE, RunManifest};

fn main() {
    // Runtime self-check of the statistics core — see `stats::self_check`'s
    // docs for why this replaces `#[test]` in a `harness = false` binary.
    stats::self_check();

    let config = Config::from_env();
    let run_manifest = RunManifest::capture(&config);

    println!("{run_manifest}");
    println!();
    println!("{COORDINATED_OMISSION_DISCLOSURE}");
    println!();

    println!("== Latency scenarios ==");
    let reports = scenarios::run_all(&config);
    for report in &reports {
        println!("{report}");
    }
    println!();
    println!("== Markdown table (paste into BENCH.md) ==");
    println!("{}", report::to_markdown_table(&reports));

    let artifacts = persistence::prepare(&run_manifest, &config, &reports).expect(
        "persistence::prepare: failed to write target/latency/<run-id>/ — check that target/ is \
         writable",
    );
    println!(
        "== Raw observations + manifest persisted to {} ==",
        artifacts.dir.display()
    );
    println!();

    if config.runs("snapshot_sizes") {
        // Issue #149: allocation count / bytes / peak live bytes of the
        // snapshot encode + validate paths, per level size (untimed pass).
        println!("== Snapshot size sweep allocations (separate pass, not timed) ==");
        for line in scenarios::snapshot_sizes::run_alloc(&config) {
            println!("{line}");
        }
        println!();
    }

    if !config.runs("alloc") {
        return;
    }
    println!("== Allocation measurements (separate pass, not timed) ==");
    // Element sizes behind the result-buffer byte counts (issue #148).
    println!(
        "element sizes: Trade={} bytes, Id={} bytes",
        std::mem::size_of::<pricelevel::Trade>(),
        std::mem::size_of::<pricelevel::Id>()
    );
    let alloc_reports = alloc_measurements::run_all(&config);
    for alloc_report in &alloc_reports {
        println!("{alloc_report}");
    }
}
