Rules for writing Rust in a production, lock-free price-level library
(`pricelevel`). All code, comments, docs, commit messages, and PR
descriptions in English.

`pricelevel` is the per-price-level building block for a limit order book:
one `PriceLevel` owns the orders resting at a single price, matches an
incoming taker against that queue, and tracks atomic quantity counters and
execution statistics. It is **synchronous** and built on lock-free data
structures and atomics — there is no async, no tokio, no networking, and no
feature flags. Keep it that way. The complete public methods are not all
lock-free; see Concurrency for the documented locks.

---

## Compiler Attributes

### #[must_use]
- All pure / computed accessors: `price()`, `visible_quantity()`,
  `hidden_quantity()`, `total_quantity()`, `order_count()`, `is_complete()`,
  `trade_id()`, `executed_quantity()`, `executed_value()`, `average_price()`,
  and the `Price` / `Quantity` / `TimestampMs` math and conversion helpers.
- Validation / query functions and snapshot / statistics constructors.
- `Result`-returning functions whose outcome the caller MUST handle:
  `total_quantity() -> Result<u64, PriceLevelError>`,
  `snapshot_to_json() -> Result<String, PriceLevelError>`, and the other
  snapshot / restore / checked-arithmetic returns. (`match_order` itself
  returns `MatchResult` directly, not a `Result` — failures inside it surface
  via the checked-arithmetic accessors on `MatchResult`.)

### #[inline] / #[inline(always)] / #[inline(never)]
- `#[inline]`: small frequent functions — newtype accessors
  (`Price::as_u128`, `Quantity::as_u64`), enum conversions, `OrderQueue`
  length / emptiness checks, comparison helpers.
- `#[inline(always)]`: ONLY proven hot paths — the inner step of
  `PriceLevel::match_order` and `OrderQueue` push / pop. Needs a Criterion
  benchmark before it goes in.
- `#[inline(never)]`: error construction, logging helpers, snapshot
  serialization / restoration.
- No attribute: mid-size functions (10–50 lines).

### #[cold]
- Error construction helpers, validation failures, checksum-mismatch paths,
  deserialization-failure paths, defensive branches that return typed errors.

### #[repr]
- `#[repr(u8)]`: small enums with stable values (`Side`, `TimeInForce`,
  `OrderStatus`). Already applied to `Side` and `TimeInForce` — keep it.
- `#[repr(C)]`: only if a struct crosses an FFI / `cdylib` boundary — the
  crate ships `crate-type = ["cdylib", "rlib"]`, so flag explicitly if you
  expose a type across that boundary.

### #[derive] — exact order
```
Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default
```
Derive only what is needed. No `Ord` if ordering is meaningless. No `Default`
if no sensible default exists. `Copy` only on small POD types (newtype
wrappers around `u64` / `u128`). `OrderType<T>` carries an `Arc`-shared
payload — do not slap `Copy` on it.

Serde conventions:
- `#[serde(rename_all = "snake_case")]` on public-facing DTOs unless a
  recorded fixture requires a specific casing.
- `#[serde(transparent)]` for single-field newtypes (`Price`, `Quantity`,
  `TimestampMs`).
- `#[serde(deny_unknown_fields)]` on deserialized input DTOs. **Do NOT** set
  it on snapshot-restoration DTOs (`PriceLevelSnapshot`) that must stay
  forward-compatible.

---

## Type Safety

- Use the crate newtypes at every domain boundary: `Price` (`u128`),
  `Quantity` (`u64`), `TimestampMs` (`u64`), `Id`, `Side`, `TimeInForce`,
  `OrderType`. Do not pass raw `u128` / `u64` for those concepts across
  module boundaries.
- Constructors: `new()` for infallible construction, `try_new()` returning
  `Result<Self, PriceLevelError>` for validation. No `pub` fields that can
  violate invariants — the v0.7 surface made execution / snapshot fields
  private; keep them private and expose accessors.
- Identifiers go through the `Id` enum (`Uuid` / `Ulid` / `Sequential`),
  never raw `Uuid`. Trade IDs come from `UuidGenerator`.
