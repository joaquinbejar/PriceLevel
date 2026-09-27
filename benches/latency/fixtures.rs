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
