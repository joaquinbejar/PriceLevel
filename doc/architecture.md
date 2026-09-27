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
   checksum. It is not a linearizable point-in-time view under concurrent
   same-side resizes (see #162).

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
| `fok_guard: std::sync::RwLock<()>`, one per level | Blocking reader-writer lock |

| Public method | Locks |
|---------------|-------|
| `match_order`, `Gtc` / `Ioc` / `Gtd` / `Day` | Shard write lock of each maker entry it fills, one at a time (`OrderQueue::match_front`); no level-wide guard |
| `match_order`, `Fok` | `fok_guard` exclusive across the `O(depth)` dry-run and sweep, plus the per-maker shard write locks |
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
  block for an `O(depth)` section (issue #112). The other time-in-force paths
  skip this guard but still take the per-maker shard lock.
- **Side topology.** The resting side and count are pinned in one atomic word,
  so single-side coherence holds under arbitrary concurrent admissions and
  removals (issue #126).

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