- Monetary / size math stays on `Price` / `Quantity`. Do NOT drop to raw
  `f64` in the matching path. `f64` appears only in analytics
  (`average_price`, waiting-time statistics).
- Use `u64` for counts and non-negative integer parameters. `NonZeroU64` /
  `NonZeroUsize` where zero is structurally invalid (replenish amounts,
  batch sizes).

---

## Arithmetic

- Integer arithmetic in production, including offsets, lengths and capacities:
  use `checked_add`, `checked_sub`, `checked_mul`, `checked_div`, `checked_rem`,
  `checked_shl`, `checked_shr` and `checked_neg` as applicable. Handle overflow,
  zero divisors, signed minimum divided by `-1`, and invalid shift counts as
  `PriceLevelError::InvalidOperation`; never rely on debug checks or release
  wrapping. Handle fallible integer conversions with `TryFrom` / `TryInto`
  rather than narrowing `as` casts or unwrapping a conversion result.
  v0.7 made `total_quantity()`, `executed_quantity()`, `executed_value()`,
  and `add_trade()` return `Result` for exactly this reason — never silently
  saturate or wrap.
- Never `saturating_*` or `wrapping_*` on quantity / value / counter state.
- `f64` arithmetic in analytics (`average_price`, average waiting time):
  guard against NaN / Inf at the boundary. Return `Result` or `Option`
  rather than propagating NaN.
- Guard floating-point division explicitly: zero executed quantity means no
  average price. Integer division and remainder follow the checked rules above.

---

## Error Handling

- `PriceLevelError` is the single crate error enum. It is **hand-implemented**
  (`impl Display`, `impl Debug`, `impl std::error::Error`) — this crate does
  **NOT** depend on `thiserror`, and must NOT depend on `anyhow`. New
  variants extend the same enum with hand-written `Display` arms.
- Current variants: `ParseError`, `InvalidFormat`, `InvalidFieldValue`,
  `InvalidOperation`, `SerializationError`, `DeserializationError`,
  `ChecksumMismatch`. Map new failure modes onto these where they fit;
  add a variant only when no existing one is honest.
- Error messages: lowercase, human-readable, include the offending value
  (`price`, `order_id`, `field`, `expected` / `actual` checksum) when
  possible.
- Wrap lower-level failures explicitly — `serde_json::Error` →
  `SerializationError` / `DeserializationError` with the message preserved.
- Follow the Production Panic Policy below on every error and invariant path.
  Use `?`, pattern matching and `.ok_or_else()` to propagate typed failures.
- Match / trade failures MUST be mapped to typed `PriceLevelError` variants,
  never returned as opaque `serde_json::Value` on the public surface.

---

## Production Panic Policy

Production code must not initiate a panic, including on invalid input, failed
invariants, dependency errors or exceptional branches. This applies in debug
and release builds and when production functions are compiled or called by
tests. Calling a branch "unreachable" does not exempt it. Tests and fixtures
that exist exclusively for testing may panic, assert and unwrap freely; see
Testing below.

- Never use `.unwrap()`, `.expect()`, `.unwrap_err()` or `.expect_err()` in
  production, even for a value believed to be valid.
- Never use `panic!`, `todo!`, `unimplemented!`, `unreachable!`, `panic_any`
  or `resume_unwind`. All `assert!`, `assert_eq!`, `assert_ne!` and
  `debug_assert!` / `debug_assert_eq!` / `debug_assert_ne!` calls are forbidden
  in production, including checks intended only for development builds.
- Use checked access such as `.get()` / `.get_mut()` instead of indexing or
  slicing expressions on arrays, slices, collections or strings. Validate
  ranges and UTF-8 boundaries before string operations; return an error for
  invalid offsets. Apply the Arithmetic rules to index and range calculations.
- Handle capacity arithmetic and conversions with checked operations. Use
  `try_reserve` / `try_reserve_exact` for fallible collection growth where
  applicable, and map capacity/allocation errors to a typed failure. Do not
  assume a length hint or an input-derived capacity is safe.
