use criterion::{BenchmarkId, Criterion};
use pricelevel::{
    Hash32, Id, OrderType, PegReferenceType, Price, PriceLevel, Quantity, Side, TimeInForce,
    TimestampMs, UuidGenerator,
};
use std::hint::black_box;
use uuid::Uuid;

/// Register benchmarks for matching against each special order type.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Special Order Matching");

    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let id_gen = UuidGenerator::new(namespace);

    // Benchmark matching against PostOnly orders
    group.bench_function("match_post_only", |b| {
        b.iter(|| {
            let level = setup_post_only_level(100);
            black_box(level.match_order(50, Id::from_u64(9999), &id_gen));
        })
    });

    // Benchmark matching against TrailingStop orders
    group.bench_function("match_trailing_stop", |b| {
        b.iter(|| {
            let level = setup_trailing_stop_level(100);
            black_box(level.match_order(50, Id::from_u64(9999), &id_gen));
        })
    });

    // Benchmark matching against Pegged orders
    group.bench_function("match_pegged", |b| {
        b.iter(|| {
            let level = setup_pegged_level(100);
            black_box(level.match_order(50, Id::from_u64(9999), &id_gen));
        })
    });

    // Benchmark matching against MarketToLimit orders
    group.bench_function("match_market_to_limit", |b| {
        b.iter(|| {
            let level = setup_market_to_limit_level(100);
            black_box(level.match_order(50, Id::from_u64(9999), &id_gen));
        })
    });

    // Benchmark matching against all special types mixed
    group.bench_function("match_all_special_mixed", |b| {
        b.iter(|| {
            let level = setup_all_special_mixed(100);
            black_box(level.match_order(100, Id::from_u64(9999), &id_gen));
        })
    });

    // Scaling: special order matching with different queue depths
    for order_count in [25, 100, 500].iter() {
        group.bench_with_input(
            BenchmarkId::new("match_special_mixed_scaling", order_count),
            order_count,
            |b, &order_count| {
                b.iter(|| {
                    let level = setup_all_special_mixed(order_count);
                    black_box(level.match_order(order_count / 2, Id::from_u64(9999), &id_gen));
                })
            },
        );
    }

    group.finish();
}

/// Set up a price level with PostOnly orders.
fn setup_post_only_level(order_count: u64) -> PriceLevel {
    let level = PriceLevel::new(10000);
    for i in 0..order_count {
        level.add_order(OrderType::PostOnly {
            id: Id::from_u64(i),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        });
    }
    level
}

/// Set up a price level with TrailingStop orders.
fn setup_trailing_stop_level(order_count: u64) -> PriceLevel {
    let level = PriceLevel::new(10000);
    for i in 0..order_count {
        level.add_order(OrderType::TrailingStop {
            id: Id::from_u64(i),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + i),
            time_in_force: TimeInForce::Gtc,
            trail_amount: Quantity::new(100),
            last_reference_price: Price::new(10100),
            extra_fields: (),
        });
    }
    level
}

/// Set up a price level with Pegged orders.
fn setup_pegged_level(order_count: u64) -> PriceLevel {
    let level = PriceLevel::new(10000);
    for i in 0..order_count {
        level.add_order(OrderType::PeggedOrder {
            id: Id::from_u64(i),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + i),
            time_in_force: TimeInForce::Gtc,
            reference_price_offset: -50,
            reference_price_type: PegReferenceType::BestAsk,
            extra_fields: (),
        });
    }
    level
}

/// Set up a price level with MarketToLimit orders.
fn setup_market_to_limit_level(order_count: u64) -> PriceLevel {
    let level = PriceLevel::new(10000);
    for i in 0..order_count {
        level.add_order(OrderType::MarketToLimit {
            id: Id::from_u64(i),
            price: Price::new(10000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        });
    }
    level
}

/// Set up a price level with a mix of all special order types.
fn setup_all_special_mixed(order_count: u64) -> PriceLevel {
    let level = PriceLevel::new(10000);
    for i in 0..order_count {
        let order = match i % 4 {
            0 => OrderType::PostOnly {
                id: Id::from_u64(i),
                price: Price::new(10000),
                quantity: Quantity::new(10),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
            1 => OrderType::TrailingStop {
                id: Id::from_u64(i),
                price: Price::new(10000),
                quantity: Quantity::new(10),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                trail_amount: Quantity::new(100),
                last_reference_price: Price::new(10100),
                extra_fields: (),
            },
            2 => OrderType::PeggedOrder {
                id: Id::from_u64(i),
                price: Price::new(10000),
                quantity: Quantity::new(10),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                reference_price_offset: -50,
                reference_price_type: PegReferenceType::BestAsk,
                extra_fields: (),
            },
            _ => OrderType::MarketToLimit {
                id: Id::from_u64(i),
                price: Price::new(10000),
                quantity: Quantity::new(10),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
        };
        level.add_order(order);
    }
    level
}
