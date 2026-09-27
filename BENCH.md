# Latency benchmarks

This document covers `benches/latency/` — the isolated-operation, tail-latency
harness added for issue [#142]. It is a **separate bench target** from the
Criterion suite under `benches/{price_level,concurrent,simple}/`
(`benches/mod.rs`, `[[bench]] name = "benches"`); running one never runs, or
slows down, the other. `make bench` / `bench-save` / `bench-compare` /
`bench-json` all pin `--bench benches` explicitly so they never pick up the
latency target by accident; `make bench-latency` is the only entrypoint for
this harness.

[#142]: https://github.com/joaquinbejar/PriceLevel/issues/142

## Why a second harness

The existing Criterion benches under `benches/price_level/` are useful
**lifecycle** comparisons — several of them intentionally build a level,
populate it, run a batch of operations and let Criterion measure the whole
`b.iter` closure, including construction and destruction. That is a
legitimate thing to compare release-to-release, but it is not a
per-operation latency distribution: Criterion's own statistics (mean, and
confidence intervals around the mean) are computed over *iteration batches*,
not over individual operations, and the batch size is chosen by Criterion's
calibration, not by the caller. Quoting a Criterion "mean" as a p99 is a
category error — this is why `.claude/skills/bench-hdr` and this issue both
call for a dedicated per-operation recorder instead.

`hdrhistogram` is **not** an approved dependency for this crate (see
`rules/global_rules.md`'s dependency list). This harness therefore implements
percentile computation itself: every scenario records one
`std::time::Instant` pair per operation into a pre-allocated `Vec<u64>`
(nanoseconds), and `benches/latency/stats.rs` sorts a copy of that vector to
compute p50 / p99 / p99.9 / p99.99 (see "The p99.99 caveat" below) — the
original, unsorted vector is retained and persisted alongside the run
manifest (see "Persisted artifacts").

## What is measured

Every scenario below times **exactly one public-API call** per sample.
Fixture construction (seeding a level, building the JSON to restore from,
etc.) and result destruction happen outside the timed window; where a
scenario legitimately needs fresh per-sample state (e.g. a fresh maker order
before each partial-fill match, so the level does not run dry), that setup
also runs outside the `Instant` pair — see `timing::measure_with_setup`. Where
a scenario's own operation would otherwise leave the level in a different
state than it started in (e.g. `add_order` growing the resting depth by one
per sample), an untimed teardown restores it before the next sample — see
`timing::measure_with_teardown` and "Depth is restored after every sample"
below.

| Category     | Scenarios |
|--------------|-----------|
| `isolated`   | `add_order` (GTC), `update_order(Cancel)` (found / missing), `update_order(UpdateQuantity)` (increase / decrease), `update_order(Replace)` |
| `match`      | empty book, full fill, partial fill, partial fill of one large front maker on a 1,000-deep level (#148), a many-fill sweep (20 makers in one call), iceberg replenish, reserve replenish, repeated partial fills of one large maker with no / an admission / a per-fill view `Arc` retained (#147) |
| `tif`        | GTC / IOC / DAY / GTD full match, FOK success, FOK rejection (killed), post-only rejection |
| `iteration`  | one full `iter_orders` traversal |
| `snapshot`   | `snapshot()` capture, checksum `validate()`, `from_snapshot_json` restore |
| `restore_sizes` | `from_snapshot` / `from_snapshot_json` at 100 / 10,000 / 100,000 orders, valid, failing at the last order, and failing the aggregate check at the second / last order, plus an untimed allocation / per-operation peak-memory pass (issue #150). See "Restore validation walks" below |
| `depth`      | `add_order` and a 1-unit taker match, swept across resting depth 100 / 1,000 / (opt-in) 10,000 / 100,000; order-quantity and level-price magnitude sweeps |
| `contention` | one matcher thread (`match_order`) under `N-1` concurrent admissions/cancels/reads, run once with a GTC matcher and once with an FOK matcher |
| `fok_depth`  | fill-or-kill feasibility cost versus depth (issue #143): first-maker FOK / GTC fill, rejected FOK and replenishing-iceberg FOK at depths 1 / 100 / 10,000, and writer add / cancel latency concurrent with a FOK or GTC matcher. See "Fill-or-kill feasibility depth" below |
| `stats_contention` | statistics cache contention (issue #154): matcher alone, with producers / readers on the same level, and with the same workers on an independent level; successful and overflow-rollback recording as separate cases; bare `PriceLevelStatistics` bounds. See "Statistics cache contention" below |

Every scenario asserts its own exact outcome counts (fills, rejections,
"missing" cancels, etc.) **after** the timed loop, from the operations' own
return values — never inside the timed window. A wrong count panics the
harness rather than reporting a silently-wrong number. Every scenario that
expects a trade to occur additionally asserts, after its loop,
`!PriceLevelStatistics::stats_degraded()` and an exact
`quantity_executed()` — see "Execution timestamps" below for why that
assertion exists.

### Execution timestamps

`PriceLevelStatistics::record_execution` rejects a fill whose maker
`order_timestamp` is strictly greater than the match's `execution_timestamp`
(a maker "arriving in the future" of the execution). Every maker this harness
builds stamps its own timestamp as `BASE_TIMESTAMP_MS + id`
(`fixtures::standard_order` and friends), so every scenario that expects
trades passes `fixtures::EXECUTION_TIMESTAMP_MS` — a fixed constant with a
two-billion-millisecond margin over every constructed maker id — as the
match's execution timestamp. An earlier version of this harness passed
`TimestampMs::new(0)` instead: the trade itself still happened (statistics
recording cannot retroactively fail an already-committed trade), but
`stats_degraded()` silently flipped `true` and `quantity_executed()` never
advanced on **every single sample in every scenario**, so that version was
actually measuring the degraded/error-accounting path inside
`record_execution`, not the intended fill path, without any visible signal
in its output. Every scenario now asserts `!stats_degraded()` and an exact
`quantity_executed()` after its loop specifically to catch a regression of
this kind — see `fixtures::assert_stats_healthy` and
`fixtures::EXECUTION_TIMESTAMP_MS`'s docs.

### Depth is restored after every sample

Every `add_order`-only scenario (`isolated_add_gtc`, `depth_sweep_add@*`,
`scaled_quantity@*`, `scaled_price@*`) adds one order at a fixed extra id and
then, in an untimed teardown, cancels that exact order before the next
sample — so `order_count()` is asserted equal to the scenario's declared
depth both immediately before and immediately after the whole measured loop,
not merely at fixture setup. An earlier version of this harness never
cancelled what it added: the level grew by one order per warmup and measured
sample, so a scenario labelled e.g. `depth_sweep_add@100` was, by the last
sample of a 20,000-sample default run, actually measuring `add_order` against
a level holding roughly 22,100 orders, not 100. Scenarios where the timed
operation cannot grow the level this way (`update_order` resizes/replaces in
place; matching only ever shrinks or removes a maker) do not need this
teardown and are unaffected.

Two "full match" fixtures (`match_full`, `match_partial` and every
`tif_*_full_match` variant) add exactly one fresh dedicated maker immediately
before the timed call and that maker is fully consumed by it, so the reported
depth for those is `1` (there is exactly one resting order in flight at any
instant) rather than the cumulative warmup-plus-sample count an earlier
version of this harness reported there.

### TIF coverage and what it actually shows

`PriceLevel::match_order` does not itself rest an unfilled taker (see the
doc comment on `MatchOutcome::PartiallyFilled`) and does not enforce a
resting maker's own GTD/DAY expiry (see `doc/architecture.md`). Consequently
GTC / IOC / DAY / a non-expired GTD taker exercise the **identical** code
path inside `match_order` — only the `TimeInForce` discriminant passed in
differs. `tif.rs` measures this directly instead of assuming a difference;
the example run below shows exactly that (GTC/IOC/DAY/GTD full-match numbers
are the same distribution within noise). FOK is the one taker TIF that
branches differently (it takes the level-wide fill-or-kill guard), which
shows up in the uncontended `tif_fok_success` / `tif_fok_reject` numbers and,
far more dramatically, in the contention comparison below.

### The contention scenario, and its fixture-parity fixes

`contention_gtc_matcher` and `contention_fok_matcher` run one matcher thread
against `N-1` writer threads that continuously add/cancel/read against a
disjoint "churn" id pool, using a `Barrier` + `AtomicBool` start protocol so
every thread begins at (as close as possible to) the same instant.

An earlier version of this scenario pre-seeded `matcher_ops` dedicated
resting makers up front (thousands, at the default sample count) while the
uncontended `tif_fok_success` / `tif_gtc_full_match` baselines keep exactly
**one** dedicated maker in flight at any instant. That made "contended vs.
its own uncontended baseline" an apples-to-oranges comparison — some of the
gap could have come from the much larger matcher-target depth itself, not
from contention or the FOK guard. This version instead adds one fresh
dedicated target maker per matcher iteration, untimed, immediately before the
timed `match_order` call (the same shape `tif.rs::full_match_with_tif` uses).

That first fix introduced a second problem a follow-up review caught:
`match_order` sweeps the level's resting queue strictly FIFO, and the churn
pool was seeded before the matcher loop starts, so every per-iteration target
maker is admitted *behind* the churn pool in sequence order. A 1-quantity
taker is satisfied by whichever order is currently at the front — typically a
churn order, not the fresh target — so the timed call is **not** guaranteed
to consume the target that same iteration just added. Left unconsumed, that
target stayed resting forever; matcher-owned depth grew by up to one order
per iteration instead of staying bounded, which also inflated an FOK
matcher's `O(depth)` preflight cost across the run — silently invalidating
both the "matcher-owned depth is 1" claim and the churn-pool-only depth
parity with the uncontended baseline as the run progressed. The fix: an
UNTIMED cancel of that same iteration's own target id immediately after the
timed call — a harmless `Ok(None)` if the match already consumed it, a real
removal if it is still resting — so a target survives past its own iteration
only as a transient state, never accumulates. `run_contention` asserts this
bound after the loop by attempting the identical cancel on every target id
and requiring `Ok(None)` for all of them (i.e. none are still resting).

This still does not make "contended vs. uncontended" a fully controlled
comparison (the churn pool and writer threads are real, by-design
differences — that *is* contention). The claim below is restricted to
**GTC-vs-FOK under the same load**, which is controlled. It also reports
writer throughput as a **rate**, not a raw count: the matcher's fixed op
count means the GTC and FOK runs cover different wall-clock windows (the GTC
matcher finishes in well under a millisecond; the FOK matcher takes tens of
milliseconds at the same op count), so a raw writer-op count comparison would
conflate "writers did less work" with "writers had less time" — see the
example run's numbers below, where writer *throughput* (not count) drops by
roughly two orders of magnitude under FOK.

That rate itself needed a second fix: an earlier version divided the summed
writer op count by the *matcher's* elapsed window, but each writer keeps
counting completed ops from `go` until it next observes `stop` — strictly
later than the matcher's own window ends, by an unbounded amount if a writer
is mid-iteration when `stop` flips. A numerator spanning a longer interval
than its denominator is not a rate over any single interval. This version
instead has each writer time its own active window (the exact span its own
`completed` counter spans) and reports the **sum of each writer's own
`completed / that writer's own elapsed`** — every individual ratio is a rate
over one consistent interval, so summing them is a well-defined aggregate.

## The p99.99 caveat

A sample count alone does not make p99.99 a validated tail quantile: it is a
single order statistic from **one run**, and nothing in this harness repeats
a run to check whether that one point is stable from run to run. Every
p99.99 figure this harness prints or writes is therefore always labelled
`unvalidated exploratory estimate` (`stats::P9999_CAVEAT`) — in the stdout
table, the Markdown table's own column header, and `manifest.json`'s
`p9999_caveat` field — rather than silently implied to be trustworthy once a
sample-count floor is cleared. To actually check stability, rerun this
harness (optionally with a different `PL_LATENCY_SEED`) and compare the
persisted raw observations (see below) across runs; this document does not
claim to have done that itself.

## Persisted artifacts

Every run writes `target/latency/<run-id>/manifest.json` (the run
environment, the actual `Config` used, and every scenario's measured
boundary and percentile summary) plus one `<scenario name>.csv` per scenario
— its raw, unsorted, per-sample nanosecond observations, one per line, in
original call order. `target/` is already gitignored; these are ordinary
local run artifacts, not something this repository commits. This exists so a
percentile (especially the p99.99 above) can be independently reprocessed,
diffed against another run, or checked for stability, rather than trusted as
a single printed number.

## Allocation measurements

`benches/latency/alloc.rs` installs a `#[global_allocator]` wrapper around
`std::alloc::System` for this binary only (`unsafe impl GlobalAlloc`,
required by the trait; confined to this bench binary, never `src/` — see the
module doc for the full justification). Counting is **disabled** during every
latency scenario above, so those numbers are not inflated by counter
bookkeeping; a separate, untimed pass in `alloc_measurements.rs` pre-builds
every harness-owned buffer (input orders, result vectors), resets the
counters, enables counting, runs `PL_LATENCY_ALLOC_REPS` repetitions of one
representative operation, disables counting, and reports the per-operation
average. Every harness-owned buffer is deliberately allocated **before**
counting starts and dropped **after** it stops (an earlier version allocated
one result buffer per measurement after enabling counting, and consumed its
input-orders buffer — triggering that buffer's own deallocation — while
still inside the counted window, both of which counted the harness's own
bookkeeping as if it were the operation's cost). `add_order` / `match_full`
are cheap (a handful of allocations); `checksum_validate` and `restore` are
not — see the example numbers below and treat them as a baseline to catch a
regression against, not as an absolute performance claim (no comparable
"before" run exists yet).

## Coordinated omission disclosure

Every scenario in this harness — including the contention scenario, on the
matcher's own thread — is **closed-loop**: the next operation is issued only
after the previous one returns. Reported percentiles are **service time**,
not offered-load latency, and a closed-loop measurement systematically
under-reports the tail under saturation because it cannot see queueing delay
that never happened (there was no queue — the driver was blocked waiting,
not queuing new work). These numbers are a regression signal and a
service-time lower bound for the single-matcher-per-level contract in
`src/lib.rs`'s "Concurrency Model" section. They are **not** a production SLO
and **not** evidence about latency under offered load; an open-loop
(fixed-arrival-rate) experiment would be required for that and is out of
scope here. The harness also measures and reports its own timer overhead
(mean of 10,000 back-to-back `Instant::now()` calls) in the run manifest, as
the practical floor below which two samples cannot be told apart.

## How to run

```sh
make bench-latency
# equivalently:
cargo bench --bench latency
```

Every knob is an environment variable (`benches/latency/config.rs`):

| Variable                          | Default | Meaning |
|-----------------------------------|---------|---------|
| `PL_LATENCY_SAMPLES`              | 20,000  | Measured samples per single-threaded scenario |
| `PL_LATENCY_WARMUP`               | 2,000   | Discarded warmup iterations per scenario |
| `PL_LATENCY_SEED`                 | fixed   | Recorded in the manifest (this harness uses deterministic sequential ids, not a PRNG, so this is provenance, not a workload input, today) |
| `PL_LATENCY_LARGE_DEPTHS`         | off     | Set to `1` to additionally sweep 10,000 / 100,000 resting-order depth |
| `PL_LATENCY_STRICT_FIFO`          | off     | Set to `1` to fail the `fok_depth` writer contention cases on any starvation anomaly instead of counting it (issue #206) |
| `PL_LATENCY_CONTENTION_THREADS`   | 4       | Total threads in the contention scenario (1 matcher + N-1 writers) |
| `PL_LATENCY_CONTENTION_OPS`       | 5,000   | Matcher-thread operations measured per contention run |
| `PL_LATENCY_ALLOC_REPS`           | 2,000   | Repetitions per operation in the allocation-measurement pass |
| `PL_LATENCY_STATS_PRODUCERS`      | 2       | Producer (cancel + re-add) threads per `stats_contention` case |
| `PL_LATENCY_STATS_READERS`        | 2       | Statistics-reader threads per `stats_contention` case |
| `PL_LATENCY_STATS_OPS`            | 20,000  | Matcher operations measured per `stats_contention` case |
| `PL_LATENCY_ONLY`                 | all     | Comma-separated groups to run: `isolated`, `match`, `tif`, `snapshot` (includes `iteration`), `snapshot_sizes` (issue #149, with its own allocation pass), `depth`, `fok_depth`, `contention`, `stats_contention`, `alloc` |

A short validation run (a few minutes at most, typically a few seconds):

```sh
PL_LATENCY_SAMPLES=300 PL_LATENCY_WARMUP=50 PL_LATENCY_CONTENTION_OPS=300 \
  PL_LATENCY_ALLOC_REPS=200 make bench-latency
```

## Example run

**This is one example run for reproducibility and interpretation, not a
performance claim.** 300 samples is a deliberately small validation-run size
(see "How to run"); every p99.99 figure below carries the exploratory caveat
from "The p99.99 caveat" section regardless of sample count. Run it yourself
with the command above; do not cite these specific nanosecond figures as a
crate performance guarantee.

### Manifest

```
== Run manifest ==
commit             : 9ee1ddee31dc0878e8040ffcc16fe2fb46515988 (dirty)
cpu                : Apple M5 Max
logical cores      : 18
os/arch            : macos/aarch64
rustc              : rustc 1.98.1 (48a229cea 2026-09-01)
profile            : release (cargo bench profile)
allocator          : counting wrapper around std::alloc::System (benches/latency/alloc.rs; counting toggled off during latency runs)
seed               : 0xA5A5A5A5A5A5A5A5
samples/scenario   : 300
warmup/scenario    : 50
large depth sweeps : false
LOGLEVEL           : unset
timer overhead     : 13 ns (mean of 10,000 back-to-back Instant::now() calls)
loop model         : closed-loop / service-time only — see coordinated-omission disclosure below
```

`(dirty)` above reflects the worktree state at the moment this example was
captured during development of this harness, not a property of the harness
itself; a clean checkout on a tagged commit reports `(clean)`. This same
information, plus the `Config` used and every scenario's measured boundary
and percentile summary, is written to
`target/latency/1790505358408/manifest.json` for this particular run (see
"Persisted artifacts" — the run id is a wall-clock millisecond timestamp, so
yours will differ).

### Results

Every `p99.99 (ns)` value below is the same [exploratory
estimate](#the-p9999-caveat) `stats::P9999_CAVEAT` describes; it is not
repeated per cell here for readability, matching the harness's own Markdown
table output.

| Scenario | Category | Depth | Samples | p50 (ns) | p99 (ns) | p99.9 (ns) | p99.99 (ns) | max (ns) | Outcomes |
|---|---|---|---|---|---|---|---|---|---|
| isolated_add_gtc | isolated | 1000 | 300 | 83 | 1041 | 1333 | 1333 | 1333 | 300/300 succeeded |
| isolated_cancel_success | isolated | 350 | 300 | 42 | 666 | 833 | 833 | 833 | 300/300 found and cancelled |
| isolated_cancel_missing | isolated | 1000 | 300 | 41 | 42 | 42 | 42 | 42 | 300/300 reported missing (Ok(None)) |
| isolated_quantity_decrease | isolated | 350 | 300 | 42 | 125 | 834 | 834 | 834 | 300/300 resized (100 -> 40) |
| isolated_quantity_increase | isolated | 350 | 300 | 125 | 750 | 1041 | 1041 | 1041 | 300/300 resized (40 -> 100) |
| isolated_replace | isolated | 350 | 300 | 125 | 792 | 875 | 875 | 875 | 300/300 replaced |
| match_empty | match | 0 | 300 | 209 | 292 | 375 | 375 | 375 | 300/300 NotFilled |
| match_full | match | 1 | 300 | 209 | 958 | 1542 | 1542 | 1542 | 300/300 Filled |
| match_partial | match | 1 | 300 | 416 | 500 | 1042 | 1042 | 1042 | 300/300 PartiallyFilled |
| many_fill_sweep | match | 7000 | 300 | 4000 | 6042 | 9167 | 9167 | 9167 | 300/300 Filled, 6000 total trades |
| iceberg_replenish | match | 1 | 300 | 291 | 917 | 958 | 958 | 958 | 300/300 Filled |
| reserve_replenish | match | 1 | 300 | 250 | 834 | 1041 | 1041 | 1041 | 300/300 Filled |
| tif_gtc_full_match | tif | 1 | 300 | 208 | 750 | 916 | 916 | 916 | 300/300 Filled (taker_tif=Gtc) |
| tif_ioc_full_match | tif | 1 | 300 | 209 | 750 | 834 | 834 | 834 | 300/300 Filled (taker_tif=Ioc) |
| tif_day_full_match | tif | 1 | 300 | 209 | 334 | 834 | 834 | 834 | 300/300 Filled (taker_tif=Day) |
| tif_gtd_full_match | tif | 1 | 300 | 209 | 333 | 750 | 750 | 750 | 300/300 Filled (taker_tif=Gtd(9999999999999)) |
| tif_fok_success | tif | 1 | 300 | 1375 | 2083 | 2875 | 2875 | 2875 | 300/300 Filled (taker_tif=Fok) |
| tif_fok_reject | tif | 1 | 300 | 1167 | 1209 | 1209 | 1209 | 1209 | 300/300 Killed |
| tif_post_only_reject | tif | 1 | 300 | 417 | 500 | 583 | 583 | 583 | 300/300 Rejected |
| iteration | iteration | 1000 | 300 | 6000 | 6542 | 8375 | 8375 | 8375 | 300/300 traversals visited exactly 1000 orders |
| snapshot_capture | snapshot | 1000 | 300 | 12292 | 12750 | 14584 | 14584 | 14584 | 300/300 snapshots carried exactly 1000 orders |
| checksum_validate | snapshot | 1000 | 300 | 903084 | 1008167 | 1009834 | 1009834 | 1009834 | 300/300 validated OK |
| restore | snapshot | 1000 | 300 | 1282083 | 1428750 | 1493583 | 1493583 | 1493583 | 300/300 restored with exactly 1000 orders |
| depth_sweep_add@100 | depth | 100 | 300 | 83 | 125 | 959 | 959 | 959 | 300/300 succeeded |
| depth_sweep_add@1000 | depth | 1000 | 300 | 83 | 125 | 750 | 750 | 750 | 300/300 succeeded |
| depth_sweep_small_taker@100 | depth | 100 | 300 | 167 | 291 | 292 | 292 | 292 | 300/300 Filled |
| depth_sweep_small_taker@1000 | depth | 1000 | 300 | 208 | 250 | 292 | 292 | 292 | 300/300 Filled |
| scaled_quantity@1 | depth | 100 | 300 | 83 | 167 | 208 | 208 | 208 | 300/300 succeeded |
| scaled_quantity@10000 | depth | 100 | 300 | 83 | 167 | 708 | 708 | 708 | 300/300 succeeded |
| scaled_quantity@1000000000 | depth | 100 | 300 | 83 | 167 | 625 | 625 | 625 | 300/300 succeeded |
| scaled_price@1 | depth | 100 | 300 | 83 | 167 | 625 | 625 | 625 | 300/300 succeeded |
| scaled_price@10000 | depth | 100 | 300 | 83 | 125 | 1000 | 1000 | 1000 | 300/300 succeeded |
| scaled_price@18446744073709551615 | depth | 100 | 300 | 83 | 167 | 583 | 583 | 583 | 300/300 succeeded |
| contention_gtc_matcher | contention | 1 | 300 | 708 | 3375 | 17917 | 17917 | 17917 | matcher: 300/300 Filled (403746 ops/s over 0.000743s); writers: 4348 completed (3259 successful, 4 missing, 1085 rejected) across 3 threads = 5866452 ops/s (sum of each writer's own completed/elapsed) |
| contention_fok_matcher | contention | 1 | 300 | 29416 | 84583 | 206750 | 206750 | 206750 | matcher: 300/300 Filled (31375 ops/s over 0.009562s); writers: 2105 completed (1776 successful, 52 missing, 277 rejected) across 3 threads = 220078 ops/s (sum of each writer's own completed/elapsed) |

**Reading the FOK contention row (GTC-vs-FOK under identical load only — see
"The contention scenario" above for why this comparison, and not "vs.
uncontended", is the controlled one).** `contention_fok_matcher`'s p50
(29,416 ns) is roughly 42x `contention_gtc_matcher`'s p50 (708 ns) under the
identical churn-pool / writer-thread load and the same bounded matcher-owned
depth. That gap is consistent with `doc/architecture.md`'s "Fill-or-kill
excludes every mutator on the level": an FOK match holds the level-wide guard
exclusively across its whole dry-run and sweep, so it now also waits behind
the writer threads' admissions/cancels contending for that same guard's
shared side — not the per-maker shard lock GTC pays alone. Writer
*throughput* during the FOK run also dropped by roughly an order of
magnitude (220,078 ops/s, summed per-writer, vs. 5,866,452 ops/s for GTC) —
each figure is itself a sum of each writer's own `completed / that writer's
own elapsed`, not a raw count over a shared window (see "The contention
scenario" above for why a shared-window rate would be invalid here). The
FOK run's matcher loop also ran roughly 13x longer in wall-clock time
(9.6 ms vs. 0.74 ms) at the same fixed op count — consistent with, not
independent of, the throughput drop. This is exactly the effect the issue
asks this harness to make visible, separated from uncontended service time.

### Allocation measurements (same run)

```
add_order            reps=200    alloc_count/op=2.17     alloc_bytes/op=368.36     dealloc_count_total=33       dealloc_bytes_total=13984
match_full           reps=200    alloc_count/op=3.12     alloc_bytes/op=1793.93    dealloc_count_total=548      dealloc_bytes_total=64362
snapshot_capture     reps=200    alloc_count/op=138.00   alloc_bytes/op=43776.00   dealloc_count_total=27400    dealloc_bytes_total=7155200
checksum_validate    reps=200    alloc_count/op=36014.00 alloc_bytes/op=936224.00  dealloc_count_total=7202800  dealloc_bytes_total=187244800
restore              reps=200    alloc_count/op=41348.28 alloc_bytes/op=1790290.74 dealloc_count_total=7843666  dealloc_bytes_total=293434460
```

`checksum_validate` and `restore` allocate far more than `add_order` /
`match_full` because both go through `serde_json` deserialization of the
whole snapshot package (`checksum_validate` re-derives the checksum by
re-serializing the payload to bytes to hash it — see
`PriceLevelSnapshotPackage::validate`'s doc comment — and `restore` additionally
reconstructs every order and re-admits it into a fresh level). This is a
plausible, named explanation, not a `perf`-verified one; treat it as a
hypothesis to check with a profiler before optimizing, not as a settled root
cause.

## Statistics cache contention (issue #154)

Question: does the `PriceLevelStatistics` layout add latency when producers,
the one matcher and statistics readers share a level? The work stays inside
the writer contract of issue #153 (one `record_execution` writer per level;
`record_order_added` / `record_order_removed` from any thread). Nothing here
weakens checked arithmetic or all-or-nothing recording, and no CAS is
replaced by a load / store.

### Layout (measured)

`PriceLevelStatistics::field_layout()` is a test-only probe built on
`std::mem::offset_of!`, and
`test_statistics_layout_64bit_is_compact_single_block` pins its result. The
layout is identical on `aarch64-apple-darwin` and `x86_64-apple-darwin`:

| Offset | Size | Field | Written by |
|---|---|---|---|
| 0 | 16 | `value_executed` (`AtomicU128`) | matcher |
| 16 | 8 | `orders_added` | producers (any thread) |
| 24 | 8 | `orders_removed` | producers (any thread) |
| 32 | 8 | `orders_executed` | matcher |
| 40 | 8 | `quantity_executed` | matcher |
| 48 | 8 | `last_execution_time` | matcher |
| 56 | 8 | `first_arrival_time` | construction / `reset` |
| 64 | 8 | `sum_waiting_time` | matcher |
| 72 | 8 | `stats_seq` | matcher (readers load it) |
| 80 | 1 | `stats_degraded` | any failed record |

`size_of` is 96 and `align_of` is 16. The level holds it as
`Arc<PriceLevelStatistics>`, so each level costs one 112-byte heap
allocation (16-byte `ArcInner` counts followed by the 96 bytes) plus the
8-byte pointer. The allocator guarantees only 16-byte alignment, so which
fields share a line depends on the address. Over the 16-byte-aligned
placements:

| Pair on one line | 64-byte lines (x86_64) | 128-byte lines (Apple aarch64) |
|---|---|---|
| `orders_added` and `orders_executed` | 3 of 4 | 7 of 8 |
| `orders_added` and `value_executed` | 3 of 4 | 7 of 8 |
| `orders_added` and `stats_seq` | 1 of 4 | 5 of 8 |
| `Arc` strong count and `value_executed` | 3 of 4 | 7 of 8 |

The whole object covers at most 2 lines of 128 bytes and at most 3 lines of
64 bytes. The observed data offset inside a 128-byte line changed from run
to run (16, 80, 96). The strong count matters because `PriceLevel::stats()`
returns an `Arc` clone, so every reader that calls it performs an atomic RMW
next to the statistics fields.

### Scenarios

All cases live in `benches/latency/scenarios/stats_contention.rs`. The
matcher level is pre-seeded, in FIFO order, with `warmup + ops` one-unit
makers, followed by 500 churn orders per producer. Each timed
`match_order(1)` consumes exactly maker `i` on call `i`; this is asserted
after the loop. Every case builds its levels the same way, through
`from_snapshot`, so depth (23,000 at the start) and construction path match
across topologies.

- `statsc_{ok,overflow}_single`: the matcher runs alone (control).
- `statsc_ok_same_producers` / `_same_readers` / `_same_mixed`: 2 producers
  (cancel then re-add their own churn orders) and/or 2 readers (point reads
  and a seqlock `Clone` through `PriceLevel::stats()`, plus a full
  `snapshot()` every 1,024 ops) run on the matcher's level.
- `statsc_{ok,overflow}_indep_mixed`: the same workers run on a second level
  that starts out identical. This is a **background-load reference**, not an
  equal-work control. The matcher drains its own level toward the churn
  pool, while the independent level keeps all 22,000 initial makers.
  Readers there run every periodic `snapshot()` over roughly 23,000 orders
  instead of a shrinking set, so they scan and allocate far more per
  snapshot and complete fewer `Clone`s (see the worker-row sample counts
  below). The row controls for thread count and machine load. It does not
  control for the reader and producer work mix.
- `*_producer_add` / `*_reader_clone`: producer 0's `add_order` and reader
  0's statistics `Clone`, recorded only while the matcher measures.
- `statsc_raw_*`: a bare `PriceLevelStatistics`. One sample is a batch of 16
  `record_execution` calls, because a single call is below the 41.67 ns
  timer tick. Producers call `record_order_added` / `record_order_removed` on
  the same object (they write different fields from the matcher, so any
  cost is false sharing) or on another object (control). Readers `Clone` the
  same object.

Recording outcome is a separate axis. In `ok` cases every
`record_execution` succeeds (asserted: `!stats_degraded()`, exact
`quantity_executed()`). `overflow` cases restore `sum_waiting_time` at
`u64::MAX`, so every record commits three aggregates, overflows, rolls them
back and formats its error inside the write section (asserted:
`stats_degraded()`, zero `quantity_executed()` / `orders_executed()`). This
is the longest rejected path. Since #140, scaled-value workloads take the
successful path. The two paths are never merged into one number.

### Method

- **Hardware and build:** Apple M5 Max (18 logical cores: 6 performance, 12
  efficiency), macOS aarch64, rustc 1.98.1, `bench` profile. Threads are not
  pinned; macOS exposes no affinity API. `Instant` ticks are 41.67 ns, so a
  `reader_clone` p50 of 0 means under one tick.
- **Sampling:** 20,000 measured matcher samples and 2,000 warmup per case.
  Five interleaved rounds per binary (A, B, A, B, ...). Matcher and raw rows
  pool 100,000 observations. `PL_LATENCY_STATS_OPS` is only a **cap** for
  worker rows (`*_producer_add`, `*_reader_clone`): recording stops when the
  matcher's measured loop ends, so each round records fewer samples. The
  pooled worker counts are in the Samples column below; per-run counts are
  in each run's `manifest.json`. Percentiles are nearest-rank over the
  pooled CSVs from `target/latency/<run-id>/`.
- **Worker rates (baseline, same level, mixed):** matcher about 1.1 M ops/s;
  producers about 2.4 M ops/s combined; readers about 3.2 M ops/s combined.
  In the raw case, producers reached about 29 M ops/s.
- **Loop model:** closed-loop service time; see "Coordinated omission
  disclosure".
- **Prototype (B):** `#[repr(C, align(128))]` with `orders_added` /
  `orders_removed` first and a 112-byte pad, so the producer counters, the
  matcher / seqlock fields and the `Arc` counts each get their own 128-byte
  line. `ArcInner` grows from 112 to 384 bytes (+272 bytes per level, with
  128-byte alignment). This was a throwaway branch and is not committed.

### Results

p99.99 is an [exploratory estimate](#the-p9999-caveat). Values are ns.

Samples are pooled counts (baseline / padded).

| Scenario | Samples | Baseline p50 | Padded p50 | Baseline p99 | Padded p99 | Baseline p99.9 | Padded p99.9 | Baseline p99.99 | Padded p99.99 |
|---|---|---|---|---|---|---|---|---|---|
| statsc_ok_single | 100000 / 100000 | 250 | 333 | 875 | 1000 | 1917 | 8917 | 12042 | 79791 |
| statsc_ok_same_producers | 100000 / 100000 | 625 | 500 | 2250 | 2000 | 12667 | 15875 | 49250 | 427000 |
| statsc_ok_same_readers | 100000 / 100000 | 292 | 292 | 1375 | 2083 | 20875 | 59625 | 457250 | 943250 |
| statsc_ok_same_mixed | 100000 / 100000 | 833 | 666 | 4541 | 6833 | 63250 | 248417 | 986584 | 10012125 |
| statsc_ok_same_mixed_producer_add | 68169 / 64734 | 708 | 584 | 2291 | 2375 | 13958 | 16708 | 180208 | 404083 |
| statsc_ok_same_mixed_reader_clone | 92704 / 88616 | 0 | 0 | 333 | 166 | 583 | 250 | 8000 | 7833 |
| statsc_ok_indep_mixed | 100000 / 100000 | 333 | 333 | 1708 | 1500 | 8541 | 8250 | 34834 | 39959 |
| statsc_ok_indep_mixed_producer_add | 29336 / 22500 | 583 | 542 | 2083 | 2459 | 13959 | 12833 | 84167 | 35667 |
| statsc_ok_indep_mixed_reader_clone | 13797 / 11753 | 0 | 0 | 125 | 83 | 167 | 125 | 6250 | 7583 |
| statsc_overflow_single | 100000 / 100000 | 333 | 333 | 959 | 917 | 8500 | 1666 | 34667 | 21333 |
| statsc_overflow_same_mixed | 100000 / 100000 | 791 | 792 | 3791 | 7083 | 51334 | 72916 | 10018292 | 843208 |
| statsc_overflow_same_mixed_producer_add | 68649 / 64030 | 667 | 708 | 2167 | 2666 | 15666 | 16583 | 574167 | 246875 |
| statsc_overflow_same_mixed_reader_clone | 93610 / 93448 | 0 | 0 | 417 | 250 | 625 | 375 | 9417 | 8750 |
| statsc_overflow_indep_mixed | 100000 / 100000 | 375 | 375 | 1375 | 1459 | 4458 | 8834 | 21417 | 38292 |
| statsc_overflow_indep_mixed_producer_add | 25907 / 30307 | 583 | 541 | 1875 | 2209 | 10250 | 13125 | 30750 | 74500 |
| statsc_overflow_indep_mixed_reader_clone | 24017 / 15841 | 0 | 0 | 83 | 42 | 125 | 125 | 500 | 8333 |
| statsc_raw_ok_single (per 16) | 100000 / 100000 | 166 | 166 | 209 | 208 | 250 | 250 | 9375 | 1500 |
| statsc_raw_ok_same_producers (per 16) | 100000 / 100000 | 2708 | 166 | 10500 | 209 | 18541 | 375 | 60500 | 12958 |
| statsc_raw_ok_indep_producers (per 16) | 100000 / 100000 | 166 | 166 | 209 | 209 | 292 | 542 | 9042 | 12667 |
| statsc_raw_ok_same_readers (per 16) | 100000 / 100000 | 958 | 959 | 2958 | 2625 | 11250 | 11875 | 53916 | 47875 |
| statsc_raw_overflow_single (per 16) | 100000 / 100000 | 916 | 917 | 1166 | 1167 | 11541 | 8750 | 79417 | 85583 |
| statsc_raw_overflow_same_producers (per 16) | 100000 / 100000 | 10917 | 917 | 6075667 | 1208 | 36998000 | 9125 | 75595833 | 19458 |

Allocation: no engine code changed, so allocation per operation is the same
as in the rest of this document. The only memory change evaluated is the
prototype's +272 bytes per level.

### Reading

1. **The false sharing is real at the statistics object.** Producers that
   only touch `orders_added` / `orders_removed` raise the matcher's batch
   p50 from 166 to 2,708 ns. The independent-object control stays at 166 ns,
   and padding brings it back to 166 ns. On the rollback path, the same
   tight-loop producers push the batch p99 to about 6 ms, and one run
   reached 60 ms at p99.9. This looks like contended-atomic starvation on
   Apple silicon, and padding removes it. These cases are upper bounds: the
   producers do nothing but statistics RMWs, at about 29 M ops/s.
2. **Inside the engine, the statistics line looks like a minor share of the
   contention.** On the same level, the matcher's p50 is about 300 to
   500 ns above the independent-level background-load reference (833
   versus 333, and 791 versus 375). That reference does not run the same
   worker work (see Scenarios), so this gap is not a controlled
   same-level cost. Padding recovers about 125 to 170 ns of p50 in the `ok`
   producer and mixed cases. It recovers nothing in `overflow_same_mixed`
   (791 versus 792). **Hypothesis, not demonstrated here:** most of the
   remaining gap comes from state that producers and the matcher genuinely
   share (`topology`, `visible_quantity`, `DashMap` shards, the `SkipMap`,
   the `fok_guard` reader count), which padding the statistics cannot
   remove. Testing it would need an equal-work control and per-structure
   attribution.
3. **No consistent tail benefit.** Across five interleaved rounds, the
   padded build shows no consistent or demonstrably repeatable p99 / p99.9
   improvement on a shared level. `ok_same_producers` p99 improves (2,250
   to 2,000 ns), but `ok_same_mixed` (4,541 to 6,833 ns) and
   `overflow_same_mixed` (3,791 to 7,083 ns) get worse, and p99.9 is worse
   in all three. Run-to-run spread (for example, `ok_single` p99.9 from 1
   to 9 µs) is larger than any difference attributable to the layout. The
   address of the statistics allocation inside a line also changes between
   runs. Reader `Clone` p99 improves (333 to 166 ns), but that is a
   reader-side gain, not matcher tail latency.

### Decision: no change

The current layout stays. The only improvement the prototype shows inside
the engine is a 15 to 20% p50 gain in two same-level cases. It shows no
consistent or demonstrated repeatable p99 / p99.9 benefit (one p99
improvement, two regressions, p99.9 worse in all three), costs 3.4 times the statistics memory per level
(+272 bytes, 128-byte-aligned allocation), and would need a `repr(C)` field
order plus pad fields kept in sync across every constructor. That
does not meet the acceptance bar ("measured benefit with no unexplained tail
regression"). The raw scenarios remain as a regression tripwire. Revisit if
other shared-state contention on the level is reduced enough for the
statistics lines to become a larger share, or on hardware where a
same-level p99 difference shows up above run-to-run noise. Criterion
numbers were not collected for this investigation; the per-operation
harness above is the evidence.

## Retained Criterion benches

This harness does not modify the existing Criterion suite. `match_orders.rs`,
`update_orders.rs` and `snapshot_recovery.rs` under `benches/price_level/`
continue to measure setup + batched operations + (for some cases)
destruction together, as lifecycle comparisons — see the "Why a second
harness" section above for why that is a different, and still useful,
measurement from the per-operation numbers in this document.

## Parked-maker front scans (issue #155)

`OrderQueue::match_front` restarts its front scan at the lowest sequence on
every step and skips the sequences the current sweep has parked. A prefix of
`S` parked index keys ahead of `K` fills costs about `S * K` extra index
visits. The parking paths:

- **No-progress `SetAside`: unreachable.** The sweep parks a maker only when
  `match_against` returns `consumed == 0`, `hidden_reduced == 0`, an
  unchanged remainder and a residual. The loop runs only while the taker
  remainder is positive. For that remainder `Standard`, `PostOnly`,
  `TrailingStop`, `PeggedOrder` and `MarketToLimit` consume
  `min(quantity, remaining)` and return no residual on a full match (a
  zero-quantity maker is removed, not parked). `IcebergOrder` consumes on a
  partial match and, on a full visible match, either draws a positive
  hidden tranche (a zero-visible iceberg draws all of its hidden) or is
  removed. `ReserveOrder` does the same with a `NonZeroU64` replenish amount.
  This holds for every field value, so it covers shapes an update or
  snapshot restore can produce, not only admission-validated ones. The guard
  stays as defense in depth.
- **Self-trade skip: one live parked key, possibly several stale ones.** It
  parks a maker whose id equals the taker's. `match_order` rejects the taker
  up front when that id already rests, so the skip fires only when the
  taker's own order is admitted after that probe, during a non-`Fok` sweep
  (a `Fok` sweep holds the level guard exclusively, blocking admission).
  Order storage is keyed by id and admission is insert-if-absent
  (`DuplicateOrderId`), so at most one resting order carries the taker id
  and at most one parked key is live. Parked keys can still go stale, and
  this is reachable under the documented model (one matcher, concurrent
  cancels and admissions): `OrderQueue::remove` deletes the map entry,
  releases the shard lock, and only then deletes the index key. If another
  thread readmits the id in that gap, the same sweep parks the new
  sequence while the old key is still indexed and still in the sweep's
  parked set. Overlapping cancel / readmit pairs stack several such keys.
- **`Failed`, `IdsExhausted`, `SequenceExhausted`, `Abort`** also return
  `SetAside` but stop the sweep, so nothing is scanned again.

Before the fix the front scan filtered the parked set before its stale-entry
checks, so a stale parked key was visited on every later step until its
paused cancel resumed. `match_front` now checks each parked key it meets
against the map (a shard read lock taken only for parked keys, never on the
common path with nothing parked) and removes the key from the index and from
the parked set when its id no longer rests under that sequence. Sequences are
never reused, so such a key can never become live again; the removal is the
same self-heal as the existing `Vacant` and stale-front (#119) arms, and it
leaves admission, update, cancel, replenishment tail insertion and the
makers parked earlier in the sweep unchanged. Each stale key now costs one
visit in total and the steady cost is one extra visit per step (the live
parked key), `O(K + stale keys)` rather than `O(S * K)`. A full sweep cursor
was not needed.

Measured with the `#[cfg(test)]` visit counter in `match_front` (release and
bench builds do not compile it) on the adversarial schedule in
`overlapping_cancel_gaps_leave_stale_parked_keys_that_are_visited_once`:
`D = 4` nested cancels, each paused in the gap by a `#[cfg(test)]` hook in
`OrderQueue::remove` while the id is readmitted and re-parked by the same
sweep scratch, then `K = 64` fills. No sleeps; the schedule is exact.

| | Parking steps | Fills | Total |
|---|---|---|---|
| Before (parked set checked first) | `1 + D(D+3)/2` = 15 | `K(D+2)` = 384 | 399 |
| After (stale parked keys dropped) | `1 + 2D` = 9 | `2K` = 128 | 137 |

Visits are the metric here, not latency: the schedule needs a paused cancel,
so it is a correctness-of-bound test rather than a latency-harness scenario.
The other tests in `src/price_level/tests/parked_prefix.rs` pin the rest of
the bound: `K` fills with nothing parked visit exactly `K` entries, one
parked self-trade maker gives `2K + 2`, a demoted parked maker never forms a
two-entry prefix, and an 800-shape grid over every variant asserts the
no-progress shape never occurs.

## MatchResult capacity (issue #148)

`match_order` pre-sizes both result vectors (trades and filled order ids) to
`min(incoming quantity, resting order count)`. A partial fill of the front
maker or an iceberg / reserve replenishment emits a trade without a filled
id, so the filled-id buffer is reserved but unused on those paths. This
section records the evaluation of changing that.

### Cases

Allocation pass (`PL_LATENCY_ONLY=alloc`, `alloc_measurements.rs`), latency
scenario `match_maker_partial` (`scenarios/matching.rs`) and the Criterion
group `MatchResult capacity (#148)` (`benches/price_level/result_capacity.rs`)
cover: zero trades (empty level), a qty-10 partial fill of one huge front
maker on a 1,000-deep level (1 trade, 0 filled ids), the same as
fill-or-kill, a single full fill (1 trade, 1 filled id), a 100-maker sweep,
and iceberg / reserve replenishment (1x and 5x the visible tranche; 5x emits
5 trades from 1 resting order, above the order-count estimate).
`Trade` is 144 bytes and `Id` 32 bytes on the measurement host (printed by
the allocation pass).

### Variants

- **base**: current design (one shared estimate, joint per-step check).
- **eager**: independent estimates. Trades reserved per step; the filled-id
  slot checked inside the locked decision closure only when the step fully
  consumes, returning a new non-parking `Retry` queue action when missing.
  Filled estimate equals the trade estimate for non-fill-or-kill takers;
  fill-or-kill reserves the dry run's exact removal count.
- **defer**: as `eager`, but non-fill-or-kill takers start with no
  filled-id capacity; the first full fill retries once after reserving.

Caller-owned reusable buffers were not prototyped: they would add public
API (a reset / reuse contract on `MatchResult`) for, at best, the savings
below.

### Allocations (2,000 reps, per op; deterministic)

| Case | trades / filled | base allocs | base bytes | defer allocs | defer bytes |
|---|---|---|---|---|---|
| zero trade | 0 / 0 | 0.00 | 0 | 0.00 | 0 |
| maker partial, deep 1,000 | 1 / 0 | 3.00 | 1,920 | 2.00 | 1,600 |
| fill-or-kill maker partial | 1 / 0 | 142.00 | 44,272 | 141.00 | 44,240 |
| single full | 1 / 1 | 2.02 | 1,800 | 2.02 | 1,799 |
| sweep 100 | 100 / 100 | 8.22 | 22,744 | 8.17 | 22,723 |
| iceberg 1x | 1 / 0 | 4.02 | 437 | 3.02 | 405 |
| iceberg 5x | 5 / 0 | 14.08 | 3,208 | 13.08 | 3,176 |
| reserve 1x | 1 / 0 | 4.02 | 437 | 3.02 | 405 |

`eager` equals `base` except fill-or-kill (141.00 / 44,240). In `base` the
filled-id vector never regrows in the 5x iceberg case (it keeps its one
spare slot while only the trade vector grows 1, 4, 8), so decoupling the
per-step check saves nothing there.

### Latency

Host load 2.4 to 5.0 during the runs (Apple silicon laptop, shared), so
every comparison is interleaved: base, eager, defer, repeated three times
(latency harness, 20,000 samples, medians of the three runs shown) and
twice (Criterion, 1 s warm-up, 3 s measurement, mean of the two point
estimates). The latency clock ticks every ~41.7 ns on this host.

| Scenario | base p50 / p99 / p99.9 (ns) | defer p50 / p99 / p99.9 (ns) |
|---|---|---|
| match_empty | 208 / 250 / 333 | 208 / 292 / 334 |
| match_full | 208 / 916 / 1,458 | 250 / 916 / 1,292 |
| match_partial (taker > maker) | 375 / 791 / 875 | 416 / 833 / 958 |
| match_maker_partial | 209 / 1,167 / 5,542 | 209 / 1,084 / 3,750 |
| many_fill_sweep (20) | 3,833 / 5,709 / 11,500 | 3,834 / 5,833 / 10,792 |
| iceberg_replenish | 209 / 750 / 1,083 | 208 / 708 / 1,041 |
| reserve_replenish | 209 / 625 / 708 | 208 / 625 / 667 |
| tif_gtc_full_match | 208 / 583 / 667 | 250 / 625 / 958 |
| tif_fok_success | 1,333 / 1,750 / 2,042 | 1,250 / 1,709 / 2,042 |

`eager` medians matched `base` within one clock tick on every row.

| Criterion | base | eager | defer |
|---|---|---|---|
| zero_trade | 190 ns | 189 ns | 196 ns |
| maker_partial_deep1000 | 198 ns | 203 ns | 191 ns |
| iceberg_1x | 223 ns | 223 ns | 220 ns |
| iceberg_5x | 1.369 µs | 1.431 µs | 1.359 µs |
| reserve_1x | 226 ns | 227 ns | 219 ns |
| single_full | 299 ns | 305 ns | 339 ns |
| sweep_100 | 17.66 µs | 17.60 µs | 17.93 µs |

### Decision: keep the current design

- **defer**: saves one allocation (32 bytes per estimated slot) on partial
  and replenish fills, about 3% faster there, but every first full fill
  pays a second locked front read: single full fill +13% in Criterion and
  +1 clock tick at p50 on the single-maker full-fill latency scenarios
  outside fill-or-kill (the 20-maker sweep and FOK success did not show it:
  3,833 → 3,834 ns and 1,333 → 1,250 ns). That moves cost
  onto the common path; rejected.
- **eager**: no measurable latency change and no allocation change except
  one of 142 allocations on fill-or-kill (the dry run's resting-order
  snapshot dominates that path). Not worth a new queue action and a retry
  path in the match loop; rejected.
- **Tighter trade estimate** (for example adding hidden quantity for
  replenishing levels): no bound on trades exists without walking the
  queue; `count + hidden` over-reserves by orders of magnitude for a large
  hidden tranche. Not pursued.
- **Caller-owned buffers**: not evaluated. Reusing retained vectors could
  avoid both result allocations on repeated fills without the first-fill
  retry, so the measurements above do not bound its gain; it was left out
  because it needs a public API and a reset contract, which is outside this
  issue's scope.

The cases stay in the allocation pass, the latency harness and Criterion as
a regression tripwire for result sizing.

## Snapshot serialization buffers (issue #149)

Question: what do the temporary snapshot buffers cost, now that the
production path serializes orders borrowed (`BorrowedOrders`, no
`Vec<&OrderType<()>>`) and streams the canonical JSON into SHA-256
(`serde_json::to_writer` into an `io::Write` adapter, no `to_vec` payload)?
Both landed with #164; #149 adds the equivalence tests, the two-pass
documentation and this measurement.

### Method

`benches/latency/scenarios/snapshot_sizes.rs` (`PL_LATENCY_ONLY=snapshot_sizes`)
runs `PriceLevel::snapshot_package()`, `PriceLevel::snapshot_to_json()` and
`PriceLevelSnapshotPackage::validate()` on levels of 100, 10,000 and 100,000
standard orders (the fixture's package JSON is 24.9 KB, 2.45 MB and
24.5 MB). Each is paired with a bench-local emulation of the buffered pre-#164
path (`legacy`: collect the reference vector, hash `serde_json::to_vec`,
encode the package with `serde_json::to_string`); the fixture asserts the
two produce byte-identical JSON and checksums before timing. The two variants
are **interleaved sample by sample**, alternating which runs first, because
the host load average was above 4 (4.5 to 9.3 across the runs). Samples per
variant: 20,000 / 500 / 50 (budget of 5 M orders serialized). The untimed
allocation pass runs 2,000 / 100 / 10 repetitions; `peak_live_bytes` is the
counting allocator's new high-water mark of live bytes (`alloc.rs`), output
retained by the call included.

Host: Apple M5 Max, macOS, rustc 1.98.1, `bench` profile, system allocator
behind the counting wrapper (counting off while timing), single thread.
Baseline: `origin/main` `a597851` with this harness copied in; after: this
branch. Production code is identical in both (the #149 diff is tests, docs
and benches), so the two runs double as a run-to-run noise check.

### Latency (after run; ns)

| Operation | Orders | legacy p50 | stream p50 | legacy p99 | stream p99 | legacy p99.9 | stream p99.9 |
|---|---:|---:|---:|---:|---:|---:|---:|
| snapshot_package | 100 | 93,083 | 90,083 | 99,833 | 96,209 | 110,875 | 108,583 |
| snapshot_package | 10,000 | 8.98 M | 8.76 M | 11.88 M | 11.60 M | 12.00 M | 11.67 M |
| snapshot_package | 100,000 | 92.1 M | 88.7 M | 96.8 M | 96.0 M | = p99 | = p99 |
| snapshot_to_json | 100 | 174,834 | 172,875 | 222,666 | 219,625 | 229,708 | 226,875 |
| snapshot_to_json | 10,000 | 17.11 M | 16.95 M | 19.25 M | 19.17 M | 24.02 M | 23.59 M |
| snapshot_to_json | 100,000 | 174.8 M | 171.6 M | 188.9 M | 185.9 M | = p99 | = p99 |
| validate | 100 | 89,166 | 86,125 | 97,000 | 94,583 | 106,250 | 103,709 |
| validate | 10,000 | 8.83 M | 8.59 M | 9.70 M | 9.65 M | 12.04 M | 11.29 M |
| validate | 100,000 | 90.0 M | 86.2 M | 97.0 M | 108.1 M | = p99 | = p99 |

The baseline run (`origin/main`) agreed within 1 to 3% at p50 (for example
validate at 10,000: 9.03 M legacy / 8.80 M stream). With 500 and 50 samples,
p99 and p99.9 are the top one to five observations and move by 10 to 60%
between runs in both variants; only p50 is comparable at 10,000 and
100,000 orders.

### Allocations (per operation; deterministic, identical on both runs)

| Operation | Orders | legacy allocs | stream allocs | legacy bytes | stream bytes | legacy peak live | stream peak live |
|---|---:|---:|---:|---:|---:|---:|---:|
| snapshot_package | 100 | 3,741 | 3,731 | 112,144 | 45,936 | 34,440 | 2,400 |
| snapshot_package | 10,000 | 360,148 | 360,131 | 12.75 M | 4.28 M | 4,354,376 | 240,000 |
| snapshot_package | 100,000 | 3,600,151 | 3,600,131 | 110.7 M | 42.8 M | 35,154,504 | 2,400,000 |
| snapshot_to_json | 100 | 7,351 | 7,340 | 218,752 | 151,744 | 34,504 | 33,704 |
| snapshot_to_json | 10,000 | 720,165 | 720,147 | 25.26 M | 16.71 M | 4,354,440 | 4,274,440 |
| snapshot_to_json | 100,000 | 7,200,171 | 7,200,150 | 219.0 M | 150.3 M | 35,154,568 | 34,354,568 |
| validate | 100 | 3,611 | 3,601 | 106,672 | 40,464 | 33,640 | 72 |
| validate | 10,000 | 360,018 | 360,001 | 12.51 M | 4.04 M | 4,274,376 | 72 |
| validate | 100,000 | 3,600,021 | 3,600,001 | 108.3 M | 40.4 M | 34,354,504 | 72 |

### Reading

- **Peak temporary memory** is where the buffers mattered. `validate` now
  peaks at 72 bytes (the hex checksum) instead of the whole payload
  (34.4 MB at 100,000 orders). `snapshot_package` peaks at 24 bytes per
  order instead of that plus the payload: the capture's temporary
  `(sequence, Arc)` pairs buffer (16 bytes per order on this host) is still
  live while it is copied into the snapshot's output `Arc` vector (8 bytes
  per order). Only the checksum stage is buffer-free; the capture buffer is
  outside #149.
  `snapshot_to_json` still peaks at roughly the output JSON, which is the
  returned value.
- **Allocated bytes** fall by about 8.5 MB per 10,000 orders on every path
  (the payload buffer and its doubling growth, plus the reference vector);
  the allocation **count** only drops by 10 to 21 because those buffers were
  a handful of large allocations.
- **Latency** improves 1 to 4% at p50, interleaved. It is dominated by
  ~36 allocations (~404 bytes) per order that remain in the streaming path:
  `Id` and `Hash32` serialize through `to_string` / `to_hex`, and `Hash32`
  hex encoding formats byte by byte. That is order-model code
  (`src/orders/`, `src/utils/`), outside #149, and the next lever for
  snapshot latency.
- **Two passes remain.** `snapshot_to_json` serializes the snapshot once
  into SHA-256 and once into the package JSON; the table shows it at
  roughly twice `snapshot_package`. The package writes `version`,
  `snapshot`, `checksum` in that order, so a single-pass encoder could
  forward the snapshot bytes to both the output and SHA-256 and append the
  checksum, keeping the package bytes unchanged; it still needs a separate
  compatibility review.

Criterion numbers were not collected for this issue: no production code
changed, and the per-operation harness above carries the percentiles and
allocation counts the issue asks for.

## Restore validation walks (issue #150)

Before #150, `PriceLevel::from_snapshot` walked the orders three times
before building the queue: `refresh_aggregates`, the duplicate-id set, and
the price / side topology check, each returning its own first error. It now
walks them twice (`PriceLevelSnapshot::into_validated_restore`): the
allocation-free checked aggregate fold, then one fused pass over ids and
topology, returning the validated parts. The documented precedence
(aggregates > `RestoreScratch` refusal > first repeated id > first topology
violation > queue > topology word) is unchanged: in the fused pass a
duplicate returns at once and a topology violation is recorded while ids
keep being checked. The persisted statistics are moved instead of cloned,
with their private seqlock sequence restarted at 0 exactly as the clone did
(so the #165 rebuild recovery still works).

A first version fused all three checks into one walk, deferring lower-ranked
failures. It kept the precedence but reserved the duplicate-id set before the
aggregate fold had finished, so an aggregate rejection allocated a
depth-sized set that the old path never allocated (4.3 MB at 100,000 orders
for an overflow at the second order). Running the aggregate fold first
removes that cost; the fold reads two fields per order and costs about 0.3 ms
per 100,000 orders (`from_snapshot_total_last` below).

`src/price_level/tests/restore_validation.rs` compares the new path with the
old one (kept as `cfg(test)` `from_snapshot_legacy`). It covers every pair
and triple of violations at every position, 20,000 random snapshots, the
scratch-set refusal seam, aggregate failures at the first, second and last
order (which must not reach the reservation), fixtures, every order type,
and statistics moved in with an exhausted sequence, restored without
`try_clone`.

Orders were already decoded straight into the final `Vec<Arc<OrderType<()>>>`
(`OrdersSeed`, fallible, capped at 1 MiB of up-front reservation) since
#164; the allocation counts below confirm there is no intermediate vector on
either revision.

### Method

`PL_LATENCY_ONLY=restore_sizes PL_LATENCY_SAMPLES=2000`: 2,000 samples at
100 orders, 500 at 10,000, 50 at 100,000 (the p99.9 of the two larger sizes
is the sample maximum). `from_snapshot` receives a `try_clone` of the input
made before the clock starts; dropping a rejected input (one reference count
per order) is inside the clock on both revisions. The same harness file
(public API only) was built on `origin/main` (`93832f4`) and on this branch,
and the two binaries were run alternately, four rounds, swapping which ran
first; the tables show the median of the four runs per percentile. Apple M5
Max (18 cores), rustc 1.98.1, `bench` profile, system allocator behind the
counting wrapper (counting disabled while timing), single-threaded, no
contention. The host was shared: load average 4.7 to 11.4 during the runs.

Cases: `valid`; `dup_last` (the last order repeats the previous id);
`price_last` (the last order at another price); `total_second` /
`total_last` (an order at `u64::MAX` makes the visible sum overflow at the
second / last order); `from_json_*` run `from_snapshot_json` end to end,
`dup_last` on a correctly signed package.

### Latency (µs; base = `origin/main`, new = this branch)

| Operation | n | base p50 / p99 / p99.9 | new p50 / p99 / p99.9 | p50 |
|---|---|---|---|---|
| from_snapshot, valid | 100 | 7.4 / 8.4 / 12.2 | 7.5 / 8.3 / 11.0 | +0.3% |
| from_snapshot, dup_last | 100 | 1.4 / 1.6 / 1.8 | 1.4 / 1.6 / 1.8 | -1.4% |
| from_snapshot, price_last | 100 | 1.5 / 1.8 / 1.9 | 1.4 / 1.7 / 1.8 | -5.6% |
| from_snapshot, total_second | 100 | 0.1 / 0.1 / 0.1 | 0.1 / 0.1 / 0.2 | +1.2% |
| from_snapshot, total_last | 100 | 0.1 / 0.2 / 0.2 | 0.2 / 0.2 / 0.3 | +33.6% |
| from_snapshot_json, valid | 100 | 123.4 / 135.6 / 147.1 | 124.1 / 135.4 / 146.7 | +0.6% |
| from_snapshot_json, dup_last | 100 | 117.1 / 124.6 / 135.2 | 117.5 / 137.7 / 165.6 | +0.4% |
| from_snapshot, valid | 10,000 | 997.1 / 1,052.1 / 1,095.6 | 974.0 / 1,044.1 / 1,087.8 | -2.3% |
| from_snapshot, dup_last | 10,000 | 128.2 / 146.2 / 156.1 | 135.4 / 143.9 / 163.5 | +5.7% |
| from_snapshot, price_last | 10,000 | 140.0 / 156.1 / 170.2 | 133.0 / 145.2 / 157.7 | -5.0% |
| from_snapshot, total_second | 10,000 | 13.0 / 13.7 / 17.1 | 12.9 / 14.6 / 25.1 | -1.1% |
| from_snapshot, total_last | 10,000 | 19.3 / 22.1 / 26.4 | 20.2 / 21.5 / 34.4 | +4.6% |
| from_snapshot_json, valid | 10,000 | 12,496.2 / 13,881.5 / 14,917.5 | 12,501.6 / 14,758.6 / 15,507.0 | +0.0% |
| from_snapshot_json, dup_last | 10,000 | 11,815.7 / 13,891.6 / 14,580.7 | 11,711.7 / 13,356.1 / 13,578.0 | -0.9% |
| from_snapshot, valid | 100,000 | 12,602.3 / 13,635.1 / 13,635.1 | 11,946.3 / 13,878.2 / 13,878.2 | -5.2% |
| from_snapshot, dup_last | 100,000 | 1,645.2 / 1,796.1 / 1,796.1 | 1,625.9 / 1,717.8 / 1,717.8 | -1.2% |
| from_snapshot, price_last | 100,000 | 1,736.6 / 1,837.9 / 1,837.9 | 1,617.9 / 1,739.2 / 1,739.2 | -6.8% |
| from_snapshot, total_second | 100,000 | 148.1 / 181.8 / 181.8 | 147.0 / 156.7 / 156.7 | -0.8% |
| from_snapshot, total_last | 100,000 | 290.2 / 330.0 / 330.0 | 267.8 / 302.2 / 302.2 | -7.7% |
| from_snapshot_json, valid | 100,000 | 128,955.7 / 139,095.5 / 139,095.5 | 128,213.9 / 145,425.1 / 145,425.1 | -0.6% |
| from_snapshot_json, dup_last | 100,000 | 118,871.6 / 136,812.3 / 136,812.3 | 118,060.2 / 131,119.4 / 131,119.4 | -0.7% |

The 100-order `total_last` row is 0.1 vs 0.2 µs, one to three clock ticks
(~41.7 ns each), not a measurable change.

### Allocations (per operation)

Peak = each operation's own high-water mark of live counted bytes, rebased
to zero immediately before the operation: bytes allocated beyond what
already existed (the pre-existing input snapshot or JSON string is
excluded; input buffers the restore frees lower the live count), including
the restored level while alive. Shown as the median and maximum over the
repetitions (50, 50 and 10 at the three sizes), medians of the four runs.

| Operation | n | base allocs / bytes / peak median / peak max | new allocs / bytes / peak median / peak max |
|---|---|---|---|
| from_snapshot, valid | 100 | 174 / 42,356 / 37,902 / 39,342 | 173 / 42,229 / 37,694 / 39,036 |
| from_snapshot, dup_last | 100 | 2 / 4,268 / 4,268 / 4,268 | 2 / 4,268 / 4,268 / 4,268 |
| from_snapshot, price_last | 100 | 2 / 4,330 / 4,232 / 4,232 | 2 / 4,330 / 4,330 / 4,330 |
| from_snapshot, total_second / total_last | 100 | 1 / 34 / 34 / 34 | 1 / 34 / 34 / 34 |
| from_snapshot_json, valid | 100 | 4,184 / 111,682 / 54,820 / 55,950 | 4,184 / 111,675 / 54,760 / 56,256 |
| from_snapshot_json, dup_last | 100 | 4,012 / 73,628 / 21,292 / 21,292 | 4,012 / 73,628 / 21,292 / 21,292 |
| from_snapshot, valid | 10,000 | 10,770 / 2,832,733 / 1,512,736 / 1,519,008 | 10,770 / 2,832,231 / 1,511,336 / 1,517,348 |
| from_snapshot, dup_last | 10,000 | 2 / 540,716 / 540,716 / 540,716 | 2 / 540,716 / 540,716 / 540,716 |
| from_snapshot, price_last | 10,000 | 2 / 540,778 / 540,680 / 540,680 | 2 / 540,778 / 540,778 / 540,778 |
| from_snapshot, total_second / total_last | 10,000 | 1 / 34 / 34 / 34 | 1 / 34 / 34 / 34 |
| from_snapshot_json, valid | 10,000 | 410,787 / 9,814,550 / 3,242,240 / 3,246,852 | 410,787 / 9,814,613 / 3,242,240 / 3,248,068 |
| from_snapshot_json, dup_last | 10,000 | 400,019 / 7,522,972 / 2,271,788 / 2,271,788 | 400,019 / 7,522,972 / 2,271,788 / 2,271,788 |
| from_snapshot, valid | 100,000 | 101,155 / 24,106,952 / 13,375,824 / 13,375,824 | 101,155 / 24,106,952 / 13,375,824 / 13,375,824 |
| from_snapshot, dup_last | 100,000 | 2 / 4,325,420 / 4,325,420 / 4,325,420 | 2 / 4,325,420 / 4,325,420 / 4,325,420 |
| from_snapshot, price_last | 100,000 | 2 / 4,325,482 / 4,325,384 / 4,325,384 | 2 / 4,325,482 / 4,325,482 / 4,325,482 |
| from_snapshot, total_second / total_last | 100,000 | 1 / 34 / 34 / 34 | 1 / 34 / 34 / 34 |
| from_snapshot_json, valid | 100,000 | 4,101,175 / 93,404,216 / 30,424,400 / 30,424,400 | 4,101,175 / 93,404,216 / 30,424,400 / 30,424,400 |
| from_snapshot_json, dup_last | 100,000 | 4,000,022 / 73,622,684 / 21,373,996 / 21,373,996 | 4,000,022 / 73,622,684 / 21,373,996 / 21,373,996 |

Allocation counts and bytes match to within 0.02% in every case, early
aggregate failures included (one allocation: the error message). The
`price_last` peaks differ by 98 bytes (the order of freeing the id set and
formatting the error message).

### Reading

- Validation is a small share of restore. At 100,000 orders the rejected
  snapshots (validation only, no queue) cost 1.6 ms, the valid restore
  12 to 13 ms (mostly the queue build: one `DashMap` entry and one `SkipMap`
  node per order), and the JSON restore about 120 to 130 ms (decoding and
  the checksum re-encoding, about 40 allocations per order, outside this
  issue). The change is within noise on the JSON path.
- A topology violation at the end is rejected 5 to 7% faster at 10,000 /
  100,000 orders (one walk fewer). A duplicate at the end is unchanged
  within noise (-1.2% / +5.7%): the hash-set inserts dominate and that walk
  was already the last one before topology.
- The valid `from_snapshot` p50 moved -2% / -5% at 10,000 / 100,000 and
  +0.3% at 100; the four 100,000-order runs span 11.3 to 14.3 ms (base) and
  11.8 to 14.5 ms (new) on this shared host, so this is not a demonstrated
  speedup.

### Not pursued

Detecting duplicate ids with the queue's own rejecting insert (`try_push`)
instead of the scratch set would save roughly the set's cost (the
`dup_last` row: about 13% of a valid 100,000-order `from_snapshot`, about
1% of the JSON restore) but not peak memory (the set is freed before the
queue grows). It would also move the duplicate check after partial queue
construction and make the precedence against the queue's own failures
depend on insertion progress. Not done here; the precedence contract above
would have to be restated and retested first.
## Residual allocation reuse (issue #147)

A partial fill (`FrontAction::KeepInPlace`) or an iceberg / reserve
replenishment (`FrontAction::ReplaceAtTail`) commits a new `Arc` for the
maker: the decision closure calls `Arc::new(updated)` under the maker's
`DashMap` shard write lock, and the evicted `Arc` is dropped after the lock
(#128, #144). This section records the evaluation of reusing that
allocation when it is provably unshared.

### Ownership and commit design evaluated

- The decision returns the residual / refreshed order **by value**
  (`OrderType<()>` is `Copy`, 144 bytes on the measurement host).
- The commit, still under the entry lock and after every stop-cause check
  (#169, #163, #168, #165, #124; all of them return `SetAside` before any
  action is built, so none of them reaches the commit), calls
  `Arc::get_mut` on the stored `Arc`. Success means strong count 1 and no
  weak reference: no handle exists outside the map, and the held shard write
  lock stops anyone from cloning a new one, so the value is overwritten in
  place (no allocation, no deallocation, no drop code). Otherwise it
  allocates a fresh `Arc` exactly as today and the old one keeps its value
  for its holders.
- Nothing moves outside the lock: the fallback allocation stays in the same
  critical section as today's, the cancel / update decisions still
  serialize on the entry lock, and the replenish path keeps its sequence
  reservation, the in-closure counter RMW and the tail re-keying unchanged.
  No `unsafe`, no dependency.

Who shares the stored `Arc`: the handle returned by `add_order` (only until
the first fill, which stores a new allocation), and any live
`iter_orders` / `snapshot_orders` / `snapshot` / checksum package view. A
retained admission handle therefore costs one fallback per maker, not one
per fill.

### Allocations (2,000 reps, per op; deterministic)

`single_partial*`: one 1e12-unit standard maker alone on the level, 10-unit
takers, one trade and no filled id per call. `_adm` keeps the admission
handle for the whole run; `_view` takes an `iter_orders().next()` handle
before every fill and drops it after (outside the counted window).

| Case | base allocs / bytes / deallocs | reuse allocs / bytes / deallocs |
|---|---|---|
| single_partial | 3.00 / 336 / 1.00 | 2.00 / 176 / 0.00 |
| single_partial_adm | 3.00 / 336 / 1.00 | 2.00 / 176 / 0.00 (1 fallback in 2,000) |
| single_partial_view | 3.00 / 336 / 0 (view holder frees) | 3.00 / 336 / 0 |
| match_maker_partial (deep 1,000) | 3.00 / 1,920 / 1.00 | 2.00 / 1,760 / 0.00 |
| match_iceberg_1x | 4.02 / 437 | 3.02 / 277 |
| match_iceberg_5x | 14.08 / 3,208 | 9.08 / 2,408 |
| match_reserve_1x | 4.02 / 437 | 3.02 / 277 |
| match_fok_partial | 134.00 / 27,568 | 133.00 / 27,408 |

The remaining two allocations are the result's trade and filled-id vectors
(#148).

`get_mut` success rate, measured with a `#[cfg(test)]` commit counter in the
prototype, release test build, 2,000 one-unit fills of one maker while
reader threads loop `iter_orders().next()` and hold each handle for 16
spins (an adversarial, always-reading flow): 1 reader 66% / 84% / 90%,
2 readers 82% / 85% / 95%, 4 readers 92% / 88% / 91% (three runs each).
With no concurrent reader it is 100%, or 100% minus the first fill per
maker when the admission handle is kept.

### Latency

Apple M5 Max (18 logical cores), macOS 27.0, rustc 1.98.1, release bench
profile, system allocator (the latency harness's counting wrapper is
disabled while timing). The host was shared: load average 4.9 to 24 during
the runs. Every comparison is interleaved, base then reuse, five rounds.

Criterion (`Residual allocation reuse (#147)` and three `#148` cases;
1 s warm-up, 5 s measurement; median of the five point estimates):

| Case | base | reuse | change | reuse faster |
|---|---|---|---|---|
| partial_unique | 180.4 ns | 178.9 ns | -0.8% | 3/5 |
| partial_retained_view * | 189.9 ns | 193.6 ns | +1.9% | 2/5 |
| replenish_unique | 243.4 ns | 244.4 ns | +0.4% | 2/5 |
| replenish_retained_view * | 245.9 ns | 251.6 ns | +2.3% | 0/5 |
| maker_partial_deep1000 | 235.0 ns | 248.1 ns | +5.6% | 1/5 |
| iceberg_1x | 241.6 ns | 243.6 ns | +0.8% | 2/5 |
| reserve_1x | 242.2 ns | 244.3 ns | +0.9% | 2/5 |

\* The retained-view rows use `BatchSize::PerIteration`: the view is taken
in untimed setup immediately before each fill, so every fill sees a shared
maker. An earlier `SmallInput` version prepared a whole batch of views
before the batch ran; they all referenced the maker as it was before the
batch, the first fill replaced that allocation, and the rest of the batch
ran the unique path. These two rows come from a separate five-round
interleaved rerun (load average 3.8 to 8.8) on the rebased branch.

Latency harness (`PL_LATENCY_ONLY=match`, 20,000 samples, median of five
runs; the clock ticks every ~41.7 ns):

| Scenario | base p50 / p99 / p99.9 (ns) | reuse p50 / p99 / p99.9 (ns) |
|---|---|---|
| single_partial | 208 / 250 / 333 | 208 / 250 / 333 |
| single_partial_adm | 208 / 292 / 417 | 208 / 250 / 292 |
| single_partial_view | 208 / 291 / 375 | 209 / 292 / 458 |
| match_maker_partial | 250 / 1,375 / 6,875 | 291 / 1,541 / 8,959 |
| iceberg_replenish | 250 / 875 / 1,875 | 250 / 959 / 1,500 |
| reserve_replenish | 250 / 667 / 750 | 250 / 667 / 750 |
| match_full | 250 / 1,041 / 1,667 | 250 / 1,000 / 1,625 |

### Decision: keep the current commit

The reuse path removes one allocation and one deallocation per unique-owner
fill, but that doesn't show up as latency on this host. The Criterion means
move by -0.8% to +5.6% (the view rows +1.9% and +2.3%), reuse is faster in at most 3 of 5 rounds, and the
one consistent difference (`maker_partial_deep1000`, 4 of 5 rounds slower)
goes the wrong way. The harness p50s are identical to the clock tick, and
the p99 / p99.9 changes go both ways within run-to-run noise. The
`get_mut` check (a weak-count compare-exchange, a strong-count load and a
release store) and the 144-byte by-value action cost about what the
system allocator's fast path saves (a 160-byte `ArcInner` alloc and free).
Without a measured latency benefit the change fails this issue's adoption
bar, so it is not adopted.

Revisit if an allocator with a slower small-object path or allocator
contention between the matcher and other threads makes the in-lock
`Arc::new` visible, or if the result vectors (#148) stop allocating so the
residual becomes the dominant per-fill allocation. The design above is the
one to reuse. The ownership contract it must keep is pinned by
`src/price_level/tests/residual_reuse.rs`: retained admission handles,
`snapshot_orders` / `snapshot` views and checksum packages never change
after a fill or replenishment, replenishment still demotes to the tail, and
a racing cancel either fully wins or fully loses. The scenarios stay in the
allocation pass, the latency harness and Criterion
(`benches/price_level/residual_reuse.rs`) as a regression tripwire.

## Fill-or-kill feasibility depth (issue #143)

A fill-or-kill taker runs a dry run of the sweep under the level's
exclusive guard before it touches a maker. The dry run used to materialize
and sort the whole queue first, so a qty-1 FOK against a 10,000-order level
cost about as much as walking all 10,000 makers, and every admission and
cancel on the level waited behind it.

The dry run now walks the queue in sweep order and stops as soon as the
taker is covered (or the sweep would stop). The walk has two phases
(`SeqWalk` in `order_queue.rs`):

- **Lazy prefix**, at most `max(8, resting orders / 64)` makers: index entry,
  one `DashMap` lookup and one `Arc` clone per maker. A fill within the
  prefix never touches the makers behind it.
- **Bulk continuation**: a walk that outlives the prefix collects the
  remaining orders (sequence above the last one visited) in one pass and
  sorts them, as the former snapshot did.

The split exists because a lazy step costs more than its share of one bulk
pass. Measured per maker on a 10,000-order level in a release test: index
step about 8 ns, `DashMap` lookup about 13.5 ns, `Arc` clone about 5.5 ns
(about 27 ns in total), against about 14 ns per maker for the old
collect-and-sort. A lazy-only walk made the rejected FOK below about 2x
slower; the budget caps that overhead to the prefix.

Replenished tranches that the sweep re-sequences at the tail are buffered
by value, and only when the taker still has quantity left.

The lazy phase is only used under the fill-or-kill guard. A re-sequencing
(a GTC replenishment, a demoting resize) inserts the maker's new index key
before removing the old one, so an unguarded index walk can meet the same
maker twice. The public `matchable_quantity` takes no guard, so it always
starts in the bulk phase (one entry per maker from the id-keyed map, as
the former snapshot did) and keeps its former `O(depth log depth)` cost.

The prediction is unchanged: a property test (`src/price_level/tests/bounded_fok.rs`)
compares every field of the new dry run (fill, trades, replenishes, parks,
stop error) with the former implementation, kept only in that test. It uses
random books of standard, iceberg and reserve makers, resizes, cancels,
partial GTC sweeps, self-match taker ids, quantities near `u64::MAX` for the
visible-headroom abort, and makers whose step fails, with the lazy budget
forced to every small value so the bulk switch lands at every position.

### Workload

The issue's reproduction (`PL_LATENCY_ONLY=fok_depth`,
`benches/latency/scenarios/fok_depth.rs`): a fresh level per configuration,
standard GTC Sell makers at price 100 and timestamp 1, maker quantity 1,
Sequential maker ids `0..depth`, taker id `u64::MAX` at timestamp 2,
`TakerKind::Standard`, a trade-id generator with namespace `Uuid::nil()`.
`*_first_maker` keeps the depth constant by admitting one replacement maker
in an untimed teardown. 1,000 warmup calls, then 5,000 FOK samples or
10,000 GTC samples. Each call is asserted to execute quantity 1, and
statistics are asserted not degraded.

- `fok_rejected`: a taker one unit larger than the level; the dry run must
  visit every maker to prove the kill.
- `fok_replenish`: iceberg makers (1 visible + 1,000,000 hidden); a qty-2
  FOK drains the front tranche (re-sequenced at the tail) and one unit of
  the next maker.
- `writer_*_during_*`: one thread loops the qty-1 first-maker call (FOK,
  or GTC as the control) while this thread times 5,000 `add_order` +
  `update_order(Cancel)` pairs on its own tail orders. Since #206 the cases
  run at depths 100 and 10,000, and a `fok_rejected` matcher mode loops a
  FOK one unit larger than the level plus a writer order: a chosen long
  dry-run workload, not an upper bound for every FOK (a successful FOK that
  consumes a deep level runs the dry run and the sweep, and replenishing
  makers add steps). It consumes nothing, so its writer anomalies are
  asserted to be zero. Starvation anomalies are counted, not
  asserted; see "Writer starvation behind a looping FOK matcher" below.

### Environment

Apple M5 Max (18 logical cores), macOS arm64, Rust 1.98.1 release profile,
system allocator, unpinned shared host. Load averages were 5.9 to 9.1
during the runs, so base (`origin/main` 2afb06a) and new binaries were run
interleaved, three rounds each. The table reports the median of the three
rounds. Times are microseconds.

| Scenario | Base p50 | Base p99 | Base p99.9 | New p50 | New p99 | New p99.9 |
|---|---|---|---|---|---|---|
| fok_first_maker@1 | 1.58 | 2.29 | 3.08 | 0.25 | 0.96 | 1.33 |
| gtc_first_maker@1 | 0.21 | 0.79 | 1.17 | 0.21 | 0.75 | 1.17 |
| fok_rejected@1 | 1.38 | 1.50 | 1.58 | 0.08 | 0.08 | 0.17 |
| fok_first_maker@100 | 2.75 | 3.33 | 4.50 | 0.29 | 0.83 | 0.96 |
| gtc_first_maker@100 | 0.21 | 0.71 | 0.83 | 0.21 | 0.71 | 0.79 |
| fok_rejected@100 | 2.67 | 2.83 | 3.42 | 2.92 | 3.04 | 3.21 |
| fok_replenish@100 | 2.79 | 3.96 | 4.62 | 0.62 | 1.58 | 2.08 |
| fok_first_maker@10000 | 157 | 307 | 717 | 0.29 | 1.00 | 2.33 |
| gtc_first_maker@10000 | 0.25 | 0.79 | 2.04 | 0.29 | 1.21 | 7.12 |
| fok_rejected@10000 | 173 | 217 | 585 | 205 | 581 | 1,788 |
| fok_replenish@10000 | 156 | 171 | 242 | 0.67 | 1.75 | 2.67 |
| writer_add_during_fok@10000 | 0.17 | 0.67 | 144,505 | 0.21 | 4.75 | 15.04 |
| writer_cancel_during_fok@10000 | 0.08 | 0.71 | 74,739 | 0.12 | 4.46 | 10.00 |
| writer_add_during_gtc@10000 | 0.21 | 1.04 | 2.00 | 0.46 | 1.08 | 2.42 |
| writer_cancel_during_gtc@10000 | 0.12 | 0.96 | 2.67 | 0.42 | 1.33 | 2.83 |

Criterion (`PriceLevel - FOK depth`, `benches/price_level/fok_depth.rs`,
same workload; two interleaved rounds, point estimates; load averages 2.8
to 3.6; base is `origin/main` c4a1ab1). Every timed call starts at exactly
the stated depth (asserted in the untimed setup; the first-maker cases seed
`depth - 1` makers and admit one per iteration), and the routine returns
the `MatchResult`, which `iter_batched` drops after the measured batch, so
result destruction is not timed. An earlier revision of this table timed
calls at `depth + 1` and included the result's drop.

| Case | Base | New |
|---|---|---|
| fok_first_maker/1 | 1.48 / 1.47 µs | 257 / 253 ns |
| gtc_first_maker/1 | 207 / 208 ns | 207 / 206 ns |
| fok_rejected/1 | 1.29 / 1.28 µs | 66 / 66 ns |
| fok_first_maker/100 | 2.61 / 2.62 µs | 260 / 273 ns |
| gtc_first_maker/100 | 212 / 212 ns | 210 / 210 ns |
| fok_rejected/100 | 2.55 / 2.54 µs | 2.68 / 2.70 µs |
| fok_replenish/100 | 2.74 / 2.70 µs | 599 / 593 ns |
| fok_first_maker/10000 | 144.7 / 146.1 µs | 280 / 277 ns |
| gtc_first_maker/10000 | 227 / 231 ns | 229 / 228 ns |
| fok_rejected/10000 | 163.3 / 164.6 µs | 169.8 / 169.5 µs |
| fok_replenish/10000 | 146.9 / 148.4 µs | 642 / 640 ns |

### Reading

- A FOK filled by the front maker no longer depends on depth: at 10,000
  orders its p50 drops from 157 µs to 0.29 µs, within about 40 ns of the GTC
  control. The same holds with replenishing makers.
- Mutators no longer stall behind the FOK: with a FOK matcher looping at
  depth 10,000, the writer's p99.9 add / cancel latency goes from 145 / 75 ms
  (queued behind back-to-back 150 µs exclusive sections) to 15 / 10 µs. Its
  p99 rises from under 1 µs to about 4.5 µs because the matcher now
  completes about 350,000 to 480,000 FOK calls per second instead of about
  6,300, so the writer meets a short exclusive section far more often.
- **Regression: a FOK that must walk the whole level** (the rejected case)
  is slower by the lazy prefix plus per-step overhead: Criterion +3 to +4%
  at depth 10,000 and +5 to +6% at depth 100; in the latency harness +18%
  at p50 at depth 10,000, with a higher p99 under this host's load. An
  interleaved release micro-benchmark of the two dry runs alone measured
  +3 to +5% at depth 10,000. The lazy-only walk (no bulk switch) measured
  about +100%, so this split is kept; a smaller prefix would trade bounded
  fills for a few microseconds on this path.
- `gtc_first_maker` is unchanged within noise: the GTC path does not run the
  dry run.

### Writer starvation behind a looping FOK matcher (issue #206)

The `writers_during_fok` case also exposes a fairness problem that exists on
`origin/main` independently of this change. Admissions and cancels take the
fill-or-kill guard's shared side for their whole queue mutation, and a FOK
holds the exclusive side across its dry run and sweep, so no mutation can
land inside a FOK. But `std::sync::RwLock` gives the shared side no fairness
against a thread that retakes the exclusive side in a loop. With the former
full-depth dry run (about 150 µs per FOK at depth 10,000) the writer's
p99.9 add latency was 144,505 µs, and single waits reached about 1.5 s,
roughly depth × FOK time.

While the writer waits to cancel its own order `W`, the matcher consumes
every maker ahead of `W`. `W` becomes the true front and is filled, and the
late cancel returns `Ok(None)`. This is correct FIFO under starvation, not a
FIFO violation. An earlier revision of this scenario asserted that a writer
order is never filled, and that assertion failed intermittently for this
reason.

The scenario now counts these events and reports them in the outcome note
(and so in `manifest.json`):

- `writer-owned consumed`: a matcher call filled a writer order;
- `cancel found nothing`: a writer cancel returned `Ok(None)`;
- `matcher out of order`: the matcher filled one of its own makers out of id
  (admission) order.

The public API exposes no insertion sequence, so every writer order records
an admission bracket instead: the number of matcher replacement adds that
had completed before its `add_order` started (certainly older makers) and
the number that had started by the time it returned (every later one is
certainly younger). Each consumed `W` is classified against the matcher's
own front at that call. It is `proven front` when every possibly older
matcher maker was already consumed, `proven violation` when a certainly
older one still rested, and `ambiguous` otherwise. `PL_LATENCY_STRICT_FIFO=1`
turns any anomaly into a hard failure that prints every classified event.
The strict mode is off by default, so an unfair scheduler cannot fail
`cargo test --all-targets`, which runs this harness in debug.

On the base engine, four counting runs reported 19, 0, 0 and 10 writer
orders consumed. Every one was `proven front` with no violation and no
out-of-order fill, and each was matched by a `cancel found nothing`. A
strict run failed with, for example, `W 1000000000486 front 77045 bracket
[67044, 67045] proven front`: `W` was admitted after replacement 67,044
completed and before 67,045 started, so the oldest matcher maker still
resting (id 10,000 + 67,045) is the first one younger than `W`.

The bounded dry run shrinks the exclusive section from about 150 µs to about
0.3 µs. It therefore cuts the starvation window drastically: writer add
p99.9 falls from 144,505 µs to 15 µs in the table above. It does not fix the
lack of fairness itself; a FOK that must visit every maker still holds the
guard for about 200 µs at depth 10,000. The hand-off below addresses that.

### Bounded hand-off to waiting mutators (issue #206)

The measured workload is a looping rejected FOK: the dry run walks every
maker for about 170 µs at depth 10,000 and then kills the taker, so the
matcher releases the guard only to retake it at once. It was chosen as a
long, repeatable dry-run section that leaves the level unchanged; it is not
the worst case for every FOK workload. A successful FOK that consumes a
deep level runs the dry run and then the sweep, and replenishing makers add
steps, so its section, and a blocked writer's wait, can be longer than the
tails below. Before this change a writer blocked behind the rejected loop
could wait for thousands of consecutive sections.

The guard (`src/price_level/fok_guard.rs`) now pairs the `RwLock` with a
waiting-mutator counter. A mutator tries the shared side first; only when
that would block does it increment the counter, block in `read()` and
decrement once it holds the shared side. A FOK that sees a non-zero
counter before it requests the exclusive side waits, holding no lock, for
up to 64 `spin_loop` hints and then 256 `yield_now` calls, until the counter
is zero, and then calls `write()` regardless. The lock is free during that
wait, so each announced mutator only needs to be scheduled.

This is a bounded number of hand-off attempts, and what follows are
measured latency improvements, not guarantees. The hand-off does not
establish starvation freedom for either side: after the budget the FOK
calls `write()` even if a mutator is still announced, and Rust leaves
`RwLock` acquisition priority unspecified, so a reader-preferring lock could
keep the FOK waiting and a barging one can still delay a mutator. Total
lock-acquisition delay remains scheduler-dependent and unbounded. The
budget counts rounds, not time: on an oversubscribed host each `yield_now`
can cost a scheduler slice, so one hand-off can take hundreds of
milliseconds. Typical case only (one matcher per level as supported and as
measured here, a writer-preferring or queue-fair lock, a mutator scheduled
within the budget): a blocked mutator waits for at most two sections plus
its wake-up, the section in progress and one more if the matcher rechecks
the counter between the mutator's failed `try_read` and its announcement.
With `k` concurrent FOK matchers on a level (unsupported), a queued matcher
holds readers off on writer-preferring locks such as the Linux futex
`RwLock`, and the typical wait grows to about `k` sections.

Environment: Apple M5 Max (18 logical cores), macOS arm64, Rust 1.98.1,
`bench` profile, system allocator, unpinned shared host, load averages 2.7
to 4.9. Base is `origin/main` 3c95df5 built with the extended harness.
`PL_LATENCY_ONLY=fok_depth PL_LATENCY_SAMPLES=5000 PL_LATENCY_WARMUP=1000
PL_LATENCY_CONTENTION_OPS=5000`. New is the median of three rounds. Base is
one round: its rejected case at depth 10,000 ran for about 30 minutes
(10.4 million matcher calls while the writer waited), so it was not
repeated; an earlier base run on the same host measured writer add p99
1,224,225 µs and max 9,328,124 µs for that case, the same order. Times are
microseconds.

| Scenario | Base p50 | Base p99 | Base p99.9 | Base max | New p50 | New p99 | New p99.9 | New max |
|---|---|---|---|---|---|---|---|---|
| writer_add_during_fok@100 | 0.17 | 4.04 | 6.92 | 17.5 | 0.12 | 1.62 | 5.79 | 8.12 |
| writer_cancel_during_fok@100 | 0.08 | 4.04 | 8.12 | 11.3 | 0.08 | 1.58 | 4.75 | 9.17 |
| writer_add_during_fok_rejected@100 | 0.17 | 1,371 | 8,578 | 12,018 | 0.12 | 11.6 | 14.0 | 15.8 |
| writer_cancel_during_fok_rejected@100 | 0.08 | 1,331 | 7,158 | 17,634 | 0.08 | 11.9 | 14.4 | 19.0 |
| writer_add_during_gtc@100 | 0.46 | 0.83 | 2.33 | 2.88 | 0.46 | 0.79 | 2.21 | 3.54 |
| writer_cancel_during_gtc@100 | 0.42 | 1.00 | 2.54 | 2.96 | 0.42 | 1.04 | 2.54 | 3.29 |
| writer_add_during_fok@10000 | 0.17 | 1.46 | 5.08 | 10.0 | 0.17 | 1.54 | 4.62 | 6.96 |
| writer_cancel_during_fok@10000 | 0.12 | 1.83 | 7.00 | 12.7 | 0.12 | 1.54 | 4.96 | 6.58 |
| writer_add_during_fok_rejected@10000 | 0.21 | 4,532,026 | 8,938,396 | 12,713,064 | 0.12 | 187 | 213 | 240 |
| writer_cancel_during_fok_rejected@10000 | 0.12 | 4,644,363 | 10,902,868 | 14,513,848 | 0.12 | 195 | 215 | 269 |
| writer_add_during_gtc@10000 | 0.46 | 0.83 | 2.21 | 3.67 | 0.46 | 0.83 | 1.92 | 5.71 |
| writer_cancel_during_gtc@10000 | 0.42 | 1.08 | 2.83 | 6.46 | 0.42 | 1.12 | 2.21 | 3.75 |

No run, base or new, reported a writer order consumed, a cancel that found
nothing or an out-of-order fill.

Uncontended cost, Criterion (`PriceLevel - FOK depth`, `Add Orders`,
`Update Orders`), base and new interleaved, two rounds, point estimates:

| Case | Base | New |
|---|---|---|
| fok_first_maker/1 | 252 / 252 ns | 250 / 249 ns |
| fok_rejected/1 | 65.4 / 65.7 ns | 66.8 / 66.2 ns |
| fok_first_maker/100 | 256 / 256 ns | 255 / 258 ns |
| fok_rejected/100 | 2.66 / 2.66 µs | 2.66 / 2.67 µs |
| fok_replenish/100 | 600 / 601 ns | 587 / 583 ns |
| fok_first_maker/10000 | 278 / 280 ns | 276 / 277 ns |
| fok_rejected/10000 | 168.7 / 169.1 µs | 168.0 / 166.8 µs |
| fok_replenish/10000 | 638 / 641 ns | 631 / 628 ns |
| add_standard_order (level + 100 adds) | 8.84 / 8.80 µs | 9.26 / 9.28 µs |
| order_count_scaling/10 | 1.24 / 1.24 µs | 1.29 / 1.28 µs |
| order_count_scaling/100 | 8.74 / 8.83 µs | 9.09 / 9.24 µs |
| order_count_scaling/1000 | 97.0 / 97.9 µs | 97.6 / 97.8 µs |
| cancel_order | 12.18 / 12.22 µs | 12.34 / 12.28 µs |
| update_quantity | 14.49 / 14.47 µs | 14.46 / 15.01 µs |
| cancel_order_count_scaling/1000 | 117.9 / 117.6 µs | 117.1 / 116.6 µs |

### Reading

- A writer behind a looping rejected FOK at depth 10,000 now waits p99
  187 to 195 µs, about one exclusive section, and its worst single wait in
  three rounds was 338 µs, within the typical two-section wait, instead of
  p99 4.5 s and max 14.5 s. These are this workload's tails on this host,
  not a bound for other FOK workloads or other schedulers. At depth 100 the p99 drops from 1.4 ms to 12 µs. The
  writer's wait is still measured in sections, so it scales with the FOK's
  walk: the hand-off bounds the number of sections, not their length.
- The matcher gives way only while a mutator is blocked. In the contended
  rejected case it ran about 5,000 to 65,000 calls per second during the
  writer's short window, against 5,800 to 285,000 on base, where it never
  yielded and the writer's window lasted minutes.
- Uncontended FOK is unchanged within noise at every depth: the extra work
  is one counter load before `write()`.
- **Regression, cause unresolved:** uncontended admission is 3 to 5%
  slower in `add_standard_order` and `order_count_scaling` at 10 / 100 adds
  (for example 8.8 to 9.3 µs for a level plus 100 adds), while
  `order_count_scaling/1000` and every `Update Orders` case are unchanged
  within noise. Mutators now call `try_read` and branch to an out-of-line
  slow path instead of calling `read()`. Two isolated A/B builds on the same
  host: one that restored a plain `read()` on the mutator path measured
  like base, and one that only moved the poison branch out of line did not
  recover the difference. So the `try_read` fast path is implicated, but
  why it costs this much only in the small-level benches is not known;
  code layout is a hypothesis, not a finding.
- The first-maker FOK and GTC cases are unchanged: their sections are so
  short that a woken writer usually wins the lock on its own.
- Remaining caveats: a mutator that cannot run for the whole budget (for
  example, it is preempted while the host is oversubscribed) lets that
  section proceed and waits for another one, and on such a host the
  round-counted budget itself can stretch to hundreds of milliseconds.
  Neither side is protected from starvation by the hand-off. The typical
  bounds assume one matcher per level. Callers that need tight admission
  or cancel latency should not loop large FOK takers on one deep level from
  a hot thread; prefer IOC where all-or-nothing is not required.