- Use fallible runtime borrow operations such as `RefCell::try_borrow` and
  `try_borrow_mut`; handle conflicts explicitly. Review collection operations,
  atomic orderings, time/iterator arithmetic, serialization and dependency
  calls for their documented panic conditions. This list is not exhaustive:
  every reachable operation, including error formatting and cleanup, must be
  reviewed. Apart from the explicitly forbidden forms above, an API with
  documented panic conditions may be called only when type invariants or
  preceding checked validation establish all required preconditions; otherwise
  use a fallible alternative or redesign the path.
- Review implicit trait calls and callbacks, including `Clone`, `Drop`,
  formatting, hashing and comparison. Crate-owned implementations must follow
  this policy. Arbitrary caller-supplied implementations cannot be globally
  certified: document their obligation not to panic, audit where they execute
  relative to locks and mutations, and state the boundary's limits instead of
  promising to prevent every external panic.
- Return `PriceLevelError` or the API's documented typed failure outcome.
  Preserve queue, counter and result invariants on failure: validate and reserve
  before committing mutations, or provide a consistent rollback. Do not hide
  invariant failures by silently defaulting, dropping state or returning success.
- Never evade this policy with unchecked `unsafe` operations, `catch_unwind`,
  `panic = "abort"`, deliberate process termination, or disabled overflow
  checks. An allocator's unrecoverable OOM abort is distinct from a Rust panic;
  this policy does not promise recovery from process-wide resource exhaustion.
  Handle allocation failures through fallible APIs where they are available.

---

## Concurrency

The engine is **synchronous** and built on lock-free structures. There is no
async runtime, no tokio, no networking. The ordered index and the quantity /
statistics counters are lock-free; the complete public methods are not all
lock-free. Documented exceptions: the per-level `RwLock` that gives FOK
matching level-wide exclusion (admissions and updates take its shared side),
the DashMap shard write lock taken per maker entry during matching and
cancellation, and the `portable_atomic::AtomicU128` global-lock fallback on
targets without a native 128-bit CAS. Do not describe a method as lock-free
when it takes one of these.

