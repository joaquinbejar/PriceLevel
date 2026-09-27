// benches/latency/scenarios/mod.rs
//! Scenario registry: every latency scenario this harness runs, grouped by
//! category. `run_all` is the single entry point `main.rs` calls.

mod contention;
mod depth;
mod isolated;
mod matching;
mod snapshot;
mod stats_contention;
mod tif;

use crate::config::Config;
use crate::report::ScenarioReport;

/// Runs every single-threaded and contention scenario in this harness and
/// returns one [`ScenarioReport`] per scenario (some scenarios — the depth
/// sweeps — contribute more than one report, one per swept depth).
#[must_use]
pub fn run_all(config: &Config) -> Vec<ScenarioReport> {
    let mut reports = Vec::new();

    if config.runs("isolated") {
        reports.extend(isolated::run(config));
    }
    if config.runs("match") {
        reports.extend(matching::run(config));
    }
    if config.runs("tif") {
        reports.extend(tif::run(config));
    }
    if config.runs("snapshot") {
        reports.extend(snapshot::run(config));
    }
    if config.runs("depth") {
        reports.extend(depth::run(config));
    }
    if config.runs("contention") {
        reports.extend(contention::run(config));
    }
    if config.runs("stats_contention") {
        reports.extend(stats_contention::run(config));
    }

    reports
}
