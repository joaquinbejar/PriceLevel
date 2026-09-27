// benches/latency/manifest.rs
//! Run-manifest capture: everything a reader needs to judge whether a
//! latency number is reproducible or comparable to another run (issue #142).

use super::config::Config;
use std::process::Command;

/// Everything this harness records about the environment a run executed in.
#[derive(Debug, Clone)]
pub struct RunManifest {
    /// Git commit hash the worktree was at, or `"unknown"` if `git` is
    /// unavailable.
    pub commit: String,
    /// Whether the worktree had uncommitted changes (`git status
    /// --porcelain` non-empty) at run time, or `"unknown"`.
    pub dirty: String,
    /// Best-effort CPU model string.
    pub cpu: String,
    /// Logical core count ([`std::thread::available_parallelism`]).
    pub logical_cores: String,
    /// `std::env::consts::OS` / `ARCH`.
    pub os: String,
    /// `rustc -V` output, or `"unknown"` if `rustc` is unavailable on `PATH`.
    pub rustc_version: String,
    /// Build profile this binary was compiled with (`debug` or `release`,
    /// inferred from `debug_assertions`; `cargo bench` compiles with the
    /// optimized `bench` profile, which reports as `release` here).
    pub profile: &'static str,
    /// Allocator description (see `benches/latency/alloc.rs`).
    pub allocator: &'static str,
    /// Deterministic PRNG seed used by every scenario.
    pub seed: u64,
    /// Per-scenario measured sample count for single-threaded scenarios.
    pub samples: usize,
    /// Discarded warmup iteration count per single-threaded scenario.
    pub warmup: usize,
    /// Whether the 10,000 / 100,000 depth sweeps ran this time.
    pub large_depths: bool,
    /// Raw `LOGLEVEL` value, if set.
    pub loglevel: String,
    /// Measured single-call overhead of `Instant::now()` itself — the
    /// harness's own timer resolution / overhead disclosure (issue #142:
    /// "Record timer resolution/overhead").
    pub timer_overhead_ns: u64,
}

/// Runs `cmd`, returning its trimmed stdout **only when the process ran and
/// exited successfully**, regardless of whether that stdout is empty. A
/// clean `git status --porcelain` legitimately succeeds with empty stdout —
/// callers that need to tell "succeeded with nothing to report" apart from
/// "the command could not be run at all" must use this, not [`run_capture`]
/// (issue #142 review finding 8).
fn run_capture_raw(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Like [`run_capture_raw`], but additionally treats an empty (trimmed)
/// stdout as "not available" — appropriate for `git rev-parse HEAD` /
/// `rustc -V` / `sysctl`, where a successful-but-empty result is not a
/// meaningful answer, unlike `git status --porcelain`'s "clean" case.
fn run_capture(cmd: &str, args: &[&str]) -> Option<String> {
    run_capture_raw(cmd, args).filter(|s| !s.is_empty())
}

fn detect_commit() -> String {
    run_capture("git", &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string())
}

fn detect_dirty() -> String {
    match run_capture_raw("git", &["status", "--porcelain"]) {
        Some(s) if s.is_empty() => "clean".to_string(),
        Some(_) => "dirty".to_string(),
        // The command failed to run or exited non-zero (e.g. not a git
        // worktree, or `git` missing) — genuinely unknown, distinct from a
        // successful empty result.
        None => "unknown".to_string(),
    }
}

fn detect_cpu() -> String {
    if let Some(brand) = run_capture("sysctl", &["-n", "machdep.cpu.brand_string"]) {
        return brand;
    }
    if let Ok(contents) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in contents.lines() {
            if let Some(value) = line.strip_prefix("model name")
                && let Some(name) = value.split(':').nth(1)
            {
                return name.trim().to_string();
            }
        }
    }
    format!("unknown ({})", std::env::consts::ARCH)
}

fn detect_rustc_version() -> String {
    run_capture("rustc", &["-V"]).unwrap_or_else(|| "unknown".to_string())
}

