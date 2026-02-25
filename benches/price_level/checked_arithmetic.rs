use criterion::Criterion;
use pricelevel::{
    Hash32, Id, MatchResult, OrderType, Price, PriceLevel, Quantity, Side, TimeInForce,
    TimestampMs, Trade, UuidGenerator,
};
use std::hint::black_box;
use uuid::Uuid;

/// Register benchmarks for checked arithmetic APIs and error-path performance.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Checked Arithmetic");

    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let id_gen = UuidGenerator::new(namespace);

    // Benchmark total_quantity (checked add of visible + hidden)
    group.bench_function("total_quantity", |b| {
        let level = setup_mixed_level(200);
        b.iter(|| {
            black_box(level.total_quantity().unwrap());
        })
    });

    // Benchmark executed_quantity on a MatchResult with many trades
    group.bench_function("executed_quantity_many_trades", |b| {
        let level = setup_standard_level(100);
        let result = level.match_order(500, Id::from_u64(9999), &id_gen);
        b.iter(|| {
            black_box(result.executed_quantity().unwrap());
        })
    });

    // Benchmark executed_value on a MatchResult with many trades
    group.bench_function("executed_value_many_trades", |b| {
        let level = setup_standard_level(100);
        let result = level.match_order(500, Id::from_u64(9999), &id_gen);
        b.iter(|| {
            black_box(result.executed_value().unwrap());
        })
    });

    // Benchmark average_price computation
    group.bench_function("average_price", |b| {
        let level = setup_standard_level(100);
        let result = level.match_order(500, Id::from_u64(9999), &id_gen);
        b.iter(|| {
            black_box(result.average_price().unwrap());
        })
    });

    // Benchmark MatchResult::new + add_trade sequence
    group.bench_function("match_result_add_trades", |b| {
        b.iter(|| {
            let mut mr = MatchResult::new(Id::from_u64(1), 1000);
            for i in 0..50_u64 {
                let trade = Trade::new(
                    Id::from_u64(100 + i),
                    Id::from_u64(1),
                    Id::from_u64(i),
                    Price::new(10000),
                    Quantity::new(20),
                    Side::Buy,
                );
                let _ = mr.add_trade(trade);
            }
            black_box(mr);
        })
    });

    // Benchmark empty match result (zero trades) arithmetic
    group.bench_function("empty_match_arithmetic", |b| {
        let empty_level = PriceLevel::new(10000);
        let result = empty_level.match_order(100, Id::from_u64(1), &id_gen);
        b.iter(|| {
            black_box(result.executed_quantity().unwrap());
            black_box(result.executed_value().unwrap());
            black_box(result.average_price().unwrap());
        })
    });

    // Scaling: executed_quantity with increasing trade counts
    for trade_count in [10, 50, 100].iter() {
        let tc = *trade_count;
        group.bench_function(format!("executed_quantity_{tc}_trades"), |b| {
            let level = setup_standard_level(tc);
            let result = level.match_order(tc * 10, Id::from_u64(9999), &id_gen);
            b.iter(|| {
                black_box(result.executed_quantity().unwrap());
            })
        });
    }

    group.finish();
}

/// Set up a price level with standard orders.
fn setup_standard_level(order_count: u64) -> PriceLevel {
    let level = PriceLevel::new(10000);
    for i in 0..order_count {
        level.add_order(OrderType::Standard {
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

/// Set up a price level with a mix of standard, iceberg, and reserve orders.
fn setup_mixed_level(order_count: u64) -> PriceLevel {
    let level = PriceLevel::new(10000);
    for i in 0..order_count {
        let order = match i % 3 {
            0 => OrderType::Standard {
                id: Id::from_u64(i),
                price: Price::new(10000),
                quantity: Quantity::new(100),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
            1 => OrderType::IcebergOrder {
                id: Id::from_u64(i),
                price: Price::new(10000),
                visible_quantity: Quantity::new(20),
                hidden_quantity: Quantity::new(80),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            },
            _ => OrderType::ReserveOrder {
                id: Id::from_u64(i),
                price: Price::new(10000),
                visible_quantity: Quantity::new(15),
                hidden_quantity: Quantity::new(60),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_616_823_000_000 + i),
                time_in_force: TimeInForce::Gtc,
                replenish_threshold: Quantity::new(5),
                replenish_amount: Some(Quantity::new(10)),
                auto_replenish: true,
                extra_fields: (),
            },
        };
        level.add_order(order);
    }
    level
}
