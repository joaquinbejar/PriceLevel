//! `MatchResult` buffer sizing inside `PriceLevel::match_order` (#148).
//!
//! One `match_order` call per iteration for the result shapes the issue asks
//! to compare: zero trades, a partial fill of one large front maker on a deep
//! level (one trade, no filled id), a single full fill (one trade, one filled
//! id), a 100-maker sweep, and iceberg / reserve replenishment (trades with no
//! filled id, trade count above the order-count estimate for `iceberg_5x`).
//!
//! Levels that support unbounded repetition (huge front maker / hidden
//! tranche, or an empty level) are shared across iterations; the others are
//! rebuilt per batch outside the timed routine. The returned `MatchResult` is
//! dropped outside the timed window (`iter_with_large_drop` /
//! `iter_batched`), so only construction and the sweep are measured.

use criterion::{BatchSize, Criterion};
use pricelevel::{
    Hash32, Id, OrderType, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce, TimestampMs,
    UuidGenerator,
};
use std::hint::black_box;
use std::num::NonZeroU64;
use uuid::Uuid;

const PRICE: u128 = 10_000;
const TAKER_ID: u64 = 1_000_000_000;
const EXEC_TS: u64 = 1_800_000_000_000;

fn generator() -> UuidGenerator {
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .expect("hard-coded namespace UUID literal must parse");
    UuidGenerator::new(namespace)
}

fn standard(id: u64, quantity: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        quantity: Quantity::new(quantity),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_700_000_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn iceberg_level() -> PriceLevel {
    let level = PriceLevel::new(PRICE);
    level
        .add_order(OrderType::IcebergOrder {
            id: Id::from_u64(0),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(10),
            hidden_quantity: Quantity::new(1_000_000_000_000),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_700_000_000_000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        })
        .expect("seed iceberg");
    level
}

fn reserve_level() -> PriceLevel {
    let level = PriceLevel::new(PRICE);
    level
        .add_order(OrderType::ReserveOrder {
            id: Id::from_u64(0),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(10),
            hidden_quantity: Quantity::new(1_000_000_000_000),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_700_000_000_000),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(1),
            replenish_amount: NonZeroU64::new(10),
            auto_replenish: true,
            extra_fields: (),
        })
        .expect("seed reserve");
    level
}

fn deep_level_with_large_front(depth: u64) -> PriceLevel {
    let level = PriceLevel::new(PRICE);
    level
        .add_order(standard(0, 1_000_000_000_000))
        .expect("seed large front");
    for i in 1..depth {
        level.add_order(standard(i, 10)).expect("seed maker");
    }
    level
}

fn standard_level(makers: u64, quantity: u64) -> PriceLevel {
    let level = PriceLevel::new(PRICE);
    for i in 0..makers {
        level.add_order(standard(i, quantity)).expect("seed maker");
    }
    level
}

/// Registers the #148 result-capacity group.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("MatchResult capacity (#148)");
    let ids = generator();
    let taker = Id::from_u64(TAKER_ID);
    let ts = TimestampMs::new(EXEC_TS);

    let shared =
        |name: &str,
         level: PriceLevel,
         qty: u64,
         group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>| {
            group.bench_function(name, |b| {
                b.iter_with_large_drop(|| {
                    level.match_order(
                        black_box(qty),
                        taker,
                        TimeInForce::Gtc,
                        TakerKind::Standard,
                        ts,
                        &ids,
                    )
                });
            });
        };
    shared("zero_trade", PriceLevel::new(PRICE), 10, &mut group);
    shared(
        "maker_partial_deep1000",
        deep_level_with_large_front(1_000),
        10,
        &mut group,
    );
    shared("iceberg_1x", iceberg_level(), 10, &mut group);
    shared("iceberg_5x", iceberg_level(), 50, &mut group);
    shared("reserve_1x", reserve_level(), 10, &mut group);

    let fresh =
        |name: &str,
         makers: u64,
         qty_each: u64,
         taker_qty: u64,
         group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>| {
            group.bench_function(name, |b| {
                b.iter_batched(
                    || standard_level(makers, qty_each),
                    |level| {
                        let result = level.match_order(
                            black_box(taker_qty),
                            taker,
                            TimeInForce::Gtc,
                            TakerKind::Standard,
                            ts,
                            &ids,
                        );
                        (level, result)
                    },
                    BatchSize::SmallInput,
                );
            });
        };
    fresh("single_full", 1, 10, 10, &mut group);
    fresh("sweep_100", 100, 10, 1_000, &mut group);

    group.finish();
}