/// Measures the overhead of the timer this harness uses for every sample:
/// back-to-back `Instant::now()` calls with no work between them. This is
/// the practical floor below which two samples cannot be told apart, and
/// bounds how much of a very-low-latency scenario's reported time is timer
/// overhead rather than the operation itself.
fn measure_timer_overhead_ns() -> u64 {
    const PROBES: usize = 10_000;
    let mut total_ns: u128 = 0;
    for _ in 0..PROBES {
        let t0 = std::time::Instant::now();
        let t1 = std::time::Instant::now();
        total_ns += t1.saturating_duration_since(t0).as_nanos();
    }
    u64::try_from(total_ns / PROBES as u128).unwrap_or(u64::MAX)
}

impl RunManifest {
    /// Captures the manifest for the current process. Cheap enough to call
    /// once at startup; the git / rustc subprocess calls are the only
    /// non-trivial cost and this runs exactly once per harness invocation.
    #[must_use]
    pub fn capture(config: &Config) -> Self {
        Self {
            commit: detect_commit(),
            dirty: detect_dirty(),
            cpu: detect_cpu(),
            logical_cores: std::thread::available_parallelism()
                .map(|n| n.to_string())
                .unwrap_or_else(|_| "unknown".to_string()),
            os: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
            rustc_version: detect_rustc_version(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release (cargo bench profile)"
            },
            allocator: "counting wrapper around std::alloc::System \
                        (benches/latency/alloc.rs; counting toggled off during latency runs)",
            seed: config.seed,
            samples: config.samples,
            warmup: config.warmup,
            large_depths: config.large_depths,
            loglevel: config
                .loglevel
                .clone()
                .unwrap_or_else(|| "unset".to_string()),
            timer_overhead_ns: measure_timer_overhead_ns(),
        }
    }
}

impl std::fmt::Display for RunManifest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "== Run manifest ==")?;
        writeln!(f, "commit             : {} ({})", self.commit, self.dirty)?;
        writeln!(f, "cpu                : {}", self.cpu)?;
        writeln!(f, "logical cores      : {}", self.logical_cores)?;
        writeln!(f, "os/arch            : {}", self.os)?;
        writeln!(f, "rustc              : {}", self.rustc_version)?;
        writeln!(f, "profile            : {}", self.profile)?;
        writeln!(f, "allocator          : {}", self.allocator)?;
        writeln!(f, "seed               : 0x{:016X}", self.seed)?;
        writeln!(f, "samples/scenario   : {}", self.samples)?;
        writeln!(f, "warmup/scenario    : {}", self.warmup)?;
        writeln!(f, "large depth sweeps : {}", self.large_depths)?;
        writeln!(f, "LOGLEVEL           : {}", self.loglevel)?;
        writeln!(
            f,
            "timer overhead     : {} ns (mean of 10,000 back-to-back Instant::now() calls)",
            self.timer_overhead_ns
        )?;
        writeln!(
            f,
            "loop model         : closed-loop / service-time only — see coordinated-omission \
             disclosure below"
        )
    }
}

/// The coordinated-omission disclosure every run prints, verbatim, so it can
/// never be quoted out of context from a table alone (issue #142).
pub const COORDINATED_OMISSION_DISCLOSURE: &str = "\
== Coordinated omission disclosure ==
Every scenario in this harness is CLOSED-LOOP: the driver issues the next \
operation only after the previous one returns. Reported percentiles are \
therefore SERVICE TIME (the duration of one operation once it starts \
running), not OFFERED-LOAD latency. A closed-loop measurement systematically \
UNDER-reports tail latency under saturation, because it never captures the \
queueing delay a request would see waiting for a busy server (coordinated \
omission). These numbers are a regression signal and a service-time lower \
bound for the single-matcher-per-level contract documented in \
`src/lib.rs`'s Concurrency Model section — they are NOT a production SLO and \
NOT a measurement of latency under offered load. An open-loop (fixed \
arrival-rate) experiment would be required for that and is out of scope for \
this harness.";
