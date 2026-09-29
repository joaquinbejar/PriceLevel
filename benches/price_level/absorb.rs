//! Folding one level's `MatchResult` into a multi-level aggregate
//! (issue #219): `MatchResult::try_absorb` against the pre-#219
//! reserve-and-copy pattern (`try_reserve_trades` /
//! `try_reserve_filled_order_ids`, then `add_trade` / `add_filled_order_id`
//! per entry).
//!
//! One fold per iteration, through [`time_chunked`]: the aggregate and the
//! level result are built before the clock starts and dropped after it
//! stops, so only the fold is timed. Each level result holds [`TRADES`]
//! trades, every maker consumed (one filled id per trade). The cases pick
//! the buffer shapes that select each `try_absorb` plan:
//!
//! * `empty`: a fresh aggregate (`try_absorb` swaps the buffers in);
//! * `reserved`: an aggregate holding one level with room reserved for the
//!   next (`try_absorb` appends into the reservation);
//! * `rotate`: a full aggregate and a level whose buffer has room for both
//!   (`try_absorb` adopts the level's buffer and rotates its own entries to
//!   the front);
//! * `growth`: a full aggregate and a full level (both patterns grow).
//!
//! "Full" means the trade buffer's length equals its observed capacity:
//! `MatchResult::try_with_capacity` guarantees only a lower bound, so the
//! fixtures add trades until the buffer is full instead of trusting the
//! request. The filled-id buffer's capacity is not public; it is requested
//! at the same size and gets one id per trade.

use super::isolated_ops::time_chunked;
use criterion::Criterion;
use pricelevel::{Id, MatchResult, Price, Quantity, Side, TimestampMs, Trade};

/// Trades (and filled ids) per level result.
const TRADES: u64 = 8;
/// Quantity each level executes, split across its trades.
const LEVEL_QTY: u64 = 8_000;
const TAKER: u64 = 1_000_000_000;
const PRICE: u128 = 10_000;
const EXEC_TS: u64 = 1_800_000_000_000;

/// A result for `incoming` whose first level (makers from `first_maker`)
/// executes [`LEVEL_QTY`]. With `full`, the trade count is the trade
/// buffer's observed capacity (requested [`TRADES`]); otherwise it is
/// [`TRADES`] in a buffer requested at `capacity`.
fn level(incoming: u64, first_maker: u64, capacity: usize, full: bool) -> MatchResult {
    let taker = Id::from_u64(TAKER);
    let mut result = MatchResult::try_with_capacity(taker, Quantity::new(incoming), capacity)
        .expect("level capacity");
    let trades = if full {
        result.trades().capacity() as u64
    } else {
        TRADES
    };
    assert!((1..=LEVEL_QTY).contains(&trades), "level trade count");
    let fill = LEVEL_QTY / trades;
    for step in 0..trades {
        let maker = first_maker + step;
        let quantity = if step + 1 == trades {
            LEVEL_QTY - fill * (trades - 1)
        } else {
            fill
        };
        result
            .add_trade(Trade::with_timestamp(
                Id::from_u64(TAKER + 1 + maker),
                taker,
                Id::from_u64(maker),
                Price::new(PRICE),
                Quantity::new(quantity),
                Side::Buy,
                TimestampMs::new(EXEC_TS),
            ))
            .expect("level trade");
        result
            .add_filled_order_id(Id::from_u64(maker))
            .expect("level filled id");
    }
    assert!(
        !full || result.trades().len() == result.trades().capacity(),
        "a full level's trade buffer must be full"
    );
    result
}

/// The pre-#219 fold: reserve exactly the level's entry counts, then copy.
fn copy(aggregate: &mut MatchResult, level: &MatchResult) {
    aggregate
        .try_reserve_trades(level.trades().len())
        .expect("reserve trades");
    aggregate
        .try_reserve_filled_order_ids(level.filled_order_ids().len())
        .expect("reserve filled ids");
    for &trade in level.trades().as_vec() {
        aggregate.add_trade(trade).expect("add_trade");
    }
    for &id in level.filled_order_ids() {
        aggregate
            .add_filled_order_id(id)
            .expect("add_filled_order_id");
    }
}

/// The fixture for `case`: an aggregate and the next level's result.
fn fixture(case: &str) -> (MatchResult, MatchResult) {
    let per_level = TRADES as usize;
    // The next level's result, matched with LEVEL_QTY left.
    let next = |capacity, full| level(LEVEL_QTY, 1_000, capacity, full);
    match case {
        "empty" => (
            MatchResult::new(Id::from_u64(TAKER), Quantity::new(LEVEL_QTY)),
            next(per_level, false),
        ),
        "reserved" => {
            let mut aggregate = level(2 * LEVEL_QTY, 0, per_level, false);
            aggregate.try_reserve(per_level).expect("reservation");
            (aggregate, next(per_level, false))
        }
        "rotate" => {
            let aggregate = level(2 * LEVEL_QTY, 0, per_level, true);
            let room = 2 * aggregate.trades().len();
            (aggregate, next(room, false))
        }
        "growth" => (
            level(2 * LEVEL_QTY, 0, per_level, true),
            next(per_level, true),
        ),
        _ => unreachable!("unknown absorb case {case}"),
    }
}

/// Register the aggregation benchmarks.
pub fn register_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("PriceLevel - Absorb");
    for case in ["empty", "reserved", "rotate", "growth"] {
        group.bench_function(format!("{case}/copy"), |b| {
            b.iter_custom(|iters| {
                time_chunked(
                    iters,
                    || fixture(case),
                    |(aggregate, level)| copy(aggregate, level),
                )
            });
        });
        group.bench_function(format!("{case}/absorb"), |b| {
            b.iter_custom(|iters| {
                time_chunked(
                    iters,
                    || fixture(case),
                    |(aggregate, level)| aggregate.try_absorb(level).expect("try_absorb"),
                )
            });
        });
    }
    group.finish();
}
