# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed (breaking)

- **Random `Id` constructors are fallible (#167).** `Id::new()`,
  `Id::new_uuid()`, `Id::new_ulid()` and the random `impl Default for Id` are
  removed: they could panic inside `uuid` / `ulid` / `rand` on an OS entropy or
  RNG (re)seed failure. They are replaced by `Id::try_new(&clock, &mut entropy)`
  (ULID), `Id::try_new_ulid(&clock, &mut entropy)`,
  `Id::try_new_ulid_at(timestamp, &mut entropy)` and
  `Id::try_new_uuid(&mut entropy)`, all returning
  `Result<Id, PriceLevelError>`. Entropy comes from a caller-supplied
  `EntropySource` (new trait; implementations must not panic) and ULID time
  from a caller-supplied `UnixClock` (new trait; implementations must not
  panic) or an explicit `TimestampMs`. The crate provides no clock reader,
  because `SystemTime::now` can panic inside `std`. No dependency was added. A
  timestamp above `Id::ULID_MAX_TIMESTAMP_MS` (48 bits) is a typed error
  instead of being masked. UUID v4 version/variant bits and the ULID layout
  are unchanged.

- **`Id::from_str` disambiguates by shape (#178).** The parser (and therefore
  `Id`'s serde `Deserialize`, which goes through it) now tries a 26-character
  ULID first, then any UUID text form, and only then a decimal `u64`.
  Previously `u64` came first, so an all-digit ULID such as the nil ULID
  `00000000000000000000000000` parsed as `Sequential` and `Display` →
  `FromStr` was not an identity. Now `id.to_string().parse::<Id>() == Ok(id)`
  for every `Id`. Inputs that change meaning are non-canonical
  (zero-padded) sequential spellings: 26-character all-digit texts whose
  decimal value is at most `u64::MAX` (now `Ulid`, were `Sequential`),
  32-digit simple-form texts whose decimal value is at most `u64::MAX` (now
  `Uuid`, were `Sequential`), and 26-character texts starting with `8`-`9` / a letter,
  which overflow 128 bits and used to wrap silently to a different ULID (now
  `ParseError`). Canonical `Sequential` text (at most 20 digits) and other
  non-canonical decimals (`"007"`, `"+42"`) parse exactly as before.

- **Trade and statistics clock reads are caller-supplied and fallible
  (#171).** The crate no longer reads the wall clock, narrows
  `Duration::as_millis()` with `as`, or substitutes `0` for a pre-epoch
  clock.
  - `Trade::new(..)` is removed; use
    `Trade::try_new(.., &clock) -> Result<Trade, PriceLevelError>` (reads a
    caller-supplied `UnixClock` once) or the unchanged infallible
    `Trade::with_timestamp`.
  - `PriceLevelStatistics::reset()` becomes
    `reset(&clock) -> Result<(), PriceLevelError>`; the clock is read before
    anything is mutated, so a failure leaves counters, timestamps and the
    degraded flag unchanged. New infallible `reset_at(TimestampMs)`.
  - `PriceLevelStatistics::time_since_last_execution()` becomes
    `time_since_last_execution(&clock) -> Result<Option<u64>, PriceLevelError>`
    plus `time_since_last_execution_at(TimestampMs)`. `Ok(None)` means no
    execution; a clock failure is `Err`, and a `now` before the last execution
    is `InvalidOperation` (previously `None`).
  - New `PriceLevelStatistics::new_at(TimestampMs)` and
    `PriceLevelStatistics::try_new(&clock)`. `PriceLevelStatistics` is now
    re-exported at the crate root so they are nameable.
  - Behavior change: `PriceLevelStatistics::new()` / `Default`,
    `PriceLevel::new`, `PriceLevelSnapshot::new` / `with_orders` / `from_str`
    and a snapshot omitting `statistics` are deterministic and clock-free:
    `first_arrival_time()` starts at `0` (unstamped) instead of the wall clock.
    Identical input now yields identical snapshot checksums.
  - A serialized statistics object omitting `first_arrival_time` decodes it
    as `0` (unstamped) instead of the restore instant. Packages written by the
    crate always carry the field, so v2, v3 and v4 wire compatibility and
    checksum validation are unchanged. `match_order` was already clock-free.
- **Text parsers reject unbalanced brackets and bound nesting (#174).**
  `TradeList::from_str` (and therefore the `trades=` section of
  `MatchResult::from_str`) now returns `InvalidFormat` when a `[` / `]` inside
  the list is unbalanced, and `PriceLevel::from_str` returns `ParseError` when
  a `(` / `)` / `[` inside the `orders=[...]` section is unbalanced. Such text
  was previously accepted only when the stray bracket sat in a field the
  element parser ignores (an unknown key, or a pair with a second `=`); it is
  never produced by `Display`. Segments are still split exactly as before and
  a malformed element is still reported first, so every other input keeps its
  previous outcome and error. Nesting deeper than 128 levels (the enclosing
  list bracket included) in `TradeList`, `MatchResult` or `PriceLevel` text is
  rejected with `ParseError` instead of growing an unchecked signed counter.
  An accepted/rejected corpus captured from the previous parsers pins every
  other outcome.
- **New `PriceLevelError::EntropyUnavailable { message }` variant (#167)**,
  the conventional error for a failing `EntropySource`. Exhaustive matches on
  `PriceLevelError` need a new arm.
- **Per-level `value_executed` statistic widened to `u128` (#140).**
  `PriceLevelStatistics::value_executed()` returns `u128` (was `u64`),
  matching `MatchResult::executed_value` and `Trade::total_value`. With both
  price and quantity fixed-point scaled, the `u64` accumulator overflowed under
  ordinary volume (1845 executions at 1.0 @ 1.0 with a 1e8 scale, or a single
  execution at a realistic price), after which the level's statistics were
  permanently degraded. Trades were never affected. The accumulator is a
  `portable_atomic::AtomicU128` (new dependency): lock-free on aarch64 and on
  x86_64 with `cmpxchg16b`, lock-based fallback elsewhere. A `u128` overflow is
  still rejected all-or-nothing.
- **Snapshot format v4.** New packages are written at v4, since a payload may
  carry a `value_executed` above `u64::MAX`. Pre-0.10 readers reject every v4
  package: with a version mismatch when the value fits in `u64`, or with a
  deserialization error (decoding runs before the version check) when it does
  not.
  v2, v3 and v4 packages all restore; legacy packages keep their original
  checksum. Pinned by v2 (0.8.4) and v3 (0.9.2) fixtures stored verbatim.
- **Fallible execution-result allocation and growth (#170).**
  `TradeList::with_capacity` and `MatchResult::with_capacity` (which panicked
  on capacity overflow, e.g. `usize::MAX`) are replaced by
  `TradeList::try_with_capacity` / `MatchResult::try_with_capacity` returning
  `Result<_, PriceLevelError>` (`Vec::new` + `try_reserve_exact`).
  `TradeList::add` and `MatchResult::add_filled_order_id` now return
  `Result<(), PriceLevelError>`. `MatchResult::add_trade` validates and
  reserves before committing remaining quantity, completion, outcome or the
  trade, so a failure leaves the result unchanged. The decode-time validator's
  duplicate-id set is reserved fallibly.
- **`MatchResult` carries a typed match failure (#164 contract, #170).** New
  `MatchResult::error() -> Option<&PriceLevelError>` and `is_failed()`.
  `PriceLevel::match_order` keeps returning `MatchResult`; when a step fails it
  stops the sweep and reports every committed trade and filled id, the true
  remaining quantity, and the error, with the level's queue, counters, side
  topology and statistics consistent with those trades. Storage for each step
  is reserved before the maker mutation. A fill-or-kill taker reserves the
  exact trade count (from its dry run) before touching any maker; on failure
  it is `Killed` with the error set and the level unchanged. The new `error`
  field round-trips through JSON and bincode; JSON without it decodes as "no
  error"; the text form does not carry it.
- **New `PriceLevelError::CapacityExceeded { resource, additional }` variant
  and `CapacityResource` enum (#170).** Fixed payload, so reporting an
  allocation failure never allocates. `PriceLevelError` now derives `Clone`,
  `PartialEq`, `Eq`, `Serialize` and `Deserialize`. Exhaustive matches need a
  new arm.
- **`PriceLevel::matchable_quantity` replays in sweep order (#170).** The dry
  run walks the queue by insertion sequence (the order `match_order` consumes
  it) instead of `(timestamp, sequence)`. This corrects the dry run where
  iceberg / reserve replenishment headroom depends on visit order: its total,
  and so a fill-or-kill verdict, can now differ from 0.9 and matches what the
  sweep executes.
- **Order matching arithmetic is fallible (#169).**
  `OrderType::match_against` now returns
  `Result<(u64, Option<Self>, u64, u64), PriceLevelError>` and
  `OrderType::refresh_iceberg` returns `Result<(Self, u64), PriceLevelError>`.
  Every quantity subtraction and addition in them is checked and fails as
  `PriceLevelError::InvalidOperation`. A reserve whose partial-fill
  replenishment overflows `u64` (reachable only for an order `add_order`
  rejects, e.g. visible = hidden = threshold = `u64::MAX`) returns that error
  instead of an unchanged-order "no progress" tuple; the input order is always
  unchanged. `PriceLevel::match_order` treats such an error under the #164
  contract: the sweep stops before mutating that maker and reports the
  committed prefix with `MatchResult::error` set, and a fill-or-kill taker is
  killed in its dry run with the error set and the level unchanged. Matching
  of every admitted order is unchanged. `DEFAULT_RESERVE_REPLENISH_AMOUNT`
  keeps type `NonZeroU64` and value `80`; it is now built without
  `unreachable!` (a zero literal is a compile-time trait-bound error).

- **`PriceLevel::snapshot` is fallible (#162).** It now returns
  `Result<PriceLevelSnapshot, PriceLevelError>`. The shard walk has no
  transaction over the whole level, so a same-side quantity transfer between
  two shards during the walk could capture orders whose visible or hidden sum
  overflows `u64` although every committed state fits. Debug builds then
  panicked on a `debug_assert!`; release builds stored the live atomic counter,
  an aggregate that disagreed with the snapshot's own orders. Both are gone: a
  walk that overflows, or that comes back mixed-side across a side transition
  (previously an unbounded retry), is recollected at most 8 times in total,
  after which the call returns `PriceLevelError::InvalidOperation` and leaves
  the level unchanged. A returned snapshot is coherent: its aggregates equal the
  checked sums over its own orders. It is still not a linearizable
  point-in-time view. `snapshot_package()` and `snapshot_to_json()` keep their
  signatures and propagate the new error. Snapshot format v4, checksums and
  restore order are unchanged for every successful snapshot.
- **`UuidGenerator::next` is replaced by the checked `try_next` (#168).**
  `try_next() -> Result<Uuid, PriceLevelError>` reserves the sequence value
  with a checked, allocation-free CAS instead of an unchecked `fetch_add`,
  which did not panic but wrapped `u64::MAX -> 0` and re-issued the
  counter-zero id (a duplicate-id defect reachable by deserializing a
  generator near the end of its range). `u64::MAX` is the exhaustion sentinel
  and is never issued; once reached, every request fails with
  `PriceLevelError::CapacityExceeded { resource: CapacityResource::IdSequence,
  .. }` forever. Issued ids are byte-identical to before (same namespace and
  decimal name). The serde form is unchanged; an exhausted generator
  round-trips as exhausted.
- **`match_order` reports trade-id exhaustion through `MatchResult::error`
  (#168).** Each trade id is reserved before its step's maker mutation, so an
  exhausted generator stops the sweep with the committed prefix, the true
  remainder and a consistent level. A fill-or-kill taker reserves its exact
  trade-id count before touching any maker and is `Killed` with the error set
  and the level unchanged when the generator cannot supply them.
- **Engine invariants are transactional typed failures (#163).** The
  resting-order count release is checked and validated before the queue
  removal it follows: a count that disagrees with the queue now makes a
  cancel / price-moving `update_order` return `InvalidOperation` with the
  level untouched, stops a non-fill-or-kill sweep with the committed prefix
  and `MatchResult::error`, and kills a fill-or-kill taker before its first
  mutation. It previously triggered a `debug_assert!` in debug builds and a
  silent skipped decrement in release. A failure after a committed removal,
  or a failed update-counter rollback, poisons the level (fail fast). A
  resize validates the decided order id before reserving level counters, so
  no partial reservation survives a rejection. The poisoned-level error text
  changed. On targets narrower than 64 bits, admission and restore cap the
  resting-order count at `usize::MAX` so `order_count()` is exact.

- **Internal counters refuse to wrap (#165).** New
  `PriceLevelError::CounterExhausted { counter }` variant and
  `ExhaustedCounter` enum (fixed payload, allocation-free; exhaustive matches
  need a new arm).
  - `PriceLevelStatistics::record_order_added()` /
    `record_order_removed()` now return `Result<(), PriceLevelError>`. At
    `usize::MAX` they keep the counter (it used to wrap to 0), set the sticky
    `stats_degraded` flag and return `CounterExhausted`. `add_order` /
    `update_order` still succeed in that case: the mutation has committed and
    the statistic is advisory; the first drop is logged at `WARN`.
  - `PriceLevelStatistics::reset_at(TimestampMs)` now returns
    `Result<(), PriceLevelError>`. The seqlock sequence reserves its exit on
    entry (a section opens only while the sequence is at most
    `u64::MAX - 2`), so the guard's exit can never wrap. A refused
    `record_execution` is dropped all-or-nothing and marks the statistics
    degraded; a refused `reset` / `reset_at` changes nothing.
  - FIFO sequences are reserved with a checked CAS before anything is
    committed. `add_order` and a quantity-increasing `update_order` return
    `CounterExhausted` with the level unchanged (a duplicate id still reports
    `DuplicateOrderId` first). An iceberg / reserve replenishment inside
    `match_order` that finds no sequence stops the sweep with the committed
    prefix and the error in `MatchResult::error` (#164 contract); a
    fill-or-kill taker whose dry run needs more replenishments than sequences
    remain is killed before any maker is touched.
  - Stop-cause precedence within one sweep step is fixed: `match_against`
    error (#169), then trade id (#168), then FIFO sequence, then visible
    headroom; every check runs before the step commits. Fill-or-kill checks,
    before touching any maker: dry-run arithmetic error, depth, sequence
    headroom, result storage, then the trade-id block.
  - The topology and mutation epochs are checked and stop at `u64::MAX`,
    which readers treat as "changed, unknown". `add_order`, `update_order`
    and `match_order` are refused before mutating once an epoch is within
    `2^32` of that value. A post-only taker whose depth scan cannot be
    linearized is rejected with the error.
  - `impl From<Vec<Arc<OrderType<()>>>> for OrderQueue`, which silently
    dropped orders it could not insert, is replaced by `TryFrom`, which
    returns the first `DuplicateOrderId` / `CounterExhausted`.
  The 64-bit limits (FIFO sequence, epochs, statistics seqlock sequence) are
  out of reach at any practical operation rate. The `usize` counters
  `orders_added` / `orders_removed` are reachable on 32-bit targets (about
  4.29 billion events, roughly 12 hours at 100k events/s); there the
  statistics become degraded while trading continues. All limits are
  exercised through internal near-limit fixtures. No hot-path allocation was
  added.

- **Collection growth is fallible before state mutation (#164).** Refused
  reservations return the allocation-free `CapacityExceeded` (new
  `CapacityResource` variants `OrderSnapshot`, `SweepScratch`,
  `RestoreScratch`, `SerializationBuffer`).
  - `PriceLevel::snapshot_orders`, `snapshot_by_insertion_seq`,
    `matchable_quantity`, `OrderQueue::snapshot_vec` and `to_vec` return
    `Result`; `snapshot_by_seq_into` returns `Result<(), _>` and leaves the
    caller's buffer untouched on error.
  - `From<&PriceLevel> for PriceLevelData` and
    `From<OrderQueue> for Vec<Arc<OrderType<()>>>` become `TryFrom`.
  - The timestamp-order view sorts in place (unstable sort on a unique key);
    no stable-sort scratch buffer.
  - The sweep's park set holds its first live key inline (no allocation;
    the self-trade skip has at most one live key; the slot frees itself when
    that key goes stale) and grows fallibly beyond it: a
    refusal stops a non-fill-or-kill sweep with the committed prefix and
    `MatchResult::error` carrying the original error; fill-or-kill reserves
    its dry-run copy and park set before the first mutation and is killed
    (logged at `ERROR`) with the level untouched on a refusal. Callers must
    check `result.error()` before resting a remainder.
  - Snapshot restore reserves its duplicate-id set fallibly; the checksum
    payload is streamed into SHA-256 (checksums unchanged); package JSON,
    the hex checksum and decoded order vectors / checksum strings grow
    fallibly. `snapshot()` returns a capacity error without recollecting.
  - `Display` / `Debug` of `PriceLevel` / `OrderQueue` write an error marker
    instead of `fmt::Error` when materialization is refused; `FromStr`
    rejects the marker.
  - Text-parser buffer refusals are `CapacityExceeded { resource: Text }`
    (were `InvalidOperation`).
  - The defensive post-lock replenish counter branch (unreachable since
    #128) logs at `ERROR`, poisons the level and stops the sweep on a refused
    transition instead of ignoring it; the level must then be treated as
    failed.

### Changed

- **Snapshot restore validates in two walks instead of three (#150).**
  `PriceLevel::from_snapshot` (and the package / JSON forms) runs the
  allocation-free aggregate check, then one fused pass over duplicate ids
  and price / side topology, and moves the persisted statistics instead of
  cloning them (their private seqlock sequence is restarted, as the clone
  did). Results and error precedence are unchanged and now documented on
  `from_snapshot`: aggregate overflow, then a refused duplicate-id scratch
  set, then the first repeated id, then the first topology violation,
  regardless of where each sits in the orders vector. Measured effect (see
  `BENCH.md`): 5 to 7% faster rejection of a snapshot whose last order has
  the wrong price at 10,000 / 100,000 orders; valid, duplicate and JSON
  restores within noise; allocations and peak memory unchanged, including
  aggregate rejections. The latency harness gains a `restore_sizes`
  scenario.

### Added

- `PriceLevelSnapshot::try_clone` and `PriceLevelSnapshotPackage::try_clone`
  (#164): fallible owned copies (the derived `Clone` is kept and documented
  as aborting on allocator failure).
- **Production Panic Policy CI gate (#173).** No public API change.
  `[lints.clippy]` (`Cargo.toml`) denies `unwrap_used`, `expect_used`,
  `panic`, `unreachable`, `todo`, `unimplemented`, `indexing_slicing`,
  `string_slice`, `arithmetic_side_effects`, `cast_possible_truncation`,
  `cast_sign_loss`, `cast_possible_wrap`, `manual_assert`,
  `panic_in_result_fn`, `get_unwrap` and `exit` crate-wide; `clippy.toml`'s
  `allow-*-in-tests` keys exempt real test code without exempting a
  production function's `#[cfg(test)]` branch. `scripts/check_panic_policy.py`
  (`make lint-panic`, wired into `make lint` and `make pre-push`) closes the
  gaps clippy cannot cover on its own: the `assert!` / `debug_assert!` macro
  family (no clippy restriction lint bans them) and `saturating_*` /
  `wrapping_*` on production state, both re-checked specifically inside a
  standalone `#[cfg(test)]` helper that clippy's own test heuristic would
  otherwise wrongly exempt. Fixtures proving the gate under
  `scripts/panic_policy_fixtures/` (not part of the published crate). See
  `doc/panic-boundaries.md` for what this automated coverage does and does
  not prove.
- `UuidGenerator::EXHAUSTED`, `UuidGenerator::is_exhausted`,
  `UuidGenerator::remaining`, `UuidGenerator::namespace` and
  `CapacityResource::IdSequence` (#168).
- `MatchResult::try_reserve`, `MatchResult::try_clone`,
  `TradeList::try_reserve`, `TradeList::capacity` and `TradeList::try_clone`
  (#170): fallible growth and cloning with typed `CapacityExceeded` failures.
- `TimestampMs::try_from_system_time(SystemTime)` (#167): checked conversion
  of an already-read `SystemTime` (pre-epoch and `u64` millisecond overflow
  are typed errors; no clamping). It does not read the clock.

### Fixed

- **Engine and execution panic hardening (pre-release).**
  - `MatchResult` decoding (`FromStr` / `Deserialize`) and snapshot restore
    no longer build a `HashSet` (whose `RandomState` can panic on OS RNG
    failure or during thread-local teardown) for their duplicate-id checks.
    The ids are copied into one fallibly reserved vector, sorted and scanned
    for adjacent equal keys; outcomes and error precedence are unchanged
    (pinned against the former implementation on random inputs and by the
    `restore_validation` proptest).
  - The statistics seqlock can no longer be left on an odd sequence. Every
    transition is capped at the even `u64::MAX - 1` (entry limit
    `u64::MAX - 3`, identical to before for a single writer), so even
    overlapping writers (a contract violation) cannot strand
    `read_consistent` in an endless retry.
  - Every rollback / decrement of the level's visible and hidden counters
    (`add_order` reservation rollback, cancel / price-move removal, match
    sweep fills, replenishment and stranded hidden depth) and of the
    statistics aggregates is a checked `fetch_update(checked_sub)` instead of
    a wrapping `fetch_sub`. A refusal (only possible once an invariant is
    already broken) leaves the counter unchanged, poisons the level (or
    keeps the statistics degraded), logs at `ERROR` outside every lock and,
    for a removal or a sweep, reports a typed error.
  - `add_order`, `update_order` and `snapshot` emit their `tracing` events
    only after releasing the fill-or-kill guard, following #172.
  - `cfg(test)` seams compiled into production functions use
    `LocalKey::try_with` and `RefCell::try_borrow(_mut)` and treat a failure
    as "no hook", so they cannot panic during thread-local teardown or on
    re-entry.

- **Checked access in every text parser and in `Id` byte conversion (#174,
  #152).** The `FromStr` impls of `Hash32`, `TimeInForce`, `OrderType`,
  `OrderUpdate`, `Trade`, `TradeList`, `MatchResult`, `PriceLevelSnapshot`,
  `PriceLevelStatistics`, `OrderQueue`, `PriceLevel` and the internal
  `OrderBookEntry` no longer index or slice: they use `split_once`, prefix /
  suffix stripping and borrowed slices at ASCII delimiters, with checked,
  bounded nesting counters. The temporary split vectors and per-parse
  `HashMap`s are gone (field lookup keeps the last-wins / exactly-one-`=`
  rules), and the remaining input-dependent growth reserves through
  `try_reserve`, reporting failure as `InvalidOperation`. `TradeList::from_str`
  parses each trade from a borrowed slice instead of copying it into a
  temporary `String` (#152). `Id::as_bytes` (sequential) and `Id::from_u64`
  build their fixed arrays from `to_be_bytes` without slicing or shifts; both
  byte layouts are unchanged.
- **`PriceLevelSnapshot::refresh_aggregates` is transactional (#162).** It
  computes all three aggregates with checked arithmetic before committing any
  of them; an overflow no longer leaves `order_count` updated while the
  quantity fields keep their old values.

- **No caller code under level guards or mid-bookkeeping (#172).**
  `PriceLevel` and `OrderQueue` `Debug` impls are now hand-written: they
  materialize the orders before writing, so a formatter destination no longer
  runs while `DashMap` shard read locks (and a `fok_guard` read guard) are held.
  A re-entrant destination used to deadlock. The `Debug` text changes shape:
  the internal index and the guard are no longer printed, and
  `finish_non_exhaustive` marks the omission. `Debug` output is not a stable
  format. In `match_order`, the fill-or-kill kill event is now emitted after the
  exclusive guard is released, so a panicking `tracing` subscriber can no longer
  poison a level whose state is intact. The statistics-drop warning is emitted
  after the step's queue, counter and topology bookkeeping, not before it.
  `setup_logger` emits its confirmation event after its one-time
  initialization completes, so a subscriber's `on_event` for that event can
  re-enter `setup_logger` and get the cached result. Subscriber registration
  callbacks (`register_callsite`, `max_level_hint`) still run during the
  initialization, while `set_global_default` builds the dispatcher; calling
  `setup_logger` from them deadlocks and is documented as prohibited.

### Performance

- **Fill-or-kill feasibility is bounded by the depth it consumes (#143).**
  Under its exclusive guard the FOK dry run no longer materializes and
  sorts the whole level: it walks the queue in sweep order and stops once
  the taker is covered, finishing over one sorted collection of the
  remaining makers (`O(depth log depth)`, as before) only when the walk
  outlives `max(8, resting orders / 64)` makers. A qty-1 FOK filled by the
  front maker at depth 10,000 drops from about 157 us to 0.3 us at p50, and
  mutators waiting on the level stall far less behind it (writer add p99.9
  144,505 us to 15 us; the remaining unfairness is #206). The FOK verdict
  and the preflight order are unchanged.
  - **Regression: a FOK that must visit every maker** (a rejected FOK) is
    slower. Criterion: +3 to +4% at depth 10,000, +5 to +6% at depth 100.
    Latency harness at depth 10,000 (median of three interleaved rounds on
    an unpinned host at load averages 5.9 to 9.1, so the tails are
    load-sensitive): p50 173 to 205 us, p99 217 to 581 us, p99.9 585 to
    1,788 us. See `BENCH.md`, "Fill-or-kill feasibility depth".
  - **`PriceLevel::matchable_quantity` (unguarded)** keeps its totals and
    its full collect-and-sort cost: it does not take the guard, so it
    collects from the id-keyed order storage and never double counts a
    maker re-sequenced during the call (its estimate can still be stale).
    Its error surface changed slightly: `CapacityExceeded`
    (`OrderSnapshot`) comes from the order collection (`additional` = the
    resting count) or, new, from growing the buffer of replenished tranches
    (`additional` = 1), which is only attempted when a replenished tranche
    must be revisited.
  - **FOK kill reasons under memory pressure:** the guarded dry run
    reserves nothing for a fill within its lazy budget, so where the former
    snapshot reservation killed such a FOK with `CapacityExceeded`
    (`OrderSnapshot`), it now proceeds and can instead fail at a later
    preflight reservation (result storage, trade ids, park set) or not at
    all. Past the budget, the bulk collection and the tranche buffer are
    the `OrderSnapshot` failure sources.

### Documentation

- **Irreducible panic / abort limits (pre-release).**
  `doc/panic-boundaries.md` now documents the dependency limits the crate
  cannot remove: infallible `DashMap` / `SkipMap` growth (allocator abort),
  `RandomState` in `DashMap` and the sweep's park spill set (kept on purpose
  for HashDoS resistance on caller-controlled ids), `FokGuard` re-entrancy
  (also on `FokGuard::read` / `write`), the `sha2` length counter (2^61
  bytes) and `tracing-subscriber`'s thread-local buffer during teardown. The
  subscriber obligation is stated on `add_order`, `update_order` and
  `snapshot`; the match sweep's events for a `Fok` taker still run under
  the exclusive guard, and the document explains why.

- **Snapshot encoding buffers measured and pinned (#149).** The borrowed
  order serializer and the streamed SHA-256 checksum (both from #164) are
  now covered by equivalence tests against a test-only copy of the old
  buffered path (random snapshots over every order variant, id kind and
  wide / degraded statistics), and every pinned fixture, including a new
  v4 fixture written by 0.10.0 (`snapshot_v4_pricelevel_0_10_0.json`),
  re-encodes byte-identically. `PriceLevel::snapshot_to_json` documents
  that it still serializes the snapshot twice (hash pass, then package
  JSON). The latency harness gains a `snapshot_sizes` group (100 / 10,000
  / 100,000 orders, allocations and peak live bytes); results in
  `BENCH.md`. No production code or output bytes changed.
- **Concurrency and performance claims corrected (#156).** The crate docs,
  README, rustdoc and package description no longer call the `Gtc` / `Ioc` /
  `Day` match path lock-free: each fill is committed under the maker's
  `DashMap` shard write lock, and `Fok` also holds the level-wide guard
  exclusively. Only the `SkipMap` index and the atomic counters are described
  as lock-free (`value_executed` with its platform fallback). A new
  "Concurrency Model" section documents the per-method locks and the one
  logical matcher per level contract, superseding the 0.9.0 wording below.
  The unprovenanced throughput tables (237,347.51 vs "over 264,000" ops/s,
  measured with ten concurrent takers on one level) are withdrawn, and an
  operation-accounting guide for future results is added.
- **Caller-supplied code boundaries documented (#172).** New
  `doc/panic-boundaries.md` inventories every call into caller code (generic
  payload `Clone` / `Debug` / `PartialEq` / serde / `Default`,
  `map_extra_fields`, formatter destinations, serializers, `iter_orders` loop
  bodies, the `tracing` subscriber) with the guard held and the mutation state
  at each. Rustdoc on `OrderType`, `map_extra_fields`, `match_against`,
  `PriceLevel::match_order`, `iter_orders`, `PegReferenceType` and
  `setup_logger` states the no-panic obligation and that the library does not
  recover from caller panics or OOM aborts. A "Caller-Supplied Code" section is
  added to the crate docs.
- **Examples respect one matcher per level (#156).** `hft_simulation`,
  `contention_test` and `simple` now run a single matcher thread per shared
  level, keep maker and taker ids disjoint, and report successful operations
  separately from rejected and missing-order calls.
- **Statistics writer contract stated (#153).** `PriceLevelStatistics`
  supports exactly one concurrent writer of its execution aggregates:
  `record_execution` is driven by the single logical matcher and `reset` /
  `reset_at` require quiescence. Under that contract `Clone` (and so
  `PriceLevel::snapshot`), serde and `Display` return a complete execution
  state from any number of reader threads, including across an overflow
  rollback. Overlapping `record_execution` calls are unsupported: the
  `stats_seq` guard is a reader protocol, not a writer lock, and a reader can
  then accept a partial tuple (final totals stay correct). Comments that
  claimed the guard keeps `reset` from interleaving a rollback are corrected;
  quiescence is what rules that out. The rustdoc, the "Concurrency Model"
  crate docs and `doc/architecture.md` agree. No API or behavior change and
  no extra atomics on the fill path. A loom model
  (`RUSTFLAGS="--cfg loom" cargo test --test loom_stats_seqlock --release`)
  checks one writer with rollback against concurrent readers and pins the
  unsupported two-writer schedule; a single-writer / multi-reader stress test
  covers the production atomics. Existing tests with several concurrent
  recorders are marked as final-state arithmetic checks only.

## [0.9.2] - 2026-09-18

### Changed

- Dependencies updated to latest stable versions: `ulid` 1.2 -> 3.0
  (`Ulid::new()` -> `Ulid::generate()` in `src/utils/id.rs`; no public API
  change), `uuid` 1.23 -> 1.26, `dashmap` 6.1 -> 6.2 (#138).

## [0.9.1] - 2026-07-14

### Fixed

- **`MatchResult` round-trips non-self-describing serde formats again
  (#135).** 0.9.0's decode-time validation (#117) deserializes through a wire
  struct whose `outcome` is `Option<MatchOutcome>`, but `Serialize` still
  emitted a bare `MatchOutcome`. JSON tolerated the asymmetry; a positional
  decoder (bincode) read the enum variant index where the option tag was
  expected and failed with `UnexpectedVariant` on every 0.9.0 payload.
  `outcome` is now serialized as `Some(outcome)`: the JSON payload is
  byte-identical (serde flattens `Some`), and bincode encode/decode are
  symmetric. Bincode payloads written by 0.9.0 do not decode (they never
  did — 0.9.0 could not decode its own output); JSON payloads from any
  version are unaffected. Pinned by a bincode leg on the round-trip
  property test plus deterministic shape guards (`bincode` added as a
  dev-dependency only).

## [0.9.0] - 2026-07-14

Major hardening release: ten engine-correctness issues (#111–#120, PRs
#121–#130) plus five adversarial review rounds. Every fix ships with
regression tests; the full suite grew from ~440 to 526 tests.

### Changed (breaking)

- **`PriceLevel::add_order` returns `Result<Arc<OrderType<()>>, PriceLevelError>`.**
  Admission validates before mutating: the order's own visible + hidden total,
  the level's counters (checked CAS reservations), price and side topology,
  and id uniqueness — a rejected admission leaves the level byte-identical
  (#111, #113, #120). A duplicate id reports the new
  `PriceLevelError::DuplicateOrderId` and takes precedence over a
  counter-capacity error.
- **`PriceLevel::matchable_quantity(quantity, taker_id)`** — the fill-or-kill
  dry run applies the same self-match skip as the sweep so the two can never
  diverge (#120).
- **`OrderQueue::push` and `OrderQueue::from_vec` are no longer public**, and
  **`impl From<&PriceLevelSnapshot> for PriceLevel` is replaced by `TryFrom`**
  delegating to the validating `from_snapshot` — no public path can overwrite
  a live id or restore counters over silently-dropped duplicates (#113 +
  reviews).
- **Snapshot format v3.** Statistics carry a sticky `stats_degraded` flag
  (serialized only when `true`); v3 packages are written, v2 packages are
  still accepted on read (legacy 8-field statistics), v1 remains rejected
  (#117 + review). Pre-0.9 snapshots that captured a queue-priority demotion
  restore with the old (wrong) front priority — re-snapshot to pin the
  corrected order.
- **Concurrency contract wording:** the Gtc/Ioc/Day match path is lock-free;
  `add_order` / `update_order` (cancel included) are shared-lock mutators —
  normally uncontended, but they can block behind an O(depth) fill-or-kill
  writer (#112).

### Fixed

- Partial fills resize every matchable order variant (TrailingStop, Pegged,
  MarketToLimit previously kept their original size and could double-execute)
  (#118).
- `MatchResult::from_str` never panics on malformed UTF-8, and both decoders
  route through one invariant validator (outcome consistency, taker identity,
  filled-id ordering, checked sums) (#114, #116).
- Counter overflow is rejected before any state mutates, including reserve
  replenishment (which can no longer bypass FIFO or wrap the level counter —
  the sweep aborts in front of a replenishment the level cannot represent)
  (#111 + review).
- Duplicate-id admission is atomic (identity decided first, publication under
  one held entry lock), and the index re-key is new-before-old with a
  sequence-validated destructive pop — a demoted maker can neither vanish
  from a front scan nor drain out of FIFO order (#113, #119 + reviews).
- Level topology is pinned in a single side+count atomic word (opposite-side
  admissions into an empty level serialize on one CAS), snapshots retry on a
  topology epoch so they never capture a torn side transition, and a
  self-match attempt is rejected terminally with zero trades (#120 + review).
- Quantity updates derive from the live maker under the entry lock (no
  resurrection of executed quantity, no stale priority policy), with replenish
  counter transitions published inside the lock (#115, #119 + reviews).
- Execution statistics record all-or-nothing behind a seqlock (consistent
  clones/serialization, enforced reset serialization, checked aggregates,
  monotonic last-execution timestamp), and a dropped recording is observable
  via `stats_degraded` with a single WARN on transition (#117 + review).
- PostOnly can never trade (structural early return, linearized depth verdict
  via a mutation epoch) and FOK is all-or-nothing under every interleaving
  (level guard across an exact feasibility projection and the sweep; poisoned
  guards fail fast instead of reopening a half-mutated level) (#112 + review).
- The plain `PriceLevelData` serde round-trip preserves FIFO order (#131).

## [0.8.5] - 2026-07-14

Patch release: a **bug fix** to the snapshot round-trip's queue-priority
preservation. The snapshot JSON *shape* is unchanged, but the `orders` array
order — and therefore the package checksum — changes for levels whose
consumption order diverges from timestamp order (a demoted or
non-monotonically-timestamped maker).

### Fixed

- **A snapshot round-trip no longer undoes a queue-priority demotion.**
  `PriceLevel::snapshot()` sorted its orders by `(timestamp, sequence)` and
  did not serialize the insertion sequence, while `from_snapshot` re-enqueues
  in vector order. An order demoted to the back of the queue with its original
  admission timestamp intact — a quantity *increase* via `update_order`, or an
  iceberg / reserve replenishment — therefore sorted back to its old timestamp
  position on restore and wrongly regained front priority. The snapshot now
  materializes orders in **queue-consumption order** (ascending insertion
  sequence, exactly as `match_order` sweeps), so a restore reproduces the live
  queue's price-time priority in all cases, demotions included. Found via the
  queue-priority contract review in joaquinbejar/OrderBook-rs#204 (#109).

### Notes

- `PriceLevelSnapshot::orders()` / `iter_orders()` / `into_orders()` now
  yield consumption order, not timestamp order. A consumer that wants
  admission-time order should sort by `order.timestamp()` itself (or read
  `PriceLevel::snapshot_orders()` on a live level, which keeps the
  `(timestamp, sequence)` view).
- A snapshot serialized by ≤ 0.8.4 that captured a demotion restores with the
  old (wrong) front priority — the fix cannot repair data already persisted
  in timestamp order. Re-snapshot with 0.8.5 to pin the correct order.

## [0.8.4] - 2026-07-10

Patch release: a **documentation fix** to `TimeInForce::Gtd`'s payload unit. No
API, behavior, or wire-format change.

### Fixed

- **`TimeInForce::Gtd`'s doc no longer claims the payload is seconds — it is
  Unix MILLISECONDS since the epoch.** Every other timestamp in this crate is
  milliseconds (`TimestampMs`, trade timestamps, statistics), this crate's own
  tests always used 13-digit millisecond epochs for GTD, and `orderbook-rs`
  compares the payload against `Clock::now_millis`. A caller following the old
  doc and passing seconds got orders that appeared expired immediately. The
  contract is now pinned by the `gtd_payload_unit_is_milliseconds` test
  (found via joaquinbejar/OrderBook-rs#187).

## [0.8.3] - 2026-06-25

Patch release: a **performance fix** to `match_order`'s transient allocation. No
API, behavior, or wire-format change.

### Fixed

- **`PriceLevel::match_order` no longer pre-allocates the `MatchResult` buffers
  to the whole level depth.** It reserved `trades` / `filled_order_ids` for
  `order_count()` entries on every match, so a qty-1 taker against a deep level
  reserved a multi-MB buffer (~176 B × depth) it immediately freed — pure
  allocator pressure, not a leak. The pre-size is now bounded by
  `min(incoming_quantity, order_count)`, a tight upper bound on the number of
  fills (each trade consumes ≥1 unit of the taker). A qty-1 match against a
  100k-deep level now allocates KB, not MB; large sweeps are unaffected. Matching
  semantics, FIFO/price-time priority, and trade output are unchanged.

## [0.8.2] - 2026-06-24

Small, **non-breaking** release exposing two primitives an order book needs to
compose this level without re-deriving its internal sweep order.

### Added

- **`PriceLevel::matchable_quantity(incoming_quantity: u64) -> u64`** is now
  `pub`. It was already the deterministic fill-or-kill dry run — a no-mutation
  replay of the FIFO sweep (including iceberg / reserve replenishment) that
  returns exactly what `match_order` would consume. Making it public lets a
  composing order book delegate per-level all-or-nothing feasibility to this
  single upstream source of truth instead of re-implementing the sweep and
  risking drift.
- **`PriceLevel::snapshot_by_seq_into(&self, out: &mut Vec<Arc<OrderType<()>>>)`**
  — buffer-reuse variant of `snapshot_by_insertion_seq()`. Clears `out` and
  refills it in ascending insertion sequence (the order `match_order` consumes
  orders), so a consumer that walks every level repeatedly (e.g. a
  self-trade-prevention pre-scan) can reuse one pooled scratch buffer and avoid
  the per-call allocation the owned-`Vec` variant pays.

No breaking changes, no new dependencies, no change to matching semantics or the
snapshot wire format.

## [0.8.1] - 2026-06-24

### Added

- **`PriceLevel::snapshot_by_insertion_seq() -> Vec<Arc<OrderType<()>>>`** —
  returns the resting orders in ascending insertion sequence, i.e. the exact
  order `match_order` consumes them. Unlike `snapshot_orders()` (sorted by
  `(timestamp, sequence)`, which equals the sweep only when timestamps are
  monotonic with insertion) and `iter_orders()` (no stable order), this faithfully
  predicts the sweep, giving a downstream consumer a public primitive to walk
  orders in consumption order.

No breaking changes, no new dependencies, snapshot wire format unchanged.

## [0.8.0] - 2026-06-23

Roadmap hardening release: a sweep of correctness, robustness, and tooling
improvements across the price level. Highlights:

### Added

- Taker time-in-force handling inside `match_order` (`Gtc` / `Ioc` / `Fok` /
  `Gtd` / `Day`), with a `TakerKind` (`Standard` / `PostOnly` / `MarketToLimit`)
  and a `MatchOutcome` (`Filled` / `PartiallyFilled` / `NotFilled` / `Killed` /
  `Rejected`).
- A property-test harness (`proptest`) covering the nine price-level invariants.
- A `loom` linearization model for the cancel-vs-partial-fill protocol.

### Changed

- Unified order matchability behind `OrderType::is_matchable`, shared by the
  post-only pre-check and the fill-or-kill dry run so they can never disagree.
- Bumped `sha2` to `0.11`.

### Fixed

- A zero-visible iceberg / auto-replenishing reserve backed by hidden quantity is
  now correctly treated as matchable depth, closing a fill-or-kill no-progress
  loop.
- Lost-cancel race on the order queue.

[0.8.3]: https://github.com/joaquinbejar/PriceLevel/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/joaquinbejar/PriceLevel/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/joaquinbejar/PriceLevel/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/joaquinbejar/PriceLevel/releases/tag/v0.8.0
