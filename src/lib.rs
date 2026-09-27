#![allow(unknown_lints)]
#![allow(clippy::literal_string_with_formatting_args)]
#![warn(clippy::missing_errors_doc)]

//!  # PriceLevel
//!
//!  A price level implementation for limit order books in Rust. A [`PriceLevel`] owns every order resting at one price: it matches an incoming taker against that queue in strict price-time order, tracks visible / hidden quantity counters, records execution statistics, and round-trips through checksum-protected snapshots. It is the building block an order book composes across prices, not a full order book.
//!
//!  The crate is synchronous and built from lock-free components (a `crossbeam-skiplist` ordered index and atomic counters) plus a small number of documented locks. The complete public methods are **not** lock-free: see [Concurrency Model](#concurrency-model) for which method takes which lock.
//!
//!  ## Features
//!
//!  - Strict price-time (FIFO) matching at a single price, with deterministic trade emission
//!  - Support for diverse order types including standard limit orders, iceberg orders, post-only, fill-or-kill, and more
//!  - Thread-safe concurrent admissions, updates (cancel / resize) and reads alongside one logical matcher per level (see [Concurrency Model](#concurrency-model))
//!  - Lock-free ordered index (`crossbeam-skiplist`) and atomic quantity / statistics counters; order storage is a sharded `DashMap`
//!  - Checked arithmetic on the quantity / value accessors (`total_quantity`, `executed_quantity`, `executed_value`) with typed errors; removing the remaining production panic paths (for example the `snapshot()` aggregate assertions) is tracked in #161
//!  - Checksum-protected (SHA-256) snapshots for persistence and recovery
//!  - Designed with domain-driven principles for financial markets
//!  - Comprehensive test suite, including concurrent usage scenarios
//!  - Optimized statistics tracking for each price level
//!
//!  Intended as a building block for matching engines, market data systems, algorithmic trading platforms, and financial exchanges.
//!
//!  ## Supported Order Types
//!
//!  The library provides comprehensive support for various order types used in modern trading systems:
//!
//!  - **Standard Limit Order**: Basic price-quantity orders with specified execution price
//!  - **Iceberg Order**: Orders with visible and hidden quantities that replenish automatically
//!  - **Post-Only Order**: Orders that will not execute immediately against existing orders
//!  - **Trailing Stop Order**: Orders that adjust based on market price movements
//!  - **Pegged Order**: Orders that adjust their price based on a reference price
//!  - **Market-to-Limit Order**: Orders that convert to limit orders after initial execution
//!  - **Reserve Order**: Orders with custom replenishment logic for visible quantities
//!
//!  ## Time-in-Force Options
//!
//!  The library supports the following time-in-force policies:
//!
//!  - **Good Till Canceled (GTC)**: Order remains active until explicitly canceled
//!  - **Immediate Or Cancel (IOC)**: Order must be filled immediately (partially or completely) or canceled
//!  - **Fill Or Kill (FOK)**: Order must be filled completely immediately or canceled entirely
//!  - **Good Till Date (GTD)**: Order remains active until a specified date/time (Unix milliseconds)
//!  - **Day Order**: Order valid only for the current trading day
//!
//!  ## Implementation Details
//!
//!  - **Thread Safety**: Lock-free ordered index and atomic counters, a sharded `DashMap` for order storage (per-shard locks), and a per-level reader-writer guard used to make fill-or-kill all-or-nothing. See [Concurrency Model](#concurrency-model)
//!  - **Order Queue Management**: Specialized order queue keeping strict price-time priority via a lock-free `crossbeam-skiplist` ordered index keyed by insertion sequence
//!  - **Statistics Tracking**: Each price level tracks execution statistics in real-time
//!  - **Snapshot Capabilities**: Create point-in-time snapshots of price levels for market data distribution
//!  - **Efficient Matching**: Matching walks the ordered index from the front in price-time order
//!  - **Support for Special Order Types**: Custom handling for iceberg orders, reserve orders, and other special types
//!
//!  ## Price Level Features
//!
//!  - **Atomic Counters**: Uses atomic types for thread-safe quantity tracking
//!  - **Efficient Order Storage**: Optimized data structures for order storage and retrieval
//!  - **Visibility Controls**: Separate tracking of visible and hidden quantities
//!  - **Performance Monitoring**: Built-in statistics for monitoring execution performance
//!  - **Order Matching Logic**: Sophisticated algorithms for matching orders at each price level
//!
//! ## Concurrency Model
//!
//! "Lock-free" describes **components**, not complete public methods.
//!
//! | Component | Progress |
//! |-----------|----------|
//! | Ordered index (`crossbeam-skiplist` `SkipMap`, insertion sequence to order id) | Lock-free |
//! | Quantity, count, topology and most statistics counters (`std` atomics) | Lock-free |
//! | `value_executed` statistics accumulator (`portable_atomic::AtomicU128`) | Lock-free where the CPU has a native 128-bit CAS (aarch64; x86_64 with `cmpxchg16b`); elsewhere `portable-atomic` falls back to a global lock for this one counter |
//! | Order storage (`dashmap::DashMap`, order id to order) | Sharded reader-writer locks, one per shard |
//! | Fill-or-kill guard (`std::sync::RwLock<()>`, one per level) | Blocking reader-writer lock |
//!
//! What each public method acquires:
//!
//! | Method | Locks taken |
//! |--------|-------------|
//! | [`PriceLevel::match_order`], `Gtc` / `Ioc` / `Gtd` / `Day` taker | The `DashMap` shard **write** lock of each maker entry it fills, one at a time (the internal `OrderQueue::match_front` step). No level-wide guard |
//! | [`PriceLevel::match_order`], `Fok` taker | The level-wide fill-or-kill guard's **exclusive** side across its `O(depth)` feasibility dry-run and sweep, plus the per-maker shard write locks above |
//! | [`PriceLevel::match_order`], post-only taker | No sweep and no maker write lock; its depth scan iterates order storage under `DashMap` shard **read** locks |
//! | [`PriceLevel::add_order`] | Fill-or-kill guard's **shared** side, plus the shard write lock of the new id |
//! | [`PriceLevel::update_order`] (every [`OrderUpdate`] variant) | Fill-or-kill guard's **shared** side, plus the shard write lock of the target id |
//! | [`PriceLevel::snapshot`] | Fill-or-kill guard's **shared** side, plus `DashMap` shard read locks while it materializes the orders |
//! | Counter accessors ([`PriceLevel::visible_quantity`], [`PriceLevel::order_count`], statistics) | Atomic loads only (advisory, eventually consistent; `value_executed` subject to the fallback above) |
//!
//! The supported execution model:
//!
//! - **One logical matcher per level.** Two concurrent [`PriceLevel::match_order`]
//!   calls on the same level are not made safe by the crate; the caller must
//!   serialize them (an order book typically matches a level from one thread).
//! - **Concurrent mutators are supported.** [`PriceLevel::add_order`] and
//!   [`PriceLevel::update_order`] may run from any number of threads, concurrently
//!   with the matcher and with each other.
//! - **The maker entry is the serialization point.** The matcher applies each fill
//!   while holding that maker's `DashMap` shard write lock, the same lock a cancel
//!   or resize of that order takes, so a cancel racing the fill either fully wins
//!   or fully loses; it is never lost. Admissions, cancels and resizes of other
//!   orders that hash to the same shard also wait on that lock.
//! - **Fill-or-kill excludes every mutator on the level.** A `Fok` match holds the
//!   level guard exclusively for its whole dry-run and sweep, so admissions,
//!   updates and snapshots on that level block for an `O(depth)` section. The
//!   other time-in-force paths skip that guard, but skipping it is **not** the
//!   absence of locking: they still take the per-maker shard lock.
//! - **Readers are always allowed.** Counter reads never block. A
//!   [`PriceLevel::snapshot`] waits only behind an in-flight fill-or-kill or a
//!   held shard lock; it walks the shards without a transaction over the whole
//!   level, so under concurrent same-side resizes it is not a linearizable
//!   point-in-time view (tracked in #162).
//!
//! ## Caller-Supplied Code
//!
//! Some operations run code the crate does not own: trait impls on a generic
//! [`OrderType<T>`] payload, the [`OrderType::map_extra_fields`] closure, a
//! caller's formatter destination, serializer or deserializer, the body of an
//! [`PriceLevel::iter_orders`] loop, and the process-installed `tracing`
//! subscriber. Trait bounds cannot express "does not panic", so that is a
//! caller obligation: supplied code **must not panic** and must not re-enter
//! the level that is calling it, except where documented.
//!
//! - **Generic payloads are pure.** `OrderType<T>` utilities hold no lock and
//!   mutate no library state. The engine stores only `OrderType<()>`, so no
//!   payload code runs under its locks.
//! - **No caller code under a shard write lock.** Formatting and serializing a
//!   level or queue materialize first and hold no lock, and no `tracing` event
//!   is emitted under a `DashMap` shard write lock or between a match step's
//!   queue commit and its counter bookkeeping.
//! - **Remaining boundaries.** `iter_orders` holds a shard read lock while the
//!   loop body runs. A subscriber panic during a sweep loses the `MatchResult`
//!   for trades already committed, and during a `Fok` sweep it poisons the
//!   level.
//! - **No recovery promise.** The library does not catch caller panics,
//!   installs no panic hook and never aborts deliberately. An allocator OOM
//!   abort is not a typed error.
//!
//! The per-call inventory (guard held, partial mutation, unwind effect) is in
//! [`doc/panic-boundaries.md`](https://github.com/joaquinbejar/PriceLevel/blob/main/doc/panic-boundaries.md).
//!
//! ## Performance Evidence
//!
//! This crate currently publishes **no** throughput or latency figures. The
//! Criterion benchmarks under `benches/` (`make bench`) are the supported way to
//! measure the build you run, on your hardware and toolchain.
//!
//! ### Withdrawn historical figures
//!
//! Releases up to 0.9.x printed a "High-Frequency Trading Simulation" table and a
//! contention table produced by the `hft_simulation` and `contention_test`
//! examples. Those numbers are withdrawn and excluded from any current performance
//! conclusion:
//!
//! - They have no provenance: no commit, compiler version, build profile, or
//!   workload manifest was recorded.
//! - They were internally inconsistent: the table reported 237,347.51 total
//!   operations per second, while the analysis below it claimed more than 264,000.
//! - The simulation ran ten taker threads calling `match_order` on one shared
//!   level, outside the single-matcher contract above.
//! - The example's periodic counter flush over-counted matches and cancellations
//!   whenever a thread's success count sat on a flush boundary, and the contention
//!   tables counted rejected and missing-order calls as operations.
//! - Aggregate throughput is not an operation latency, so the figures never
//!   supported the "microsecond-level" or production-suitability claims made
//!   alongside them.
//!
//! No replacement run is published in their place.
//!
//! ### Operation accounting for future results
//!
//! Any number published for this crate must state which of these distinct metrics
//! it measures, together with the commit, toolchain, build profile, hardware and
//! workload (thread roles, id ranges, order mix, run length):
//!
//! - **Attempted calls**: every call to a public method, whatever its outcome.
//! - **Successful admissions / cancels / updates**: calls that changed the level,
//!   reported separately from calls that were rejected (for example a duplicate id)
//!   or that targeted a missing order.
//! - **Successful takers**: `match_order` calls that executed a non-zero quantity.
//! - **Emitted fills**: the number of [`Trade`] values produced; one taker may emit
//!   many.
//! - **Whole-lifecycle throughput**: complete order lifecycles (admit, then fill or
//!   cancel) per second.
//!
//! Throughput of any kind is not an operation-latency percentile; a latency claim
//! needs per-operation timing and a reported distribution (for example p50 / p99 /
//! p99.9 / max).
//!
//! ## Changes in v0.8.0
//!
//! - **Price-time priority across partial fills** (issue #39). A partial fill
//!   previously re-queued the resting maker's residual at the *back* of its
//!   price level, so the next aggressor at that price matched a later arrival
//!   instead of the older, partially-filled maker (a wrong `maker_order_id` in
//!   the trade stream). The order queue now keeps strict price-time priority:
//!   the residual stays at the front. Iceberg / reserve replenishment keeps its
//!   existing semantics (a refreshed tranche still loses time priority).
//! - **Internal queue moved to a lock-free `crossbeam-skiplist` ordered
//!   index.** The method surface of [`OrderQueue`] is unchanged, but because
//!   the new index relies on interior mutability, [`OrderQueue`] and
//!   [`PriceLevel`] no longer implement [`std::panic::UnwindSafe`] /
//!   [`std::panic::RefUnwindSafe`] (they remain `Send + Sync`). This is the
//!   only breaking change and is why this release is `0.8.0` rather than a
//!   patch. Callers that wrapped these types in [`std::panic::catch_unwind`]
//!   are affected; nothing else is.
//! - **Matching concurrency contract.** [`PriceLevel::match_order`] assumes a
//!   single logical matcher per level at a time. Concurrent `add_order` /
//!   `update_order` (including a `cancel` of the resting order the matcher is
//!   currently consuming) from other threads are safe and linearizable — the
//!   match and the cancel serialize on the maker's per-entry lock (issue #81),
//!   and a fill-or-kill match additionally takes a level-exclusive guard so it
//!   stays all-or-nothing against those mutators (issue #112). Only two
//!   concurrent `match_order` calls on the *same* level remain the caller's
//!   responsibility to serialize.
//! - **Reserve replenish amounts are now `NonZeroU64`** (issue #70). A
//!   replenish amount of `0` is structurally invalid: it would draw an empty
//!   visible tranche from the hidden quantity, silently leaving nothing
//!   visible. The reserve replenish surface therefore moved from `Quantity`
//!   (which permits `0`) and raw `u64` to [`std::num::NonZeroU64`]:
//!
//!   | v0.8 (before) | v0.8 (now) |
//!   |---------------|------------|
//!   | `ReserveOrder.replenish_amount: Option<Quantity>` | `Option<NonZeroU64>` |
//!   | `DEFAULT_RESERVE_REPLENISH_AMOUNT: u64` | `NonZeroU64` (value `80`) |
//!   | `OrderType::refresh_iceberg(&self, u64)` | `refresh_iceberg(&self, NonZeroU64)` |
//!
//!   Constructing a reserve order with a zero replenish is now impossible at
//!   the type level. Build the amount with [`std::num::NonZeroU64::new`], which
//!   returns an `Option`. For a known-good literal, a compile-time constant is
//!   simplest. For a **runtime** value `n`, match on `NonZeroU64::new(n)` and
//!   treat `None` as an invalid amount to reject — do **not** blindly
//!   `.unwrap()` it (that panics on `0`), and do **not** pass `NonZeroU64::new(n)`
//!   straight into the `Option` field (that silently maps `0` to `None`, which
//!   falls back to the default replenish instead of flagging the bad input). On
//!   the text / JSON deserialization path a `replenish_amount` of `0` is rejected
//!   with a typed [`PriceLevelError::InvalidFieldValue`] (text) or a
//!   deserialization error (JSON) rather than silently accepted — never a panic.
//!   Reading the default as a raw integer now requires
//!   `DEFAULT_RESERVE_REPLENISH_AMOUNT.get()`.
//!
//! ## Migration Guide (v0.6 → v0.7)
//!
//! Version 0.7.0 introduces several intentional breaking changes to improve type safety,
//! correctness, and API ergonomics. This section provides a complete mapping from the old
//! API surface to the new one.
//!
//! ### Execution Domain Rename
//!
//! The execution domain was renamed from `Transaction` to `Trade` to align with standard
//! financial terminology.
//!
//! | v0.6 | v0.7 |
//! |------|------|
//! | `Transaction` | [`Trade`] |
//! | `TransactionList` | [`TradeList`] |
//! | `transaction_id` field | [`Trade::trade_id()`] accessor |
//! | `Transaction:` parsing prefix | `Trade:` parsing prefix |
//!
//! ### Identifier Types
//!
//! Raw `Uuid` identifiers were replaced with the [`Id`] enum, which supports UUID, ULID, and
//! sequential (`u64`) formats. Trade IDs are generated via [`UuidGenerator`].
//!
//! | v0.6 | v0.7 |
//! |------|------|
//! | `Uuid` (raw) | [`Id`] enum (`Uuid`, `Ulid`, `Sequential`) |
//! | `Uuid::new_v4()` | `Id::new()` or `Id::new_uuid()` (v0.10: [`Id::try_new`] / [`Id::try_new_uuid`], see below) |
//! | `u64` order/trade IDs | [`Id::from_u64()`] or [`Id::sequential()`] |
//! | `AtomicU64` trade counter | [`UuidGenerator::next()`] |
//!
//! ### Domain Newtypes
//!
//! Raw numeric primitives used in the public API were replaced with validated domain
//! newtypes. Each provides `new()`, `try_new()`, `Display`, `FromStr`, and serde support.
//!
//! | v0.6 | v0.7 | Inner |
//! |------|------|-------|
//! | `u128` (price) | [`Price`] | `u128` |
//! | `u64` (quantity) | [`Quantity`] | `u64` |
//! | `u64` (timestamp) | [`TimestampMs`] | `u64` |
//!
//! ```rust
//! use pricelevel::{Price, Quantity, TimestampMs};
//!
//! let price = Price::new(10_000);
//! let qty   = Quantity::new(100);
//! let ts    = TimestampMs::new(1_716_000_000_000);
//!
//! // Convert back to primitives
//! assert_eq!(price.as_u128(), 10_000);
//! assert_eq!(qty.as_u64(), 100);
//! assert_eq!(ts.as_u64(), 1_716_000_000_000);
//! ```
//!
//! ### Checked Arithmetic
//!
//! All arithmetic in financial-critical paths now uses checked operations and returns
//! `Result<T, PriceLevelError>` instead of raw values. No silent saturation or wrapping
//! is performed.
//!
//! | Method | v0.6 Return | v0.7 Return |
//! |--------|-------------|-------------|
//! | [`PriceLevel::total_quantity()`] | `u64` | `Result<u64, PriceLevelError>` |
//! | [`MatchResult::executed_quantity()`] | `u64` | `Result<u64, PriceLevelError>` |
//! | [`MatchResult::executed_value()`] | `u128` | `Result<u128, PriceLevelError>` |
//! | [`MatchResult::average_price()`] | `Option<f64>` | `Result<Option<f64>, PriceLevelError>` |
//! | [`MatchResult::add_trade()`] | `()` | `Result<(), PriceLevelError>` |
//!
//! ```rust
//! use pricelevel::{PriceLevel, PriceLevelError};
//!
//! let level = PriceLevel::new(10_000);
//! // total_quantity() now returns Result
//! let total: Result<u64, PriceLevelError> = level.total_quantity();
//! assert_eq!(total.unwrap(), 0);
//! ```
//!
//! ### Private Fields and Accessor Methods
//!
//! All struct fields in the execution and snapshot modules are now private. Use the
//! provided accessor methods instead of direct field access.
//!
//! **Trade:**
//!
//! | v0.6 (field) | v0.7 (accessor) |
//! |--------------|-----------------|
//! | `trade.trade_id` | [`trade.trade_id()`](Trade::trade_id) |
//! | `trade.taker_order_id` | [`trade.taker_order_id()`](Trade::taker_order_id) |
//! | `trade.maker_order_id` | [`trade.maker_order_id()`](Trade::maker_order_id) |
//! | `trade.price` | [`trade.price()`](Trade::price) |
//! | `trade.quantity` | [`trade.quantity()`](Trade::quantity) |
//! | `trade.taker_side` | [`trade.taker_side()`](Trade::taker_side) |
//! | `trade.timestamp` | [`trade.timestamp()`](Trade::timestamp) |
//!
//! **MatchResult:**
//!
//! | v0.6 (field) | v0.7 (accessor) |
//! |--------------|-----------------|
//! | `result.order_id` | [`result.order_id()`](MatchResult::order_id) |
//! | `result.trades` | [`result.trades()`](MatchResult::trades) |
//! | `result.remaining_quantity` | [`result.remaining_quantity()`](MatchResult::remaining_quantity) |
//! | `result.is_complete` | [`result.is_complete()`](MatchResult::is_complete) |
//! | `result.filled_order_ids` | [`result.filled_order_ids()`](MatchResult::filled_order_ids) |
//!
//! **TradeList:**
//!
//! | v0.6 (field) | v0.7 (accessor) |
//! |--------------|-----------------|
//! | `list.trades` (direct `Vec`) | [`list.as_vec()`](TradeList::as_vec) / [`list.into_vec()`](TradeList::into_vec) |
//! | `list.trades.push(t)` | [`list.add(t)`](TradeList::add) |
//! | `list.trades.len()` | [`list.len()`](TradeList::len) |
//! | `list.trades.is_empty()` | [`list.is_empty()`](TradeList::is_empty) |
//!
//! ### Iterator API Changes
//!
//! The `iter_orders()` method now returns an iterator instead of a `Vec`, reducing
//! allocations on the hot path. Use `snapshot_orders()` when a materialized `Vec` is needed.
//!
//! | v0.6 | v0.7 |
//! |------|------|
//! | `level.iter_orders() -> Vec<Arc<OrderType<()>>>` | [`level.iter_orders()`](PriceLevel::iter_orders) `-> impl Iterator` |
//! | (no equivalent) | [`level.snapshot_orders()`](PriceLevel::snapshot_orders) `-> Vec<Arc<OrderType<()>>>` |
//!
//! ### Snapshot Persistence and Recovery
//!
//! Snapshots are now protected with SHA-256 checksums via [`PriceLevelSnapshotPackage`].
//! The full persistence/recovery flow is:
//!
//! ```rust
//! use pricelevel::PriceLevel;
//!
//! let level = PriceLevel::new(10_000);
//!
//! // Serialize to JSON (includes checksum)
//! let json = level.snapshot_to_json().unwrap();
//!
//! // Restore from JSON (validates checksum)
//! let restored = PriceLevel::from_snapshot_json(&json).unwrap();
//! ```
//!
//! ### Compiler Attributes
//!
//! - **`#[must_use]`** is now applied to all pure/computed methods (`price()`, `quantity()`,
//!   `trade_id()`, `order_count()`, `visible_quantity()`, `is_complete()`, etc.).
//!   Ignoring a return value from these methods will produce a compiler warning.
//! - **`#[repr(u8)]`** is applied to small enums exposed in the public API ([`Side`],
//!   [`TimeInForce`]).
//!
//! ### Error Handling
//!
//! [`PriceLevelError`] gained new variants for the expanded error surface:
//!
//! | Variant | Purpose |
//! |---------|---------|
//! | `InvalidOperation { message }` | Checked arithmetic overflow, invalid state transitions |
//! | `SerializationError { message }` | JSON/serde serialization failures |
//! | `DeserializationError { message }` | JSON/serde deserialization failures |
//! | `ChecksumMismatch { expected, actual }` | Snapshot integrity validation failure |
//!
//! ### Quick Migration Checklist
//!
//! 1. Replace `Transaction` / `TransactionList` with [`Trade`] / [`TradeList`].
//! 2. Replace raw `Uuid` with [`Id`]; use [`UuidGenerator`] for trade IDs.
//! 3. Wrap raw price/quantity/timestamp literals with [`Price::new()`](Price::new),
//!    [`Quantity::new()`](Quantity::new), [`TimestampMs::new()`](TimestampMs::new).
//! 4. Replace direct field access on `Trade`, `MatchResult`, `TradeList` with accessors.
//! 5. Handle `Result` returns from `total_quantity()`, `executed_quantity()`,
//!    `executed_value()`, `average_price()`, and `add_trade()`.
//! 6. Replace `iter_orders()` collecting into `Vec` with `snapshot_orders()` if needed.
//! 7. Update snapshot code to use [`PriceLevelSnapshotPackage`] for checksum validation.
//! 8. Address new `#[must_use]` warnings on query methods.
//!
//! ## Migration Guide (deterministic `match_order` timestamp)
//!
//! [`PriceLevel::match_order`] now takes an explicit `timestamp: TimestampMs`
//! argument, inserted **between** `taker_order_id` and the trade-id generator:
//!
//! | Before | After |
//! |--------|-------|
//! | `level.match_order(qty, taker_id, &gen)` | `level.match_order(qty, taker_id, ts, &gen)` |
//!
//! **Why.** The match path previously read the wall clock once per emitted
//! [`Trade`] (`SystemTime::now()`) and once per fill inside the statistics
//! update. That made the trade stream non-deterministic (each replay produced
//! different `Trade::timestamp` values) and put two syscalls per fill on the
//! hot path. The caller now threads a single taker timestamp in; it is stamped
//! onto every [`Trade`] and used as the execution time for statistics. No clock
//! is read on the match path, so matching the same input twice with the same
//! `timestamp` yields a byte-identical trade stream — a prerequisite for
//! snapshot/replay equivalence.
//!
//! Pass the taker's arrival timestamp (or any deterministic value for
//! tests/replay), e.g. [`TimestampMs::new`].
//!
//! ## Migration Guide (taker time-in-force / kind semantics — breaking)
//!
//! [`PriceLevel::match_order`] now **honors the taker's** [`TimeInForce`] and a
//! new [`TakerKind`]. Two parameters are inserted **between** `taker_order_id`
//! and `timestamp`:
//!
//! | Before | After |
//! |--------|-------|
//! | `level.match_order(qty, taker_id, ts, &gen)` | `level.match_order(qty, taker_id, tif, kind, ts, &gen)` |
//!
//! To preserve the previous "fill what you can, report the remainder" behavior,
//! pass [`TimeInForce::Gtc`] and [`TakerKind::Standard`].
//!
//! **New single-level semantics.** Let `available` be the quantity this level
//! can actually fill for the taker, capped at the incoming quantity:
//!
//! - [`TakerKind::PostOnly`]: rejected if `available > 0` (would take
//!   liquidity) — zero trades, full remainder, queue untouched.
//! - [`TimeInForce::Fok`]: killed if `available < incoming` — zero trades, full
//!   remainder, queue untouched; otherwise filled completely.
//! - [`TimeInForce::Ioc`]: fills `available`, discards the remainder (the taker
//!   is never rested by this layer).
//! - [`TimeInForce::Gtc`] / [`TimeInForce::Gtd`] / [`TimeInForce::Day`] and
//!   [`TakerKind::MarketToLimit`]: fill `available`, report the remainder in
//!   [`MatchResult::remaining_quantity`] for the order book to rest / convert.
//!
//! **New `MatchResult` signal.** A fill-or-kill *kill* and a post-only
//! *rejection* both leave zero trades and the full remainder — indistinguishable
//! through the old fields from "the level had no liquidity". [`MatchResult`]
//! gains an additive [`MatchOutcome`] (`Filled` / `PartiallyFilled` /
//! `NotFilled` / `Killed` / `Rejected`), read via
//! [`MatchResult::outcome`](crate::execution::MatchResult::outcome),
//! [`MatchResult::was_killed`](crate::execution::MatchResult::was_killed), and
//! [`MatchResult::was_rejected`](crate::execution::MatchResult::was_rejected).
//! All existing fields and accessors are unchanged. The field is
//! `#[serde(default)]` so older JSON deserializes (as `NotFilled`); the text
//! `Display` / `FromStr` format is unchanged and re-derives the benign outcome
//! on parse (a `Killed` / `Rejected` signal is not carried by the text format).
//!
//! Resting-maker time-in-force expiry is still **not** enforced by the match
//! path — only the *taker's* intent is honored here. Skipping / evicting expired
//! makers remains the order book's responsibility.
//!
//! [`TakerKind`]: crate::execution::TakerKind
//! [`MatchOutcome`]: crate::execution::MatchOutcome
//! [`MatchResult::remaining_quantity`]: crate::execution::MatchResult::remaining_quantity
//!
//! ## Migration Guide (snapshot format v1 → v2)
//!
//! The checksum-protected snapshot format now persists per-level statistics
//! (issue #63). [`PriceLevelSnapshot`] carries the eight `PriceLevelStatistics`
//! counters — orders added / removed / executed, quantity and value executed,
//! last-execution and first-arrival timestamps, and the waiting-time sum — and
//! [`PriceLevel::from_snapshot_json`] / [`PriceLevel::from_snapshot`] restore
//! them instead of resetting to a fresh, zeroed set. The new field is covered by
//! the package SHA-256 checksum automatically.
//!
//! The snapshot format version (`SNAPSHOT_FORMAT_VERSION`) is bumped from `1` to
//! `2`. Snapshot packages written by an earlier release carry `version: 1` and
//! no statistics; they are **no longer accepted** —
//! [`PriceLevelSnapshotPackage::validate`] rejects them up-front with a
//! [`PriceLevelError::InvalidOperation`] version mismatch (not a confusing
//! checksum error). Re-take any persisted snapshots with this release. No code
//! changes are required at the call sites: `snapshot_to_json()` /
//! `from_snapshot_json()` keep the same signatures.
//!
//! ## Migration Guide (snapshot format v2 → v3)
//!
//! `SNAPSHOT_FORMAT_VERSION` is bumped from `2` to `3` (issue #129). Version 3
//! owns the optional 9th statistics field, `stats_degraded` (issue #117): a
//! **degraded** level — one where an execution's statistics contribution was
//! dropped all-or-nothing — serializes that field, and such a payload is now a
//! v3 package rather than a v2 package mislabelled with an extra field an old
//! 8-field-only reader would reject.
//!
//! Restore is **backward compatible**: [`PriceLevelSnapshotPackage::validate`]
//! accepts **both** v2 (legacy, 8-field statistics, `stats_degraded` defaults
//! `false`) and v3, so snapshots written by the previous release keep restoring
//! unchanged; only v1 is still rejected. Checksum recomputation is
//! version-agnostic — a non-degraded level serializes the same 8 fields under
//! either version, so a legacy v2 package's SHA-256 still matches. New snapshots
//! are written at v3. No code changes are required at the call sites.
//!
//! ## Migration Guide (`value_executed` is `u128`, snapshot format v3 → v4 — breaking)
//!
//! `PriceLevelStatistics::value_executed()` (reached through
//! [`PriceLevel::stats`]) now returns `u128` instead of `u64` (issue #140). It
//! accumulates `quantity * price`, the same product that
//! [`MatchResult::executed_value`](crate::execution::MatchResult::executed_value)
//! and [`Trade::total_value`](crate::execution::Trade::total_value) already
//! return as `u128`. With a `u64` accumulator, a caller scaling both price and
//! quantity to fixed point (e.g. 1e8 each) exhausted it under ordinary volume
//! (after 1845 executions of 1.0 @ 1.0), after which every execution's
//! statistics were dropped and the level was permanently marked degraded. The
//! trade stream was never affected. Callers that bind the result to a `u64`
//! must widen it (or convert with `u64::try_from`).
//!
//! The accumulator is a lock-free `AtomicU128` from the `portable-atomic`
//! crate (a new dependency) on targets with a native 128-bit CAS (aarch64, and
//! x86_64 with `cmpxchg16b`); elsewhere `portable-atomic` falls back to a lock
//! for this one counter. A `u128` overflow is still rejected all-or-nothing and
//! marks the statistics degraded.
//!
//! `SNAPSHOT_FORMAT_VERSION` is bumped from `3` to `4`. A v4 payload may carry a
//! `value_executed` above `u64::MAX`, which a v3 reader cannot represent, so new
//! packages are labelled v4. A pre-0.10 reader rejects every v4 package, but it
//! deserializes the whole package before checking the version: a v4 package
//! whose value fits in `u64` fails with a version mismatch
//! ([`PriceLevelError::InvalidOperation`]), while one whose value exceeds
//! `u64::MAX` fails earlier with a [`PriceLevelError::DeserializationError`].
//! Either way the old reader errors and never restores wrong statistics.
//! Restore is **backward compatible**:
//! [`PriceLevelSnapshotPackage::validate`] accepts v2, v3 and v4, and the JSON
//! of a legacy `u64` value is unchanged, so snapshots written by earlier
//! releases keep restoring with their original SHA-256 checksum. The
//! `Display` / `FromStr` text form likewise parses both widths.
//!
//! ## Migration Guide (`Trade::total_value` is now checked)
//!
//! [`Trade::total_value`](crate::execution::Trade::total_value) now returns
//! `Result<u128, PriceLevelError>` instead of `u128`. It computes
//! `price * quantity` with `checked_mul` and returns
//! [`PriceLevelError::InvalidOperation`] on overflow, matching the checked
//! arithmetic of [`MatchResult::executed_value`](crate::execution::MatchResult::executed_value),
//! which previously used an unchecked `*` that could panic in debug or wrap in
//! release. Callers must handle the `Result` (e.g. `trade.total_value()?`).
//!
//! ## Migration Guide (newtypes at the accessor boundary — breaking)
//!
//! Accessors that previously returned raw integers for a domain concept now
//! return the crate newtype, so raw `u64` / `u128` no longer leak across module
//! boundaries (`OrderType::price` / `id` / `side` already returned newtypes —
//! this completes the quantity / timestamp surface). Call `.as_u64()` /
//! `.as_u128()` to recover the primitive, or keep working in the newtype.
//!
//! | Method | Before | After |
//! |--------|--------|-------|
//! | [`OrderType::visible_quantity`] | `u64` | [`Quantity`] |
//! | [`OrderType::hidden_quantity`] | `u64` | [`Quantity`] |
//! | [`OrderType::timestamp`] | `u64` | [`TimestampMs`] |
//! | [`MatchResult::new`] (`initial_quantity`) | `u64` | [`Quantity`] |
//! | [`MatchResult::with_capacity`] (`initial_quantity`) | `u64` | [`Quantity`] |
//! | [`MatchResult::remaining_quantity`] | `u64` | [`Quantity`] |
//! | [`MatchResult::executed_quantity`] | `Result<u64, _>` | `Result<`[`Quantity`]`, _>` |
//! | [`PriceLevelSnapshot::new`] (`price`) | `u128` | [`Price`] |
//! | [`PriceLevelSnapshot::with_orders`] (`price`) | `u128` | [`Price`] |
//! | [`PriceLevelSnapshot::with_orders_and_stats`] (`price`) | `u128` | [`Price`] |
//! | [`PriceLevelSnapshot::price`] | `u128` | [`Price`] |
//! | [`PriceLevelSnapshot::visible_quantity`] | `u64` | [`Quantity`] |
//! | [`PriceLevelSnapshot::hidden_quantity`] | `u64` | [`Quantity`] |
//! | [`PriceLevelSnapshot::total_quantity`] | `Result<u64, _>` | `Result<`[`Quantity`]`, _>` |
//!
//! [`MatchResult::executed_value`] / [`Trade::total_value`](crate::execution::Trade::total_value)
//! still return `u128` — there is no monetary newtype. [`PriceLevel::match_order`]
//! keeps its `incoming_quantity: u64` input (it is converted to [`Quantity`] at
//! the [`MatchResult`] boundary internally); its 124 call sites are unchanged.
//!
//! **Snapshot wire format is unchanged.** [`Price`] and [`Quantity`] are
//! `#[serde(transparent)]`, so a snapshot serializes the same JSON numbers as
//! before; the snapshot format version is **not** bumped and the SHA-256
//! checksum over an unchanged payload still validates. Existing snapshot JSON
//! restores without migration.
//!
//! ## Migration Guide (`PriceLevel::add_order` is now checked — breaking)
//!
//! [`PriceLevel::add_order`] now returns
//! `Result<Arc<OrderType<()>>, PriceLevelError>` instead of
//! `Arc<OrderType<()>>`. It reserves the order's visible / hidden quantity and
//! its count slot on the level's atomic counters (with checked `fetch_update`)
//! **before** publishing the order to the queue, and returns
//! [`PriceLevelError::InvalidOperation`] if any counter would overflow `u64` —
//! leaving the level completely unchanged rather than wrapping a counter while
//! the queue already holds the admitted order. Callers must handle the
//! `Result` — propagate with `level.add_order(order)?` (test fixtures and
//! binaries may prefer `.expect(...)`); the returned `Arc` is unchanged on
//! success. Admissions that stay within `u64` (all normal use) behave exactly
//! as before.
//!
//! `add_order` also now **rejects a duplicate id**: publishing is an
//! insert-if-absent, so reusing the id of an order already resting at the level
//! returns the new [`PriceLevelError::DuplicateOrderId`] variant (again leaving
//! the level unchanged) instead of overwriting the live order and leaving the
//! id-keyed map and the ordered index disagreeing. Snapshot restore
//! ([`PriceLevel::from_snapshot`] and the JSON / package forms) likewise
//! rejects an orders vector that repeats an id rather than silently
//! overwriting. Submitting genuinely distinct ids (all normal use) is
//! unaffected.
//!
//! ## Migration Guide (v0.9 — duplicate-id safety on restore + queue surface)
//!
//! Three intentional breaking changes remove infallible / overwriting paths
//! that could desync a level's counters from its queue:
//!
//! - **`impl From<&PriceLevelSnapshot> for PriceLevel` is removed; use
//!   [`TryFrom`].** The old `From` swallowed aggregate-overflow errors and built
//!   the queue keep-first, so a snapshot repeating an id restored counters
//!   computed over every copy while the queue kept one. Replace
//!   `PriceLevel::from(&snapshot)` / `let lvl: PriceLevel = (&snapshot).into();`
//!   with `PriceLevel::try_from(&snapshot)?` (or `.expect(...)` in tests). It
//!   delegates to [`PriceLevel::from_snapshot`], returning
//!   [`PriceLevelError::DuplicateOrderId`] on a repeated id and the
//!   per-order / level aggregate-overflow errors instead of hiding them.
//! - **`OrderQueue::push` is now `pub(crate)`.** Unconditional overwriting
//!   publication is never safe for an external caller (reusing a live id would
//!   silently replace the resting order and strand its old index entry).
//!   Admission goes through `add_order` (or, at the queue layer, the
//!   insert-if-absent `try_push`); there is no public overwriting insert.
//! - **`OrderQueue::from_vec` is now `pub(crate)`.** It is a keep-first
//!   constructor that drops duplicates silently; the public restore path is
//!   [`PriceLevel::from_snapshot`], which rejects them.
//! ## Migration Guide (level topology invariants — breaking)
//!
//! A [`PriceLevel`] now enforces that every resting order sits at the level's
//! price and shares a single side (the first admitted maker pins the side; a
//! fully drained level accepts either side again). [`PriceLevel::add_order`]
//! returns [`PriceLevelError::InvalidOperation`] for an order whose price does
//! not match the level, or whose side is incompatible with the resting side,
//! and [`PriceLevel::from_snapshot`] rejects a snapshot that violates either
//! (previously such orders were admitted, trading at the level price rather
//! than their own and producing contradictory taker sides in one
//! [`MatchResult`]). Callers that composed a level from mixed-price or
//! mixed-side orders must route each order to the correct level.
//!
//! Single-side coherence is a **correctness invariant**, not an
//! eventually-consistent one like the advisory counters: it holds only when a
//! given level's admissions arrive from a single logical writer (the composing
//! order book routes each price to one admission path). The side is derived
//! from the live queue, so under genuinely concurrent multi-writer admission a
//! narrow race — an opposite side slipping into a momentarily empty level — can
//! still admit a mixed side; see the note on the [`PriceLevel`] type.
//!
//! [`PriceLevel::matchable_quantity`] gains a `taker_id` parameter:
//! `matchable_quantity(incoming_quantity)` becomes
//! `matchable_quantity(incoming_quantity, taker_id)`. A resting maker sharing
//! the taker id is skipped (self-trade prevention), matching the sweep, so a
//! fill-or-kill dry run and the real sweep agree. `match_order` applies the
//! same **self-trade skip** deterministically in every build profile (it used
//! to be a debug-only assertion): a resting maker whose id equals the taker's
//! is skipped — no self-trade is emitted and the other makers still match.
//!
//! This self-trade guard is **order-id identity** — an order can never match
//! itself. It is NOT account/owner-level self-trade prevention: two distinct
//! order ids owned by the same `user_id` will still trade. Account-level STP is
//! the responsibility of the order book composing these levels, which owns the
//! account relationships a single price level does not.
//!
//! ## Migration Guide (atomic quantity-increase re-sequencing)
//!
//! A quantity increase via [`PriceLevel::update_order`] still demotes the maker
//! to the back of the queue (fresh tail sequence, original timestamp), but it
//! now does so **in place** — the order id never leaves the internal map. This
//! closes the concurrency window the previous `remove` + re-insert opened
//! (issue #119): a concurrent cancel can no longer be lost or resurrect the
//! order, a concurrent same-id admission can no longer slip into the gap
//! (`add_order` for a live id is always rejected), and the match sweep can no
//! longer act on a stale front position. The public behaviour of `update_order`
//! is unchanged; only its concurrency safety improves.
//!
//! The internal `OrderQueue::push` — a blind, overwrite-on-collision insert
//! with no remaining production caller — is removed from the public API (it is
//! now test-only). Admission uses `try_push` (insert-if-absent) and the
//! quantity-increase demotion uses the internal atomic re-sequence, so `push`
//! was a footgun with no safe use; construct queues through [`PriceLevel`]'s
//! public surface instead.
//!
//! ## Migration Guide (fallible random `Id` constructors — breaking)
//!
//! The random [`Id`] constructors could panic inside `uuid` / `ulid` / `rand`
//! when the operating system failed to provide entropy (or an RNG failed to
//! seed or reseed), and `Default` hid that behind an infallible trait. They
//! are replaced by fallible constructors that draw bytes from a
//! **caller-supplied** [`EntropySource`] and, for ULIDs, take their timestamp
//! from a **caller-supplied** [`UnixClock`] or an explicit [`TimestampMs`].
//! The crate owns no randomness source and no clock reader, and adds no
//! dependency.
//!
//! | v0.9 | v0.10 |
//! |------|-------|
//! | `Id::new()` | [`Id::try_new(&clock, &mut entropy)`](Id::try_new) (ULID) |
//! | `Id::new_ulid()` | [`Id::try_new_ulid(&clock, &mut entropy)`](Id::try_new_ulid) |
//! | — | [`Id::try_new_ulid_at(timestamp, &mut entropy)`](Id::try_new_ulid_at) |
//! | `Id::new_uuid()` | [`Id::try_new_uuid(&mut entropy)`](Id::try_new_uuid) |
//! | `Id::default()` / `#[derive(Default)]` over `Id` | removed; construct an id explicitly |
//!
//! All return `Result<Id, PriceLevelError>`:
//!
//! - an entropy failure is returned unchanged from the source (conventionally
//!   the new [`PriceLevelError::EntropyUnavailable`] variant);
//! - a clock failure is returned unchanged from the caller's [`UnixClock`];
//! - a timestamp above [`Id::ULID_MAX_TIMESTAMP_MS`] (the 48-bit ULID time
//!   field) is rejected with [`PriceLevelError::InvalidFieldValue`] instead of
//!   being silently masked, and before any entropy is drawn.
//!
//! No nil, repeated or predictable fallback identifier is ever substituted.
//! Successful ids keep their wire formats: UUIDs are RFC 4122 / 9562 version 4
//! (version and variant bits set exactly as `Uuid::new_v4` does), ULIDs carry
//! the 48-bit timestamp and 80 random bits.
//!
//! Implement [`EntropySource`] over the randomness facility your application
//! already uses (an OS call such as `getrandom`, or a CSPRNG) and map its
//! failure into a [`PriceLevelError`]. **Implementations must not panic** and
//! must never report success without writing fresh unpredictable bytes into
//! the whole buffer; return `Err` instead. A caller-supplied [`UnixClock`]
//! carries the same no-panic obligation. Note that
//! `std::time::SystemTime::now` panics inside `std` if the platform clock
//! call fails, so a strictly panic-free clock needs a fallible time source;
//! [`TimestampMs::try_from_system_time`] converts an already-read
//! `SystemTime` with checked arithmetic (pre-epoch and `u64` overflow are
//! typed errors).
//!
//! The adapter below delegates to a fill function the application owns and
//! maps its error. Until a real source is wired in, the example's stand-in
//! fails, and the constructors surface that failure instead of producing an id:
//!
//! ```rust
//! use pricelevel::{EntropySource, Id, PriceLevelError, TimestampMs};
//!
//! /// Adapts an application-owned fallible fill function, for example
//! /// `FillEntropy(getrandom::fill)` with `getrandom` as your own dependency.
//! struct FillEntropy<F>(F);
//!
//! impl<F, E> EntropySource for FillEntropy<F>
//! where
//!     F: FnMut(&mut [u8]) -> Result<(), E>,
//!     E: std::fmt::Display,
//! {
//!     fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), PriceLevelError> {
//!         (self.0)(dest).map_err(|error| PriceLevelError::EntropyUnavailable {
//!             message: error.to_string(),
//!         })
//!     }
//! }
//!
//! // Stand-in for an entropy source that is not configured (or has failed).
//! let mut entropy = FillEntropy(|_dest: &mut [u8]| Err("entropy source not configured"));
//!
//! let uuid = Id::try_new_uuid(&mut entropy);
//! assert!(matches!(uuid, Err(PriceLevelError::EntropyUnavailable { .. })));
//!
//! let ulid = Id::try_new_ulid_at(TimestampMs::new(1_716_000_000_000), &mut entropy);
//! assert!(matches!(ulid, Err(PriceLevelError::EntropyUnavailable { .. })));
//! ```
//!
//! Deterministic ids that need no entropy are unchanged: [`Id::sequential`],
//! [`Id::from_u64`], [`Id::from_uuid`], [`Id::from_ulid`], [`Id::nil`] and
//! [`UuidGenerator`].
//!
//! ## Migration Guide (`Id` text parsing disambiguates by shape)
//!
//! `Id::from_str` (and `Id`'s serde `Deserialize`, which parses the same text)
//! now tries a 26-character ULID first, then any UUID text form, and only then
//! a decimal `u64`. Previously `u64` came first, so an all-digit ULID such as
//! the nil ULID `00000000000000000000000000` came back as
//! [`Id::Sequential`]. Now `id.to_string().parse::<Id>() == Ok(id)` holds for
//! every [`Id`], and canonical sequential text (at most 20 digits) is
//! unaffected.
//!
//! Only non-canonical sequential spellings change meaning (the nil ULID text is
//! the canonical ULID spelling; what changes is that it no longer reads as a
//! zero-padded sequential id):
//!
//! | Input | Before | Now |
//! |-------|--------|-----|
//! | 26 digits whose decimal value is at most `u64::MAX` (so at least 6 leading zeros) | `Sequential` | `Ulid` |
//! | 32 digits whose decimal value is at most `u64::MAX` (so at least 12 leading zeros) | `Sequential` | `Uuid` (simple form) |
//! | 26 Crockford characters starting above `7` | `Ulid` (top bits silently lost) | `ParseError` |
//!
//! If you store sequential ids zero-padded to 26 or 32 characters, strip the
//! padding (or build them with [`Id::sequential`]) before parsing.
//!
//! ```rust
//! use pricelevel::Id;
//!
//! let nil_ulid: Id = "00000000000000000000000000".parse().unwrap();
//! assert!(nil_ulid.is_ulid());
//! assert_eq!("18446744073709551615".parse::<Id>().unwrap(), Id::sequential(u64::MAX));
//! ```
//!
//! ## Migration Guide (trade and statistics clock reads — breaking)
//!
//! `Trade::new` and the statistics helpers read the wall clock, narrowed
//! `Duration::as_millis()` from `u128` to `u64` with `as`, and substituted `0`
//! for a pre-epoch clock. The crate now reads no clock at all: time is either
//! supplied by the caller as a [`TimestampMs`] or read once from a
//! **caller-supplied** [`UnixClock`] whose failure is returned unchanged.
//!
//! | v0.9 | v0.10 |
//! |------|-------|
//! | `Trade::new(id, taker, maker, price, qty, side)` | [`Trade::try_new(id, taker, maker, price, qty, side, &clock)`](Trade::try_new) `-> Result<Trade, _>`, or the unchanged infallible [`Trade::with_timestamp`] |
//! | `stats.reset()` | [`stats.reset(&clock)`](PriceLevelStatistics::reset) `-> Result<(), _>`, or [`stats.reset_at(ts)`](PriceLevelStatistics::reset_at) |
//! | `stats.time_since_last_execution() -> Option<u64>` | [`stats.time_since_last_execution(&clock)`](PriceLevelStatistics::time_since_last_execution) / [`time_since_last_execution_at(now)`](PriceLevelStatistics::time_since_last_execution_at) `-> Result<Option<u64>, _>` |
//! | — | [`PriceLevelStatistics::new_at(ts)`](PriceLevelStatistics::new_at), [`PriceLevelStatistics::try_new(&clock)`](PriceLevelStatistics::try_new) |
//!
//! Semantics:
//!
//! - `reset(&clock)` reads the clock **before** mutating anything; on failure
//!   every counter, timestamp and the degraded flag is left unchanged.
//! - `time_since_last_execution*` returns `Ok(None)` only when no execution
//!   was recorded (the clock is not read then); a clock failure is `Err`, and a
//!   `now` earlier than the last execution is
//!   [`PriceLevelError::InvalidOperation`] (it used to be `None`).
//! - **Behavior change, no signature change:** [`PriceLevelStatistics::new`],
//!   its [`Default`], [`PriceLevel::new`], [`PriceLevelSnapshot::new`],
//!   [`PriceLevelSnapshot::with_orders`], `PriceLevelSnapshot::from_str` and a
//!   snapshot payload that omits `statistics` no longer stamp the wall clock:
//!   `first_arrival_time()` starts at `0`, meaning *unstamped*. They are now
//!   deterministic (identical input gives byte-identical, identically
//!   checksummed snapshots), and no clock failure can hide behind them. Use
//!   `new_at` / `try_new`, or `reset_at` / `reset` on a still-quiescent
//!   level, to record a start time.
//! - A serialized statistics object that **omits** `first_arrival_time` now
//!   decodes it as `0` (unstamped) instead of the restore instant, which was
//!   never the original start time. Every package this crate writes carries the
//!   field, so v2, v3 and v4 packages and their checksums are unaffected.
//! - [`PriceLevel::match_order`] was already clock-free; trade fields,
//!   explicit timestamps and matching determinism are unchanged.
//! - [`PriceLevelStatistics`] is now re-exported at the crate root so the new
//!   constructors are nameable (it was previously reachable only through
//!   [`PriceLevel::stats`] and [`PriceLevelSnapshot::statistics`]).
//!
//! A conforming [`UnixClock`] must not panic. `std::time::SystemTime::now`
//! can panic inside `std` if the platform clock call fails, so an
//! implementation built on it does **not** meet that contract. The simplest
//! path needs no clock trait: read the time in your own code (with whatever
//! failure policy your application accepts), convert it with the checked
//! [`TimestampMs::try_from_system_time`] (pre-epoch and `u64` overflow are
//! typed errors), and pass the explicit timestamp to the `_at` APIs or
//! [`Trade::with_timestamp`]. A clock you inject for tests or replay can be a
//! fixed value:
//!
//! ```rust
//! use pricelevel::{PriceLevelError, PriceLevelStatistics, TimestampMs, UnixClock};
//! use std::time::{Duration, UNIX_EPOCH};
//!
//! // Explicit-timestamp path: the application owns the clock read.
//! // (`UNIX_EPOCH + ...` stands in for a time your code already read.)
//! let read_by_caller = UNIX_EPOCH + Duration::from_millis(1_716_000_000_500);
//! let now = TimestampMs::try_from_system_time(read_by_caller)?;
//!
//! let stats = PriceLevelStatistics::new_at(TimestampMs::new(1_716_000_000_000));
//! stats.record_execution(10, 100, 0, 1_716_000_000_000)?;
//! assert_eq!(stats.time_since_last_execution_at(now)?, Some(500));
//! stats.reset_at(now);
//!
//! // Injected clock path: a fixed clock that cannot panic.
//! struct FixedClock(TimestampMs);
//!
//! impl UnixClock for FixedClock {
//!     fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
//!         Ok(self.0)
//!     }
//! }
//!
//! stats.reset(&FixedClock(now))?;
//! assert_eq!(stats.first_arrival_time(), now.as_u64());
//! assert_eq!(stats.time_since_last_execution(&FixedClock(now))?, None);
//! # Ok::<(), PriceLevelError>(())
//! ```
//!
//! ## Migration Guide (text parsers reject unbalanced brackets)
//!
//! The text (`FromStr`) parsers now use checked access and a bounded nesting
//! counter. Text written by `Display` parses exactly as before, and so does
//! almost every malformed input. Two contracts tightened:
//!
//! | Input | Before | Now |
//! |-------|--------|-----|
//! | `TradeList` / `MatchResult` `trades=` text with an unbalanced `[` / `]` inside an ignored trade field (e.g. `Trades:[Trade:...;x=]]`) | accepted | [`PriceLevelError::InvalidFormat`] |
//! | `PriceLevel` text with an unbalanced `(` / `)` / `[` inside the `orders=[...]` section, in an ignored order field | accepted | [`PriceLevelError::ParseError`] |
//! | `TradeList`, `MatchResult` or `PriceLevel` text nesting brackets more than 128 deep (list bracket included) | scanned with an unchecked signed counter | [`PriceLevelError::ParseError`] (`nesting depth exceeds the limit of 128`) |
//!
//! Segmentation is unchanged, and an element that fails to parse is still
//! reported before a bracket imbalance, so errors for other malformed input
//! are the same. A parser that cannot grow its output vector reports
//! [`PriceLevelError::InvalidOperation`] instead of aborting.
//!
//! ```rust
//! use pricelevel::{PriceLevelError, TradeList};
//! use std::str::FromStr;
//!
//! let trade = "Trade:trade_id=1;taker_order_id=2;maker_order_id=3;price=4;quantity=5;taker_side=BUY;timestamp=6";
//! assert!(TradeList::from_str(&format!("Trades:[{trade};note=[ok]]")).is_ok());
//! assert!(matches!(
//!     TradeList::from_str(&format!("Trades:[{trade};note=]]")),
//!     Err(PriceLevelError::InvalidFormat)
//! ));
//! ```
//!

mod orders;
mod price_level;
mod utils;

mod errors;
mod execution;

pub mod prelude;

pub use errors::PriceLevelError;
pub use execution::{MatchOutcome, MatchResult, TakerKind, Trade, TradeList};
pub use orders::DEFAULT_RESERVE_REPLENISH_AMOUNT;
pub use orders::PegReferenceType;
pub use orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
pub use price_level::{
    OrderQueue, PriceLevel, PriceLevelData, PriceLevelSnapshot, PriceLevelSnapshotPackage,
    PriceLevelStatistics,
};
pub use utils::{
    EntropySource, Price, Quantity, TimestampMs, UnixClock, UuidGenerator, setup_logger,
};
