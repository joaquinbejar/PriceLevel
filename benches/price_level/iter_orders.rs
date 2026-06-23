use criterion::{BenchmarkId, Criterion};
use pricelevel::{
    Hash32, Id, OrderType, Price, PriceLevel, Quantity, Side, TimeInForce, TimestampMs,
};
use std::hint::black_box;

/// Register benchmarks for the zero-alloc `iter_orders` read path.
///
/// `iter_orders` returns an `impl Iterator` over the resting orders without
/// materializing an intermediate `Vec`. These benchmarks populate a level with
/// `N` resting orders and traverse it via `iter_orders()`, so a future
/// regression that re-introduces materialization (an allocating `Vec` return)
/// is caught by a throughput drop.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Iter Orders");

    // Traverse the iterator, touching each order's quantity so the loop body is
    // not optimized away.
    for order_count in [10, 100, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("iter_orders_sum_visible", order_count),
            order_count,
            |b, &order_count| {
                let price_level = setup_level(order_count);
                b.iter(|| {
                    let mut total: u64 = 0;
                    for order in price_level.iter_orders() {
                        total = total.wrapping_add(black_box(order.visible_quantity().as_u64()));
                    }
                    black_box(total)
                })
            },
        );
    }

    // Pure traversal: count the resting orders via the iterator without
    // materializing a Vec.
    for order_count in [10, 100, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("iter_orders_count", order_count),
            order_count,
            |b, &order_count| {
                let price_level = setup_level(order_count);
                b.iter(|| black_box(price_level.iter_orders().count()))
            },
        );
    }

    group.finish();
}

/// Populate a price level with `order_count` resting standard orders.
fn setup_level(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);
    for i in 0..order_count {
        let order = OrderType::Standard {
            id: Id::from_u64(i),
            price: Price::new(10000),
            quantity: Quantity::new(100),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level.add_order(order);
    }
    price_level
}
