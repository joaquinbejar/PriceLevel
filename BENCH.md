# Latency benchmarks

This document covers `benches/latency/` — the isolated-operation, tail-latency
harness added for issue [#142]. It is a **separate bench target** from the
Criterion suite under `benches/{price_level,concurrent,simple}/`
(`benches/mod.rs`, `[[bench]] name = "benches"`); running one never runs, or
slows down, the other.

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
(nanoseconds), and `benches/latency/stats.rs` sorts that vector once, after
the loop, to compute p50 / p99 / p99.9 / (p99.99 when justified). See
`stats::MIN_SAMPLES_FOR_P9999` for the refusal rule.

## What is measured

Every scenario below times **exactly one public-API call** per sample.
Fixture construction (seeding a level, building the JSON to restore from,
etc.) and result destruction happen outside the timed window; where a
scenario legitimately needs fresh per-sample state (e.g. a fresh maker order
before each partial-fill match, so the level does not run dry), that setup
also runs outside the `Instant` pair — see `timing::measure_with_setup`.

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
harness rather than reporting a silently-wrong number.

### TIF coverage and what it actually shows

`PriceLevel::match_order` does not itself rest an unfilled taker (see the
doc comment on `MatchOutcome::PartiallyFilled`) and does not enforce a
resting maker's own GTD/DAY expiry (see `doc/architecture.md`). Consequently
GTC / IOC / DAY / a non-expired GTD taker exercise the **identical** code
path inside `match_order` — only the `TimeInForce` discriminant passed in
differs. `tif.rs` measures this directly instead of assuming a difference;
the example run below shows exactly that (GTC/IOC/DAY/GTD full-match numbers
are the same distribution within noise). FOK is the one taker TIF that
branches differently (it takes the level-wide fill-or-kill guard), and that
shows up clearly both in the uncontended `tif_fok_success` /
`tif_fok_reject` numbers and, much more dramatically, in the contention
comparison below.

### The contention scenario

`contention_gtc_matcher` and `contention_fok_matcher` seed a level with one
resting maker per matcher operation (so the matcher never runs dry) plus a
disjoint "churn" id pool that `N-1` writer threads continuously
add/cancel/read against, using a `Barrier` + `AtomicBool` start protocol so
every thread begins at (as close as possible to) the same instant. The
matcher's own per-operation latency is what is recorded; the writer threads'
op counts are reported for outcome accounting only, not for their own
latency. Comparing `contention_fok_matcher` against `contention_gtc_matcher`
under the *same* writer load isolates the fill-or-kill guard's blocking
effect; comparing either against the equivalent uncontended `tif.rs` scenario
(`tif_gtc_full_match` / `tif_fok_success`) isolates the writer-thread
contention's effect on top of that. The example run below shows both: FOK
under this contention load is roughly two orders of magnitude slower than
GTC under the same load, and the FOK contended number is itself far above
its own uncontended `tif_fok_success` baseline.

## Allocation measurements

`benches/latency/alloc.rs` installs a `#[global_allocator]` wrapper around
`std::alloc::System` for this binary only (`unsafe impl GlobalAlloc`,
required by the trait; confined to this bench binary, never `src/` — see the
module doc for the full justification). Counting is **disabled** during
every latency scenario above, so those numbers are not inflated by counter
bookkeeping; a separate, untimed pass in `alloc_measurements.rs` resets the
counters, enables counting, runs `PL_LATENCY_ALLOC_REPS` repetitions of one
representative operation, disables counting, and reports the per-operation
average. `add_order` / `match_full` are cheap (a handful of allocations);
`checksum_validate` and `restore` are not — see the example numbers below
and treat them as a baseline to catch a regression against, not as an
absolute performance claim (no comparable "before" run exists yet).

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
performance claim.** No comparable "before" baseline exists yet (this is the
first version of this harness), and 300 samples is below
`stats::MIN_SAMPLES_FOR_P9999` (20,000), so every p99.99 column below
correctly reads "insufficient samples" rather than showing a number. Run it
yourself with the command above; do not cite these specific nanosecond
figures as a crate performance guarantee.

### Manifest

```
== Run manifest ==
commit             : 17374c39b10ed6f336199fe99d7bcec809f4260d (dirty)
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
timer overhead     : 14 ns (mean of 10,000 back-to-back Instant::now() calls)
loop model         : closed-loop / service-time only — see coordinated-omission disclosure below
```

`(dirty)` above reflects the worktree state at the moment this example was
captured during development of this harness, not a property of the harness
itself; a clean checkout on a tagged commit reports `(clean)`.

### Results

| Scenario | Category | Depth | Samples | p50 (ns) | p99 (ns) | p99.9 (ns) | p99.99 (ns) | max (ns) | Outcomes |
|---|---|---|---|---|---|---|---|---|---|
| isolated_add_gtc | isolated | 1000 | 300 | 84 | 1083 | 1208 | insufficient samples | 1208 | 300/300 succeeded |
| isolated_cancel_success | isolated | 350 | 300 | 42 | 292 | 833 | insufficient samples | 833 | 300/300 found and cancelled |
| isolated_cancel_missing | isolated | 1000 | 300 | 41 | 42 | 42 | insufficient samples | 42 | 300/300 reported missing (Ok(None)) |
| isolated_quantity_decrease | isolated | 350 | 300 | 42 | 125 | 208 | insufficient samples | 208 | 300/300 resized (100 -> 40) |
| isolated_quantity_increase | isolated | 350 | 300 | 125 | 833 | 1250 | insufficient samples | 1250 | 300/300 resized (40 -> 100) |
| isolated_replace | isolated | 350 | 300 | 125 | 750 | 833 | insufficient samples | 833 | 300/300 replaced |
| match_empty | match | 0 | 300 | 42 | 42 | 791 | insufficient samples | 791 | 300/300 NotFilled |
| match_full | match | 350 | 300 | 250 | 1250 | 1333 | insufficient samples | 1333 | 300/300 Filled |
| match_partial | match | 0 | 300 | 250 | 833 | 917 | insufficient samples | 917 | 300/300 PartiallyFilled |
| many_fill_sweep | match | 7000 | 300 | 4667 | 6375 | 10500 | insufficient samples | 10500 | 300/300 Filled, 6000 total trades |
| iceberg_replenish | match | 1 | 300 | 291 | 875 | 1042 | insufficient samples | 1042 | 300/300 Filled |
| reserve_replenish | match | 1 | 300 | 291 | 875 | 917 | insufficient samples | 917 | 300/300 Filled |
| tif_gtc_full_match | tif | 350 | 300 | 250 | 375 | 875 | insufficient samples | 875 | 300/300 Filled (taker_tif=Gtc) |
| tif_ioc_full_match | tif | 350 | 300 | 250 | 792 | 4833 | insufficient samples | 4833 | 300/300 Filled (taker_tif=Ioc) |
| tif_day_full_match | tif | 350 | 300 | 250 | 375 | 916 | insufficient samples | 916 | 300/300 Filled (taker_tif=Day) |
| tif_gtd_full_match | tif | 350 | 300 | 250 | 334 | 833 | insufficient samples | 833 | 300/300 Filled (taker_tif=Gtd(9999999999999)) |
| tif_fok_success | tif | 350 | 300 | 1459 | 2209 | 2833 | insufficient samples | 2833 | 300/300 Filled (taker_tif=Fok) |
| tif_fok_reject | tif | 1 | 300 | 1209 | 1542 | 4792 | insufficient samples | 4792 | 300/300 Killed |
| tif_post_only_reject | tif | 1 | 300 | 667 | 750 | 792 | insufficient samples | 792 | 300/300 Rejected |
| iteration | iteration | 1000 | 300 | 6167 | 7667 | 7750 | insufficient samples | 7750 | 300/300 traversals visited exactly 1000 orders |
| snapshot_capture | snapshot | 1000 | 300 | 12875 | 14333 | 18542 | insufficient samples | 18542 | 300/300 snapshots carried exactly 1000 orders |
| checksum_validate | snapshot | 1000 | 300 | 831333 | 1012167 | 1043041 | insufficient samples | 1043041 | 300/300 validated OK |
| restore | snapshot | 1000 | 300 | 1207667 | 1268292 | 1295750 | insufficient samples | 1295750 | 300/300 restored with exactly 1000 orders |
| depth_sweep_add@100 | depth | 100 | 300 | 83 | 167 | 37625 | insufficient samples | 37625 | 300/300 succeeded |
| depth_sweep_add@1000 | depth | 1000 | 300 | 83 | 958 | 2917 | insufficient samples | 2917 | 300/300 succeeded |
| depth_sweep_small_taker@100 | depth | 100 | 300 | 208 | 292 | 334 | insufficient samples | 334 | 300/300 Filled |
| depth_sweep_small_taker@1000 | depth | 1000 | 300 | 208 | 291 | 292 | insufficient samples | 292 | 300/300 Filled |
| scaled_quantity@1 | depth | 100 | 300 | 83 | 208 | 750 | insufficient samples | 750 | 300/300 succeeded |
| scaled_quantity@10000 | depth | 100 | 300 | 83 | 208 | 208 | insufficient samples | 208 | 300/300 succeeded |
| scaled_quantity@1000000000 | depth | 100 | 300 | 84 | 167 | 208 | insufficient samples | 208 | 300/300 succeeded |
| scaled_price@1 | depth | 100 | 300 | 84 | 250 | 375 | insufficient samples | 375 | 300/300 succeeded |
| scaled_price@10000 | depth | 100 | 300 | 84 | 208 | 209 | insufficient samples | 209 | 300/300 succeeded |
| scaled_price@18446744073709551615 | depth | 100 | 300 | 83 | 167 | 209 | insufficient samples | 209 | 300/300 succeeded |
| contention_gtc_matcher | contention | 300 | 300 | 666 | 2041 | 2875 | insufficient samples | 2875 | matcher: 300/300 Filled; writers: 1448 completed (1108 successful, 0 missing, 340 rejected) across 3 threads |
| contention_fok_matcher | contention | 300 | 300 | 66000 | 83709 | 105708 | insufficient samples | 105708 | matcher: 300/300 Filled; writers: 190 completed (154 successful, 0 missing, 36 rejected) across 3 threads |

**Reading the FOK contention row.** `contention_fok_matcher`'s p50 (66,000
ns) is roughly 100x `contention_gtc_matcher`'s p50 (666 ns) under the
identical writer-thread load, and roughly 45x `tif_fok_success`'s own
uncontended p50 (1,459 ns). That gap is exactly what
`doc/architecture.md`'s "Fill-or-kill excludes every mutator on the level"
predicts: an FOK match holds the level-wide guard exclusively across its
whole dry-run and sweep, so it now also waits behind the writer threads'
admissions/cancels contending for that same guard's shared side — not the
per-maker shard lock GTC pays alone. Writer-thread throughput during the FOK
run also dropped (190 completed vs. 1,448 for GTC over the same wall-clock
matcher-side window), which is the guard blocking the writers, not the
writers blocking themselves. This is exactly the effect the issue asks this
harness to make visible, separated from uncontended service time.

### Allocation measurements (same run)

```
add_order            reps=200    alloc_count/op=2.17     alloc_bytes/op=378.28     dealloc_count_total=33       dealloc_bytes_total=42972
match_full           reps=200    alloc_count/op=4.10     alloc_bytes/op=1981.93    dealloc_count_total=744      dealloc_bytes_total=81394
snapshot_capture     reps=200    alloc_count/op=138.00   alloc_bytes/op=43936.00   dealloc_count_total=27400    dealloc_bytes_total=7155200
checksum_validate    reps=200    alloc_count/op=36014.00 alloc_bytes/op=936224.00  dealloc_count_total=7202800  dealloc_bytes_total=187244800
restore              reps=200    alloc_count/op=41348.29 alloc_bytes/op=1790969.60 dealloc_count_total=7843670  dealloc_bytes_total=293425868
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
