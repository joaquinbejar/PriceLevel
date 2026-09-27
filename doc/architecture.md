# pricelevel architecture

`pricelevel` implements one price level of a limit order book. A `PriceLevel`
owns every order resting at a single price, matches an incoming taker against
that queue in price-time (FIFO) order, keeps visible / hidden quantity
counters, records execution statistics and round-trips through
checksum-protected snapshots. An order book composes many levels; this crate
is not a full book. The crate is synchronous: no async runtime, no networking,
no feature flags.

## Modules

| Module | Role | May depend on |
|--------|------|---------------|
| `src/errors/` | `PriceLevelError` (hand-written `Display` / `Debug` / `Error`) | `std`, `serde` |
| `src/utils/` | `Price`, `Quantity`, `TimestampMs`, `Id`, `Hash32`, `UuidGenerator`, `setup_logger` | `errors` |
| `src/orders/` | `OrderType<T>`, `TimeInForce`, `OrderStatus`, `OrderUpdate`, pegging | `utils`, `errors` |
| `src/execution/` | `MatchResult`, `Trade`, `TradeList` | `orders`, `utils`, `errors` |
| `src/price_level/` | `PriceLevel`, `OrderQueue`, snapshots, statistics | everything above |
| `src/lib.rs`, `src/prelude.rs` | Public re-exports; nothing in `src/` imports from them | |

## Data flow

1. `PriceLevel::add_order` validates the order, reserves the atomic counters
   and publishes it into `OrderQueue` (id-keyed `DashMap` storage plus a
   `SkipMap` index keyed by a monotonic insertion sequence).
2. `PriceLevel::match_order` walks the index from the front. For each maker it
   decides a full consume, an in-place partial fill (the residual keeps its
   sequence, so its time priority) or an iceberg / reserve replenish (the new
   tranche is re-sequenced at the tail), emits a `Trade`, and updates the
   counters and statistics. The result is a `MatchResult`.
