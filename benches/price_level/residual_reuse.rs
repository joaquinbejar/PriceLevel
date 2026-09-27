//! Issue #147: repeated partial fills of one large maker, with and without an
//! externally retained `Arc` of that maker. `unique` and `retained_view` are
//! the two ownership cases a uniqueness-based reuse of the resting allocation
//! would separate (evaluated and not adopted); the replenish cases cover the
//! tail re-sequencing commit. See the "Residual allocation reuse" section of
//! `BENCH.md`.

use criterion::{BatchSize, Criterion};
use pricelevel::{
    Hash32, Id, OrderType, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce, TimestampMs,
    UuidGenerator,
};
use std::hint::black_box;
use uuid::Uuid;

const PRICE: u128 = 10_000;
const TAKER_ID: u64 = 1_000_000_000;
const EXEC_TS: u64 = 1_800_000_000_000;

fn generator() -> UuidGenerator {
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .expect("hard-coded namespace UUID literal must parse");
    UuidGenerator::new(namespace)
}

fn level_with(order: OrderType<()>) -> PriceLevel {
    let level = PriceLevel::new(PRICE);
    drop(level.add_order(order).expect("seed maker"));
    level
}

fn large_standard() -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(0),
        price: Price::new(PRICE),
        quantity: Quantity::new(1_000_000_000_000),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_700_000_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn large_iceberg() -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::from_u64(0),
        price: Price::new(PRICE),
        visible_quantity: Quantity::new(10),
        hidden_quantity: Quantity::new(1_000_000_000_000),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_700_000_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("Residual allocation reuse (#147)");
    let ids = generator();
    let taker = Id::from_u64(TAKER_ID);
    let ts = TimestampMs::new(EXEC_TS);

    for (name, order) in [
        ("partial_unique", large_standard()),
        ("replenish_unique", large_iceberg()),
    ] {
        let level = level_with(order);
        group.bench_function(name, |b| {
            b.iter_with_large_drop(|| {
                level.match_order(
                    black_box(10),
                    taker,
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    ts,
                    &ids,
                )
            });
        });
    }

    for (name, order) in [
        ("partial_retained_view", large_standard()),
        ("replenish_retained_view", large_iceberg()),
    ] {
        let level = level_with(order);
        group.bench_function(name, |b| {
            b.iter_batched(
                || level.iter_orders().next(),
                |view| {
                    let result = level.match_order(
                        black_box(10),
                        taker,
                        TimeInForce::Gtc,
                        TakerKind::Standard,
                        ts,
                        &ids,
                    );
                    (view, result)
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}
