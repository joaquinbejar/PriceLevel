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
| `match`      | empty book, full fill, partial fill, partial fill of one large front maker on a 1,000-deep level (#148), a many-fill sweep (20 makers in one call), iceberg replenish, reserve replenish |
| `tif`        | GTC / IOC / DAY / GTD full match, FOK success, FOK rejection (killed), post-only rejection |
| `iteration`  | one full `iter_orders` traversal |
| `snapshot`   | `snapshot()` capture, checksum `validate()`, `from_snapshot_json` restore |
| `depth`      | `add_order` and a 1-unit taker match, swept across resting depth 100 / 1,000 / (opt-in) 10,000 / 100,000; order-quantity and level-price magnitude sweeps |
| `contention` | one matcher thread (`match_order`) under `N-1` concurrent admissions/cancels/reads, run once with a GTC matcher and once with an FOK matcher |
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
| `PL_LATENCY_CONTENTION_THREADS`   | 4       | Total threads in the contention scenario (1 matcher + N-1 writers) |
| `PL_LATENCY_CONTENTION_OPS`       | 5,000   | Matcher-thread operations measured per contention run |
| `PL_LATENCY_ALLOC_REPS`           | 2,000   | Repetitions per operation in the allocation-measurement pass |
| `PL_LATENCY_STATS_PRODUCERS`      | 2       | Producer (cancel + re-add) threads per `stats_contention` case |
| `PL_LATENCY_STATS_READERS`        | 2       | Statistics-reader threads per `stats_contention` case |
| `PL_LATENCY_STATS_OPS`            | 20,000  | Matcher operations measured per `stats_contention` case |
| `PL_LATENCY_ONLY`                 | all     | Comma-separated groups to run: `isolated`, `match`, `tif`, `snapshot` (includes `iteration`), `depth`, `contention`, `stats_contention`, `alloc` |

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
  +1 clock tick at p50 on every full-fill latency scenario. That moves cost
  onto the common path; rejected.
- **eager**: no measurable latency change and no allocation change except
  one of 142 allocations on fill-or-kill (the dry run's resting-order
  snapshot dominates that path). Not worth a new queue action and a retry
  path in the match loop; rejected.
- **Tighter trade estimate** (for example adding hidden quantity for
  replenishing levels): no bound on trades exists without walking the
  queue; `count + hidden` over-reserves by orders of magnitude for a large
  hidden tranche. Not pursued.
- **Caller-owned buffers**: public API churn for a gain bounded by the
  numbers above; rejected.

The cases stay in the allocation pass, the latency harness and Criterion as
a regression tripwire for result sizing.
