// benches/latency/report.rs
//! One reported row per scenario, with an unambiguous measured boundary
//! (issue #142 acceptance criterion: "Each reported number has an
//! unambiguous measured boundary").

use super::stats::{self, Percentiles};

/// One scenario's result: its identity, what exactly was timed, and the
/// resulting percentiles plus an outcome-accounting note that was computed
/// entirely outside the timed loop.
#[derive(Debug, Clone)]
pub struct ScenarioReport {
    /// Stable scenario name, used as the `BENCH.md` table key.
    pub name: String,
    /// Coarse grouping for the printed output (`isolated`, `tif`, `depth`,
    /// `snapshot`, `contention`).
    pub category: &'static str,
    /// Resting depth the scenario ran against (0 for depth-independent
    /// scenarios such as `match_empty`).
    pub depth: u64,
    /// One-line description of exactly which public call was timed — the
    /// "measured boundary" a reader needs to compare this number to anything
    /// else.
    pub measured_call: &'static str,
    /// The computed percentile summary.
    pub percentiles: Percentiles,
    /// Outcome-accounting note (e.g. "20000/20000 Filled"), computed after
    /// the timed loop from the operations' own return values.
    pub outcome_note: String,
}

impl ScenarioReport {
    /// Builds a report from raw nanosecond samples, computing percentiles
    /// here (sorting `durations_ns` in place) so every scenario module calls
    /// exactly one function to go from samples to a reportable row.
    #[must_use]
    pub fn from_samples(
        name: impl Into<String>,
        category: &'static str,
        depth: u64,
        measured_call: &'static str,
        mut durations_ns: Vec<u64>,
        outcome_note: impl Into<String>,
    ) -> Self {
        let percentiles = stats::compute(&mut durations_ns);
        Self {
            name: name.into(),
            category,
            depth,
            measured_call,
            percentiles,
            outcome_note: outcome_note.into(),
        }
    }
}

impl std::fmt::Display for ScenarioReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{:<11}] {:<28} depth={:<8} {} | {} | outcomes: {}",
            self.category,
            self.name,
            self.depth,
            self.measured_call,
            self.percentiles,
            self.outcome_note
        )
    }
}

/// Renders every report as a Markdown table, in the shape `BENCH.md`
/// expects (issue #142: "write results/docs to a new BENCH.md").
#[must_use]
pub fn to_markdown_table(reports: &[ScenarioReport]) -> String {
    let mut out = String::new();
    out.push_str(
        "| Scenario | Category | Depth | Samples | p50 (ns) | p99 (ns) | p99.9 (ns) | p99.99 (ns) | max (ns) | Outcomes |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    for r in reports {
        let p9999 = r
            .percentiles
            .p9999_ns
            .map(|v| v.to_string())
            .unwrap_or_else(|| "insufficient samples".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.name,
            r.category,
            r.depth,
            r.percentiles.samples,
            r.percentiles.p50_ns,
            r.percentiles.p99_ns,
            r.percentiles.p999_ns,
            p9999,
            r.percentiles.max_ns,
            r.outcome_note,
        ));
    }
    out
}
