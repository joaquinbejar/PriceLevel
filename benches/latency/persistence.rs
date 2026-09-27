// benches/latency/persistence.rs
//! Writes raw per-scenario observations and the run manifest to
//! `target/latency/<run-id>/`.
//!
//! Issue #142 review finding 5: "#142 requires retaining raw observations or
//! histogram artifacts with the manifest" — a percentile alone (especially
//! the p99.99 [`crate::stats::P9999_CAVEAT`]) cannot be checked for
//! stability, reprocessed with a different quantile definition, or diffed
//! against another run without the underlying samples. This module writes
//! those samples out, one plain `duration_ns` column per scenario, plus one
//! `manifest.json` per run covering the actual `Config` used and every
//! scenario's measured boundary and percentile summary (not just the run
//! environment `RunManifest` already prints to stdout).

use crate::config::Config;
use crate::manifest::RunManifest;
use crate::report::ScenarioReport;
use crate::stats::P9999_CAVEAT;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directory this run's artifacts were written to.
pub struct RunArtifacts {
    /// `target/latency/<run-id>/`.
    pub dir: PathBuf,
}

/// A run id derived from wall-clock time (milliseconds since the Unix
/// epoch). This is bench-harness plumbing, not a production timestamp: a
/// clock that reads before the epoch only ever happens on a misconfigured
/// host, and falling back to `0` in that case still produces a valid,
/// if collision-prone, directory name rather than panicking.
fn run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    millis.to_string()
}

/// Creates `target/latency/<run-id>/`, writes `manifest.json` (run
/// environment + the actual `Config` used + one entry per scenario's
/// measured boundary and percentile summary) and one `<scenario
/// name>.csv` per report (its raw, unsorted nanosecond observations),
/// and returns the directory.
///
/// # Errors
///
/// Returns the underlying `io::Error` if the directory cannot be created or
/// a file cannot be written (e.g. a read-only `target/`).
pub fn prepare(
    run_manifest: &RunManifest,
    config: &Config,
    reports: &[ScenarioReport],
) -> io::Result<RunArtifacts> {
    // One `<name>.csv` per report: a repeated name would silently overwrite
    // another report's observations and leave its manifest entry pointing at
    // the wrong data (issue #149 review). Reject it before writing anything.
    let mut seen = std::collections::HashSet::with_capacity(reports.len());
    for report in reports {
        if !seen.insert(report.name.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "duplicate scenario name {:?}: artifacts would collide",
                    report.name
                ),
            ));
        }
    }

    let dir = PathBuf::from("target/latency").join(run_id());
    fs::create_dir_all(&dir)?;

    let manifest_json = build_manifest_json(run_manifest, config, reports);
    let manifest_text = serde_json::to_string_pretty(&manifest_json)
        .unwrap_or_else(|e| format!("{{\"error\": \"manifest serialization failed: {e}\"}}"));
    fs::write(dir.join("manifest.json"), manifest_text)?;

    for report in reports {
        persist_scenario(&dir, report)?;
    }

    // Post-run check: exactly one distinct observations file per report.
    let csv_files = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "csv"))
        .count();
    if csv_files != reports.len() {
        return Err(io::Error::other(format!(
            "persisted {csv_files} observation files for {} reports",
            reports.len()
        )));
    }

    Ok(RunArtifacts { dir })
}

/// Writes `report`'s raw, unsorted nanosecond observations as a
/// single-column CSV (`duration_ns`), one sample per line, in original call
/// order (so a reader can also look for drift across the run, not only the
/// sorted distribution `stats::compute` used).
fn persist_scenario(dir: &Path, report: &ScenarioReport) -> io::Result<()> {
    let mut buf = String::with_capacity(report.observations_ns.len() * 8 + 16);
    buf.push_str("duration_ns\n");
    for d in &report.observations_ns {
        buf.push_str(itoa_u64(*d).as_str());
        buf.push('\n');
    }
    fs::write(dir.join(format!("{}.csv", report.name)), buf)
}

/// `u64::to_string` via a tiny local wrapper so the formatting call site
/// above reads the same as every other numeric-to-string conversion in this
/// module; kept as a named function rather than inlined `d.to_string()` so
/// a future switch to a faster integer-formatting routine (if this ever
/// shows up as a bottleneck for a very large sample count) has one call
/// site to change.
fn itoa_u64(value: u64) -> String {
    value.to_string()
}

fn build_manifest_json(
    run_manifest: &RunManifest,
    config: &Config,
    reports: &[ScenarioReport],
) -> serde_json::Value {
    let scenarios: Vec<serde_json::Value> = reports
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "category": r.category,
                "depth": r.depth,
                "measured_call": r.measured_call,
                "samples": r.percentiles.samples,
                "p50_ns": r.percentiles.p50_ns,
                "p99_ns": r.percentiles.p99_ns,
                "p999_ns": r.percentiles.p999_ns,
                "p9999_ns": r.percentiles.p9999_ns,
                "p9999_caveat": P9999_CAVEAT,
                "max_ns": r.percentiles.max_ns,
                "min_ns": r.percentiles.min_ns,
                "outcome_note": r.outcome_note,
                "observations_file": format!("{}.csv", r.name),
            })
        })
        .collect();

    serde_json::json!({
        "commit": run_manifest.commit,
        "dirty": run_manifest.dirty,
        "cpu": run_manifest.cpu,
        "logical_cores": run_manifest.logical_cores,
        "os": run_manifest.os,
        "rustc_version": run_manifest.rustc_version,
        "profile": run_manifest.profile,
        "allocator": run_manifest.allocator,
        "seed": format!("0x{:016X}", run_manifest.seed),
        "timer_overhead_ns": run_manifest.timer_overhead_ns,
        "config": {
            "samples": config.samples,
            "warmup": config.warmup,
            "large_depths": config.large_depths,
            "contention_threads": config.contention_threads,
            "contention_ops": config.contention_ops,
            "stats_producers": config.stats_producers,
            "stats_readers": config.stats_readers,
            "stats_ops": config.stats_ops,
            "only": config.only,
            "alloc_reps": config.alloc_reps,
            "loglevel": config.loglevel,
        },
        "scenarios": scenarios,
    })
}
