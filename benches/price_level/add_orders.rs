use criterion::{BenchmarkId, Criterion};
use pricelevel::{OrderId, OrderType, PriceLevel, Side, TimeInForce};
use std::hint::black_box;

/// Register all benchmarks for adding orders to a price level
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Add Orders");

    // Benchmark adding standard orders
    group.bench_function("add_standard_order", |b| {
        b.iter(|| {
            let price_level = PriceLevel::new(10000);
            for i in 0..100 {
                let order = create_standard_order(i, 10000, 100);
                black_box(price_level.add_order(order));
            }
        })
    });

    // Benchmark adding iceberg orders
    group.bench_function("add_iceberg_order", |b| {
        b.iter(|| {
            let price_level = PriceLevel::new(10000);
            for i in 0..100 {
                let order = create_iceberg_order(i, 10000, 50, 150);
                black_box(price_level.add_order(order));
            }
        })
    });

    // Benchmark adding reserve orders
    group.bench_function("add_reserve_order", |b| {
        b.iter(|| {
            let price_level = PriceLevel::new(10000);
            for i in 0..100 {
                let order = create_reserve_order(i, 10000, 50, 150, 10, true, None);
                black_box(price_level.add_order(order));
            }
        })
    });

    // Benchmark adding mixed order types
    group.bench_function("add_mixed_orders", |b| {
        b.iter(|| {
            let price_level = PriceLevel::new(10000);
            for i in 0..100 {
                let order = match i % 5 {
                    0 => create_standard_order(i, 10000, 100),
                    1 => create_iceberg_order(i, 10000, 50, 150),
                    2 => create_post_only_order(i, 10000, 100),
                    3 => create_reserve_order(i, 10000, 50, 150, 10, true, None),
                    _ => create_pegged_order(i, 10000, 100),
                };
                black_box(price_level.add_order(order));
            }
        })
    });

    // Parametrized benchmark with different order counts
    for order_count in [10, 100, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("order_count_scaling", order_count),
            order_count,
            |b, &order_count| {
                b.iter(|| {
                    let price_level = PriceLevel::new(10000);
                    for i in 0..order_count {
                        let order = create_standard_order(i, 10000, 100);
                        black_box(price_level.add_order(order));
                    }
                })
            },
        );
    }

    group.finish();
}

// Helper functions to create different types of orders for benchmarking

/// Create a standard limit order for testing
fn create_standard_order(id: u64, price: u64, quantity: u64) -> OrderType {
    OrderType::Standard {
        id: OrderId::from_u64(id),
        price,
        quantity,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
    }
}

/// Create an iceberg order for testing
fn create_iceberg_order(id: u64, price: u64, visible: u64, hidden: u64) -> OrderType {
    OrderType::IcebergOrder {
        id: OrderId::from_u64(id),
        price,
        visible_quantity: visible,
        hidden_quantity: hidden,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
    }
}

/// Create a post-only order for testing
fn create_post_only_order(id: u64, price: u64, quantity: u64) -> OrderType {
    OrderType::PostOnly {
        id: OrderId::from_u64(id),
        price,
        quantity,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
    }
}

/// Create a reserve order for testing
fn create_reserve_order(
    id: u64,
    price: u64,
    visible: u64,
    hidden: u64,
    threshold: u64,
    auto_replenish: bool,
    replenish_amount: Option<u64>,
) -> OrderType {
    OrderType::ReserveOrder {
        id: OrderId::from_u64(id),
        price,
        visible_quantity: visible,
        hidden_quantity: hidden,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: threshold,
        replenish_amount,
        auto_replenish,
    }
}

/// Create a pegged order for testing
fn create_pegged_order(id: u64, price: u64, quantity: u64) -> OrderType {
    use pricelevel::PegReferenceType;

    OrderType::PeggedOrder {
        id: OrderId::from_u64(id),
        price,
        quantity,
        side: Side::Buy,
        timestamp: 1616823000000,
        time_in_force: TimeInForce::Gtc,
        reference_price_offset: -50,
        reference_price_type: PegReferenceType::BestAsk,
    }
}
