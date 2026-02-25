use criterion::Criterion;
use pricelevel::{
    Hash32, Id, OrderType, Price, PriceLevel, PriceLevelSnapshotPackage, Quantity, Side,
    TimeInForce, TimestampMs,
};
use std::hint::black_box;

/// Register benchmarks for snapshot checksum creation, JSON roundtrip, and recovery.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Snapshot Recovery");

    // Benchmark snapshot_package creation (includes checksum computation)
    group.bench_function("snapshot_package_creation", |b| {
        let price_level = setup_mixed_level(200);
        b.iter(|| {
            black_box(price_level.snapshot_package().unwrap());
        })
    });

    // Benchmark snapshot to JSON serialization
    group.bench_function("snapshot_to_json", |b| {
        let price_level = setup_mixed_level(200);
        b.iter(|| {
            black_box(price_level.snapshot_to_json().unwrap());
        })
    });

    // Benchmark JSON deserialization + validation
    group.bench_function("snapshot_from_json_validate", |b| {
        let price_level = setup_mixed_level(200);
        let json = price_level.snapshot_to_json().unwrap();
        b.iter(|| {
            let pkg = PriceLevelSnapshotPackage::from_json(&json).unwrap();
            pkg.validate().unwrap();
            black_box(pkg);
        })
    });

    // Benchmark full roundtrip: snapshot → JSON → restore PriceLevel
    group.bench_function("snapshot_full_roundtrip", |b| {
        let price_level = setup_mixed_level(200);
        let json = price_level.snapshot_to_json().unwrap();
        b.iter(|| {
            let restored = PriceLevel::from_snapshot_json(&json).unwrap();
            black_box(restored);
        })
    });

    // Benchmark snapshot_package validation only (checksum verify)
    group.bench_function("snapshot_checksum_validate", |b| {
        let price_level = setup_mixed_level(200);
        let pkg = price_level.snapshot_package().unwrap();
        let json = pkg.to_json().unwrap();
        let deserialized = PriceLevelSnapshotPackage::from_json(&json).unwrap();
        b.iter(|| {
            deserialized.validate().unwrap();
            black_box(());
        })
    });

    // Benchmark from_snapshot_package (validation + reconstruction)
    group.bench_function("from_snapshot_package", |b| {
        let price_level = setup_mixed_level(200);
        let json = price_level.snapshot_to_json().unwrap();
        b.iter(|| {
            let pkg = PriceLevelSnapshotPackage::from_json(&json).unwrap();
            let restored = PriceLevel::from_snapshot_package(pkg).unwrap();
            black_box(restored);
        })
    });

    // Scaling: snapshot creation with increasing order counts
    for order_count in [50, 200, 500].iter() {
        group.bench_function(format!("snapshot_package_{order_count}_orders"), |b| {
            let price_level = setup_mixed_level(*order_count);
            b.iter(|| {
                black_box(price_level.snapshot_package().unwrap());
            })
        });
    }

    group.finish();
}

/// Set up a price level with a mix of standard, iceberg, and reserve orders.
fn setup_mixed_level(order_count: u64) -> PriceLevel {
    let price_level = PriceLevel::new(10000);
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
        price_level.add_order(order);
    }
    price_level
}
