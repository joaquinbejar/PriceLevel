// benches/latency/fixtures.rs
//! Order and `PriceLevel` builders shared by every latency scenario.
//!
//! Every builder here is a **fixture** in the sense the issue (#142) uses the
//! word: construction that happens *outside* a timed window. Scenario code
//! calls these to prepare a level or a batch of orders, then times exactly
//! one public-API call per sample.

use pricelevel::prelude::*;
use std::num::NonZeroU64;
use uuid::Uuid;

/// Fixed price used by every scenario unless the scenario itself sweeps price.
pub const LEVEL_PRICE: u128 = 10_000;

/// Base timestamp for constructed orders (arbitrary, deterministic).
pub const BASE_TIMESTAMP_MS: u64 = 1_700_000_000_000;

/// Id range every scenario uses for taker ids, kept disjoint from any
/// resting maker id range (which always starts at 0) so a taker can never
/// collide with — and self-match-reject against — one of its own makers.
pub const TAKER_ID_BASE: u64 = 1_000_000_000;

/// Execution timestamp every scenario in this harness passes to
/// `PriceLevel::match_order`.
///
/// Every maker built by this module stamps its own `timestamp` field as
/// `BASE_TIMESTAMP_MS + id`. `PriceLevelStatistics::record_execution`
/// rejects a fill whose maker `order_timestamp` is strictly greater than the
/// match's `execution_timestamp` (a maker "arriving in the future" of the
/// execution) — see its `# Errors` doc. Passing `TimestampMs::new(0)` as the
/// execution timestamp, as an earlier version of this harness did, made
/// EVERY fill in EVERY scenario fail that check: the trade itself still
/// happened (statistics recording cannot retroactively fail an
/// already-committed trade), but `PriceLevelStatistics::stats_degraded()`
/// silently flipped `true` and `quantity_executed()` never advanced, so the
/// harness was measuring the degraded/error-accounting path instead of the
/// intended one on every single sample (issue #142 review finding 1).
///
/// No maker id constructed anywhere in this harness exceeds a few hundred
/// thousand; this constant carries a two-billion-millisecond margin over
/// [`BASE_TIMESTAMP_MS`] so it is unambiguously past every eligible maker's
/// own timestamp, however ids are combined across scenarios.
pub const EXECUTION_TIMESTAMP_MS: u64 = BASE_TIMESTAMP_MS + 2_000_000_000;

/// Builds the deterministic trade-id generator every scenario shares. A
/// fixed namespace UUID (same convention as the existing Criterion benches
/// under `benches/price_level/`) keeps trade ids reproducible across runs.
#[must_use]
pub fn trade_id_generator() -> UuidGenerator {
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .expect("fixture: hard-coded namespace UUID literal must parse");
    UuidGenerator::new(namespace)
}

/// Builds a standard limit order.
#[must_use]
pub fn standard_order(id: u64, side: Side, quantity: u64, tif: TimeInForce) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(LEVEL_PRICE),
        quantity: Quantity::new(quantity),
        side,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(BASE_TIMESTAMP_MS + id),
        time_in_force: tif,
        extra_fields: (),
    }
}

/// Builds a standard limit order at an explicit price (for scaled-price
/// workloads); the level itself still lives at [`LEVEL_PRICE`] in every
/// scenario that uses this, so this is for varying the *order's* recorded
/// price field, not for cross-level routing (this crate has no order book).
#[must_use]
pub fn standard_order_at_price(
    id: u64,
    price: u128,
    side: Side,
    quantity: u64,
    tif: TimeInForce,
) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(BASE_TIMESTAMP_MS + id),
        time_in_force: tif,
        extra_fields: (),
    }
}

/// Builds an iceberg order with the given visible / hidden split.
#[must_use]
pub fn iceberg_order(id: u64, side: Side, visible: u64, hidden: u64) -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::from_u64(id),
        price: Price::new(LEVEL_PRICE),
        visible_quantity: Quantity::new(visible),
        hidden_quantity: Quantity::new(hidden),
        side,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(BASE_TIMESTAMP_MS + id),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

/// Builds a reserve order with the given visible / hidden split and
/// replenishment policy.
#[must_use]
pub fn reserve_order(
    id: u64,
    side: Side,
    visible: u64,
    hidden: u64,
    replenish_threshold: u64,
    replenish_amount: u64,
) -> OrderType<()> {
    OrderType::ReserveOrder {
        id: Id::from_u64(id),
        price: Price::new(LEVEL_PRICE),
        visible_quantity: Quantity::new(visible),
        hidden_quantity: Quantity::new(hidden),
        side,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(BASE_TIMESTAMP_MS + id),
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: Quantity::new(replenish_threshold),
        replenish_amount: NonZeroU64::new(replenish_amount),
        auto_replenish: true,
        extra_fields: (),
    }
}

/// Builds a level pre-seeded with `depth` standard resting orders on `side`,
/// each of `quantity_each`, ids `0..depth`. Returns the level with the
/// caller free to use ids `>= depth` for anything timed afterward.
#[must_use]
pub fn seeded_standard_level(depth: u64, side: Side, quantity_each: u64) -> PriceLevel {
    let level = PriceLevel::new(LEVEL_PRICE);
    for i in 0..depth {
        level
            .add_order(standard_order(i, side, quantity_each, TimeInForce::Gtc))
            .expect("fixture seeding: add_order must succeed for a fresh sequential id");
    }
    level
}

/// Asserts, outside any timed window, that every fill `level` has recorded
/// since it was created (or since its statistics were last reset) landed in
/// `PriceLevelStatistics` cleanly: never dropped into the sticky degraded
/// path, and with the accumulated executed quantity equal to exactly
/// `expected_quantity_executed`.
///
/// Every matching scenario that expects trades to occur calls this once
/// after its measured loop — see [`EXECUTION_TIMESTAMP_MS`]'s docs for why
/// this check exists (issue #142 review finding 1).
pub fn assert_stats_healthy(level: &PriceLevel, expected_quantity_executed: u64, context: &str) {
    let stats = level.stats();
    assert!(
        !stats.stats_degraded(),
        "{context}: PriceLevelStatistics reports stats_degraded() == true — a maker's own \
         timestamp was not <= the match's execution timestamp (or another record_execution \
         failure), so this run measured the degraded/error-accounting path instead of the \
         intended fill path"
    );
    assert_eq!(
        stats.quantity_executed(),
        expected_quantity_executed,
        "{context}: PriceLevelStatistics::quantity_executed() must equal exactly the expected \
         executed total for this scenario"
    );
}

/// Builds a level pre-seeded with `depth` iceberg resting orders on `side`.
#[must_use]
pub fn seeded_iceberg_level(depth: u64, side: Side, visible: u64, hidden: u64) -> PriceLevel {
    let level = PriceLevel::new(LEVEL_PRICE);
    for i in 0..depth {
        level
            .add_order(iceberg_order(i, side, visible, hidden))
            .expect("fixture seeding: add_order must succeed for a fresh sequential id");
    }
    level
}

/// Builds a level pre-seeded with `depth` reserve resting orders on `side`.
#[must_use]
pub fn seeded_reserve_level(
    depth: u64,
    side: Side,
    visible: u64,
    hidden: u64,
    replenish_threshold: u64,
    replenish_amount: u64,
) -> PriceLevel {
    let level = PriceLevel::new(LEVEL_PRICE);
    for i in 0..depth {
        level
            .add_order(reserve_order(
                i,
                side,
                visible,
                hidden,
                replenish_threshold,
                replenish_amount,
            ))
            .expect("fixture seeding: add_order must succeed for a fresh sequential id");
    }
    level
}
