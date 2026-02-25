use criterion::Criterion;
use pricelevel::{
    Hash32, Id, OrderType, OrderUpdate, Price, PriceLevel, Quantity, Side, TimeInForce,
    TimestampMs, UuidGenerator,
};
use std::hint::black_box;
use uuid::Uuid;

/// Register benchmarks for full order lifecycle: add → update → cancel → match → stats.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Lifecycle");

    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let id_gen = UuidGenerator::new(namespace);

    // Full lifecycle: add → update quantity → match → cancel remainder → stats
    group.bench_function("full_lifecycle", |b| {
        b.iter(|| {
            let level = PriceLevel::new(10000);

            // Phase 1: Add 100 orders
            for i in 0..100_u64 {
                level.add_order(create_standard_order(i, 10000, 50));
            }

            // Phase 2: Update quantity on 20 orders
            for i in 10..30_u64 {
                let _ = level.update_order(OrderUpdate::UpdateQuantity {
                    order_id: Id::from_u64(i),
                    new_quantity: Quantity::new(25),
                });
            }

            // Phase 3: Cancel 10 orders
            for i in 50..60_u64 {
                let _ = level.update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(i),
                });
            }

            // Phase 4: Match 500 units
            let result = level.match_order(500, Id::from_u64(9999), &id_gen);
            let _ = black_box(result.executed_quantity());

            // Phase 5: Read stats
            let stats = level.stats();
            black_box(stats.orders_added());
            black_box(stats.orders_removed());
            black_box(stats.orders_executed());
            black_box(stats.quantity_executed());
        })
    });

    // Add-heavy lifecycle: 80% add, 20% match
    group.bench_function("add_heavy_lifecycle", |b| {
        b.iter(|| {
            let level = PriceLevel::new(10000);

            for i in 0..200_u64 {
                if i % 5 == 0 {
                    // Match every 5th iteration
                    black_box(level.match_order(10, Id::from_u64(10000 + i), &id_gen));
                } else {
                    // Add order
                    level.add_order(create_standard_order(i, 10000, 20));
                }
            }
            black_box(level.order_count());
        })
    });

    // Cancel-heavy lifecycle: many adds followed by many cancels
    group.bench_function("cancel_heavy_lifecycle", |b| {
        b.iter(|| {
            let level = PriceLevel::new(10000);

            // Add 200 orders
            for i in 0..200_u64 {
                level.add_order(create_standard_order(i, 10000, 10));
            }

            // Cancel 150 of them
            for i in 0..150_u64 {
                let _ = level.update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(i),
                });
            }

            black_box(level.order_count());
            black_box(level.visible_quantity());
        })
    });

    // Match-drain lifecycle: fill level then drain completely
    group.bench_function("match_drain_lifecycle", |b| {
        b.iter(|| {
            let level = PriceLevel::new(10000);

            // Add 100 orders with quantity 10 each = 1000 total
            for i in 0..100_u64 {
                level.add_order(create_standard_order(i, 10000, 10));
            }

            // Drain completely
            let result = level.match_order(1000, Id::from_u64(9999), &id_gen);
            black_box(result.is_complete());
            black_box(result.filled_order_ids().len());
        })
    });

    // Replace-heavy lifecycle: add orders then replace them
    group.bench_function("replace_heavy_lifecycle", |b| {
        b.iter(|| {
            let level = PriceLevel::new(10000);

            // Add 100 orders
            for i in 0..100_u64 {
                level.add_order(create_standard_order(i, 10000, 50));
            }

            // Replace 50 orders (same price = quantity update)
            for i in 0..50_u64 {
                let _ = level.update_order(OrderUpdate::Replace {
                    order_id: Id::from_u64(i),
                    price: Price::new(10000),
                    quantity: Quantity::new(30),
                    side: Side::Buy,
                });
            }

            // Replace 25 orders (different price = removal)
            for i in 50..75_u64 {
                let _ = level.update_order(OrderUpdate::Replace {
                    order_id: Id::from_u64(i),
                    price: Price::new(10100),
                    quantity: Quantity::new(30),
                    side: Side::Buy,
                });
            }

            black_box(level.order_count());
        })
    });

    // Lifecycle with stats query interleaved
    group.bench_function("lifecycle_with_stats_queries", |b| {
        b.iter(|| {
            let level = PriceLevel::new(10000);

            for i in 0..100_u64 {
                level.add_order(create_standard_order(i, 10000, 20));

                // Query stats every 10 adds
                if i % 10 == 0 {
                    black_box(level.visible_quantity());
                    black_box(level.hidden_quantity());
                    let _ = black_box(level.total_quantity());
                    black_box(level.order_count());
                    let stats = level.stats();
                    black_box(stats.orders_added());
                }
            }

            // Match and query stats
            let result = level.match_order(100, Id::from_u64(9999), &id_gen);
            let _ = black_box(result.executed_quantity());

            let stats = level.stats();
            black_box(stats.average_execution_price());
            black_box(stats.average_waiting_time());
            black_box(stats.time_since_last_execution());
        })
    });

    group.finish();
}

/// Create a standard limit order.
fn create_standard_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_616_823_000_000 + id),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}
