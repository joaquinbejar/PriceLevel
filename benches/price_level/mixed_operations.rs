use criterion::{Criterion, black_box};
use pricelevel::{OrderId, OrderType, OrderUpdate, PriceLevel, Side, TimeInForce};
use std::sync::atomic::AtomicU64;

/// Register benchmarks for mixed/realistic price level operations
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Mixed Operations");

    // Benchmark a realistic trading scenario with mixed operations
    group.bench_function("realistic_trading_scenario", |b| {
        b.iter(|| {
            let price_level = PriceLevel::new(10000);
            let transaction_id_generator = AtomicU64::new(1);

            // Phase 1: Add initial orders (70% standard, 20% iceberg, 10% reserve)
            for i in 0..100 {
                let order = match i % 10 {
                    0..=6 => create_standard_order(i, 10000, 10 + i % 5),
                    7..=8 => create_iceberg_order(i, 10000, 5, 15),
                    _ => create_reserve_order(i, 10000, 5, 15, 2, true, None),
                };
                price_level.add_order(order);
            }

            // Phase 2: Execute some matches
            for _ in 0..5 {
                let _ =
                    black_box(price_level.match_order(50, OrderId(999), &transaction_id_generator));
            }

            // Phase 3: Update some orders
            for i in 20..40 {
                if i % 2 == 0 {
                    let _ = black_box(price_level.update_order(OrderUpdate::UpdateQuantity {
                        order_id: OrderId(i),
                        new_quantity: 20,
                    }));
                } else {
                    let _ = black_box(price_level.update_order(OrderUpdate::Cancel {
                        order_id: OrderId(i),
                    }));
                }
            }

            // Phase 4: Add more orders
            for i in 100..150 {
                let order = match i % 10 {
                    0..=6 => create_standard_order(i, 10000, 10 + i % 5),
                    7..=8 => create_iceberg_order(i, 10000, 5, 15),
                    _ => create_reserve_order(i, 10000, 5, 15, 2, true, None),
                };
                price_level.add_order(order);
            }

            // Phase 5: Execute final matches
            for _ in 0..3 {
                black_box(price_level.match_order(100, OrderId(1000), &transaction_id_generator));
            }
        })
    });

    // Benchmark high-frequency trading scenario (many small matches)
    group.bench_function("high_frequency_scenario", |b| {
        b.iter(|| {
            let price_level = PriceLevel::new(10000);
            let transaction_id_generator = AtomicU64::new(1);

            // Add initial orders
            for i in 0..200 {
                let order = create_standard_order(i, 10000, 5);
                price_level.add_order(order);
            }

            // Execute many small matches interspersed with new orders and cancellations
            for i in 0..100 {
                // Match a small amount
                black_box(price_level.match_order(2, OrderId(1000 + i), &transaction_id_generator));

                // Add a new order
                let order = create_standard_order(200 + i, 10000, 5);
                price_level.add_order(order);

                // Cancel an order
                if i % 10 == 0 {
                    let _ = black_box(price_level.update_order(OrderUpdate::Cancel {
                        order_id: OrderId(i),
                    }));
                }
            }
        })
    });

    // Benchmark large order throughput
    group.bench_function("large_order_throughput", |b| {
        b.iter(|| {
            let price_level = PriceLevel::new(10000);
            let transaction_id_generator = AtomicU64::new(1);

            // Add a large number of small orders
            for i in 0..500 {
                let order = create_standard_order(i, 10000, 2);
                price_level.add_order(order);
            }

            // Execute a few large matches
            black_box(price_level.match_order(300, OrderId(1001), &transaction_id_generator));
            black_box(price_level.match_order(400, OrderId(1002), &transaction_id_generator));
            black_box(price_level.match_order(300, OrderId(1003), &transaction_id_generator));
        })
    });

    // Benchmark snapshot creation
    group.bench_function("create_snapshots", |b| {
        b.iter(|| {
            let price_level = setup_mixed_orders(200);

            // Create multiple snapshots
            for _ in 0..10 {
                black_box(price_level.snapshot());
            }
        })
    });

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
        timestamp: 1616823000000 + id,
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
        timestamp: 1616823000000 + id,
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
        timestamp: 1616823000000 + id,
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: threshold,
        replenish_amount,
        auto_replenish,
    }
}

/// Set up a price level with mixed order types
fn setup_mixed_orders(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);

    for i in 0..order_count {
        let order = match i % 3 {
            0 => create_standard_order(i, 10000, 10),
            1 => create_iceberg_order(i, 10000, 5, 15),
            _ => create_reserve_order(i, 10000, 5, 15, 2, true, None),
        };
        price_level.add_order(order);
    }

    price_level
}
