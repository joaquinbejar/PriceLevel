//! Isolated per-operation benchmarks (issue #214).
//!
//! Every other `PriceLevel - Match Orders` / `Update Orders` case builds its
//! level INSIDE `b.iter`, so each sample also times the seeding admissions
//! and the level's teardown. The cases here time exactly ONE public call:
//!
//! * the seeded level is built in the setup closure of
//!   [`criterion::Bencher::iter_batched_ref`] (untimed);
//! * the routine receives `&mut (level, ...)` and performs one operation;
//! * Criterion drops both the routine's output and the input level after
//!   the timed region closes, so teardown is untimed too.
//!
//! `BatchSize::PerIteration`: each timed call runs on a level whose setup
//! has just finished on the same thread (cache-warm, like a hot book), and
//! at most one seeded level is alive at a time. The price is one
//! `Instant::now()` pair per iteration inside the timed region, identical on
//! both sides of any comparison. The seed depth is fixed at [`SEED_DEPTH`]
//! standard (or iceberg) makers of quantity 10 at price 10000, side `Buy`.
//!
//! Only public API that is identical between the pre-hardening baseline
//! (`a5a94fc`) and later trees is used, so this file measures both with the
//! same code.

use criterion::{BatchSize, Criterion};
use pricelevel::{
    Hash32, Id, OrderType, OrderUpdate, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce,
    TimestampMs, UuidGenerator,
};
use std::hint::black_box;
use uuid::Uuid;

/// Resting makers seeded into every level before the timed call.
const SEED_DEPTH: u64 = 32;
/// Level price used by every case.
const LEVEL_PRICE: u128 = 10_000;
/// Quantity of every seeded maker (visible tranche for icebergs).
const MAKER_QTY: u64 = 10;
/// Id of the maker cancelled / updated / moved: the middle of the queue.
const TARGET_ID: u64 = SEED_DEPTH / 2;

fn standard(id: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(LEVEL_PRICE),
        quantity: Quantity::new(MAKER_QTY),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_616_823_000_000 + id),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn iceberg(id: u64) -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::from_u64(id),
        price: Price::new(LEVEL_PRICE),
        visible_quantity: Quantity::new(MAKER_QTY),
        hidden_quantity: Quantity::new(3 * MAKER_QTY),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_616_823_000_000 + id),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

fn seeded(make: fn(u64) -> OrderType<()>) -> PriceLevel {
    let level = PriceLevel::new(LEVEL_PRICE);
    for id in 0..SEED_DEPTH {
        level.add_order(make(id)).expect("seed admission");
    }
    level
}

/// Register the isolated per-operation benchmarks.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Isolated Ops");

    // One admission into a seeded level (the next id after the seed).
    group.bench_function("add_order_seeded/standard", |b| {
        b.iter_batched_ref(
            || (seeded(standard), Some(standard(SEED_DEPTH))),
            |(level, order)| level.add_order(order.take().expect("one order per input")),
            BatchSize::PerIteration,
        );
    });
    group.bench_function("add_order_seeded/iceberg", |b| {
        b.iter_batched_ref(
            || (seeded(iceberg), Some(iceberg(SEED_DEPTH))),
            |(level, order)| level.add_order(order.take().expect("one order per input")),
            BatchSize::PerIteration,
        );
    });

    // One match that fully consumes the front maker (one trade, one removal).
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("uuid");
    let generator = UuidGenerator::new(namespace);
    group.bench_function("match_order_seeded/full_front_maker", |b| {
        b.iter_batched_ref(
            || seeded(standard),
            |level| {
                level.match_order(
                    MAKER_QTY,
                    Id::from_u64(1_000_000),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    TimestampMs::new(1_716_000_000_000),
                    black_box(&generator),
                )
            },
            BatchSize::PerIteration,
        );
    });

    // One cancel of the middle maker.
    group.bench_function("cancel_seeded/standard", |b| {
        b.iter_batched_ref(
            || seeded(standard),
            |level| {
                level.update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(TARGET_ID),
                })
            },
            BatchSize::PerIteration,
        );
    });
    group.bench_function("cancel_seeded/iceberg", |b| {
        b.iter_batched_ref(
            || seeded(iceberg),
            |level| {
                level.update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(TARGET_ID),
                })
            },
            BatchSize::PerIteration,
        );
    });

    // One in-place quantity decrease of the middle maker (keeps priority).
    group.bench_function("update_quantity_seeded/decrease", |b| {
        b.iter_batched_ref(
            || seeded(standard),
            |level| {
                level.update_order(OrderUpdate::UpdateQuantity {
                    order_id: Id::from_u64(TARGET_ID),
                    new_quantity: Quantity::new(MAKER_QTY / 2),
                })
            },
            BatchSize::PerIteration,
        );
    });

    // One price-moving replace of the middle maker (removal from this level).
    group.bench_function("replace_seeded/different_price", |b| {
        b.iter_batched_ref(
            || seeded(standard),
            |level| {
                level.update_order(OrderUpdate::Replace {
                    order_id: Id::from_u64(TARGET_ID),
                    price: Price::new(LEVEL_PRICE + 100),
                    quantity: Quantity::new(MAKER_QTY),
                    side: Side::Buy,
                })
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}