- Lock-free primitives only: `crossbeam-skiplist` (a `SkipMap` ordered index
  backs `OrderQueue`, keyed by a monotonic sequence for price-time priority),
  `dashmap::DashMap` (order storage), and atomics (`AtomicU64`, `AtomicUsize`,
  `AtomicBool`, and `portable_atomic::AtomicU128` for the `value_executed`
  statistics accumulator, issue #140).
  Reach for `std::sync::RwLock` / `Mutex` only where no lock-free structure
  fits and contention is rare — and justify it in review.
- **Atomic ordering is explicit and deliberate.** Never default to
  `Ordering::SeqCst` out of laziness, and never use `Ordering::Relaxed` on a
  load / store that establishes a happens-before relationship with another
  field. Counter increments that gate visibility of queue mutations need
  `Acquire` / `Release` pairing. State the reasoning in a comment next to any
  non-`SeqCst` ordering.
- The atomic quantity counters (`visible_quantity`, `hidden_quantity`) and
  the `OrderQueue` contents MUST stay mutually consistent under concurrent
  `add_order` / `match_order` / `remove`. A counter that can transiently
  disagree with the queue is a bug, not a tolerance.
- No blocking work inside a tight CAS / atomic-update loop. Keep
  compare-exchange retry bodies allocation-free.
- A `snapshot()` taken under concurrent mutation must be internally
  consistent (no torn read where the counter and the order list disagree).
  If a fully consistent snapshot requires a brief guard, document why.
- `unsafe` is NOT used for concurrency (or anywhere in production code) — see
  the unsafe policy below.

---

## Minimize Copies

- Orders are shared as `Arc<OrderType<()>>`. Clone the `Arc`, not the order.
- Move ownership when storing; references for read-only access.
- Reserve storage before hot loops when the size is known or estimable
  (`MatchResult` trade vectors, `snapshot_orders`, queue materialization).
  Check capacity calculations and use fallible reservation where applicable,
  following the Production Panic Policy; a capacity hint is not validation.
- Prefer the iterator surface (`iter_orders`) over the materializing one
  (`snapshot_orders` / `snapshot_vec`) on read paths — v0.7 turned
  `iter_orders` into an `impl Iterator` specifically to cut hot-path
  allocations. Do not regress it back to a `Vec` return.
- Avoid cloning whole snapshots; snapshot once and read from it.

---

## Code Organization

- One concern per file. Preserve the current layout:
  - `src/orders/` — order model (`order_type.rs`, `base.rs`, `pegged.rs`,
    `status.rs`, `time_in_force.rs`, `update.rs`).
  - `src/price_level/` — the level itself (`level.rs`, `order_queue.rs`,
    `entry.rs`, `snapshot.rs`, `statistics.rs`).
  - `src/execution/` — execution results (`match_result.rs`, `trade.rs`,
    `list.rs`).
  - `src/utils/` — domain newtypes and helpers (`value.rs`, `id.rs`,
    `uuid.rs`, `logger.rs`).
  - `src/errors/` — `PriceLevelError`.
- Group by domain concept, not by mechanism.
- Re-export the user-facing API from `lib.rs` and `prelude.rs`. Keep the
  prelude minimal and stable. Any change to the prelude or the `lib.rs`
  re-export block is a public-API change.
- Constants: module-level `const` for crate-wide values
  (`DEFAULT_RESERVE_REPLENISH_AMOUNT`, snapshot format version), associated
  `const` for type-scoped values. No magic numbers in matching logic — name
  them.

---

## Logging & Observability

- Use `tracing` for all logging. Never `println!`, `eprintln!`, `dbg!`, or
  the `log` crate.
- `setup_logger()` (in `utils/logger.rs`) is the test / example / bench entry
  point and reads `LOGLEVEL`. Library code must NOT install a global
  subscriber on a normal call path.
- Structured fields, not string interpolation:
  `tracing::debug!(order_id = %id, side = ?side, price = %price, "matched")`.
- Log levels:
  - `ERROR`: unrecoverable failures — checksum mismatch on restore,
    arithmetic overflow that aborts a match.
  - `WARN`: recoverable anomalies — rejected order, deserialization fallback.
  - `INFO`: high-level lifecycle — level created, snapshot taken / restored.
  - `DEBUG`: per-operation internals — match steps, add / remove, status
    transitions.
  - `TRACE`: per-order queue traversal — disabled by default.
- The generic `T` on `OrderType<T>` can carry arbitrary data. If it ever
  holds user-identifying data, use a redacting `Debug` before logging it.

---

## Documentation

- Every `pub` item: `///` doc comment with a one-line summary, then details.
  For `Result`-returning functions add a `# Errors` section. Document failure
  outcomes and requirements on external trait implementations or callbacks.
  A `# Panics` section does not authorize a production panic or an exception
  for a supposedly unreachable branch; remove the panic path instead.
- Include units: "price ticks", "quantity units", "milliseconds".
- Examples should compile. Use `?`, not `.unwrap()` (test fixtures aside).
- `README.md` is generated from `src/lib.rs` module-level docs via
  `make readme` (cargo-readme). Update `lib.rs` first, then regenerate. The
  v0.6 → v0.7 migration guide lives in `lib.rs` — keep it current when the
  public surface moves.
- `cargo clippy -- -W missing-docs` (the `make doc` target) must be clean —
  fix missing-doc warnings before merging.

---

## Matching / Price-Level Discipline

The matching at a single price level IS the product. Wire-level correctness
of `MatchResult` and `Trade` matters as much as throughput.

- `PriceLevel::match_order` consumes resting orders in **strict FIFO order**
  (price-time priority within the level). Order of consumption and of emitted
  trades MUST be deterministic for a fixed input — a non-deterministic stream
  breaks snapshot/replay equivalence downstream.
- Every order type ships with unit tests covering: empty level, partial fill,
  full fill, and the order-type-specific branch (iceberg / reserve
  replenishment, post-only rejection, market-to-limit conversion,
  pegged / trailing-stop reprice).
- `Time-in-force` semantics are exact: `Ioc` fills what it can and discards
  the rest; `Fok` fills completely or not at all (no partial state left
  behind); `Gtc` rests; `Gtd` / `Day` carry expiry. Test the boundary
  between "filled" and "killed".
- `MatchResult` carries `trades`, `remaining_quantity`, `is_complete`, and
  `filled_order_ids`. These must agree: `is_complete` ⇔ `remaining_quantity
  == 0`; `executed_quantity()` equals the sum of trade quantities.
- `Trade` records both `maker_order_id` and `taker_order_id`, the matched
  `price`, `quantity`, `taker_side`, and `timestamp`. Do not drop fields.
- Atomic counters (visible / hidden quantity, statistics) update in lockstep
  with the queue mutation they describe.

---

## Snapshot / Persistence

- `PriceLevelSnapshotPackage` wraps a `PriceLevelSnapshot` with a **SHA-256
  checksum**. `snapshot_to_json()` embeds the checksum; `from_snapshot_json()`
  validates it and returns `PriceLevelError::ChecksumMismatch { expected,
  actual }` on failure — never panics.
- Snapshots MUST round-trip: `level.snapshot_to_json()` →
  `PriceLevel::from_snapshot_json(&json)` reproduces an equivalent level
  (price, visible / hidden quantity, order queue contents and order, stats).
  Every new field added to the level adds a corresponding preservation path —
  non-negotiable.
- Bumping the snapshot format version forces a migration note in `lib.rs`
  (and `CHANGELOG.md` if the repo adopts one) plus a compatibility test
  covering the previous format.

---

## Performance Discipline

Hot paths: the `match_order` inner loop, `OrderQueue` push / pop / find /
remove, atomic counter updates, and `iter_orders` traversal.

- Zero heap allocation inside the inner match loop where feasible. Reuse
  buffers; pre-size the `MatchResult` trade vector.
- `iter_orders` is the zero-alloc read path. Do not regress it to a `Vec`
  return; `snapshot_orders` / `snapshot_vec` exist for when a materialized
  `Vec` is genuinely needed.
- Prefer monomorphized generics over `dyn Trait` on the hot path. Use match
  dispatch over the bounded `OrderType` / `Side` / `TimeInForce` sets.
- Benchmark with Criterion before claiming a speedup. No perf PR without
  numbers. `make bench` (cargo-criterion) is the entry point; compare with
  `make bench-compare`, save a baseline with `make bench-save`. Benches live
  under `benches/{price_level,concurrent,simple}/` wired through
  `benches/mod.rs`.
- Keep atomic compare-exchange retry loops tight and allocation-free.

---

## Safety / `unsafe` Policy

- There is **no `#![deny(unsafe_code)]`** on `lib.rs`, because Rust 2024 made
  `std::env::set_var` `unsafe`, and the only `unsafe` in the crate is the
  `env::set_var("LOGLEVEL", …)` calls inside the `utils/logger.rs` **test
  helpers**. Production code is `unsafe`-free.
- Do NOT introduce `unsafe` in production paths (orders, price_level,
  execution, utils newtypes) without explicit user approval. Lock-free
  concurrency here is built on `crossbeam-skiplist` / `dashmap` / atomics — not raw
  pointers.
- No hard-coded credentials anywhere. The crate has no network surface, so
  there are no connection strings to leak — keep it that way.

---

## Testing

- Unit tests are co-located under each module's `tests/` submodule
  (`src/<module>/tests/*.rs`, wired via `mod tests;` under `#[cfg(test)]`).
- Cross-module / integration tests live under `tests/unit/` — the canonical
  integration tree wired in `Cargo.toml` (`[[test]] name = "tests" path =
  "tests/unit/mod.rs"`).
- Runnable, end-to-end examples live in the `examples/` workspace member and
  are exercised by `make integration-examples`.
- Every test covers the happy path AND all documented error cases
  (overflow → `InvalidOperation`, bad JSON → `DeserializationError`,
  tampered snapshot → `ChecksumMismatch`).
- Name tests `test_<unit>_<scenario>_<expected>`, e.g.
  `test_match_order_partial_fill_ioc_discards_remainder`.
- Panics, assertions, `debug_assert!`, `unwrap` / `expect` and their error
  variants are allowed in tests and their test-only fixtures/helpers, including
  deliberate panic tests. This permission does not extend to production
  functions, including their `cfg(test)` branches, or to helpers shared with
  production. Compiling or calling production code from a test does not exempt
  it from the Production Panic Policy.
- Tests of production failure paths must check typed failures and preserved
  state; an expected production panic is not an acceptable error contract.
- Concurrency tests: seed any RNG deterministically; use
  `std::thread::Barrier` to start workers in lockstep; never `sleep` for
  synchronization.

---

## Pre-Submission Checklist

All must pass — failing any means not ready:

- `make pre-push` (runs `fix`, `lint-fix`, `fmt`, `lint-panic`, `test`,
  `readme`, `doc` — `lint-fix` before `fmt` so a `clippy --fix` rewrite is
  reformatted, not left dirty)
  OR the explicit five:
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `make lint-panic` (`scripts/check_panic_policy.py`; see below)
  - `cargo fmt --all --check`
  - `cargo test` (the `make test` target sets `LOGLEVEL=WARN`)
  - `cargo build --release` (zero warnings)
- Production Panic Policy reviewed in debug and release configurations:
  no explicit panic/assertion forms or panicking extraction/indexing; checked
  arithmetic, capacities and dependency preconditions; external boundaries
  documented; failure paths preserve state. Test-only code may panic.
  `[lints.clippy]` (`Cargo.toml`) plus `clippy.toml`'s `allow-*-in-tests` keys
  enforce most of this automatically; `make lint-panic` additionally denies
  the `assert!` / `debug_assert!` macro family (no clippy restriction lint
  covers them) and `saturating_*` / `wrapping_*` on production state
  (including inside a standalone `#[cfg(test)]` helper that is not a `mod
  tests { ... }` block — clippy's own test heuristic would otherwise exempt
  it too). Neither tool proves the crate is panic-free; the manual review
  above still applies, and `doc/panic-boundaries.md` covers what automated
  coverage cannot see (callback obligations, dependency preconditions,
  allocator OOM).
- `#[must_use]` on all pure functions and query accessors
- `#[inline]` on small hot-path helpers, `#[cold]` on error paths
- No new production `unsafe`
- Module boundaries respected (see `CLAUDE.md` — `errors` / `utils` are
  leaves; `execution` and `price_level` build on `orders` + `utils`; nothing
  imports from `prelude`)
- Tests cover happy path AND all error cases
- Doc comments on all `pub` items; `# Errors` on fallible functions
- `README.md` regenerated via `make readme` if the public surface or
  `lib.rs` module docs changed; migration guide in `lib.rs` updated on a
  breaking change
- Perf-relevant change includes Criterion numbers before / after

---

## DO NOT

- Introduce a production panic, assertion or unchecked failure path, or bypass
  the Production Panic Policy. Test-only panic/assertion code is permitted.
- Add dependencies without explicit approval (current set: `tracing`,
  `tracing-subscriber`, `serde`, `serde_json`, `crossbeam-skiplist`, `uuid`, `ulid`,
  `dashmap`, `sha2`, `portable-atomic` (for `AtomicU128`, issue #140); dev:
  `criterion`, `proptest`, `bincode`).
- Add `thiserror` or `anyhow` — `PriceLevelError` is hand-implemented and all
  errors are concrete.
- Use `println!`, `eprintln!`, `dbg!`, or the `log` crate — use `tracing`.
- Add an async runtime, tokio, or any networking — this crate is synchronous
  and lock-free by design.
- Skip any pre-submission check.
- Use `saturating_*` or `wrapping_*` on quantity / value / counter state.
- Install a global `tracing` subscriber from a normal library path.
- Allocate inside the inner match loop or a CAS retry body when avoidable.
- Regress `iter_orders` back to a `Vec` return.
- Default to `Ordering::SeqCst` without thought, or use `Relaxed` where a
  happens-before relationship is required.
- Drop fields from `Trade` or let `MatchResult` fields disagree
  (`is_complete` vs `remaining_quantity`, trade sum vs `executed_quantity`).
- Bump the snapshot format version without a migration note and a
  compatibility test.
- Introduce production `unsafe` without explicit user approval.
