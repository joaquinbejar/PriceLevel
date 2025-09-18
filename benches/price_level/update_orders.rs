use criterion::{BenchmarkId, Criterion};
use pricelevel::{OrderId, OrderType, OrderUpdate, PriceLevel, Side, TimeInForce};
use std::hint::black_box;

/// Register all benchmarks for updating orders at a price level
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Update Orders");

    // Benchmark canceling orders
    group.bench_function("cancel_order", |b| {
        b.iter(|| {
            let price_level = setup_standard_orders(100);
            // Cancel orders from the middle to avoid best/worst case scenarios
            for i in 25..75 {
                let _ = black_box(price_level.update_order(OrderUpdate::Cancel {
                    order_id: OrderId::from_u64(i),
                }));
            }
        })
    });

    // Benchmark updating order quantities
    group.bench_function("update_quantity", |b| {
        b.iter(|| {
            let price_level = setup_standard_orders(100);
            for i in 25..75 {
                let _ = black_box(price_level.update_order(OrderUpdate::UpdateQuantity {
                    order_id: OrderId::from_u64(i),
                    new_quantity: 200,
                }));
            }
        })
    });

    // Benchmark replacing orders (same price)
    group.bench_function("replace_order_same_price", |b| {
        b.iter(|| {
            let price_level = setup_standard_orders(100);
            for i in 25..75 {
                let _ = black_box(price_level.update_order(OrderUpdate::Replace {
                    order_id: OrderId::from_u64(i),
                    price: 10000, // Same price
                    quantity: 150,
                    side: Side::Buy,
                }));
            }
        })
    });

    // Benchmark replacing orders (different price)
    group.bench_function("replace_order_different_price", |b| {
        b.iter(|| {
            let price_level = setup_standard_orders(100);
            for i in 25..75 {
                let _ = black_box(price_level.update_order(OrderUpdate::Replace {
                    order_id: OrderId::from_u64(i),
                    price: 10100, // Different price
                    quantity: 150,
                    side: Side::Buy,
                }));
            }
        })
    });

    // Benchmark updating iceberg orders
    group.bench_function("update_iceberg_quantity", |b| {
        b.iter(|| {
            let price_level = setup_iceberg_orders(100);
            for i in 25..75 {
                // In a real scenario we'd be updating both visible and hidden
                // but for benchmark we're just using the UpdateQuantity which works on visible
                let _ = black_box(price_level.update_order(OrderUpdate::UpdateQuantity {
                    order_id: OrderId::from_u64(i),
                    new_quantity: 15,
                }));
            }
        })
    });

    // Parametrized benchmark with different order counts for cancellation
    for order_count in [10, 100, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("cancel_order_count_scaling", order_count),
            order_count,
            |b, &order_count| {
                b.iter(|| {
                    let price_level = setup_standard_orders(order_count);
                    // Cancel 25% of orders
                    let cancel_count = order_count / 4;
                    for i in 0..cancel_count {
                        let _ = black_box(price_level.update_order(OrderUpdate::Cancel {
                            order_id: OrderId::from_u64(i),
                        }));
                    }
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
            id: OrderId::from_u64(i),
            price: 10000,
            quantity: 10,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
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
            id: OrderId::from_u64(i),
            price: 10000,
            visible_quantity: 5,
            hidden_quantity: 15,
            side: Side::Buy,
            timestamp: 1616823000000 + i,
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level.add_order(order);
    }

    price_level
}