3. `PriceLevel::update_order` cancels, resizes or replaces an order in place.
4. `PriceLevel::snapshot` materializes the orders and recomputes the
   aggregates from them; `PriceLevelSnapshotPackage` wraps it with a SHA-256
   checksum. It is fallible (#162): a collected set whose totals overflow or
   whose sides are inconsistent is discarded and recollected, up to 8 attempts,
   then a typed error is returned, so the stored aggregates always agree with
   the collected orders. It is still not a linearizable point-in-time view
   under concurrent same-side resizes.

## Concurrency model

The authoritative, user-facing version of this section is the "Concurrency
Model" section of the crate docs in `src/lib.rs` (rendered into `README.md`).

### Components versus methods

"Lock-free" is a property of individual components, not of the complete
public methods:

| Component | Progress |
|-----------|----------|
| Ordered index: `crossbeam-skiplist` `SkipMap<u64, Id>` | Lock-free |
| Quantity, count, topology and most statistics counters (`std` atomics) | Lock-free |
| `value_executed` accumulator (`portable_atomic::AtomicU128`) | Lock-free with a native 128-bit CAS (aarch64; x86_64 with `cmpxchg16b`); a global lock for this one counter elsewhere |
| Order storage: `dashmap::DashMap<Id, (u64, Arc<OrderType<()>>)>` | Sharded reader-writer locks |
| `fok_guard: FokGuard` (a `std::sync::RwLock<()>` plus a waiting-mutator counter), one per level | Blocking reader-writer lock with a bounded hand-off to waiting mutators (#206) |

| Public method | Locks |
|---------------|-------|
| `match_order`, `Gtc` / `Ioc` / `Gtd` / `Day` | Shard write lock of each maker entry it fills, one at a time (`OrderQueue::match_front`); no level-wide guard |
| `match_order`, `Fok` | `fok_guard` exclusive across the dry-run and sweep (bounded by the makers the fill visits within the lazy budget, `O(depth log depth)` past it; #143), after a bounded hand-off to announced mutators (#206), plus the per-maker shard write locks |
| `match_order`, post-only taker | No sweep; the depth scan iterates storage under shard read locks |
| `add_order` | `fok_guard` shared, plus the new id's shard write lock |
| `update_order` (all variants) | `fok_guard` shared, plus the target id's shard write lock |
| `snapshot` | `fok_guard` shared, plus shard read locks while materializing |
| Counter / statistics accessors | Atomic loads only (advisory, eventually consistent) |

### Supported execution model

- **One logical matcher per level.** Concurrent `match_order` calls on the
  same level are the caller's responsibility to serialize.
- **Mutators and readers may run concurrently** from any number of threads,
  with each other and with the one matcher.
- **Serialization point.** A fill is committed while the maker's `DashMap`
  shard write lock is held; a cancel or resize of that order takes the same
  lock, so a racing cancel either fully wins or fully loses (issue #81).
  Operations on other orders in the same shard wait on that lock too.
- **Fill-or-kill exclusion.** A `Fok` match holds `fok_guard` exclusively for
  its dry-run and sweep, so admissions, updates and snapshots on that level
  block for that section (issue #112; bounded by #143). The other
  time-in-force paths skip this guard but still take the per-maker shard lock.
- **Fill-or-kill hand-off (issue #206).** `std::sync::RwLock` promises no
  fairness, and a matcher looping `Fok` calls on one level retook the
  exclusive side before the mutators it had just woken could run. A mutator
  whose shared acquisition would block now announces itself on a per-level
  counter (`src/price_level/fok_guard.rs`); a `Fok` match that sees an
  announcement waits, holding no lock, until every announced mutator holds
  the shared side or a fixed budget (64 spin hints, then 256 `yield_now`
  calls) runs out, and then calls `write()` regardless. This is a bounded
  number of hand-off attempts with measured latency improvements, not a
  fairness guarantee: it does not establish starvation freedom for either
  side, because `RwLock` acquisition priority is unspecified in Rust, and
  total lock-acquisition delay remains scheduler-dependent and unbounded.
  The budget counts rounds, not time, so under oversubscription a hand-off
  can take hundreds of milliseconds. Typical case only (one matcher per
  level as supported, a writer-preferring or queue-fair lock, a mutator
  scheduled within the budget): a blocked mutator waits for at most two
  sections plus its wake-up, the one in progress and one more if the
  matcher rechecks between the mutator's failed `try_read` and its
  announcement; with `k` concurrent matchers (unsupported) about `k`. The
  counter is a scheduling hint only: exclusion and all-or-nothing still come
  from the lock alone. `tests/loom/fok_handoff.rs` model checks the
  production `fok_guard.rs` against loom for exclusion, termination of the
  hand-off loop and a conditional property (a drained hand-off admits the
  announced mutator first); it does not prove starvation freedom (see that
  file for the model's limits). A section is still the unit of wait: a
  `Fok` that must walk the level (a kill, or a fill deep into the queue)
  holds it for `O(depth log depth)`, so a caller that needs tight mutator
  latency on a deep level should not loop such takers on it from a hot
  thread. Measurements: `BENCH.md`, "Bounded hand-off to waiting mutators".
- **Side topology.** The resting side and count are pinned in one atomic word,
  so single-side coherence holds under arbitrary concurrent admissions and
  removals (issue #126).
- **Statistics: single writer (issue #153).** `PriceLevelStatistics` supports
  exactly one concurrent writer of the execution aggregates:
  `record_execution` is driven by the one logical matcher and `reset` /
  `reset_at` require quiescence. The `stats_seq` seqlock (issue #129) makes
  `Clone` (and so `snapshot`), serde and `Display` return a complete execution
  state from any number of reader threads under that contract, including
  across an overflow rollback. It is a reader protocol, not a writer lock:
  entry is an unconditional increment, so two overlapping recorders (or a
  reset during a record) are unsupported, can let a reader accept a partial
  tuple, and a reset racing a rollback can wrap a counter. The final totals
  of overlapping recorders are still arithmetically correct. The decision is
  to document this contract rather than pay for an exclusive writer entry on
  every fill. `record_order_added` / `record_order_removed` are single
  atomic increments outside the write section and may come from any thread.
  The supported schedule is model-checked in `tests/loom/stats_seqlock.rs`.

## Caller-supplied code

Generic payload impls, `map_extra_fields`, formatter destinations,
serializers, `iter_orders` loop bodies and the `tracing` subscriber are
external code. They must not panic, and the library does not recover if they
do. `doc/panic-boundaries.md` records, for each call, where it runs, whether a
guard is held and whether state has been partially mutated (#172). The
"Caller-Supplied Code" section of `src/lib.rs` is the user-facing summary.

## Performance evidence

No throughput or latency figures are currently published. The historical
tables printed by releases up to 0.9.x are withdrawn (no provenance, an
internal 237,347.51 versus "over 264,000" ops/s inconsistency, and a workload
of ten concurrent takers on one level); see "Performance Evidence" in
`src/lib.rs` for the reasons and the operation-accounting rules any future
result must follow. The Criterion benches under `benches/` are the supported
measurement tool.

## Examples

The `examples/` workspace member contains runnable demos. The concurrent ones
(`simple`, `hft_simulation`, `contention_test`) use exactly one matcher thread
per shared level, keep maker and taker ids disjoint, and report successful
operations separately from rejected and missing-order calls. They are smoke
demonstrations, not benchmarks.
