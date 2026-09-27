//! Trade-id emission on the match path (issue #168): every emitted trade
//! reserves one checked sequence value from the `UuidGenerator` (fill-or-kill
//! reserves its whole block up front). The level is rebuilt in the untimed
//! `iter_batched` setup so only the sweep itself is measured.

use criterion::{BatchSize, Criterion};
use pricelevel::{
    Hash32, Id, OrderType, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce, TimestampMs,
    UuidGenerator,
};
use std::hint::black_box;
use uuid::Uuid;

const MAKERS: u64 = 100;
const MAKER_QTY: u64 = 10;

fn level() -> PriceLevel {
    let level = PriceLevel::new(10_000);
    for i in 0..MAKERS {
        level
            .add_order(OrderType::Standard {
                id: Id::from_u64(i),
                price: Price::new(10_000),
                quantity: Quantity::new(MAKER_QTY),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            })
            .expect("add_order should succeed");
    }
    level
}

/// Register the trade-id emission benchmarks.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Trade Id Emission");
    let namespace = Uuid::from_u128(0x6ba7_b810_9dad_11d1_80b4_00c0_4fd4_30c8);

    for (name, tif) in [
        ("sweep_100_trades_gtc", TimeInForce::Gtc),
        ("sweep_100_trades_fok", TimeInForce::Fok),
    ] {
        group.bench_function(name, |b| {
            b.iter_batched(
                || (level(), UuidGenerator::new(namespace)),
                |(level, generator)| {
                    black_box(level.match_order(
                        MAKERS * MAKER_QTY,
                        Id::from_u64(999_999),
                        tif,
                        TakerKind::Standard,
                        TimestampMs::new(1_716_000_000_000),
                        &generator,
                    ))
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}
