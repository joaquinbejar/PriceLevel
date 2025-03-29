use criterion::{BenchmarkId, Criterion, black_box};
use pricelevel::{OrderId, OrderType, PriceLevel, Side, TimeInForce};
use std::sync::atomic::AtomicU64;

/// Register all benchmarks for matching orders at a price level
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Match Orders");
    group.sample_size(100); // Adjust sample size for more consistent results

    // Benchmark matching against standard orders
    group.bench_function("match_standard_orders", |b| {
        b.iter(|| {
            let price_level = setup_standard_orders(100);
            let transaction_id_generator = AtomicU64::new(1);
            black_box(price_level.match_order(50, OrderId(999), &transaction_id_generator));
        })
    });

    // Benchmark matching against iceberg orders
    group.bench_function("match_iceberg_orders", |b| {
        b.iter(|| {
            let price_level = setup_iceberg_orders(100);
            let transaction_id_generator = AtomicU64::new(1);
            black_box(price_level.match_order(75, OrderId(999), &transaction_id_generator));
        })
    });

    // Benchmark matching against reserve orders
    group.bench_function("match_reserve_orders", |b| {
        b.iter(|| {
            let price_level = setup_reserve_orders(100);
            let transaction_id_generator = AtomicU64::new(1);
            black_box(price_level.match_order(60, OrderId(999), &transaction_id_generator));
        })
    });

    // Benchmark matching against mixed order types
    group.bench_function("match_mixed_orders", |b| {
        b.iter(|| {
            let price_level = setup_mixed_orders(100);
            let transaction_id_generator = AtomicU64::new(1);
            black_box(price_level.match_order(100, OrderId(999), &transaction_id_generator));
        })
    });

    // Benchmark with different match quantities against standard orders
    for match_quantity in [10, 50, 100, 200, 500].iter() {
        group.bench_with_input(
            BenchmarkId::new("match_quantity_scaling_standard", match_quantity),
            match_quantity,
            |b, &match_quantity| {
                b.iter(|| {
                    let price_level = setup_standard_orders(50);
                    let transaction_id_generator = AtomicU64::new(1);
                    black_box(price_level.match_order(
                        match_quantity,
                        OrderId(999),
                        &transaction_id_generator,
                    ));
                })
            },
        );
    }

    // Benchmark with different match quantities against iceberg orders
    for match_quantity in [10, 50, 100, 200, 500].iter() {
        group.bench_with_input(
            BenchmarkId::new("match_quantity_scaling_iceberg", match_quantity),
            match_quantity,
            |b, &match_quantity| {
                b.iter(|| {
                    let price_level = setup_iceberg_orders(25);
                    let transaction_id_generator = AtomicU64::new(1);
                    black_box(price_level.match_order(
                        match_quantity,
                        OrderId(999),
                        &transaction_id_generator,
                    ));
                })
            },
        );
    }

    group.finish();
}

// Helper functions to set up price levels with different order types

/// Set up a price level with standard orders
fn setup_standard_orders(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);

    for i in 0..order_count {
        let order = OrderType::Standard {
            id: OrderId(i),
            price: 10000,
            quantity: 10,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
        };
        price_level.add_order(order);
    }

    price_level
}

/// Set up a price level with iceberg orders
fn setup_iceberg_orders(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);

    for i in 0..order_count {
        let order = OrderType::IcebergOrder {
            id: OrderId(i),
            price: 10000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
        };
        price_level.add_order(order);
    }

    price_level
}

/// Set up a price level with reserve orders
fn setup_reserve_orders(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);

    for i in 0..order_count {
        let order = OrderType::ReserveOrder {
            id: OrderId(i),
            price: 10000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: 2,
            replenish_amount: Some(5),
            auto_replenish: true,
        };
        price_level.add_order(order);
    }

    price_level
}

/// Set up a price level with mixed order types
fn setup_mixed_orders(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);

    for i in 0..order_count {
        let order = match i % 3 {
            0 => OrderType::Standard {
                id: OrderId(i),
                price: 10000,
                quantity: 10,
                side: Side::Buy,
                timestamp: 1616823000000 + i,
                time_in_force: TimeInForce::Gtc,
            },
            1 => OrderType::IcebergOrder {
                id: OrderId(i),
                price: 10000,
                visible_quantity: 5,
                hidden_quantity: 15,
                side: Side::Buy,
                timestamp: 1616823000000 + i,
                time_in_force: TimeInForce::Gtc,
            },
            _ => OrderType::ReserveOrder {
                id: OrderId(i),
                price: 10000,
                visible_quantity: 5,
                hidden_quantity: 15,
                side: Side::Buy,
                timestamp: 1616823000000 + i,
                time_in_force: TimeInForce::Gtc,
                replenish_threshold: 2,
                replenish_amount: Some(5),
                auto_replenish: true,
            },
        };
        price_level.add_order(order);
    }

    price_level
}
