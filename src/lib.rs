#![allow(unknown_lints)]
#![allow(clippy::literal_string_with_formatting_args)]
#![warn(clippy::missing_errors_doc)]

//!  # PriceLevel
//!
//!  A high-performance, lock-free price level implementation for limit order books in Rust. This library provides the building blocks for creating efficient trading systems with support for multiple order types and concurrent access patterns.
//!
//!  ## Features
//!
//!  - Lock-free architecture for high-throughput trading applications
//!  - Support for diverse order types including standard limit orders, iceberg orders, post-only, fill-or-kill, and more
//!  - Thread-safe operations with atomic counters and lock-free data structures
//!  - Efficient order matching and execution logic
//!  - Designed with domain-driven principles for financial markets
//!  - Comprehensive test suite demonstrating concurrent usage scenarios
//!  - Built with crossbeam's lock-free data structures (`crossbeam-skiplist`)
//!  - Optimized statistics tracking for each price level
//!  - Memory-efficient implementations suitable for high-frequency trading systems
//!
//!  Perfect for building matching engines, market data systems, algorithmic trading platforms, and financial exchanges where performance and correctness are critical.
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
//!  - **Thread Safety**: Uses atomic operations and lock-free data structures to ensure thread safety without mutex locks
//!  - **Order Queue Management**: Specialized order queue keeping strict price-time priority via a lock-free `crossbeam-skiplist` ordered index
//!  - **Statistics Tracking**: Each price level tracks execution statistics in real-time
//!  - **Snapshot Capabilities**: Create point-in-time snapshots of price levels for market data distribution
//!  - **Efficient Matching**: Optimized algorithms for matching incoming orders against existing orders
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
//! ## Performance Benchmark Results
//!
//! The `pricelevel` library has been thoroughly tested for performance in high-frequency trading scenarios. Below are the results from recent simulations conducted on an M4 Max processor, demonstrating the library's capability to handle intensive concurrent trading operations.
//!
//! ### High-Frequency Trading Simulation
//!
//! #### Simulation Parameters
//! - **Price Level**: 10000
//! - **Duration**: 5002 ms (5.002 seconds)
//! - **Threads**: 30 total
//!   - 10 maker threads (adding orders)
//!   - 10 taker threads (executing matches)
//!   - 10 canceller threads (cancelling orders)
//! - **Initial Orders**: 1000 orders seeded before simulation
//!
//! #### Performance Metrics
//!
//! | Metric | Total Operations | Rate (per second) |
//! |--------|-----------------|-------------------|
//! | Orders Added | 715,814 | 143,095.10 |
//! | Matches Executed | 374,910 | 74,946.54 |
//! | Cancellations | 96,575 | 19,305.87 |
//! | **Total Operations** | **1,187,299** | **237,347.51** |
//!
//! #### Final State After Simulation
//! - **Price**: 10000
//! - **Visible Quantity**: 4,590,308
//! - **Hidden Quantity**: 4,032,155
//! - **Total Quantity**: 8,622,463
//! - **Order Count**: 704,156
//!
//! #### Price Level Statistics
//! - **Orders Added**: 716,814
//! - **Orders Removed**: 215
//! - **Orders Executed**: 401,864
//! - **Quantity Executed**: 1,124,714
//! - **Value Executed**: 11,247,140,000
//! - **Average Execution Price**: 10,000.00
//! - **Average Waiting Time**: 1,788.31 ms
//! - **Time Since Last Execution**: 1 ms
//!
//! ### Contention Pattern Analysis
//!
//! #### Hot Spot Contention Test
//! Performance under different levels of contention targeting specific price levels:
//!
//! | Hot Spot % | Operations/second |
//! |------------|-------------------|
//! | 0% | 7,548,438.05 |
//! | 25% | 7,752,860.57 |
//! | 50% | 7,584,981.59 |
//! | 75% | 7,267,749.39 |
//! | 100% | 6,970,720.77 |
//!
//! #### Read/Write Ratio Test
//! Performance under different read/write operation ratios:
//!
//! | Read % | Operations/second |
//! |--------|-------------------|
//! | 0% | 6,353,202.47 |
//! | 25% | 34,727.89 |
//! | 50% | 28,783.28 |
//! | 75% | 31,936.73 |
//! | 95% | 54,316.57 |
//!
//! ### Analysis
//!
//! The simulation demonstrates the library's exceptional performance capabilities:
//!
//! - **High-Frequency Trading**: Over **264,000 operations per second** in realistic mixed workloads
//! - **Hot Spot Performance**: Up to **7.75 million operations per second** under optimal conditions
//! - **Write-Heavy Workloads**: Over **6.3 million operations per second** for pure write operations
//! - **Lock-Free Architecture**: Maintains high throughput with minimal contention overhead
//!
//! The performance characteristics demonstrate that the `pricelevel` library is suitable for production use in high-performance trading systems, matching engines, and other financial applications where microsecond-level performance is critical.
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
//!   `cancel` from other threads are safe, but two concurrent `match_order`
//!   calls on the *same* level — or a `match_order` racing a `cancel` of the
//!   resting order it is currently matching — are the caller's responsibility
//!   to serialize.
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
//! | `Uuid::new_v4()` | [`Id::new()`] or [`Id::new_uuid()`] |
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
};
pub use utils::{Price, Quantity, TimestampMs, UuidGenerator, setup_logger};
