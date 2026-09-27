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
    /// The raw, unsorted, per-sample nanosecond observations behind
    /// [`Self::percentiles`], retained so `persistence.rs` can write them
    /// alongside the run manifest (issue #142 review finding 5: "#142
    /// requires retaining raw observations or histogram artifacts with the
    /// manifest").
    pub observations_ns: Vec<u64>,
}

impl ScenarioReport {
    /// Builds a report from raw nanosecond samples, computing percentiles
    /// from a sorted copy (sorting is required for `stats::compute`, but the
    /// original call-order sequence is worth keeping for later analysis —
    /// e.g. checking whether latency drifts across the run — so this clones
    /// rather than sorting `durations_ns` in place).
    #[must_use]
    pub fn from_samples(
        name: impl Into<String>,
        category: &'static str,
        depth: u64,
        measured_call: &'static str,
        durations_ns: Vec<u64>,
        outcome_note: impl Into<String>,
    ) -> Self {
        let mut sorted = durations_ns.clone();
        let percentiles = stats::compute(&mut sorted);
        Self {
            name: name.into(),
            category,
            depth,
            measured_call,
            percentiles,
            outcome_note: outcome_note.into(),
            observations_ns: durations_ns,
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
/// expects (issue #142: "write results/docs to a new BENCH.md"). The
/// `p99.99 (ns)` column header itself carries the exploratory-estimate
/// caveat (see `stats::P9999_CAVEAT`) rather than repeating it in every row.
#[must_use]
pub fn to_markdown_table(reports: &[ScenarioReport]) -> String {
    let mut out = String::new();
    out.push_str(
        "| Scenario | Category | Depth | Samples | p50 (ns) | p99 (ns) | p99.9 (ns) | p99.99 (ns, exploratory — see stats::P9999_CAVEAT) | max (ns) | Outcomes |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    for r in reports {
        let p9999 = r
            .percentiles
            .p9999_ns
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a (0 samples)".to_string());
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
