//! Fill-or-kill feasibility cost versus resting depth (issue #143).
//!
//! Every case keeps one long-lived level. The timed routine is one
//! `match_order` call that returns its `MatchResult`; `iter_batched` drops
//! the outputs after the measured batch, so result destruction is not
//! timed. An untimed setup asserts that the level holds exactly the stated
//! depth when each timed call starts.
//!
//! * `fok_first_maker@depth` — a qty-1 FOK taker filled by the front qty-1
//!   standard maker. `depth - 1` makers are seeded and the per-iteration
//!   setup (`BatchSize::PerIteration`, one setup per routine) admits one
//!   more, so every call starts at `depth`. The dry run only needs the
//!   front maker.
//! * `gtc_first_maker@depth` — the same taker with GTC: the control.
//! * `fok_rejected@depth` — a FOK taker one unit larger than the level: the
//!   dry run must walk every maker to prove the kill (bounded work cannot
//!   help here; the case guards against a regression). A kill leaves the
//!   level unchanged.
//! * `fok_replenish@depth` — iceberg makers (1 visible + 1,000,000 hidden):
//!   a qty-2 FOK taker takes the front tranche (a replenishment re-sequenced
//!   at the tail) and one unit from the next maker; the depth never changes.

use criterion::{BatchSize, BenchmarkId, Criterion};
use pricelevel::{
    Hash32, Id, MatchResult, OrderType, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce,
    TimestampMs, UuidGenerator,
};
use std::cell::Cell;
use uuid::Uuid;

const PRICE: u128 = 100;
const DEPTHS: [u64; 3] = [1, 100, 10_000];
const ICEBERG_HIDDEN: u64 = 1_000_000;

fn standard(id: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        quantity: Quantity::new(1),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn iceberg(id: u64) -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::from_u64(id),
        price: Price::new(PRICE),
        visible_quantity: Quantity::new(1),
        hidden_quantity: Quantity::new(ICEBERG_HIDDEN),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn level_of(depth: u64, make: fn(u64) -> OrderType<()>) -> PriceLevel {
    let level = PriceLevel::new(PRICE);
    for id in 0..depth {
        level.add_order(make(id)).expect("seed add_order");
    }
    level
}

/// The timed call. It returns the whole `MatchResult`: `iter_batched`
/// collects routine outputs and drops them only after the measured batch,
/// so result destruction is never timed.
fn take(
    level: &PriceLevel,
    quantity: u64,
    tif: TimeInForce,
    generator: &UuidGenerator,
) -> MatchResult {
    level.match_order(
        quantity,
        Id::from_u64(u64::MAX),
        tif,
        TakerKind::Standard,
        TimestampMs::new(2),
        generator,
    )
}

/// Untimed input check: the level holds exactly `depth` resting orders
/// when the timed call starts.
fn assert_depth(level: &PriceLevel, depth: u64) {
    assert_eq!(
        level.order_count() as u64,
        depth,
        "the timed call must start at the stated depth"
    );
}

/// Register the fill-or-kill depth benchmarks.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - FOK depth");

    for depth in DEPTHS {
        for (name, tif) in [
            ("fok_first_maker", TimeInForce::Fok),
            ("gtc_first_maker", TimeInForce::Gtc),
        ] {
            group.bench_function(BenchmarkId::new(name, depth), |b| {
                // `depth - 1` seeded; the untimed setup admits one more, so
                // every timed call starts at exactly `depth` and consumes
                // the front maker, leaving `depth - 1` again.
                let level = level_of(depth - 1, standard);
                let generator = UuidGenerator::new(Uuid::nil());
                let next_id = Cell::new(depth - 1);
                b.iter_batched(
                    || {
                        level
                            .add_order(standard(next_id.get()))
                            .expect("replacement");
                        next_id.set(next_id.get() + 1);
                        assert_depth(&level, depth);
                    },
                    |()| take(&level, 1, tif, &generator),
                    BatchSize::PerIteration,
                )
            });
        }

        group.bench_function(BenchmarkId::new("fok_rejected", depth), |b| {
            let level = level_of(depth, standard);
            let generator = UuidGenerator::new(Uuid::nil());
            // A kill leaves the level untouched: no replacement needed.
            b.iter_batched(
                || assert_depth(&level, depth),
                |()| take(&level, depth + 1, TimeInForce::Fok, &generator),
                BatchSize::SmallInput,
            )
        });

        if depth >= 2 {
            group.bench_function(BenchmarkId::new("fok_replenish", depth), |b| {
                let level = level_of(depth, iceberg);
                let generator = UuidGenerator::new(Uuid::nil());
                // Each call consumes one visible unit from two makers; every
                // maker holds a million hidden units, so the level keeps its
                // depth for the whole measurement without replacements.
                b.iter_batched(
                    || assert_depth(&level, depth),
                    |()| take(&level, 2, TimeInForce::Fok, &generator),
                    BatchSize::SmallInput,
                )
            });
        }
    }
    group.finish();
}
