// benches/latency/scenarios/mod.rs
//! Scenario registry: every latency scenario this harness runs, grouped by
//! category. `run_all` is the single entry point `main.rs` calls.

mod contention;
mod depth;
mod isolated;
mod matching;
mod snapshot;
mod tif;

use crate::config::Config;
use crate::report::ScenarioReport;

/// Runs every single-threaded and contention scenario in this harness and
/// returns one [`ScenarioReport`] per scenario (some scenarios — the depth
/// sweeps — contribute more than one report, one per swept depth).
#[must_use]
pub fn run_all(config: &Config) -> Vec<ScenarioReport> {
    let mut reports = Vec::new();

    reports.extend(isolated::run(config));
    reports.extend(matching::run(config));
    reports.extend(tif::run(config));
    reports.extend(snapshot::run(config));
    reports.extend(depth::run(config));
    reports.extend(contention::run(config));

    reports
}
