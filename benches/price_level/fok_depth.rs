//! Fill-or-kill feasibility cost versus resting depth (issue #143).
//!
//! Every case keeps one long-lived level at a fixed resting depth. The timed
//! routine is one `match_order` call; the untimed per-iteration setup admits
//! one replacement maker so the depth stays constant (`BatchSize::PerIteration`
//! runs exactly one setup per routine, so the depth never drifts by a batch).
//!
//! * `fok_first_maker@depth` — a qty-1 FOK taker filled by the front qty-1
//!   standard maker. The dry run only needs the front maker.
//! * `gtc_first_maker@depth` — the same taker with GTC: the control.
//! * `fok_rejected@depth` — a FOK taker one unit larger than the level: the
//!   dry run must walk every maker to prove the kill (bounded work cannot
//!   help here; the case guards against a regression).
//! * `fok_replenish@depth` — iceberg makers (1 visible + 1,000,000 hidden):
//!   a qty-2 FOK taker takes the front tranche (a replenishment re-sequenced
//!   at the tail) and one unit from the next maker.

use criterion::{BatchSize, BenchmarkId, Criterion};
use pricelevel::{
    Hash32, Id, MatchOutcome, OrderType, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce,
    TimestampMs, UuidGenerator,
};
use std::cell::Cell;
use std::hint::black_box;
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

fn take(
    level: &PriceLevel,
    quantity: u64,
    tif: TimeInForce,
    generator: &UuidGenerator,
) -> MatchOutcome {
    black_box(level.match_order(
        quantity,
        Id::from_u64(u64::MAX),
        tif,
        TakerKind::Standard,
        TimestampMs::new(2),
        generator,
    ))
    .outcome()
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
                let level = level_of(depth, standard);
                let generator = UuidGenerator::new(Uuid::nil());
                let next_id = Cell::new(depth);
                b.iter_batched(
                    || {
                        // Replacement maker, untimed: depth stays `depth`
                        // after the timed call consumes the front.
                        level
                            .add_order(standard(next_id.get()))
                            .expect("replacement");
                        next_id.set(next_id.get() + 1);
                    },
                    |()| {
                        let outcome = take(&level, 1, tif, &generator);
                        debug_assert_eq!(outcome, MatchOutcome::Filled);
                        outcome
                    },
                    BatchSize::PerIteration,
                )
            });
        }

        group.bench_function(BenchmarkId::new("fok_rejected", depth), |b| {
            let level = level_of(depth, standard);
            let generator = UuidGenerator::new(Uuid::nil());
            // A kill leaves the level untouched: no replacement needed.
            b.iter(|| take(&level, depth + 1, TimeInForce::Fok, &generator))
        });

        if depth >= 2 {
            group.bench_function(BenchmarkId::new("fok_replenish", depth), |b| {
                let level = level_of(depth, iceberg);
                let generator = UuidGenerator::new(Uuid::nil());
                // Each call consumes one visible unit from two makers; every
                // maker holds a million hidden units, so the level keeps its
                // depth for the whole measurement without replacements.
                b.iter(|| take(&level, 2, TimeInForce::Fok, &generator))
            });
        }
    }
    group.finish();
}
