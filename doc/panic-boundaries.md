# Caller-supplied code and panic boundaries

Issue #172. Companion to the Production Panic Policy in
`rules/global_rules.md` and to the "Concurrency model" section of
`doc/architecture.md`.

## Contract

The Production Panic Policy in `rules/global_rules.md` requires that
crate-owned code not initiate panics. That is the required policy, not a
completed state: remaining crate-owned panic paths (the `snapshot()`
aggregate assertions were removed in #162) are being removed under the audit in
[#161](https://github.com/joaquinbejar/PriceLevel/issues/161) and its
sub-issues, and are out of scope here. This document covers the separate
question of code the crate does not own. Some public operations call it: trait impls on a caller payload, caller closures, a caller's
`fmt::Write` / `Serializer` / `Deserializer`, and the process-installed
`tracing` subscriber. Rust bounds such as `Clone`, `Default`, `Debug`,
`Serialize` or `FnOnce` cannot express "does not panic", so:

- **Obligation.** Caller-supplied implementations and callbacks must not
  panic, and must not call back into the `PriceLevel` / `OrderQueue` that is
  invoking them unless the row below says re-entry is safe.
- **What the library guarantees.** It never runs caller code while a
  `DashMap` shard **write** lock is held or between a queue commit and the
  counter bookkeeping of the same step. Where a guard or partial mutation
  remains, the row below says so.
- **What it does not guarantee.** It does not recover from a caller panic
  and does not certify third-party code. It installs no panic hook, uses no
  `catch_unwind`, and never aborts on purpose. An allocator's OOM abort is a
  process-wide failure and is not reported as a typed error.

## Two scopes

1. **Pure generic utilities**: `OrderType<T>` with an arbitrary payload `T`.
   These are value transformations. They hold no lock and mutate no library
   state.
2. **The matching engine**: `PriceLevel` / `OrderQueue`, which only ever store
   `OrderType<()>`. `()`'s impls are in `core` and cannot panic, so no caller
   payload code runs in the engine. The remaining engine boundaries are
   formatting destinations, serializers, iterator consumers and the `tracing`
   subscriber.

## Inventory

"Guard" means a lock held while the caller code runs. "Partial mutation"
means that library state was changed before the caller code runs and would be
left behind by an unwind.

### Generic payload `T` (`src/orders/order_type.rs`)

| Call | Where | Guard | Partial mutation | Unwind effect |
|------|-------|-------|------------------|---------------|
| `T::clone` | `with_reduced_quantity`, `refresh_iceberg`, `match_against` (partial-fill residual), derived `Clone` | none | none (`&self`) | crate-controlled fields of the source order unchanged; payload side effects (interior mutability in `T`) are not covered |
| `T: Debug` into caller formatter | derived `Debug` | none | none | none |
| `T: PartialEq` / `Eq` | derived `PartialEq` | none | none | none |
| `T: Serialize` + caller `Serializer` | derived `Serialize` | none | none | partial output is the caller's |
| `T: Deserialize` + caller `Deserializer` | derived `Deserialize` | none | none | partially built value dropped |
| `T::default` | `FromStr for OrderType<T>` | none | none | parse abandoned |
| `F: FnOnce(T) -> U` | `map_extra_fields` | none | none | consumed `self` dropped |
| `&mut T` handed out | `extra_fields_mut` | none | caller-owned value | caller's responsibility |

"Unwind effect" describes crate-controlled state only. A caller impl can
mutate its own payload through interior mutability before panicking; the
library makes no statement about the payload after such a panic. The
payload test in `src/orders/tests/order_type.rs` shows preservation only for
its own side-effect-free payload.

`Display for OrderType<T>` does not touch `T`; it writes to the caller's
formatter only. `OrderMetadata` and `()` are crate/core payloads and are
compliant.

### Formatting destinations (`fmt::Write` behind a `Formatter`)

| Call | Guard | Partial mutation | Notes |
|------|-------|------------------|-------|
| `Display` for `PriceLevel`, `OrderQueue` | none | none | materialize via `snapshot_orders` / `snapshot_vec`, then write. The materialization is fallible (#164): a refusal writes an `orders=!<error>` marker (rejected by `FromStr`) instead of returning `fmt::Error`, which would make `to_string` panic |
| `Debug` for `PriceLevel`, `OrderQueue` | none (since #172) | none | manual impls materialize first; `fok_guard` is omitted. The derived impls held `DashMap` shard read locks (and a `fok_guard` read guard) while writing, so a re-entrant destination deadlocked. A refused materialization shows `<unavailable: ..>` (#164) |
| `Display` / `Debug` for the other crate types | none | none | plain values |

Re-entry from a formatting destination into the level is safe.

### Serializers and deserializers

| Call | Guard | Partial mutation | Notes |
|------|-------|------------------|-------|
| `Serialize for PriceLevel` | none | none | materializes the orders (fallibly, #164) first, then writes the `PriceLevelData` shape with the orders borrowed |
| `Serialize for OrderQueue` | none | none | `snapshot_by_seq` (fallible, #164) first |
| `Serialize for PriceLevelStatistics` | none | none | one seqlock-consistent read into locals first |
| `Serialize` for snapshots, `Id`, `Hash32`, `PegReferenceType`, `Trade`, `MatchResult`, ... | none | none | owned values |
| `Deserialize` for `PriceLevel`, `OrderQueue`, snapshots | none | a fresh, not-yet-shared value only | an unwind drops the partially built value |

`PriceLevelSnapshotPackage` uses `serde_json` internally; that is a crate
dependency, not caller code, and its errors map to typed variants.

### Iterator consumers

| Call | Guard | Partial mutation | Notes |
|------|-------|------------------|-------|
| `PriceLevel::iter_orders`, `OrderQueue::iter_orders` loop body / adapters | **`DashMap` shard read lock**, held between `next()` calls | none | a body that mutates the same level on the same thread can deadlock; writers to that shard (including the matcher) wait. A panic releases the read lock without poisoning. Use `snapshot_orders` to run arbitrary code with no lock. Kept lazy because v0.7 made `iter_orders` non-allocating on purpose |
| `PriceLevelSnapshot::iter_orders` | none | none | iterates an owned `Vec` |

### Clock and entropy traits (`src/utils/entropy.rs`, `src/utils/id.rs`, `src/execution/trade.rs`, `src/price_level/statistics.rs`)

| Call | Where | Guard | Partial mutation | Notes |
|------|-------|-------|------------------|-------|
| `EntropySource::try_fill_bytes` | `Id::try_new`, `Id::try_new_ulid`, `Id::try_new_ulid_at`, `Id::try_new_uuid` | none | none | failures must be returned as `Err`; a panic unwinds with no library state touched (#167) |
| `UnixClock::try_now_ms` | `Id::try_new`, `Id::try_new_ulid`, `Trade::try_new`, `PriceLevelStatistics::try_new`, `PriceLevelStatistics::reset`, `PriceLevelStatistics::time_since_last_execution` | none (`reset` reads the clock before entering its seqlock write section) | none | same; a failed or panicking read leaves the statistics unchanged (#171). `PriceLevel::match_order` never calls a clock |

### `tracing` subscriber

Events are dispatched synchronously into the process-installed subscriber.
The library never installs one; `setup_logger` is a convenience for binaries
and tests.

| Event site (`src/price_level/level.rs`) | Guard | Partial mutation at the event |
|-----------------------------------------|-------|-------------------------------|
| `mark_poisoned` `error!` | the already-poisoned `fok_guard` (read or write) | the level is already flagged poisoned; no new mutation |
| `match_order` self-match reject `debug!` | none | none |
| `match_order` post-only reject `debug!` | none | none |
| `match_order` FOK kill `debug!` | none (guard dropped first since #172) | none |
| `match_order` FOK kill `error!` with an error (dry-run stop, sequence headroom, result storage, trade ids, dry-run working snapshot and park set reservations, #164) | none (guard dropped first) | none |
| `snapshot` recollection `debug!` (rejected walk: mixed sides or aggregate overflow) and attempts-exhausted `warn!` (#162) | none since the pre-release hardening: each rejection is recorded in a fixed per-attempt slot under the shared guard and logged after the guard is released | none: a rejected walk is discarded and the level is never mutated by `snapshot` |
| `add_order` statistics-drop `warn!` (#165) | none since the pre-release hardening (recorded under the shared guard, emitted after it is released) | the admission is committed |
| `add_order` refused rollback `error!` (checked visible / hidden rollback after a failed reservation or pin; pre-release hardening) | none (deferred as above) | the admission was rejected; the counter that refused kept its value (it already disagreed with the queue) and the level is poisoned before the event |
| sweep set-aside `warn!`, self-trade skip `debug!`, overflow abort `error!` | `fok_guard` write side for a `Fok` taker; nothing otherwise | this step is a no-op. **Earlier steps are committed** to the queue and counters, and their trades live only in the local `MatchResult` |
| sweep statistics-drop `warn!` | as above | the step's queue, counter and topology bookkeeping is complete (moved after the bookkeeping in #172). **The step and earlier steps are committed**, as above |
| sweep park refusal (`SweepScratch`, #164; logged with the sweep stop `error!`) | as the set-aside row | this step is a no-op: `SetAside` mutates nothing and the parked-sequence set's refused reservation leaves it unchanged. **Earlier steps are committed**, as above. The first live park uses an inline slot that frees itself when its key goes stale (cancel, readmission, demotion), so a single live parked maker never allocates; this needs two simultaneously live parks |
| sweep post-lock replenish counter refusal `error!` (#128 defensive branch, #164; unreachable today; also reported by the sweep stop `error!`) | as the set-aside row | the maker was re-sequenced with its new split but the level counters could not follow, so the level is poisoned before the event. **The step and earlier steps are committed** |
| sweep counter refusal `error!` (checked visible decrement of a fill, checked hidden decrement of a replenish or of stranded hidden depth; pre-release hardening) | as the set-aside row | the step committed; the refusing counter kept its value (it already disagreed with the queue), the level is poisoned and the sweep stops after this step with a typed error. **The step and earlier steps are committed** |
| sweep resting-order count refusal `debug!` (`TopologyUnderflow`, #163) | as the set-aside row | this step is a no-op: the count check ran inside the step's `match_front` entry critical section and refused the full consume before `Remove` committed; the error is built after the entry lock is released. **Earlier steps are committed**, as above |
| sweep post-removal release failure (#163; logged with the sweep stop `error!`) | as the set-aside row | the maker is removed and the step's counters moved; the topology count could not be released (it already disagreed with the queue), so the level is poisoned before the event. **The step and earlier steps are committed** |
| removal count refusal `warn!` (`update_order` cancel / price move, #163) | none since the pre-release hardening: recorded under the `fok_guard` shared side and emitted by `update_order` after releasing it; no `DashMap` lock | none: the count check ran inside `OrderQueue::remove_if`'s entry critical section, refused, and released the entry lock before the error is built |
| removal post-release failure `error!` (`update_order` cancel / price move, #163) | none (deferred as above) | the order is removed and the quantity counters moved; the level is already poisoned (the event reports it) |
| removal counter refusal `error!` (`update_order` cancel / price move; checked visible / hidden decrement, pre-release hardening) | none (deferred as above) | the order is removed; the refusing counter kept its value and the level is poisoned; the call returns a typed error instead of the order |
| update counter-rollback failure `error!` (`update_order` resize, #163) | none (deferred as above) | the resize was rejected with the queue untouched; one level counter could not be restored and the level is already poisoned |
| `update_order` statistics-drop `warn!` (#165) | none (deferred as above) | the removal is committed |
| `setup_logger` `debug!` | none; emitted after the `OnceLock` initialization completes (since #172) | global subscriber installed; init result cached, so a `setup_logger` call from this event's `on_event` returns it instead of blocking |
|  `setup_logger` → `set_global_default` → `Dispatch` construction: callsite-interest rebuild invoking live subscribers' `register_callsite` / `max_level_hint` | **`LOGGER_INIT_RESULT` `OnceLock` initialization in progress** | none yet (the global default is not set until these return). a callback that calls `setup_logger` blocks on the same initialization and deadlocks; **re-entry from registration callbacks is prohibited**. A panic unwinds out of `get_or_init`, leaving it uninitialized |

No event is emitted inside the `OrderQueue::match_front` /
`update_entry_with` / `remove_if` / `try_push_with` closures, so none runs
under a `DashMap` shard write lock.

Since the pre-release hardening, `add_order`, `update_order` and `snapshot`
emit nothing while they hold the `fok_guard` shared side: their events are
recorded in fixed, non-allocating slots (`DeferredEvents`, the per-attempt
snapshot rejections) and emitted after the guard is released. The one
remaining event under that guard is `mark_poisoned`'s `error!`, raised while
the acquisition itself recovers an already-poisoned guard.

The match sweep's events for a `Fok` taker still run under the exclusive
guard (rows above). Deferring them was not straightforward: a sweep can
raise one event per visited maker, so collecting them would need an
unbounded buffer (an allocation on the hot path, or a fallible reservation
that can fail after trades are committed) or would drop events. They are
kept in place and documented instead; the consequence of a subscriber panic
there is described below.

#### Removal event boundaries (issue #163)

A cancel or price-moving `update_order` removes through
`OrderQueue::remove_if`, which runs three phases:

1. **Under the entry lock:** select the occupied entry, evaluate the
   crate-owned count check (`PriceLevel::topology_releasable`: one atomic load,
   no allocation, no event), then remove the map entry or refuse. An absent id
   returns before any check or error construction.
2. **After the entry lock:** remove the index key, move the quantity
   counters (checked decrements; a refusal poisons the level), release the
   topology count (`release_after_removal`).
3. **Last:** build any error. Events are recorded and emitted by
   `update_order` after it releases the `fok_guard` shared side
   (pre-release hardening).

A full consume in the sweep follows the same boundary: the count check runs in
the `match_front` decision closure, in the entry critical section that commits
`FrontAction::Remove`; the release, error and event follow after the lock.
Because the check and the removal share one critical section, a concurrent
admission of the same id either publishes before the selection (and is
removed with its count) or after it (and the removal reports not-found):
the check cannot be misled into a spurious count error.

Consequence of a subscriber panic inside a sweep: the queue and counters
remain mutually consistent at step granularity, but the unwinding
`match_order` loses the `MatchResult` for trades it already committed. For a
`Fok` taker the unwind also poisons `fok_guard`'s lock, so the level fails
fast (issue #130), which is the right outcome for a fill-or-kill that is no
longer all-or-nothing. The panic itself does not set the level's sticky
poison flag: the next acquisition of the guard (`add_order`, `update_order`,
`snapshot` or a fill-or-kill `match_order`) recovers the lock poison and
trips it. From then on mutators return `InvalidOperation` and every
`match_order` refuses before touching a maker, carrying that same error in
`MatchResult::error` (issue #217), so a caller sweeping several levels stops
there instead of treating the level as empty. Removing the loss entirely
would require deferring every sweep event until after `match_order` returns.
That is a proposed follow-up, not a
current guarantee.

## Allocation limits (issue #164)

Every owned collection the engine and the snapshot / serialization paths
grow is reserved through `try_reserve*` (`src/utils/alloc.rs`) before the
state it describes is mutated. A refusal is reported as
`PriceLevelError::CapacityExceeded { resource, additional }`: a `Copy` tag and
a `usize`, so the report itself never allocates. Capacity arithmetic is
checked. Covered sites: the queue views and their sort buffer (in-place
unstable sort, no scratch), the fill-or-kill dry run's bulk collection of
the remaining makers and its buffer of replenished tranches (#143), the sweep's
parked-sequence set, `MatchResult` / `TradeList` (#170), the restore
duplicate-id set, `PriceLevelData`, snapshot `try_clone`, package JSON, the
hex checksum, decoded order vectors and checksum strings, and the text
parsers (#174).

The restore duplicate-id check and the `MatchResult` filled-id duplicate
check are hasher-free since the pre-release hardening: the ids are copied
into one `try_reserve_exact` scratch vector (resources `RestoreScratch` /
`ValidationScratch`), sorted with the in-place `sort_unstable` on a total
order (variant tag, 16 id bytes, position) and scanned for adjacent equal
keys. The former `HashSet` built a `RandomState`, which is a panic source
(below) on caller-controlled input.

The dry run's replenished-tranche buffer (#143) starts empty and grows one
element at a time through the fallible `try_push_back_deque` helper,
holding residuals by value (no `Arc`); a residual is only buffered when
the taker still has quantity left. The lazy phase of the dry run's walk
clones each visited maker's existing `Arc` and allocates nothing.

Proven-capacity sites that do not reserve: pushes / extends that follow a
successful reservation of their exact size.

What is **not** covered, and why:

| Allocation | Where | Why it is not fallible | Caller precondition |
|------------|-------|------------------------|---------------------|
| `DashMap` entry / shard growth | `OrderQueue` admission, `try_from_vec`, restore | no stable fallible insertion API | size levels to available memory; an allocator failure aborts |
| `SkipMap` node | `OrderQueue` index insert / re-key | no fallible API | as above |
| `Arc::new` | admission, live-sweep residual / refreshed orders, decoded snapshot orders, `stats` | fixed-size; `Arc::try_new` is unstable | as above |
| `serde_json` internals | error boxing (`serde_json::Error` is a `Box`), scratch buffers while decoding, the `io::Error` wrapper after a refused `FallibleWriter` write | dependency code | a refusal inside `serde_json` aborts; our own buffers report `CapacityExceeded` (on decode, through `serde`'s error type) |
| error text | `InvalidOperation` / `DeserializationError` messages built for non-allocation failures | small, bounded by the failure, not input growth | none |
| `tracing` subscriber | events | caller code (see above) | the subscriber must not allocate unboundedly; capacity refusals in `snapshot` and the fill-or-kill dry run are returned without an event |

An allocator failure in any of these is a process-wide abort
(`handle_alloc_error`), not a Rust panic: it cannot be caught and the library
does not promise recovery from it. The derived `Clone` of
`PriceLevelSnapshot` / `PriceLevelSnapshotPackage` is kept for convenience
and aborts the same way; use their `try_clone`.

### Irreducible dependency limits (pre-release hardening)

These are panic or abort paths inside the standard library or a dependency
that the crate cannot remove without a redesign. They are documented rather
than claimed away:

- **`DashMap` / `SkipMap` growth is infallible.** Neither offers a
  concurrent `try_reserve` / fallible insert, so order admission, restore and
  re-keying grow them infallibly. Allocator failure aborts the process
  (`handle_alloc_error`), it does not panic. `hashbrown`'s capacity-overflow
  panic inside `DashMap` requires more entries than `isize::MAX` bytes can
  address, which allocator failure precedes, so it is unreachable in
  practice.
- **`RandomState` can panic.** `DashMap` (order storage) and the sweep's
  `ParkedSeqs` spill set hash with the default `RandomState`. Building one
  reads a thread-local key and, the first time on a thread, the OS random
  source; on some platforms that panics if the OS RNG fails, and it panics
  when a thread-local is accessed during thread-local destruction. The
  randomized hasher is kept deliberately: both maps are keyed by
  caller-controlled `Id`s and need HashDoS resistance, which a fixed or
  non-randomized hasher would give up. Input-validation duplicate checks
  (restore, `MatchResult` decode) no longer hash at all (above).
- **`FokGuard` is not re-entrant.** It wraps `std::sync::RwLock`, which may
  deadlock or panic when the thread holding one side acquires the guard
  again. The level acquires it exactly once per public call and never while
  it is already held; re-entry can only come from caller code (the
  subscriber during a `Fok` sweep, an iterator body), which the global
  no-re-entry obligation above rules out.
- **`sha2` debug overflow.** SHA-256 counts the processed length in bits;
  the counter overflows (a debug-build panic in the dependency) only after
  about 2^61 bytes, far beyond any snapshot the crate can materialize.
- **`tracing-subscriber`'s fmt layer uses `BUF.with`.** The formatting layer
  that `setup_logger` installs keeps a thread-local buffer accessed with
  `LocalKey::with`, so an event emitted during thread-local destruction
  panics inside the dependency. The crate emits no event from a `Drop` impl
  or a thread-local destructor, but a caller that drops a level (or calls
  into it) from its own thread-local destructor with that layer installed
  can hit it.

The crate's own `cfg(test)` seams compiled into production functions use
`LocalKey::try_with` and `RefCell::try_borrow(_mut)` (`utils::test_tls`) and
treat a failure as "no hook / not armed", so they add no panic source of
their own.

## Automated enforcement scope and its limits (issue #173)

The Production Panic Policy gate (`[lints.clippy]` in `Cargo.toml`,
`clippy.toml`, and `scripts/check_panic_policy.py` via `make lint-panic`,
wired into `make lint` / `make pre-push` and CI's `lint.yml`) is syntax-level
enforcement of the "no explicit panic form in crate-owned production code"
half of the policy. It is not, and does not claim to be, a proof that any
code path here — let alone the caller-supplied code this document is about —
never panics:

- It denies `.unwrap()` / `.expect()` / `.unwrap_err()` / `.expect_err()` /
  `panic!` / `unreachable!` / `todo!` / `unimplemented!` / indexing /
  string-slicing / narrowing-or-sign-changing casts / raw arithmetic
  (clippy), and `assert!` / `assert_eq!` / `assert_ne!` / `debug_assert!` /
  `debug_assert_eq!` / `debug_assert_ne!` / `saturating_*` / `wrapping_*`
  (the script — clippy has no lint for the assert family at all). Both tools
  exempt real test code and re-check a standalone `#[cfg(test)]` production
  helper (this crate's `test_seam` modules, hook installers/firers) that
  clippy's own `#[cfg(test)]` heuristic would otherwise wrongly wave
  through.
- It does **not** see through a documented panic condition on a dependency
  call, an atomic-ordering assumption, an iterator/time arithmetic edge
  case, or a caller-supplied `Clone` / `Drop` / formatter / callback — every
  boundary in the inventory above. Those stay a manual review question:
  "Review collection operations, atomic orderings, time/iterator
  arithmetic, serialization and dependency calls for their documented panic
  conditions... every reachable operation... must be reviewed"
  (`rules/global_rules.md`'s Production Panic Policy) is retained as a
  checklist item, not replaced by a green CI run.
- It does not run over `benches/`, `examples/`, or the `tests` integration
  targets — those carry their own crate-root `#![allow(...)]` (see each
  file's header comment) because they are not production code, not because
  they are exempt from review as demos / harnesses in their own right.
- **Indexing/slicing inside a `#[cfg(test)]` test seam (PR #207 review).**
  `clippy.toml`'s `allow-indexing-slicing-in-tests` exempts `clippy::
  indexing_slicing` for ANY `#[cfg(test)]` item — the same coarseness that
  makes `scripts/check_panic_policy.py` re-check unwrap/expect/panic/
  saturating on a standalone production-adjacent test seam. Two ways to
  close this were evaluated: (a) drop the clippy.toml key and add an
  explicit `#[allow(clippy::indexing_slicing, clippy::string_slice)]` to
  every co-located `mod tests { ... }` block that indexes or slices, or (b)
  add a syntax-aware indexing check to the script, scoped the same way as
  its other checks. (a) was rejected: nearly every one of the ~30
  co-located test files indexes or slices somewhere, so dropping the
  toggle would require touching most of them for no behavioural change.
  (b) is what shipped: `INDEXING_PATTERN` in the script matches an
  identifier, or a closing `)` / `]`, immediately followed by `[` (so
  `v[1]` and `matrix[0][1]` count, but a type `&[u8]` / `[u8; 4]` or a
  literal `[1, 2, 3]` — never preceded by an identifier or closing
  delimiter — do not), excluding a short keyword denylist
  (`return`/`yield`/`break`/`in`/...) that can precede an array literal
  instead of indexing. It runs ONLY inside the production-adjacent
  `#[cfg(test)]` spans `check_panic_policy.py` already tracks (never over
  ordinary production code, where clippy's own AST-accurate lint already
  applies). Known limitation: it does not follow a field access or a more
  complex expression before `[` (`self.buf[i]`, `(a + b)[i]`) — a false
  negative, not a false positive, and it is one heuristic layer, not a
  parser, same as the rest of the script. The keyword denylist also
  excludes `mut` (`&mut [u8]` parameter/return types, `&mut [1u8, 2]`
  borrowed mutable array literals) and `as` (`x as [T; N]`) — both looked
  like an identifier directly before `[`, the same shape as real indexing,
  until a second review pass (PR #207) found the false positive.
- **Item-scope terminator with an array type in the signature (PR #207
  review).** The item-scope scan that locates an `fn`/`impl`/`struct`/
  `type`/`thread_local!` item's extent originally stopped at the first
  literal `{` or `;`, full stop. An array-type parameter or return type
  (`fn check(v: &[u8; 2]) -> u8`) has a `;` INSIDE `[u8; 2]` that is not
  the signature's terminator; the naive scan stopped there, computed a
  scope ending mid-signature, and the function's real body — with its
  real indexing — fell outside `cfg_test_scope` entirely, escaping the
  indexing check. `find_item_terminator` now tracks `(`/`[` nesting depth
  and only accepts a `{`/`;` at depth 0, so a `;` nested inside a type is
  correctly skipped. `<...>` generics are deliberately not depth-tracked
  (ambiguous with comparison operators outside a signature); a `{`/`;`
  nested only inside one is a residual, documented limitation.
- A narrow, reviewed exception is still an exception, not a fix: the
  `f64`-to-integer boundary casts in `src/utils/value.rs` carry a
  function-scoped `#[allow(clippy::cast_possible_truncation,
  clippy::cast_sign_loss)]` with a comment naming the preceding range check
  that makes the cast exact; `src/utils/uuid.rs`'s `DECIMAL_RADIX` carries
  the script's own `panic-policy-allow-saturating` marker for the same
  reason (a provably-exact, compile-time-only value). Every other finding
  the gate would otherwise raise on `main` at the time of #173 was fixed,
  not allowed.

## Tests

`src/price_level/tests/fallible_growth.rs` injects reservation refusals
through the `cfg(test)`-only `utils::alloc::test_seam` (no production knob)
and checks the typed error with queue, counters, caller buffers and the
`MatchResult` prefix preserved.

`src/price_level/tests/caller_boundaries.rs` and
`src/orders/tests/order_type.rs` use deliberately panicking subscribers,
formatting destinations, payload `Clone` impls and `map_extra_fields`
closures, with test-only `catch_unwind`, to pin the behaviour above.
`scripts/check_panic_policy.py --self-test` (`scripts/panic_policy_fixtures/`)
pins the gate's own scanner behaviour: which forms fail, which test shapes
pass, and that comments / string literals mentioning a forbidden form in
prose are never mistaken for code. Each macro-delimiter form
(`(...)`/`{...}`/`[...]`, and whitespace before `!`) has its own
single-violation fixture, so one caught form cannot mask another that was
missed; separate fixtures also cover a char literal containing `"` or `{`/
`}`, escaped and Unicode-escaped char/byte-char literals, lifetimes/labels,
and raw strings, each immediately followed by a real violation the scanner
must still catch. The `#[cfg(test)]`-scoped indexing check has its own
fail/pass pairs, including the exact `fn check(v: &[u8]) -> u8 { v[1] }`
shape and a `mod test_seam { ... }` variant, against array-type/array-
literal and keyword-prefixed-literal shapes that must not be flagged.
