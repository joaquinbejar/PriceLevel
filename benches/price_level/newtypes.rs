use criterion::Criterion;
use pricelevel::{Id, Price, Quantity, TimestampMs};
use std::hint::black_box;
use std::str::FromStr;

/// Register benchmarks for domain newtype construction, parsing, and Display/FromStr.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Newtypes");

    // --- Price ---

    group.bench_function("price_new", |b| {
        b.iter(|| {
            black_box(Price::new(10000));
        })
    });

    group.bench_function("price_display_fromstr", |b| {
        let p = Price::new(12345);
        let s = p.to_string();
        b.iter(|| {
            let displayed = p.to_string();
            let parsed = Price::from_str(&s).unwrap();
            black_box((displayed, parsed));
        })
    });

    group.bench_function("price_serde_roundtrip", |b| {
        let p = Price::new(99999);
        let json = serde_json::to_string(&p).unwrap();
        b.iter(|| {
            let ser = serde_json::to_string(&p).unwrap();
            let de: Price = serde_json::from_str(&json).unwrap();
            black_box((ser, de));
        })
    });

    // --- Quantity ---

    group.bench_function("quantity_new", |b| {
        b.iter(|| {
            black_box(Quantity::new(500));
        })
    });

    group.bench_function("quantity_display_fromstr", |b| {
        let q = Quantity::new(500);
        let s = q.to_string();
        b.iter(|| {
            let displayed = q.to_string();
            let parsed = Quantity::from_str(&s).unwrap();
            black_box((displayed, parsed));
        })
    });

    group.bench_function("quantity_serde_roundtrip", |b| {
        let q = Quantity::new(12345);
        let json = serde_json::to_string(&q).unwrap();
        b.iter(|| {
            let ser = serde_json::to_string(&q).unwrap();
            let de: Quantity = serde_json::from_str(&json).unwrap();
            black_box((ser, de));
        })
    });

    // --- TimestampMs ---

    group.bench_function("timestamp_new", |b| {
        b.iter(|| {
            black_box(TimestampMs::new(1_616_823_000_000));
        })
    });

    group.bench_function("timestamp_display_fromstr", |b| {
        let ts = TimestampMs::new(1_616_823_000_000);
        let s = ts.to_string();
        b.iter(|| {
            let displayed = ts.to_string();
            let parsed = TimestampMs::from_str(&s).unwrap();
            black_box((displayed, parsed));
        })
    });

    // --- Id ---

    group.bench_function("id_from_u64", |b| {
        b.iter(|| {
            black_box(Id::from_u64(42));
        })
    });

    group.bench_function("id_from_uuid", |b| {
        let uuid = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        b.iter(|| {
            black_box(Id::from_uuid(uuid));
        })
    });

    group.bench_function("id_display_fromstr", |b| {
        let id = Id::from_u64(42);
        let s = id.to_string();
        b.iter(|| {
            let displayed = id.to_string();
            let parsed = Id::from_str(&s).unwrap();
            black_box((displayed, parsed));
        })
    });

    group.bench_function("id_serde_roundtrip", |b| {
        let id = Id::from_u64(999);
        let json = serde_json::to_string(&id).unwrap();
        b.iter(|| {
            let ser = serde_json::to_string(&id).unwrap();
            let de: Id = serde_json::from_str(&json).unwrap();
            black_box((ser, de));
        })
    });

    // Batch construction scaling
    for count in [100, 1000, 10000].iter() {
        let n = *count;
        group.bench_function(format!("id_from_u64_batch_{n}"), |b| {
            b.iter(|| {
                for i in 0..n as u64 {
                    black_box(Id::from_u64(i));
                }
            })
        });
    }

    group.finish();
}
