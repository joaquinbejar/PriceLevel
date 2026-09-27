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
| `match`      | empty book, full fill, partial fill, a many-fill sweep (20 makers in one call), iceberg replenish, reserve replenish |
| `tif`        | GTC / IOC / DAY / GTD full match, FOK success, FOK rejection (killed), post-only rejection |
| `iteration`  | one full `iter_orders` traversal |
| `snapshot`   | `snapshot()` capture, checksum `validate()`, `from_snapshot_json` restore |
| `depth`      | `add_order` and a 1-unit taker match, swept across resting depth 100 / 1,000 / (opt-in) 10,000 / 100,000; order-quantity and level-price magnitude sweeps |
| `contention` | one matcher thread (`match_order`) under `N-1` concurrent admissions/cancels/reads, run once with a GTC matcher and once with an FOK matcher |

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

## Retained Criterion benches

This harness does not modify the existing Criterion suite. `match_orders.rs`,
`update_orders.rs` and `snapshot_recovery.rs` under `benches/price_level/`
continue to measure setup + batched operations + (for some cases)
destruction together, as lifecycle comparisons — see the "Why a second
harness" section above for why that is a different, and still useful,
measurement from the per-operation numbers in this document.
