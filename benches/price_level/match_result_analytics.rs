//! `MatchResult` analytics cost (#151).
//!
//! Separates the four phases the issue asks to tell apart, for sweeps of
//! 0, 1, 32, 256 and 4096 trades:
//!
//! - `build`: result construction only (`try_with_capacity` + `add_trade`),
//!   the cost any cached aggregate would add to;
//! - `executed_quantity` / `executed_value` / `average_price`: one query;
//! - `all_queries`: the three getters once each (the common consumer pattern);
//! - `repeated_all_x8`: the three getters eight times over the same result;
//! - `build_plus_all`: construction followed by one `all_queries` pass.
//!
//! Results are built through the public API only, so the fixture is the
//! shape the matching engine emits (one taker, distinct makers, one price).

use criterion::{BatchSize, BenchmarkId, Criterion};
use pricelevel::{Id, MatchResult, Price, Quantity, Side, TimestampMs, Trade};
use std::hint::black_box;

/// Trade counts required by #151.
const TRADE_COUNTS: [u64; 5] = [0, 1, 32, 256, 4096];

/// Number of full analytics passes in the repeated-read case.
const REPEATED_READS: usize = 8;

const TAKER_ID: u64 = 1;
const PRICE: u128 = 10_000;
const TRADE_QUANTITY: u64 = 10;

/// Pre-built trades for a sweep of `count` fills.
fn trades(count: u64) -> Vec<Trade> {
    (0..count)
        .map(|i| {
            Trade::with_timestamp(
                Id::from_u64(1_000_000 + i),
                Id::from_u64(TAKER_ID),
                Id::from_u64(10 + i),
                Price::new(PRICE + u128::from(i % 7)),
                Quantity::new(TRADE_QUANTITY + i % 5),
                Side::Buy,
                TimestampMs::new(1_716_000_000_000),
            )
        })
        .collect()
}

/// Builds a result holding exactly `trades` (plus one unit of remainder so
/// every trade count, including zero, yields a valid non-complete result).
fn build(trades: &[Trade]) -> MatchResult {
    let total: u64 = trades.iter().map(|t| t.quantity().as_u64()).sum();
    let mut result = MatchResult::try_with_capacity(
        Id::from_u64(TAKER_ID),
        Quantity::new(total + 1),
        trades.len(),
    )
    .expect("capacity reservation");
    for trade in trades {
        result.add_trade(*trade).expect("add_trade");
    }
    result
}

#[inline]
fn all_queries(result: &MatchResult) {
    black_box(result.executed_quantity().expect("quantity"));
    black_box(result.executed_value().expect("value"));
    black_box(result.average_price().expect("average"));
}

/// Register the `MatchResult` analytics benchmarks.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("MatchResult - Analytics");

    for count in TRADE_COUNTS {
        let fixture = trades(count);
        let result = build(&fixture);

        group.bench_with_input(BenchmarkId::new("build", count), &fixture, |b, f| {
            b.iter_batched_ref(
                || f.clone(),
                |f| black_box(build(black_box(f))),
                BatchSize::SmallInput,
            )
        });

        group.bench_with_input(
            BenchmarkId::new("executed_quantity", count),
            &result,
            |b, r| b.iter(|| black_box(black_box(r).executed_quantity().expect("quantity"))),
        );

        group.bench_with_input(
            BenchmarkId::new("executed_value", count),
            &result,
            |b, r| b.iter(|| black_box(black_box(r).executed_value().expect("value"))),
        );

        group.bench_with_input(BenchmarkId::new("average_price", count), &result, |b, r| {
            b.iter(|| black_box(black_box(r).average_price().expect("average")))
        });

        group.bench_with_input(BenchmarkId::new("all_queries", count), &result, |b, r| {
            b.iter(|| all_queries(black_box(r)))
        });

        group.bench_with_input(
            BenchmarkId::new("repeated_all_x8", count),
            &result,
            |b, r| {
                b.iter(|| {
                    for _ in 0..REPEATED_READS {
                        all_queries(black_box(r));
                    }
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("build_plus_all", count),
            &fixture,
            |b, f| {
                b.iter_batched_ref(
                    || f.clone(),
                    |f| {
                        let result = build(black_box(f));
                        all_queries(&result);
                        black_box(result)
                    },
                    BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}
