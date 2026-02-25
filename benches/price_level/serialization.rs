use criterion::Criterion;
use pricelevel::{
    Hash32, Id, MatchResult, OrderType, Price, PriceLevel, Quantity, Side, TimeInForce,
    TimestampMs, Trade, TradeList, UuidGenerator,
};
use std::hint::black_box;
use std::str::FromStr;
use uuid::Uuid;

/// Register benchmarks for Trade, TradeList, and MatchResult serialization roundtrips.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Serialization");

    // --- Trade benchmarks ---

    let trade = Trade::with_timestamp(
        Id::from_u64(100),
        Id::from_u64(1),
        Id::from_u64(2),
        Price::new(9500),
        Quantity::new(42),
        Side::Buy,
        TimestampMs::new(1_616_823_000_000),
    );

    // Trade Display
    group.bench_function("trade_display", |b| {
        b.iter(|| {
            black_box(trade.to_string());
        })
    });

    // Trade FromStr
    let trade_str = trade.to_string();
    group.bench_function("trade_from_str", |b| {
        b.iter(|| {
            black_box(Trade::from_str(&trade_str).unwrap());
        })
    });

    // Trade serde JSON serialize
    group.bench_function("trade_serde_serialize", |b| {
        b.iter(|| {
            black_box(serde_json::to_string(&trade).unwrap());
        })
    });

    // Trade serde JSON deserialize
    let trade_json = serde_json::to_string(&trade).unwrap();
    group.bench_function("trade_serde_deserialize", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<Trade>(&trade_json).unwrap());
        })
    });

    // --- TradeList benchmarks ---

    let mut trade_list = TradeList::new();
    for i in 0..20_u64 {
        trade_list.add(Trade::with_timestamp(
            Id::from_u64(100 + i),
            Id::from_u64(1),
            Id::from_u64(i),
            Price::new(10000),
            Quantity::new(10),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000 + i),
        ));
    }

    // TradeList Display
    group.bench_function("trade_list_display_20", |b| {
        b.iter(|| {
            black_box(trade_list.to_string());
        })
    });

    // TradeList FromStr
    let tl_str = trade_list.to_string();
    group.bench_function("trade_list_from_str_20", |b| {
        b.iter(|| {
            black_box(TradeList::from_str(&tl_str).unwrap());
        })
    });

    // --- MatchResult benchmarks ---

    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let id_gen = UuidGenerator::new(namespace);
    let level = setup_standard_level(50);
    let match_result = level.match_order(200, Id::from_u64(9999), &id_gen);

    // MatchResult Display
    group.bench_function("match_result_display", |b| {
        b.iter(|| {
            black_box(match_result.to_string());
        })
    });

    // MatchResult FromStr
    let mr_str = match_result.to_string();
    group.bench_function("match_result_from_str", |b| {
        b.iter(|| {
            black_box(MatchResult::from_str(&mr_str).unwrap());
        })
    });

    // MatchResult serde JSON serialize
    group.bench_function("match_result_serde_serialize", |b| {
        b.iter(|| {
            black_box(serde_json::to_string(&match_result).unwrap());
        })
    });

    // MatchResult serde JSON deserialize
    let mr_json = serde_json::to_string(&match_result).unwrap();
    group.bench_function("match_result_serde_deserialize", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<MatchResult>(&mr_json).unwrap());
        })
    });

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
