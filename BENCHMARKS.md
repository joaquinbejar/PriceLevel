# Benchmarks: 0.9.2 vs 0.10.0

This document compares `pricelevel` 0.9.2 (published on crates.io, tag
[`v0.9.2`] = commit `aaafd39`) against the local 0.10.0 tree (commit
`3b05d81`, branch `docs-benchmarks-0-10`), plus current (0.10.0) absolute
numbers from this repo's own two bench suites. For per-operation latency
methodology, tail-latency numbers, allocation counts and the individual
performance investigations behind specific 0.10 changes (issues #140, #143,
#148, #149, #150, #154, #155), see [`BENCH.md`](./BENCH.md) — this document
does not repeat that detail; it links to it.

[`v0.9.2`]: https://github.com/joaquinbejar/PriceLevel/releases/tag/v0.9.2

## Summary

0.10.0 is faster than 0.9.2 in every case with a clear, round-to-round
consistent signal, and slower in a handful of small, also-consistent cases.
The honest headline:

**Large, consistent wins** (median across 2 interleaved rounds, apples-to-apples
harness in `benches/compare/`):

| Area | Change | Notes |
|---|---|---|
| Snapshot `snapshot_package()` @ depth 100 / 10,000 | **-76% / -78%** | issue #149/#162 |
| Snapshot `snapshot_to_json()` @ depth 100 / 10,000 | **-79% / -81%** | issue #149 (`BorrowedOrders` streaming) |
| Snapshot `from_snapshot_json` restore @ depth 100 / 10,000 | **-55% / -56%** | issue #150 (two-walk restore) |
| Allocations for `snapshot_to_json` @ depth 100 | **-93% (7,729 → 513 allocs/op)** | see allocation table below |
| Allocations for `from_snapshot_json` restore @ depth 100 | **-92% (11,922 → 896 allocs/op)** | |
| `TradeList::from_str` parse @ n=32 / n=1024 | **-39% / -38%** | issue #152/#174 checked parsers |
| Allocations for `TradeList::from_str` @ n=32 | **-87% (373 → 47 allocs/op)** | |
| FOK success @ depth 1 / 100 / 10,000 | **-40% / -47% / -61%** | issue #143 |
| FOK rejected @ depth 10,000 | **-24%** | issue #143 |
| FOK matcher throughput under writer contention, depth 10,000 | **~12x** (3,151 → 37,388 ops/s) | issue #143/#206; see contention table |
| Writer p99.9 under a looping FOK matcher, depth 10,000 | **~5,700x** (158.7 ms → 27.9 µs) | same guard-hold-time fix; 0.9.2's sample here is n=7 (writers were almost completely starved) |

**Small, consistent regressions** (all under 13%, all in the same
direction across both rounds):

| Area | Change | Likely cause |
|---|---|---|
| `add_orders_batch_100` (standard / reserve, 100 orders) | +3% / +4% | added checked-arithmetic / epoch-headroom guards (issues #163–#165) |
| `matching/ioc_partial` | +9% | same |
| `matching/post_only_reject` | +4% | same |
| `matching/sweep_100_makers` | +2% | same |
| `iter_orders/depth_100` | +4% | same |
| `snapshot/capture_depth_100` | +7% | same (depth 10,000 is flat, +0.4%) |
| `match_result_analytics` @ n=256 / n=4096 | **-6% / -15%** (faster, not a regression) | listed here only because it is the odd case where a small-n and large-n result differ in magnitude |

These regressions are the expected, disclosed cost of the Production Panic
Policy hardening in 0.10 (checked arithmetic, fallible growth, epoch
headroom checks — see `CHANGELOG.md`'s `[Unreleased]` section): a few
nanoseconds of extra validation per call, in exchange for eliminating
`unwrap`/`panic`/wrapping-arithmetic failure modes. None of them are close
to the wins above in magnitude.

**Noisy — not claimed as a difference either way** (round-to-round sign
flip or >10 percentage-point spread under this session's short
`sample_size(10)` runs on a shared, loaded host — see Methodology):
`add_orders_batch_100/iceberg`, `isolated_updates/cancel`,
`isolated_updates/replace_diff_price`, `isolated_updates/replace_same_price`,
`isolated_updates/update_quantity_decrease`, `iter_orders/depth_10000`,
`matching/iceberg`. Full round-by-round numbers are in
[`benches/compare/results/criterion_0.9.2_vs_0.10.0.csv`](./benches/compare/results/criterion_0.9.2_vs_0.10.0.csv).

**Not compared**: `match_order`'s inner hot path itself (the sweep, FIFO
consumption, atomic counters) has an *identical public signature* on both
versions and is exercised by every `matching/*` and `fok_depth/*` row above,
so those rows ARE the hot-path comparison. What is genuinely not compared
here is the main crate's own Criterion `Concurrent`/`Contention Patterns`
groups (`benches/concurrent/`) on 0.9.2: the full default `cargo bench
--bench benches` run on this session's shared host did not finish those
groups inside a time-boxed session (see "Current absolute results" below),
and re-running them a second time under `--features old` for a proper
comparison was out of budget for this session. `benches/compare/`'s own
`contention_compare` binary (not Criterion, see its doc comment) fills part
of that gap and is included above.

## System information

| | |
|---|---|
| CPU | Apple M5 Max, 18 logical cores (6 performance + 12 efficiency) |
| RAM | 128 GiB |
| OS | macOS 27.0 (build 26A428), Darwin 27.0.0, arm64 |
| rustc | 1.98.1 (`48a229cea`, 2026-09-01), LLVM 22.1.8, host `aarch64-apple-darwin` |
| cargo | 1.98.1 (`797e8a9bc`, 2026-08-05) |
| Toolchain | `stable` (`rust-toolchain.toml`) |
| Build profile | Cargo's default `bench` profile (inherits `release`: `opt-level = 3`, `debug = false`, `lto = false`, `codegen-units = 16`, `panic = "unwind"`) — neither the root `Cargo.toml` nor `benches/compare/Cargo.toml` overrides `[profile.bench]`, so both sides of every comparison compile under the same settings |
| Allocator | system allocator (`std::alloc::System`); the allocation-count pass installs a **counting wrapper** around it (`benches/compare/src/alloc_compare.rs`), disabled during timing |
| CPU pinning | none (macOS exposes no thread-affinity API; same caveat `BENCH.md` documents) |
| Power | `pmset -g batt`: "Now drawing from 'AC Power'" (not on battery, no throttling from that source) |
| Load average during runs | `uptime` sampled before/after each round: `3.96 5.61 6.44` → `5.63 5.95 6.53` → `4.71 5.64 6.37` → `4.30 5.40 6.24` (1/5/15-min load; host shared with desktop processes throughout, consistent with `BENCH.md`'s own "Apple silicon laptop, shared" note — this is a Mac Studio, not a laptop, but the same "shared with desktop processes" caveat applies) |
| Date | 2026-09-27 |
| Commits compared | 0.9.2 = `aaafd39a503a7c88e01dd9266c4fb0f096ed8215` (crates.io `pricelevel = "=0.9.2"`); 0.10.0 = `3b05d815ea5f84859f8b77f99ed843418d29dc9b` (local path dependency, `../..` from `benches/compare/`) |

## Methodology

### Harness: `benches/compare/`

A standalone crate, **not** part of the published `pricelevel` package and
**not** a member of the root workspace:

- `benches/compare/Cargo.toml` declares its own empty `[workspace]` table,
  so cargo treats it as its own workspace root rather than attaching it to
  the root `Cargo.toml`'s workspace (which lists only `examples`).
- The root `Cargo.toml`'s `include` list no longer globs `benches/**/*`; it
  lists every OTHER `benches/` subdirectory explicitly
  (`benches/mod.rs`, `benches/price_level/**/*`, `benches/concurrent/**/*`,
  `benches/simple/**/*`, `benches/latency/**/*`), so `benches/compare/` is
  excluded from `cargo package` / `cargo publish` by construction.
- It depends on the SAME upstream crate twice, under two local names,
  selected by a Cargo feature: `pricelevel_old = { package = "pricelevel",
  version = "=0.9.2" }` behind `--features old`, and `pricelevel_new =
  { package = "pricelevel", path = "../.." }` behind `--features new`. Both
  builds compile the identical `benches/compare/src/workloads.rs` and
  `benches/compare/benches/compare.rs` — the only version-conditional code
  in the whole crate is `benches/compare/src/shim.rs` and the `pl` alias in
  `benches/compare/src/lib.rs`.
- No new dependency was added to the main crate. `criterion` in the
  compare crate is the same version (`0.8`) the main crate already uses as
  a dev-dependency.

### The shim: what actually differs between 0.9.2 and 0.10, and what doesn't

Reading the two source trees side by side (`~/.cargo/registry/src/.../pricelevel-0.9.2/`
vs this worktree) turned up a **much shorter list than expected**. Byte-for-byte
identical between the two versions and called directly with no shim:
`PriceLevel::new`, `add_order`, `match_order`, `update_order` (all four keep
their exact 0.9.2 signatures — the hot path itself did not change shape),
the `OrderType<T>` struct-variant field layout (`Standard` /
`IcebergOrder` / `ReserveOrder` / `PostOnly` / …), `OrderUpdate`,
`Trade::with_timestamp`, `MatchResult::new` / `add_trade`,
`TradeList::from_str` / `Display`, `Price::new` / `Quantity::new` /
`TimestampMs::new`, `Id::sequential` / `from_u64`, `UuidGenerator::new`,
`snapshot_to_json` / `snapshot_package` / `from_snapshot_json`,
`PriceLevelSnapshot::orders()` / `order_count()`.

What `benches/compare/src/shim.rs` DOES paper over (see its doc comment for
the full list with issue numbers): `PriceLevel::snapshot()` (infallible in
0.9.2, `Result` in 0.10, issue #162), `MatchResult::with_capacity` /
`TradeList::with_capacity` (panic on overflow in 0.9.2 vs
`try_with_capacity` returning `Result` in 0.10, issue #170),
`MatchResult::add_filled_order_id` and `TradeList::add` (infallible `()` in
0.9.2 vs `Result` in 0.10, issue #170). Each shim function's two arms
perform the identical mutation; the 0.10 arm's `.expect(...)` is
harness-only code (this crate is not part of the published `pricelevel`
Production Panic Policy) and would only fire on a harness bug, never as a
measured outcome. `UuidGenerator::next` vs `try_next`, `Id`'s fallible
random constructors, and `PriceLevelStatistics::reset` vs `reset_at` are
**not used at all** by this harness — every id here is `Id::sequential(n)`
and no workload needs a statistics reset, so those particular 0.10 breaking
changes (issues #167, #168, #171) simply don't come up in an
apples-to-apples workload built this way.

### Measurement boundaries

Every scenario follows the same discipline as `BENCH.md`'s latency harness:
fixture construction (building a fresh level, seeding it to a target depth,
producing the JSON a restore parses) happens in Criterion's
`iter_batched` / `iter_batched_ref` *setup* closure, run **outside** the
timed region; only the one operation under test is timed. Outcome
validation (fill counts, `MatchOutcome`, resulting depth, trade counts) runs
in a one-off `self_check()` function that executes every scenario ONCE
before the Criterion suite starts and `assert_eq!`s the expected outcome —
never inside a timed closure. `add_orders_batch_*` is the one deliberate
exception: building the level AND adding N orders is the workload itself
("add standard/iceberg/reserve orders (batches, e.g. 100)"), so both steps
are timed together there, matching the task's own framing.

### What "same workload" means here

Both builds run the literal same Rust source in `workloads.rs` — same order
construction, same ids, same timestamps, same depths, same taker
quantities. The only difference the compiler sees is which `pricelevel`
crate `pl` resolves to. Where 0.10 rejected a would-be workload with a new
typed error 0.9.2 didn't have (none of the scenarios below hit that; see
"Notable behaviour differences" for where it matters elsewhere in the
crate), that would be recorded as a behaviour difference rather than timed
as if both did equal work — this run did not encounter one.

### Interleaving, rounds, and the noise policy

Per the task protocol: `old, new, old, new` (2 full interleaved rounds),
`uptime` recorded before/after each round (see System information above),
and the **median of the two rounds' point estimates** is the reported
number per scenario. A scenario is flagged **noisy** (not claimed as a real
difference) when the two rounds' `%change` either disagree in sign (both
beyond 2%, to avoid flagging true near-zero results as a "sign flip") or
differ from each other by more than 10 percentage points. This host runs at
load average 4–6.5 throughout (shared with desktop processes, matching
`BENCH.md`'s own disclosed environment), so no speedup smaller than the
observed round-to-round spread is claimed as real — see the noisy list in
the Summary above.

Each Criterion group in `benches/compare/benches/compare.rs` deliberately
overrides Criterion's own defaults (100 samples, ~3s warm-up + 5s
measurement per benchmark) down to `sample_size(10)` (Criterion's own
minimum) with a 300ms warm-up / 700ms measurement window — a ~35-scenario
suite at Criterion's defaults, run twice per version, twice per round, would
not fit this session's time budget. This trade-off is exactly why the noise
policy above exists: short runs are noisier, so anything within round-to-
round spread is disclosed as noise instead of a claimed result.

### Allocation counting

`benches/compare/src/alloc_compare.rs` mirrors `benches/latency/alloc.rs`'s
counting `#[global_allocator]` pattern (same `unsafe impl GlobalAlloc`
justification: bench-only, confined to one binary, never compiled by `cargo
build` of the published crate). It differs from `BENCH.md`'s allocation
pass in one way, disclosed in its own header comment: each measured closure
includes its own per-iteration fixture construction (building the level,
its orders) INSIDE the counted window, because that construction itself
differs in shape between 0.9.2 and 0.10 and the point of this comparison is
"same total workload, either version" rather than an isolated single-call
count. Treat these as "allocations for this whole workload unit", not as
`BENCH.md`'s isolated per-call numbers.

### Contention comparison

`benches/compare/src/contention_compare.rs` runs one matcher thread (GTC or
a looping FOK) against 3 writer threads doing add-then-cancel churn on a
`depth`-deep level, and reports matcher throughput plus writer
p50/p99/p99.9 from the pooled, sorted per-op nanosecond samples (no
hdrhistogram — not an approved dependency, same as `BENCH.md`). This is
**not** the same rigor as `BENCH.md`'s `contention_{gtc,fok}_matcher`
scenario, which untimed-cancels each iteration's own target maker so
matcher-owned depth never drifts; this comparison's `workloads::
run_contention` documents the simplification in its own doc comment. Treat
the resulting numbers as directional (0.9.2 vs 0.10 under the same
generated load), not as a strict per-call proof of FIFO coverage.

## Comparison tables

Full data (4 raw runs, both rounds, both versions) is in
[`benches/compare/results/criterion_0.9.2_vs_0.10.0.csv`](./benches/compare/results/criterion_0.9.2_vs_0.10.0.csv).
Selected rows (median ns/iter, `%change` = (new − old) / old):

| Scenario | 0.9.2 median (ns) | 0.10.0 median (ns) | Change | Noisy? |
|---|---|---|---|---|
| `fok_depth/success_depth_1` | 1,677 | 1,004 | **-40%** | no |
| `fok_depth/success_depth_100` | 7,205 | 3,795 | **-47%** | no |
| `fok_depth/success_depth_10000` | 550,236 | 216,439 | **-61%** | no |
| `fok_depth/reject_depth_100` | 7,548 | 6,939 | -8% | no |
| `fok_depth/reject_depth_10000` | 575,567 | 439,091 | **-24%** | no |
| `fok_depth/replenish_depth_10000` | 2,255,356 | 2,087,307 | -7% | no |
| `snapshot/package_depth_100` | 88,842 | 20,916 | **-76%** | no |
| `snapshot/package_depth_10000` | 8,711,249 | 1,880,801 | **-78%** | no |
| `snapshot/to_json_depth_100` | 169,238 | 36,020 | **-79%** | no |
| `snapshot/to_json_depth_10000` | 17,163,374 | 3,255,246 | **-81%** | no |
| `snapshot/restore_depth_100` | 122,999 | 54,805 | **-55%** | no |
| `snapshot/restore_depth_10000` | 12,353,166 | 5,490,372 | **-56%** | no |
| `snapshot/capture_depth_100` | 3,532 | 3,787 | +7% | no |
| `snapshot/capture_depth_10000` | 185,480 | 186,163 | +0.4% | no |
| `trade_list_parse/n_32` | 18,252 | 11,141 | **-39%** | no |
| `trade_list_parse/n_1024` | 567,849 | 349,535 | **-38%** | no |
| `match_result_analytics/n_256` | 533 | 501 | -6% | no |
| `match_result_analytics/n_4096` | 12,499 | 10,683 | **-15%** | no |
| `matching/standard_full` | 1,048 | 916 | -13% | borderline (rounds -8%/-17%) |
| `matching/reserve` | 1,001 | 899 | -10% | no |
| `matching/ioc_partial` | 991 | 1,083 | +9% | no |
| `matching/post_only_reject` | 1,079 | 1,117 | +4% | no |
| `matching/sweep_100_makers` | 19,418 | 19,889 | +2% | no |
| `iter_orders/depth_100` | 2,656 | 2,770 | +4% | no |
| `add_orders_batch_100/standard` | 10,513 | 10,949 | +4% | no |
| `add_orders_batch_100/reserve` | 10,462 | 10,795 | +3% | no |
| `isolated_updates/update_quantity_increase` | 9,595 | 9,284 | -3% | no |

Noisy rows (excluded from the table above — see CSV for the raw numbers):
`add_orders_batch_100/iceberg`, `isolated_updates/cancel`,
`isolated_updates/replace_diff_price`, `isolated_updates/replace_same_price`,
`isolated_updates/update_quantity_decrease`, `iter_orders/depth_10000`,
`matching/iceberg`.

### Allocation comparison

[`benches/compare/results/alloc_0.9.2_vs_0.10.0.csv`](./benches/compare/results/alloc_0.9.2_vs_0.10.0.csv),
1,000 reps/op, single run (not interleaved/round-checked — allocation counts
are deterministic per input on both versions, so round-to-round noise is
not expected here the way timing noise is):

| Operation | 0.9.2 allocs/op | 0.10.0 allocs/op | Change |
|---|---|---|---|
| `snapshot_to_json_100` | 7,728.61 | 512.60 | **-93%** |
| `restore_100` | 11,922.21 | 896.36 | **-92%** |
| `trade_list_parse_32` | 373.00 | 47.00 | **-87%** |
| `snapshot_capture_100` | 507.70 | 502.76 | -1% |
| `match_sweep_100` | 372.70 | 372.80 | +0.03% |
| `match_full` | 10.02 | 9.02 | -10% |
| `add_order_standard` | 6.00 | 6.00 | 0% |
| `match_result_analytics_256` | 2.00 | 2.00 | 0% |

The snapshot/restore/parse drops match `CHANGELOG.md`'s `[Unreleased]`
entries for issues #149 (`BorrowedOrders`, streamed SHA-256), #150 (two-walk
restore) and #152/#174 (checked, allocation-light text parsers that no
longer build a temporary `Vec`/`HashMap` per parse).

### Contention comparison

[`benches/compare/results/contention_0.9.2_vs_0.10.0.csv`](./benches/compare/results/contention_0.9.2_vs_0.10.0.csv),
1 matcher + 3 writers, 500 matcher ops, single run per version/matcher/depth
combination (this is the directional comparison the Methodology section
above caveats, not a strict per-call proof):

| Matcher | Depth | Version | Matcher ops/s | Writer p50 (ns) | Writer p99.9 (ns) |
|---|---|---|---|---|---|
| GTC | 100 | 0.9.2 | 441,940 | 2,333 | 8,167 |
| GTC | 100 | 0.10.0 | 436,872 | 1,666 | 557,375 (outlier, n=1,097) |
| FOK | 100 | 0.9.2 | 72,136 | 1,125 | 103,709 |
| FOK | 100 | 0.10.0 | 27,423 | 2,083 | 40,792 |
| GTC | 10,000 | 0.9.2 | 562,561 | 2,084 | 7,875 |
| GTC | 10,000 | 0.10.0 | 363,978 | 3,209 | 9,875 |
| FOK | 10,000 | 0.9.2 | **3,151** | 18,505,042 | 158,699,000 |
| FOK | 10,000 | 0.10.0 | **37,388** | 1,959 | 27,959 |

The FOK-at-depth-10,000 row is the headline: 0.9.2's FOK dry run walked and
sorted the whole 10,000-order level under its exclusive guard on every
match, starving the writers (only 7 writer samples completed across the
whole 158.7ms run — everything else was blocked waiting for the guard);
0.10's bounded dry run (issue #143) finishes in time for writers to get
through 11,488 ops at a p99.9 of 28µs. This is the same effect
`CHANGELOG.md` and `BENCH.md`'s own `fok_depth` contention section
document, reproduced independently here against the actual 0.9.2 release
rather than against 0.10's own pre-#143 history.

## Current absolute results (0.10.0)

### Criterion (`cargo bench --bench benches`)

A full default-settings run on this host did not finish the `Concurrent`/
`Contention Patterns` groups within this session's time budget (Criterion's
own defaults — 100 samples, ~3s warm-up + 5s measurement per benchmark —
across every `benches/{price_level,concurrent,simple}/` case, several of
which spawn real OS threads per sample); the run was time-boxed at 590s and
captured every `PriceLevel - *` and `Data Operations` group (61 benchmark
functions) before being cut off. Selected results (ns/iter, default
Criterion sampling, single run — not interleaved, since this section is
reporting 0.10's own absolute numbers, not a comparison):

| Benchmark | Result |
|---|---|
| `Add Orders/add_standard_order` | 9,365 ns |
| `Add Orders/order_count_scaling/1000` | 98,053 ns |
| `Match Orders/match_standard_orders` | 9,958 ns |
| `Match Orders/match_mixed_orders` | 12,870 ns |
| `Match Orders/match_quantity_scaling_standard/500` | 13,268 ns |
| `Update Orders/cancel_order` | 13,071 ns |
| `Update Orders/cancel_order_count_scaling/1000` | 125,566 ns |
| `Mixed Operations/large_order_throughput` | 137,221 ns |
| `Snapshot Recovery/snapshot_full_roundtrip` | 129,598 ns |
| `Snapshot Recovery/snapshot_package_500_orders` | 111,402 ns |
| `Checked Arithmetic/match_result_add_trades` | 522 ns |
| `Iter Orders/iter_orders_count/1000` | 5,456 ns |
| `Serialization/trade_display` | 175 ns |

Reproduce with `make bench` (`cargo criterion --bench benches`) or `cargo
bench --bench benches` directly; the `Concurrent`/`Contention Patterns`
groups need their own, longer, dedicated run — `cargo bench --bench benches
"Contention"` to scope it to just those.

### Latency harness (`make bench-latency`)

The harness's true defaults (`PL_LATENCY_SAMPLES=20000`,
`PL_LATENCY_WARMUP=2000`) did not finish in this session's time budget
either — `restore_sizes`/`snap_sizes` alone sweep 100 / 10,000 / 100,000
resting orders at 20,000 samples each, and `stats_contention` runs several
multi-thread cases at 20,000 matcher ops. The run below instead used
`PL_LATENCY_SAMPLES=2000 PL_LATENCY_WARMUP=200 PL_LATENCY_CONTENTION_OPS=1000
PL_LATENCY_STATS_OPS=2000 PL_LATENCY_ALLOC_REPS=500` (a real, complete run
of every scenario at 1/10th the default sample count, not a fabricated
extrapolation) and finished in under a minute. `BENCH.md`'s own worked
example (300 samples) and this repo's `target/latency/<run-id>/manifest.json`
from any run remain the canonical source for the full default-sample-count
numbers; this table is a representative excerpt at a larger, still-honest
sample count than `BENCH.md`'s own quoted example.

p99.99 carries `BENCH.md`'s own [exploratory caveat](./BENCH.md#the-p9999-caveat)
regardless of sample count and is omitted here for brevity — see the run's
own `manifest.json` for it.

| Scenario | Depth | Samples | p50 (ns) | p99 (ns) | p99.9 (ns) |
|---|---|---|---|---|---|
| `isolated_add_gtc` | 1,000 | 2,000 | 83 | 625 | 1,084 |
| `isolated_cancel_success` | 2,200 | 2,000 | 83 | 583 | 667 |
| `match_full` | 1 | 2,000 | 208 | 667 | 917 |
| `match_maker_partial` | 1,000 | 2,000 | 250 | 1,041 | 3,584 |
| `many_fill_sweep` | 44,000 | 2,000 | 3,958 | 6,209 | 11,583 |
| `tif_fok_success` | 1 | 2,000 | 250 | 666 | 750 |
| `tif_fok_reject` | 1 | 2,000 | 83 | 84 | 209 |
| `iteration` | 1,000 | 2,000 | 11,792 | 12,125 | 15,042 |
| `snapshot_capture` | 1,000 | 2,000 | 11,792 | 12,209 | 13,375 |
| `checksum_validate` | 1,000 | 2,000 | 178,208 | 182,958 | 199,167 |
| `restore` | 1,000 | 2,000 | 555,833 | 573,375 | 592,209 |
| `from_snapshot_valid@100000` | 100,000 | 50 | 11,334,333 | 11,598,042 | 11,598,042 |
| `from_json_valid@100000` | 100,000 | 50 | 57,393,292 | 60,016,125 | 60,016,125 |
| `fok_first_maker@10000` | 10,000 | 2,000 | 250 | 792 | 875 |
| `fok_rejected@10000` | 10,000 | 2,000 | 175,708 | 188,917 | 201,375 |
| `fok_replenish@10000` | 10,000 | 2,000 | 667 | 1,417 | 1,834 |
| `contention_gtc_matcher` | 1 | 1,000 | 709 | 2,542 | 4,333 (matcher 397,720 ops/s; writers 6,318,046 ops/s) |
| `contention_fok_matcher` | 1 | 1,000 | 26,291 | 187,459 | 269,000 (matcher 24,937 ops/s; writers 7,262,904 ops/s) |
| `statsc_ok_single` | 3,200 | 2,000 | 250 | 791 | 917 |
| `statsc_ok_same_mixed` | 3,200 | 2,000 | 709 | 1,459 | 3,500 |
| `statsc_overflow_same_mixed` | 3,200 | 2,000 | 791 | 1,750 | 3,459 |

Allocation pass (500 reps/op, `PL_LATENCY_ALLOC_REPS=500`):

| Operation | alloc_count/op | alloc_bytes/op |
|---|---|---|
| `add_order` | 2.16 | 392.13 |
| `match_full` | 2.08 | 1,807.63 |
| `match_sweep_100` | 8.25 | 22,755.94 |
| `snapshot_capture` | 130.00 | 27,072.00 |
| `checksum_validate` | 1.00 | 64.00 |
| `restore` | 3,334.24 | 468,123.15 |

`contention_fok_matcher`'s p50 here (26,291 ns) is roughly 37x
`contention_gtc_matcher`'s (709 ns) at the same load — consistent with
`BENCH.md`'s own documented "GTC-vs-FOK under identical load" finding
(there: ~42x on its own 300-sample example run). `fok_rejected@10000`'s cost
(p50 175.7 µs) vs `fok_first_maker@10000` (p50 250 ns) is the depth-scaling
effect `BENCH.md`'s "Fill-or-kill feasibility depth" section documents in
full.

Reproduce with `make bench-latency` (full defaults, budget ~20+ minutes on
this host) or the reduced command above for a faster, still-real run.

## Notable behaviour differences (0.9.2 vs 0.10.0)

Not measured above because they are correctness/API changes, not timing —
see `CHANGELOG.md`'s `[Unreleased]` section and `src/lib.rs`'s migration
guides for the complete list. The ones most likely to surprise someone
porting a benchmark or workload from 0.9.2:

- **`PriceLevel::snapshot()` now returns `Result`** (issue #162): a same-side
  quantity transfer between shards during the shard walk could overflow a
  `u64` aggregate in 0.9.2, where it either panicked a `debug_assert!` in
  debug builds or silently stored a disagreeing aggregate in release. 0.10
  recollects (bounded, at most 8 times) and returns
  `PriceLevelError::InvalidOperation` instead of either.
- **Random `Id` construction is fallible** (issue #167): `Id::new()` /
  `Id::new_uuid()` / `Id::new_ulid()` / the random `Default` are removed
  (could panic on OS entropy/RNG failure) in favour of `Id::try_new(&clock,
  &mut entropy)` and friends. This harness never exercised the removed
  constructors — every id here is `Id::sequential`, which is unchanged.
- **`UuidGenerator::next` → `try_next`** (issue #168): the old unchecked
  `fetch_add` wrapped `u64::MAX → 0` and re-issued the counter-zero id; 0.10
  reserves the sequence with a checked CAS and returns `CapacityExceeded`
  once exhausted, never re-issuing.
- **Snapshot format v4** for a `value_executed` above `u64::MAX` (issue
  #140, `PriceLevelStatistics::value_executed()` widened `u64` → `u128`): a
  0.9.2 reader rejects a v4 package outright (version mismatch, or a
  deserialization error if the value doesn't fit `u64`). Every scenario in
  this benchmark stays well under that threshold, so no snapshot round-trip
  above hit this boundary.
- **`MatchResult`/`TradeList` growth is fallible** (issue #170):
  `with_capacity` (panicked on `usize::MAX`-style overflow in 0.9.2) →
  `try_with_capacity` returning `Result`; `add_filled_order_id` / `add` gain
  a `Result`. The compare harness's `shim.rs` papers over exactly this so
  both versions do identical work; see Methodology above.
- **Trade and statistics clock reads are caller-supplied and fallible**
  (issue #171): `Trade::new` is removed (`Trade::with_timestamp` — used by
  this harness — is unchanged); `PriceLevelStatistics::reset()` becomes
  fallible `reset(&clock)` plus a new infallible `reset_at`. Neither this
  harness's workloads nor `match_order` itself reads a wall clock on either
  version, so this did not affect any timed result here.

## Reproduction

```sh
# Criterion comparison (writes benches/compare/target/criterion/<group>/<version>/...)
cargo bench --manifest-path benches/compare/Cargo.toml --features old --bench compare
cargo bench --manifest-path benches/compare/Cargo.toml --features new --bench compare
# or, from the repo root:
make bench-compare-0.9

# Allocation comparison
make bench-compare-0.9-alloc

# Contention comparison
make bench-compare-0.9-contention

# Current absolute numbers (this repo's own suites)
cargo bench --bench benches            # full Criterion suite (make bench)
make bench-latency                     # latency harness, default sample counts
```

Raw data behind every table above: [`benches/compare/results/`](./benches/compare/results/)
(`criterion_0.9.2_vs_0.10.0.csv`, `alloc_0.9.2_vs_0.10.0.csv`,
`contention_0.9.2_vs_0.10.0.csv`, and the four raw `round{1,2}_{0.9.2,0.10.0}.txt`
bencher-format outputs the CSV was built from).

For the latency harness's own methodology, tail-latency numbers, allocation
measurements and the individual performance investigations (issues #140,
#143, #148, #149, #150, #154, #155) behind specific 0.10 changes, see
[`BENCH.md`](./BENCH.md).
