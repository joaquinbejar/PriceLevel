# Benchmarks: 0.9.2 vs 0.10.0

This document compares `pricelevel` 0.9.2 (published on crates.io, tag
[`v0.9.2`] = commit `aaafd39a503a7c88e01dd9266c4fb0f096ed8215`) against the
local 0.10.0 tree (`src/` as of commit `3b05d815ea5f84859f8b77f99ed843418d29dc9b`,
branch `docs-benchmarks-0-10`), plus current (0.10.0) absolute numbers from
this repo's own two bench suites. Every measurement in this document was
taken from a checkout of `docs-benchmarks-0-10` at or after commit `3b05d81`;
this document's own harness/results/doc commits on top of that commit touch
only `Cargo.toml`, `Makefile`, `benches/compare/**` and `BENCHMARKS.md`
itself — never `src/` — so the compiled `pricelevel` crate under test is
byte-for-byte the `3b05d81` tree throughout, regardless of which of this
branch's own later commits happened to be checked out when a given number
was measured. For per-operation latency methodology, tail-latency numbers,
allocation counts and the individual performance investigations behind
specific 0.10 changes (issues #140, #143, #148, #149, #150, #154, #155),
see [`BENCH.md`](./BENCH.md) — this document does not repeat that detail;
it links to it.

[`v0.9.2`]: https://github.com/joaquinbejar/PriceLevel/releases/tag/v0.9.2

## Summary

0.10.0 is faster than 0.9.2 in every large-magnitude case, with a clear,
round-to-round consistent signal at Criterion's own default sample size
(100 samples, 3s warm-up, 5s measurement) across 3 interleaved rounds. A
larger set of small-magnitude cases turned out to be **noisy** at that
sample size on this shared host than an earlier, shorter-sample-size pass
suggested — seeing more noise with more rigor, not less, is itself useful
information, and is reported below rather than smoothed over. The honest
headline:

**Large, consistent wins** (median of 3 interleaved rounds — `old, new`
repeated 3 times — apples-to-apples harness in `benches/compare/`, Criterion
defaults: 100 samples / 3s warm-up / 5s measurement; see the Comparison
table below for every round's individual numbers):

| Area | Change | Notes |
|---|---|---|
| Snapshot `snapshot_package()` @ depth 100 / 10,000 | **-76.5% / -78.0%** | issue #149/#162 |
| Snapshot `snapshot_to_json()` @ depth 100 / 10,000 | **-79.0% / -80.2%** | issue #149 (`BorrowedOrders` streaming) |
| Snapshot `from_snapshot_json` restore @ depth 100 / 10,000 | **-55.1% / -54.5%** | issue #150 (two-walk restore) |
| Allocations for `snapshot_to_json` @ depth 100 | **-93% (7,729 → 513 allocs/op)** | see allocation table below |
| Allocations for `from_snapshot_json` restore @ depth 100 | **-92% (11,922 → 896 allocs/op)** | |
| `TradeList::from_str` parse @ n=32 / n=1024 | **-38.5% / -38.3%** | issue #152/#174 checked parsers |
| Allocations for `TradeList::from_str` @ n=32 | **-87% (373 → 47 allocs/op)** | |
| FOK success @ depth 1 / 100 / 10,000 | **-41.6% / -44.9% / -60.4%** | issue #143 |
| FOK rejected @ depth 10,000 | **-27.2%** | issue #143 |
| `match_result_analytics` @ n=4096 | **-14.2%** | not isolated to a specific change |
| `matching/reserve` full match | **-7.3%** | not isolated |
| `matching/standard_full` match | **-5.8%** | not isolated |
| FOK matcher throughput under writer contention, depth 10,000 | 3,151 → 37,388 ops/s (n=500 ops each; see caveat below) | issue #143/#206; see contention table |

**FOK-under-contention writer tail at depth 10,000**: 0.9.2's writers
completed only **7 samples** in the whole 158.7ms window (almost fully
starved — see "Contention comparison" below for why a ratio computed from
n=7 is not reported as a headline number here). 0.10's writers completed
11,488 samples in a 13.4ms window at a p99.9 of 28µs. Read this as "0.9.2's
writers were nearly completely blocked; 0.10's were not", not as a precise
multiplier.

**Small, mixed-sign, and mostly noisy results.** At Criterion's default
sample size, MORE of the small-magnitude scenarios turned out to be noisy
(round-to-round sign disagreement or >10 percentage-point spread) than the
earlier short-sample-size pass suggested — see the Comparison table and the
Methodology section's noise policy. The scenarios below are NOT noisy by
that policy and show a small, consistent regression, but **the cause is not
established by this benchmark**:

| Area | Change | Hypothesis (not isolated) |
|---|---|---|
| `add_orders_batch_100/standard` | **+2.6%** | possibly the checked-arithmetic / epoch-headroom guards added across issues #163–#165, but this was not isolated (no before/after profiling of just those code paths) — treat as unresolved |
| `matching/iceberg` | **+8.4%** (borderline: round-to-round spread 9.96 pp, just under the 10 pp noise threshold) | unresolved, same caveat |
| `matching/mixed_100` | **+4.5%** | unresolved, same caveat |
| `matching/partial_fill_churn_10x10` | **+8.4%** | unresolved, same caveat |
| `snapshot/capture_depth_100` | **+4.3%** | unresolved, same caveat (depth 10,000 is a small WIN, -2.6%, so this is not a uniform effect across depth) |

**The cause of these regressions is not established by this benchmark.**
They correlate in time with the Production Panic Policy hardening in 0.10
(checked arithmetic, fallible growth, epoch headroom checks — see
`CHANGELOG.md`'s `[Unreleased]` section), which is a plausible hypothesis
given roughly the same code paths changed, but this document did not
isolate any single change with a before/after profile of just that change,
and no flamegraph or disassembly comparison was done. Do not read the table
above as a proven causal attribution — it is correlation with a named,
plausible hypothesis, not a demonstrated cause. None of these regressions
are close to the wins above in magnitude.

**Noisy — not claimed as a difference either way** (round-to-round sign
disagreement or >10 percentage-point spread across the 3 rounds, at
Criterion's default sample size, on a shared host — see Methodology):
`add_orders_batch_100/reserve`, `isolated_updates/cancel`,
`isolated_updates/replace_same_price`, `isolated_updates/update_quantity_decrease`,
`iter_orders/depth_100`, `match_result_analytics/n_256`,
`matching/ioc_partial`, `matching/partial_fill_reinsert`,
`matching/post_only_reject`, `matching/sweep_100_makers`. Full round-by-round
numbers for every scenario (noisy or not) are in
[`benches/compare/results/criterion_0.9.2_vs_0.10.0.csv`](./benches/compare/results/criterion_0.9.2_vs_0.10.0.csv).

**Not compared**: `match_order`'s inner hot path itself (the sweep, FIFO
consumption, atomic counters) has an *identical public signature* on both
versions and is exercised by every `matching/*` and `fok_depth/*` row above,
so those rows ARE the hot-path comparison. The main crate's own Criterion
`Concurrent`/`Contention Patterns` groups (`benches/concurrent/`) were run
to completion on 0.10.0 (see "Current absolute results" below) but could
NOT be run to completion on 0.9.2: this was actually attempted (a copy of
the published 0.9.2 source from the local crates.io registry cache, built
and run standalone, filtered to `-- "Concurrent"`), and it reproduced the
documented hang live — a worker thread panicked
(`add_order should succeed: Duplicate order id: ...`) and the process then
sat blocked for 3+ minutes at near-zero CPU before being killed by hand;
see "Notable behaviour differences" below for the exact mechanism and the
fix commits already present in this repo's own history. `benches/compare/`'s
own `contention_compare` binary (not Criterion, see its doc comment) is the
only 0.9.2-vs-0.10 contention comparison this document reports numbers for.

## System information

| | |
|---|---|
| Model Name | Mac Studio |
| Model Identifier | `Mac17,14` |
| Chip | Apple M5 Max |
| Total Number of Cores | 18 (6 Super and 12 Performance) |
| Memory | 128 GB |
| OS | macOS 27.0 (build 26A428), Darwin 27.0.0, arm64 |
| rustc | 1.98.1 (`48a229cea`, 2026-09-01), LLVM 22.1.8, host `aarch64-apple-darwin` |
| cargo | 1.98.1 (`797e8a9bc`, 2026-08-05) |
| Toolchain | `stable` (`rust-toolchain.toml`) |
| Build profile | Cargo's default `bench` profile (inherits `release`: `opt-level = 3`, `debug = false`, `lto = false`, `codegen-units = 16`, `panic = "unwind"`) — neither the root `Cargo.toml` nor `benches/compare/Cargo.toml` overrides `[profile.bench]`, so both sides of every comparison compile under the same settings |
| Allocator | system allocator (`std::alloc::System`); the allocation-count pass installs a **counting wrapper** around it (`benches/compare/src/alloc_compare.rs`), disabled during timing |
| CPU pinning | none (macOS exposes no thread-affinity API; same caveat `BENCH.md` documents) |
| Power | `pmset -g batt`: "Now drawing from 'AC Power'" (not on battery, no throttling from that source) |
| Load average during runs | see "Interleaving, rounds, and the noise policy" below for the full per-run `uptime` table |
| Date | 2026-09-27 |
| Commits compared | 0.9.2 = `aaafd39a503a7c88e01dd9266c4fb0f096ed8215` (crates.io `pricelevel = "=0.9.2"`, tag `v0.9.2`); 0.10.0 = `3b05d815ea5f84859f8b77f99ed843418d29dc9b` (local path dependency, `../..` from `benches/compare/` — the commit `src/` was built from; this document's own harness/results/doc commits on top of it do not touch `src/`, so the compiled crate is unchanged) |

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

Protocol: `old, new` repeated 3 times (6 runs total), `uptime` recorded
immediately before and after each run, and the **median of the 3 rounds'
point estimates** is the reported number per scenario. A scenario is
flagged **noisy** (not claimed as a real difference) when the 3 rounds'
`%change` either disagree in sign (a round counts as positive only above
+2%, negative only below -2%, so a near-zero result is never treated as a
"flip") or the spread (`max round %change − min round %change`) exceeds 10
percentage points.

Every Criterion group in `benches/compare/benches/compare.rs` runs at
Criterion's own default sample size and timing (100 samples, 3s warm-up, 5s
measurement per benchmark; see `thorough()` in that file) — an earlier
revision of this document used a short `sample_size(10)` / sub-second
window to fit inside a time-boxed session, and reported fewer noisy
scenarios as a direct consequence of that shortcut. Rerunning at full
Criterion defaults with a third round (this section) found MORE noisy
scenarios (10, versus 7 at the shorter setting), not fewer — a useful
result in its own right: several small-magnitude regressions that looked
"consistent" at `sample_size(10)` did not hold up under more samples and a
third round. This is disclosed rather than hidden.

Per-run host load (`uptime`'s 1/5/15-minute load averages), in run order:

| Run | Time | Load (1m / 5m / 15m) |
|---|---|---|
| old round 1, before | 20:21 | 5.88 / 7.16 / 6.92 |
| old round 1, after | 20:27 | 5.40 / 5.71 / 6.28 |
| new round 1, before | 20:27 | 5.38 / 5.70 / 6.26 |
| new round 1, after | 20:33 | 10.23 / 6.97 / 6.47 |
| old round 2, before | 20:33 | 8.64 / 6.81 / 6.42 |
| old round 2, after | 20:39 | 5.28 / 6.09 / 6.28 |
| new round 2, before | 20:40 | 5.01 / 6.00 / 6.25 |
| new round 2, after | 20:46 | 5.96 / 5.19 / 5.70 |
| old round 3, before | 20:46 | 5.07 / 5.03 / 5.63 |
| old round 3, after | 20:52 | 4.89 / 4.69 / 5.23 |
| new round 3, before | 20:52 | 4.30 / 4.57 / 5.18 |
| new round 3, after | 20:58 | 3.86 / 3.92 / 4.62 |

Load ranged 3.86–10.23 (1-minute average) across the whole run, shared with
desktop processes throughout (matching `BENCH.md`'s own disclosed
environment). Total wall-clock time for the 6 rounds: 20:21 to 20:58, about
37 minutes (each round takes roughly 6 minutes at Criterion's default
sample size). No speedup smaller than the observed round-to-round spread is
claimed as real — see the noisy list in the Summary above and the full
per-round numbers in the Comparison tables section.

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

Full data (3 interleaved rounds — `old, new` x3 — both versions, every
round's individual point estimate) is in
[`benches/compare/results/criterion_0.9.2_vs_0.10.0.csv`](./benches/compare/results/criterion_0.9.2_vs_0.10.0.csv).
Every scenario ran at Criterion's own default sample size (100 samples, 3s
warm-up, 5s measurement). `%change` = (new − old) / old, computed from the
median of the 3 rounds; `spread` is `max(round %change) − min(round
%change)` across the 3 rounds; **noisy** = spread > 10 percentage points OR
the rounds disagree in sign (a round's own `%change` counts as positive
only above +2%, negative only below −2%, to avoid flagging true near-zero
results as a "flip").

| Scenario | 0.9.2 median (ns) | 0.10.0 median (ns) | Change | Spread (pp) | Noisy? |
|---|---|---|---|---|---|
| `fok_depth/success_depth_1` | 1,501 | 877 | **-41.6%** | 9.97 | no |
| `fok_depth/success_depth_100` | 6,610 | 3,644 | **-44.9%** | 5.53 | no |
| `fok_depth/success_depth_10000` | 537,014 | 212,713 | **-60.4%** | 2.54 | no |
| `fok_depth/reject_depth_100` | 6,855 | 6,700 | -2.3% | 0.71 | no |
| `fok_depth/reject_depth_10000` | 585,606 | 426,352 | **-27.2%** | 0.92 | no |
| `fok_depth/replenish_depth_10000` | 2,174,180 | 2,124,062 | -2.3% | 2.13 | no |
| `snapshot/package_depth_100` | 87,984 | 20,642 | **-76.5%** | 0.15 | no |
| `snapshot/package_depth_10000` | 8,517,287 | 1,877,464 | **-78.0%** | 1.47 | no |
| `snapshot/to_json_depth_100` | 167,428 | 35,096 | **-79.0%** | 0.40 | no |
| `snapshot/to_json_depth_10000` | 16,313,478 | 3,230,401 | **-80.2%** | 0.75 | no |
| `snapshot/restore_depth_100` | 121,270 | 54,465 | **-55.1%** | 0.60 | no |
| `snapshot/restore_depth_10000` | 12,013,458 | 5,468,333 | **-54.5%** | 3.78 | no |
| `snapshot/capture_depth_100` | 3,192 | 3,329 | +4.3% | 5.52 | no |
| `snapshot/capture_depth_10000` | 188,411 | 183,569 | -2.6% | 8.34 | no |
| `trade_list_parse/n_32` | 18,005 | 11,077 | **-38.5%** | 1.89 | no |
| `trade_list_parse/n_1024` | 562,418 | 346,993 | **-38.3%** | 3.35 | no |
| `match_result_analytics/n_4096` | 12,433 | 10,666 | **-14.2%** | 2.96 | no |
| `matching/standard_full` | 848 | 799 | -5.8% | 5.01 | no |
| `matching/reserve` | 863 | 800 | -7.3% | 9.11 | no |
| `matching/mixed_100` | 22,922 | 23,962 | +4.5% | 2.96 | no |
| `matching/iceberg` | 822 | 891 | +8.4% | 9.96 | no (borderline: just under the 10pp threshold) |
| `matching/partial_fill_churn_10x10` | 2,533 | 2,745 | +8.4% | 6.93 | no |
| `add_orders_batch_100/standard` | 8,982 | 9,219 | +2.6% | 1.19 | no |
| `add_orders_batch_100/iceberg` | 9,230 | 9,249 | +0.2% | 4.92 | no |
| `isolated_updates/update_quantity_increase` | 8,985 | 8,340 | -7.2% | 7.45 | no |
| `isolated_updates/replace_diff_price` | 8,638 | 8,575 | -0.7% | 8.06 | no |
| `iter_orders/depth_10000` | 88,893 | 89,461 | +0.6% | 3.79 | no |

**Noisy rows** (round-to-round sign disagreement or >10pp spread; excluded
from the table above — see the CSV for every round's raw number):

| Scenario | 0.9.2 median (ns) | 0.10.0 median (ns) | Median change | Spread (pp) | Round %changes |
|---|---|---|---|---|---|
| `add_orders_batch_100/reserve` | 9,338 | 9,323 | -0.2% | 7.80 | -3.23 / +0.75 / +4.58 |
| `isolated_updates/cancel` | 9,130 | 8,463 | -7.3% | 25.30 | -15.70 / -7.44 / +9.60 |
| `isolated_updates/replace_same_price` | 8,721 | 8,632 | -1.0% | 13.51 | +2.76 / -10.74 / -0.97 |
| `isolated_updates/update_quantity_decrease` | 8,759 | 8,238 | -6.0% | 13.77 | -6.85 / -15.25 / -1.47 |
| `iter_orders/depth_100` | 2,289 | 2,357 | +3.0% | 12.48 | +10.77 / -1.70 / +2.93 |
| `match_result_analytics/n_256` | 534 | 514 | -3.8% | 9.00 | +2.06 / -3.75 / -6.94 |
| `matching/ioc_partial` | 854 | 913 | +6.9% | 38.44 | +4.33 / -4.70 / +33.75 |
| `matching/partial_fill_reinsert` | 948 | 1,015 | +7.1% | 21.49 | +17.59 / +5.95 / -3.90 |
| `matching/post_only_reject` | 1,054 | 1,076 | +2.1% | 11.01 | -3.62 / -6.64 / +4.36 |
| `matching/sweep_100_makers` | 18,716 | 19,902 | +6.3% | 13.02 | +15.30 / +2.28 / +2.71 |

`matching/ioc_partial`'s round 3 (+33.75%) is the most striking single
outlier in the whole dataset and is almost certainly host noise (the other
two rounds are -4.70% and +4.33%) — a reminder that even Criterion's own
default sample size does not fully tame a shared, loaded host at
sub-microsecond scale.

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
above caveats, not a strict per-call proof). **Every row's writer sample
count is reported explicitly** — a percentile computed from a handful of
samples (see the depth-10,000 FOK row) is not comparable in statistical
weight to one computed from thousands, and this table does not compute a
ratio between the two:

| Matcher | Depth | Version | Matcher ops/s | Writer samples | Writer p50 (ns) | Writer p99.9 (ns) |
|---|---|---|---|---|---|---|
| GTC | 100 | 0.9.2 | 441,940 | 1,348 | 2,333 | 8,167 |
| GTC | 100 | 0.10.0 | 436,872 | 1,097 | 1,666 | 557,375 (outlier — see below) |
| FOK | 100 | 0.9.2 | 72,136 | 4,149 | 1,125 | 103,709 |
| FOK | 100 | 0.10.0 | 27,423 | 13,341 | 2,083 | 40,792 |
| GTC | 10,000 | 0.9.2 | 562,561 | 1,178 | 2,084 | 7,875 |
| GTC | 10,000 | 0.10.0 | 363,978 | 1,216 | 3,209 | 9,875 |
| FOK | 10,000 | 0.9.2 | 3,151 | **7** | 18,505,042 | 158,699,000 |
| FOK | 10,000 | 0.10.0 | 37,388 | 11,488 | 1,959 | 27,959 |

**Reading the depth-10,000 FOK row honestly**: 0.9.2's writer row has only
**7 completed samples** in the whole 158.7ms window — 0.9.2's FOK dry run
walked and sorted the whole 10,000-order level under its exclusive guard on
every match, so the writers were almost fully starved for the run's entire
duration; a p50/p99.9 computed from 7 samples is not a stable percentile
estimate, just a description of "almost nothing got through". 0.10's
bounded dry run (issue #143) leaves the guard held for long enough that
11,488 writer ops got through in a shorter (13.4ms) window, at a p99.9 of
28µs. This is qualitatively the same starvation-vs-not effect
`CHANGELOG.md` and `BENCH.md`'s own `fok_depth` contention section
document, reproduced independently here against the actual 0.9.2 release
rather than against 0.10's own pre-#143 history — but the numeric ratio
between "7 samples" and "11,488 samples" is not reported as a multiplier
because n=7 is too small to support one.

The GTC-at-depth-100, 0.10.0 p99.9 (557,375 ns) is a single-run outlier at
n=1,097 samples, roughly two orders of magnitude above every other GTC row
in this table (which sit at 4,125–9,875 ns) and above 0.10's OWN
depth-10,000 GTC row measured in the same run. It is most plausibly one
scheduling hiccup on a shared host (see Methodology's noise policy) rather
than a real depth-100-specific effect; it was not re-run to confirm, since
this contention comparison is explicitly a single-run, directional pass
(see the note above the table).

## Current absolute results (0.10.0)

Both suites below ran to completion at their own DEFAULT settings — no
reduced sample counts, no time-boxing, no filtering to a subset of groups.

### Criterion (`cargo bench --bench benches`)

Full run, Criterion's own defaults (100 samples, 3s warm-up, 5s
measurement per benchmark), **197 benchmark functions across all 20
groups** including `Concurrent Operations` and `Contention Patterns` (both
completed with no hang — see "Notable behaviour differences" for why that
is worth stating explicitly for 0.9.2's equivalent). Wall-clock: 20:59 to
21:30 (about 31 minutes). Host load during the run (`uptime` before/after):
`3.44 3.79 4.53` → `10.90 7.14 5.36`. Selected results (ns/iter, single run
— not interleaved, since this section reports 0.10's own absolute numbers,
not a comparison):

| Benchmark | Result |
|---|---|
| `Add Orders/add_standard_order` | 9,307 ns |
| `Match Orders/match_standard_orders` | 9,958 ns |
| `Match Orders/match_mixed_orders` | 12,870 ns |
| `Update Orders/cancel_order` | 13,071 ns |
| `Snapshot Recovery/snapshot_full_roundtrip` | 129,598 ns |
| `FOK depth/fok_rejected/10000` | 167,118 ns |
| `FOK depth/fok_first_maker/10000` | 276 ns |
| `MatchResult - Analytics/build/4096` | 29,869 ns |
| `MatchResult - Analytics/repeated_all_x8/4096` | 82,522 ns |
| `MatchResult capacity (#148)/iceberg_5x` | 1,381 ns |
| `Trade Id Emission/sweep_100_trades_gtc` | 17,832 ns |
| `Lifecycle/full_lifecycle` | 12,339 ns |
| `Special Order Matching/match_special_mixed_scaling/500` | 51,544 ns |
| `Concurrent Operations/concurrent_add_standard_orders/2` (2 threads) | 491 ns (thread-count sweep 2/4/8/16 all ran; this is the 2-thread point) |
| `Concurrent Operations/concurrent_cancel_orders/16` (16 threads) | 4,175 ns |
| `Contention Patterns/read_write_ratio/95` (95% reads) | 43,769 ns |
| `Contention Patterns/hot_spot_contention/100` | 1,803 ns |

Every group that ran, in registration order: `Data Operations`,
`PriceLevel - Add Orders`, `PriceLevel - Iter Orders`, `PriceLevel - Match
Orders`, `PriceLevel - Update Orders`, `PriceLevel - Mixed Operations`,
`PriceLevel - Snapshot Recovery`, `PriceLevel - Checked Arithmetic`,
`PriceLevel - Serialization`, `PriceLevel - Newtypes`, `PriceLevel -
Special Order Matching`, `PriceLevel - Lifecycle`, `PriceLevel - Trade Id
Emission`, `PriceLevel - FOK depth`, `MatchResult - Analytics`,
`MatchResult capacity (#148)`, `Residual allocation reuse (#147)`,
`PriceLevel - TradeList parse`, `PriceLevel - Concurrent Operations`,
`PriceLevel - Contention Patterns`. Reproduce with `make bench` (`cargo
criterion --bench benches`) or `cargo bench --bench benches` directly.

### Latency harness (`make bench-latency`)

Full run at the harness's true defaults (`PL_LATENCY_SAMPLES=20000
PL_LATENCY_WARMUP=2000`, every other knob at its documented default —
`benches/latency/config.rs`). This completed in about 4 minutes, not the
hour-plus this document's earlier revision estimated: `restore_sizes` and
`snap_sizes` internally cap their OWN sample count per depth tier (`n=50`
at depth 100,000, `n=500` at depth 10,000, the full 20,000 only at depth
100), independently of `PL_LATENCY_SAMPLES` — the earlier estimate wrongly
assumed the 100,000-depth tier ran at the full 20,000 samples too, and this
revision corrects that. **124 scenarios**, all at documented default
sample/warm-up counts. Host load (`uptime` before/after): `4.27 5.86 5.13`
→ `13.29 7.48 5.77` (the 1-minute jump reflects this session's OTHER
background work landing around the same time, not the latency harness
itself, which is single-process and mostly single-threaded outside its
`contention`/`stats_contention` categories).

p99.99 carries `BENCH.md`'s own [exploratory caveat](./BENCH.md#the-p9999-caveat)
regardless of sample count and is omitted here for brevity.

| Scenario | Depth | Samples | p50 (ns) | p99 (ns) | p99.9 (ns) |
|---|---|---|---|---|---|
| `isolated_add_gtc` | 1,000 | 20,000 | 83 | 667 | 1,000 |
| `isolated_cancel_success` | 22,000 | 20,000 | 83 | 625 | 709 |
| `match_full` | 1 | 20,000 | 208 | 625 | 1,166 |
| `match_maker_partial` | 1,000 | 20,000 | 250 | 1,208 | 9,916 |
| `many_fill_sweep` | 50,000 | 2,000 | 4,125 | 6,125 | 16,125 |
| `tif_fok_success` | 1 | 20,000 | 250 | 666 | 709 |
| `tif_fok_reject` | 1 | 20,000 | 83 | 84 | 167 |
| `iteration` | 1,000 | 20,000 | 5,459 | 6,166 | 7,583 |
| `snapshot_capture` | 1,000 | 20,000 | 10,750 | 12,709 | 24,500 |
| `checksum_validate` | 1,000 | 20,000 | 166,333 | 189,083 | 206,292 |
| `restore` | 1,000 | 20,000 | 523,917 | 585,041 | 629,166 |
| `from_snapshot_valid@100000` | 100,000 | **50** | 10,980,666 | 11,305,000 | 11,305,000 |
| `from_json_valid@100000` | 100,000 | **50** | 55,370,417 | 58,392,625 | 58,392,625 |
| `snapshot_package_legacy@100000` | 100,000 | **50** | 22,206,416 | 23,508,125 | 23,508,125 |
| `snapshot_package_stream@100000` | 100,000 | **50** | 18,676,500 | 20,227,167 | 20,227,167 |
| `fok_first_maker@10000` | 10,000 | 5,000 | 292 | 875 | 1,625 |
| `fok_rejected@10000` | 10,000 | 5,000 | 165,083 | 188,500 | 200,375 |
| `fok_replenish@10000` | 10,000 | 5,000 | 625 | 1,500 | 1,792 |
| `writer_add_during_fok_rejected@10000` | 10,000 | 5,000 | 125 | 184,667 | 208,167 |
| `writer_add_during_gtc@10000` | 10,000 | 5,000 | 458 | 750 | 1,459 |
| `contention_gtc_matcher` | 1 | 5,000 | 625 | 1,458 | 3,334 (matcher 448,437 ops/s; writers 6,906,265 ops/s) |
| `contention_fok_matcher` | 1 | 5,000 | 25,500 | 161,541 | 272,333 (matcher 26,193 ops/s; writers 7,617,031 ops/s) |
| `statsc_ok_single` | 23,000 | 20,000 | 250 | 750 | 1,083 |
| `statsc_ok_same_mixed` | 23,000 | 20,000 | 625 | 2,833 | 9,500 |
| `statsc_overflow_same_mixed` | 23,000 | 20,000 | 666 | 2,125 | 9,333 |
| `statsc_raw_overflow_same_producers` (per 16) | 0 | 20,000 | 1,125 | 1,040,250 | 24,699,333 |

Allocation pass (2,000 reps/op, the harness default):

| Operation | alloc_count/op | alloc_bytes/op |
|---|---|---|
| `add_order` | 2.10 | 381.60 |
| `match_full` | 2.02 | 1,799.44 |
| `match_sweep_100` | 8.20 | 22,733.09 |
| `snapshot_capture` | 130.00 | 27,072.00 |
| `checksum_validate` | 1.00 | 64.00 |
| `restore` | 3,334.39 | 468,214.97 |

`contention_fok_matcher`'s p50 here (25,500 ns) is roughly 41x
`contention_gtc_matcher`'s (625 ns) at the same load — consistent with
`BENCH.md`'s own documented "GTC-vs-FOK under identical load" finding
(there: ~42x on its own 300-sample example run). `fok_rejected@10000`'s cost
(p50 165 µs) vs `fok_first_maker@10000` (p50 292 ns) is the depth-scaling
effect `BENCH.md`'s "Fill-or-kill feasibility depth" section documents in
full. `statsc_raw_overflow_same_producers`'s p99.9 (24.7 ms, batched per 16
calls) reproduces the false-sharing / contended-atomic-starvation finding
`BENCH.md`'s "Statistics cache contention" section documents — this is the
one row in the whole default run with a tail 4+ orders of magnitude above
its own p50, and it is the SAME documented finding, not new host noise.

Reproduce with `make bench-latency`; the full default run takes about 4
minutes on this host.

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
- **0.9.2's own `benches/concurrent/register.rs` has a documented hang, not
  present in 0.10 — confirmed live, not just cited.** This repo's own
  history fixed it on this same branch's ancestry: commits `c2746d2`
  ("repair concurrent add/match/cancel/mixed-ops workload correctness"),
  `a061922` ("repair contention workloads and register them in
  criterion_main") and `3f088e9` ("fixed book depth, real read/write mix,
  declared throughput unit"), merged via `a473ff7` (issue #141). The
  mechanism (per `c2746d2`'s own commit message, which names issue #160): a
  duplicate-id collision inside a worker thread panics that thread's
  `.expect()` call on `add_order`'s result; the other worker threads (and
  the coordinating main thread) are all waiting on a shared
  `std::sync::Barrier` that requires every thread to call `wait()` once, and
  the panicked thread never reaches its own `wait()` call, so every other
  thread blocks forever. This document did not stop at citing that history:
  a copy of the actual published 0.9.2 source (from
  `~/.cargo/registry/src/index.crates.io-*/pricelevel-0.9.2/`, which still
  ships its own pre-fix `benches/` tree — 0.9.2's own `Cargo.toml` `include`
  list had no `benches/compare`-style carve-out) was built standalone and
  run with `cargo bench --bench benches -- "Concurrent"`. It reproduced the
  panic verbatim: `thread '<unnamed>' panicked at
  benches/concurrent/register.rs:31:34: add_order should succeed: Duplicate
  order id: 00000000-000f-4240-0000-000000000000`. The process then sat
  blocked (`ps` state `S`, elapsed time climbing, CPU time frozen at ~2
  seconds) for over 3 minutes before being killed by hand — a live,
  first-hand reproduction of the documented hang, not an inference from
  reading the fix commit's description. This is why "Current absolute
  results" below runs the `Concurrent`/`Contention Patterns` groups only
  against 0.10 (where the fix already landed).

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
`contention_0.9.2_vs_0.10.0.csv`, and the six raw `round{1,2,3}_{0.9.2,0.10.0}.txt`
bencher-format outputs the CSV was built from).

For the latency harness's own methodology, tail-latency numbers, allocation
measurements and the individual performance investigations (issues #140,
#143, #148, #149, #150, #154, #155) behind specific 0.10 changes, see
[`BENCH.md`](./BENCH.md).
