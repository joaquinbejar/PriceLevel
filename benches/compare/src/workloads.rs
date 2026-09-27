//! Workload fixtures and closures shared by `benches/compare.rs` (Criterion)
//! and `src/alloc_compare.rs` (allocation counting). Every function here
//! compiles against BOTH `--features old` and `--features new`; the two
//! builds run this exact source. See `shim.rs` for the small set of API
//! differences this module defers to, and `BENCHMARKS.md` at the repo root
//! for how these numbers are used.
//!
//! # Measurement boundaries
//!
//! Fixture construction (building orders, seeding a level to a target
//! depth, producing the JSON a restore parses) happens OUTSIDE every timed
//! closure below — Criterion's `iter_batched` / `iter_batched_ref` is used
//! wherever a workload needs fresh per-iteration state (an `add_order` that
//! would otherwise grow the level's depth every sample, a `match_order`
//! that consumes its target maker). Outcome validation (fill counts,
//! `MatchOutcome`, resulting depth) happens AFTER the timed closure, from
//! the operation's own return value, never inside it — the same discipline
//! `BENCH.md`'s latency harness documents for its own scenarios.

use crate::pl;
use crate::shim;
use pl::{
    Hash32, Id, OrderType, OrderUpdate, Price, PriceLevel, Quantity, Side, TakerKind, TimeInForce,
    TimestampMs, Trade, TradeList, UuidGenerator,
};
use std::num::NonZeroU64;
use std::str::FromStr;

/// Base price used by every fixture in this module, in price ticks.
pub const BASE_PRICE: u128 = 100_000;
/// Base timestamp (ms). Every maker's own timestamp is `BASE_TS + id`, and
/// every match/trade timestamp used to record it is `EXECUTION_TS`, chosen
/// with enough margin that no maker "arrives after" its own execution (see
/// `BENCH.md`'s "Execution timestamps" section for why that ordering
/// matters to this crate's statistics bookkeeping).
pub const BASE_TS: u64 = 1_700_000_000_000;
/// Execution / match timestamp with a two-billion-ms margin over every
/// constructed maker id, matching the latency harness's own convention.
pub const EXECUTION_TS_MS: u64 = BASE_TS + 2_000_000_000;

#[must_use]
pub fn ts(id: u64) -> TimestampMs {
    TimestampMs::new(BASE_TS + id)
}

#[must_use]
pub fn execution_ts() -> TimestampMs {
    TimestampMs::new(EXECUTION_TS_MS)
}

#[must_use]
pub fn zero_user() -> Hash32 {
    Hash32([0u8; 32])
}

// These builders return `OrderType<()>` BY VALUE, not `Arc<OrderType<()>>`:
// `OrderType<()>` is `Copy` (see `src/orders/order_type.rs`'s derive list),
// and `PriceLevel::add_order` takes it by value on both versions, so there
// is nothing to share here — wrapping in an `Arc` would only add an
// allocation callers immediately discard.
#[must_use]
pub fn standard_order(id: u64, price: u128, qty: u64, side: Side) -> OrderType<()> {
    OrderType::Standard {
        id: Id::sequential(id),
        price: Price::new(price),
        quantity: Quantity::new(qty),
        side,
        user_id: zero_user(),
        timestamp: ts(id),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

#[must_use]
pub fn post_only_order(id: u64, price: u128, qty: u64, side: Side) -> OrderType<()> {
    OrderType::PostOnly {
        id: Id::sequential(id),
        price: Price::new(price),
        quantity: Quantity::new(qty),
        side,
        user_id: zero_user(),
        timestamp: ts(id),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

#[must_use]
pub fn iceberg_order(id: u64, price: u128, visible: u64, hidden: u64, side: Side) -> OrderType<()> {
    OrderType::IcebergOrder {
        id: Id::sequential(id),
        price: Price::new(price),
        visible_quantity: Quantity::new(visible),
        hidden_quantity: Quantity::new(hidden),
        side,
        user_id: zero_user(),
        timestamp: ts(id),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

#[must_use]
pub fn reserve_order(
    id: u64,
    price: u128,
    visible: u64,
    hidden: u64,
    threshold: u64,
    side: Side,
) -> OrderType<()> {
    OrderType::ReserveOrder {
        id: Id::sequential(id),
        price: Price::new(price),
        visible_quantity: Quantity::new(visible),
        hidden_quantity: Quantity::new(hidden),
        side,
        user_id: zero_user(),
        timestamp: ts(id),
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: Quantity::new(threshold),
        replenish_amount: NonZeroU64::new(20),
        auto_replenish: true,
        extra_fields: (),
    }
}

#[must_use]
pub fn fresh_level() -> PriceLevel {
    PriceLevel::new(BASE_PRICE)
}

/// Seed `level` with `depth` one-quantity resting `Buy` makers at ids
/// `0..depth`, so the level's own FIFO sweep order matches ascending id
/// order. Panics (bench-only code) if an admission is rejected.
pub fn seed_depth(level: &PriceLevel, depth: u64) {
    for id in 0..depth {
        level
            .add_order(standard_order(id, BASE_PRICE, 1, Side::Buy))
            .expect("seed admission failed");
    }
}

/// Build a fresh `UuidGenerator` with a fixed namespace, identical on both
/// versions (`UuidGenerator::new` is unchanged between 0.9.2 and 0.10).
#[must_use]
pub fn trade_id_generator() -> UuidGenerator {
    // A fixed, arbitrary namespace UUID so both versions build byte-identical
    // generators run-to-run.
    UuidGenerator::new(uuid::Uuid::from_bytes([0xAB; 16]))
}

// ---------------------------------------------------------------------------
// Add-order batches
// ---------------------------------------------------------------------------

/// Add `count` standard orders to a fresh level. Returns the level so the
/// caller can assert `order_count()` afterward; construction of the level
/// and the orders happens inside the timed closure deliberately here (this
/// IS the "add N orders" workload), matching the task's "batches" framing.
pub fn add_standard_batch(count: u64) -> PriceLevel {
    let level = fresh_level();
    for id in 0..count {
        level
            .add_order(standard_order(id, BASE_PRICE, 10, Side::Buy))
            .expect("add_order failed");
    }
    level
}

pub fn add_iceberg_batch(count: u64) -> PriceLevel {
    let level = fresh_level();
    for id in 0..count {
        level
            .add_order(iceberg_order(id, BASE_PRICE, 10, 100, Side::Buy))
            .expect("add_order failed");
    }
    level
}

pub fn add_reserve_batch(count: u64) -> PriceLevel {
    let level = fresh_level();
    for id in 0..count {
        level
            .add_order(reserve_order(id, BASE_PRICE, 10, 100, 5, Side::Buy))
            .expect("add_order failed");
    }
    level
}

// ---------------------------------------------------------------------------
// Isolated update-path fixtures (level pre-seeded, one target order touched
// per iteration; teardown restores state so depth does not drift)
// ---------------------------------------------------------------------------

/// Fixture for `cancel`: a level with `depth` resting makers plus one extra
/// target order at id `depth` (the one the timed closure cancels).
pub fn cancel_fixture(depth: u64) -> PriceLevel {
    let level = fresh_level();
    seed_depth(&level, depth);
    level
        .add_order(standard_order(depth, BASE_PRICE, 10, Side::Buy))
        .expect("target admission failed");
    level
}

pub fn cancel_update(depth: u64) -> OrderUpdate {
    OrderUpdate::Cancel {
        order_id: Id::sequential(depth),
    }
}

/// Fixture for quantity update: same shape as `cancel_fixture`.
pub fn update_quantity_fixture(depth: u64) -> PriceLevel {
    cancel_fixture(depth)
}

pub fn increase_quantity_update(depth: u64) -> OrderUpdate {
    OrderUpdate::UpdateQuantity {
        order_id: Id::sequential(depth),
        new_quantity: Quantity::new(50),
    }
}

pub fn decrease_quantity_update(depth: u64) -> OrderUpdate {
    OrderUpdate::UpdateQuantity {
        order_id: Id::sequential(depth),
        new_quantity: Quantity::new(2),
    }
}

pub fn replace_same_price_update(depth: u64) -> OrderUpdate {
    OrderUpdate::Replace {
        order_id: Id::sequential(depth),
        price: Price::new(BASE_PRICE),
        quantity: Quantity::new(25),
        side: Side::Buy,
    }
}

pub fn replace_diff_price_update(depth: u64) -> OrderUpdate {
    OrderUpdate::Replace {
        order_id: Id::sequential(depth),
        price: Price::new(BASE_PRICE + 1),
        quantity: Quantity::new(25),
        side: Side::Buy,
    }
}

// ---------------------------------------------------------------------------
// Matching fixtures
// ---------------------------------------------------------------------------

/// One fresh dedicated maker (id `u64::MAX / 2`, kept out of the way of
/// `seed_depth`'s `0..depth` range) so a full-match scenario always has
/// exactly one resting order in flight, matching `BENCH.md`'s
/// "two full-match fixtures" convention.
const TARGET_ID: u64 = u64::MAX / 2;

pub fn match_standard_full_fixture() -> PriceLevel {
    let level = fresh_level();
    level
        .add_order(standard_order(TARGET_ID, BASE_PRICE, 100, Side::Buy))
        .expect("target admission failed");
    level
}

pub fn match_iceberg_fixture() -> PriceLevel {
    let level = fresh_level();
    level
        .add_order(iceberg_order(TARGET_ID, BASE_PRICE, 10, 100, Side::Buy))
        .expect("target admission failed");
    level
}

pub fn match_reserve_fixture() -> PriceLevel {
    let level = fresh_level();
    level
        .add_order(reserve_order(TARGET_ID, BASE_PRICE, 10, 100, 5, Side::Buy))
        .expect("target admission failed");
    level
}

/// Mixed-book fixture: standard, iceberg and reserve makers interleaved,
/// FIFO id order 0..depth. A large taker sweeps through all three types.
pub fn match_mixed_fixture(depth: u64) -> PriceLevel {
    let level = fresh_level();
    for id in 0..depth {
        let order = match id % 3 {
            0 => standard_order(id, BASE_PRICE, 5, Side::Buy),
            1 => iceberg_order(id, BASE_PRICE, 5, 50, Side::Buy),
            _ => reserve_order(id, BASE_PRICE, 5, 50, 3, Side::Buy),
        };
        level.add_order(order).expect("mixed admission failed");
    }
    level
}

/// 100-maker sweep fixture: 100 one-quantity makers; a qty-100 taker
/// consumes all of them in one `match_order` call.
pub fn match_sweep_fixture() -> PriceLevel {
    let level = fresh_level();
    seed_depth(&level, 100);
    level
}

/// A single large maker on a `depth`-deep level, used for the
/// partial-fill-reinsert and partial-fill-churn scenarios (`BENCH.md`'s
/// "match_maker_partial" convention): the front maker is huge, everything
/// behind it is filler depth never actually touched by a qty-10 taker.
pub fn match_maker_partial_fixture(depth: u64) -> PriceLevel {
    let level = fresh_level();
    level
        .add_order(standard_order(TARGET_ID, BASE_PRICE, 1_000_000, Side::Buy))
        .expect("target admission failed");
    for id in 0..depth {
        level
            .add_order(standard_order(id, BASE_PRICE, 1, Side::Buy))
            .expect("filler admission failed");
    }
    level
}

/// Shared fixture for both the FOK-success (`taker qty <= depth`) and
/// FOK-reject (`taker qty > depth`) scenarios: `depth` one-quantity makers.
/// The caller chooses the taker quantity passed to `run_match`.
/// Fixture for the taker-side "partial fill, reinsert remainder" scenario:
/// one small resting maker (qty 5) so a qty-10 GTC taker is only
/// `PartiallyFilled` (`remaining_quantity() == 5`, since `match_order` itself
/// never rests a taker's own remainder — see `src/lib.rs`'s
/// `MatchOutcome::PartiallyFilled` doc comment). The benchmark closure adds
/// the leftover back as a new resting order, mirroring what a composing
/// order book does with an unfilled `Gtc` remainder.
pub fn partial_fill_reinsert_fixture() -> PriceLevel {
    let level = fresh_level();
    level
        .add_order(standard_order(TARGET_ID, BASE_PRICE, 5, Side::Buy))
        .expect("target admission failed");
    level
}

pub fn fok_fixture(depth: u64) -> PriceLevel {
    let level = fresh_level();
    seed_depth(&level, depth);
    level
}

/// FOK-replenish fixture: a `depth`-deep book whose LAST maker is a
/// large-hidden iceberg so a big FOK taker's feasibility walk must draw
/// hidden replenishment to be satisfied.
pub fn fok_replenish_fixture(depth: u64) -> PriceLevel {
    let level = fresh_level();
    for id in 0..depth.saturating_sub(1) {
        level
            .add_order(standard_order(id, BASE_PRICE, 1, Side::Buy))
            .expect("filler admission failed");
    }
    level
        .add_order(iceberg_order(
            depth.saturating_sub(1),
            BASE_PRICE,
            1,
            10_000,
            Side::Buy,
        ))
        .expect("replenish maker admission failed");
    level
}

pub fn post_only_reject_fixture() -> PriceLevel {
    let level = fresh_level();
    level
        .add_order(standard_order(TARGET_ID, BASE_PRICE, 100, Side::Sell))
        .expect("target admission failed");
    level
}

// ---------------------------------------------------------------------------
// match_order call helpers
// ---------------------------------------------------------------------------

pub fn run_match(
    level: &PriceLevel,
    qty: u64,
    taker_id: Id,
    tif: TimeInForce,
    kind: TakerKind,
    id_gen: &UuidGenerator,
) -> pl::MatchResult {
    level.match_order(qty, taker_id, tif, kind, execution_ts(), id_gen)
}

const TAKER_ID: u64 = u64::MAX - 1;

// ---------------------------------------------------------------------------
// iter_orders
// ---------------------------------------------------------------------------

pub fn iter_orders_fixture(depth: u64) -> PriceLevel {
    let level = fresh_level();
    seed_depth(&level, depth);
    level
}

#[must_use]
pub fn count_iter_orders(level: &PriceLevel) -> usize {
    level.iter_orders().count()
}

// ---------------------------------------------------------------------------
// Snapshot / restore
// ---------------------------------------------------------------------------

#[must_use]
pub fn snapshot_capture(level: &PriceLevel) -> pl::PriceLevelSnapshot {
    shim::take_snapshot(level)
}

pub fn snapshot_package(level: &PriceLevel) -> pl::PriceLevelSnapshotPackage {
    level.snapshot_package().expect("snapshot_package failed")
}

pub fn snapshot_json(level: &PriceLevel) -> String {
    level.snapshot_to_json().expect("snapshot_to_json failed")
}

pub fn restore_from_json(json: &str) -> PriceLevel {
    PriceLevel::from_snapshot_json(json).expect("from_snapshot_json failed")
}

// ---------------------------------------------------------------------------
// MatchResult analytics
// ---------------------------------------------------------------------------

/// Build a `MatchResult` carrying exactly `n` trades of quantity 1 at
/// `BASE_PRICE`, pre-sized via the version's own capacity constructor
/// (`shim::match_result_with_capacity`) so the analytics benchmark measures
/// only `executed_quantity`/`executed_value`/`average_price`, not vector
/// growth.
pub fn match_result_with_trades(n: usize) -> pl::MatchResult {
    let mut result =
        shim::match_result_with_capacity(Id::sequential(TAKER_ID), Quantity::new(n as u64), n);
    for i in 0..n as u64 {
        let trade = Trade::with_timestamp(
            Id::sequential(i),
            Id::sequential(TAKER_ID),
            Id::sequential(i),
            Price::new(BASE_PRICE),
            Quantity::new(1),
            Side::Buy,
            execution_ts(),
        );
        result.add_trade(trade).expect("add_trade failed");
    }
    result
}

// ---------------------------------------------------------------------------
// TradeList text parse
// ---------------------------------------------------------------------------

/// Build the `Display` text of a `TradeList` with `n` trades, using the
/// version's own `Display` impl (identical shape on both sides) so the
/// parse benchmark below round-trips through the version's own formatter,
/// not a hand-rolled string.
pub fn trade_list_text(n: usize) -> String {
    let mut list = shim::trade_list_with_capacity(n);
    for i in 0..n as u64 {
        let trade = Trade::with_timestamp(
            Id::sequential(i),
            Id::sequential(TAKER_ID),
            Id::sequential(i),
            Price::new(BASE_PRICE),
            Quantity::new(1),
            Side::Buy,
            execution_ts(),
        );
        shim::trade_list_add(&mut list, trade);
    }
    list.to_string()
}

pub fn parse_trade_list(text: &str) -> TradeList {
    TradeList::from_str(text).expect("TradeList::from_str failed")
}

// ---------------------------------------------------------------------------
// Contention: one matcher thread + N-1 writer threads
// ---------------------------------------------------------------------------

/// Result of one contention run: matcher outcome counts plus every writer's
/// per-op latency samples (nanoseconds), pooled, for a simple sorted-sample
/// percentile (no hdrhistogram — not an approved dependency, matching
/// `BENCH.md`'s latency harness).
pub struct ContentionReport {
    pub matcher_ops: usize,
    pub matcher_elapsed: std::time::Duration,
    pub writer_samples_ns: Vec<u64>,
}

#[must_use]
pub fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// One matcher thread running a looping FOK (or GTC) taker against a
/// `depth`-deep level, contended by `writers` threads that each add-then-
/// cancel their own disjoint id range. `matcher_ops` matcher iterations are
/// measured; each writer runs until the matcher signals `stop`.
///
/// This is a simplified stress comparison, not a per-op outcome-validated
/// scenario like `BENCH.md`'s `contention_{gtc,fok}_matcher` (which tracks
/// and untimed-cancels each iteration's own target maker so matcher-owned
/// depth never drifts). Here the matcher injects one untimed-relative-to-
/// nothing replenishment order after each timed match so the book does not
/// run dry, but individual match outcomes are not asserted per iteration —
/// only the aggregate matcher throughput and writer latency distribution are
/// reported. Treat the resulting numbers as directional (0.9.2 vs 0.10 under
/// the same generated load), not as a strict per-call proof of FIFO
/// coverage the way `BENCH.md`'s own harness is.
pub fn run_contention(
    depth: u64,
    writers: usize,
    matcher_ops: usize,
    fok: bool,
) -> ContentionReport {
    use std::sync::Arc as StdArc;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, Ordering};

    let level = StdArc::new(fresh_level());
    seed_depth(&level, depth);

    let barrier = StdArc::new(Barrier::new(writers + 1));
    let stop = StdArc::new(AtomicBool::new(false));
    let writer_id_base = 1_000_000_000_u64;

    let mut handles = Vec::with_capacity(writers);
    let samples: StdArc<std::sync::Mutex<Vec<u64>>> =
        StdArc::new(std::sync::Mutex::new(Vec::new()));

    for w in 0..writers {
        let level = StdArc::clone(&level);
        let barrier = StdArc::clone(&barrier);
        let stop = StdArc::clone(&stop);
        let samples = StdArc::clone(&samples);
        let base = writer_id_base + (w as u64) * 1_000_000;
        handles.push(std::thread::spawn(move || {
            let mut local: Vec<u64> = Vec::new();
            let mut i: u64 = 0;
            barrier.wait();
            while !stop.load(Ordering::Relaxed) {
                let id = Id::sequential(base + i % 100_000);
                let t0 = std::time::Instant::now();
                // Same side as the seeded book (`Side::Buy`): a level enforces
                // single-side coherence (see `src/lib.rs`'s "level topology
                // invariants" migration guide), and these churn orders are
                // meant to add queue pressure without being eligible for the
                // matcher's own opposite-side taker to consume ahead of the
                // original seeded makers (insertion sequence, not order id
                // magnitude, decides FIFO order).
                let added = level
                    .add_order(standard_order(base + i % 100_000, BASE_PRICE, 1, Side::Buy))
                    .is_ok();
                if added {
                    let _ = level.update_order(OrderUpdate::Cancel { order_id: id });
                }
                local.push(t0.elapsed().as_nanos() as u64);
                i = i.wrapping_add(1);
            }
            let mut guard = samples.lock().expect("samples mutex poisoned");
            guard.extend(local);
        }));
    }

    let id_gen = trade_id_generator();
    barrier.wait();
    let t0 = std::time::Instant::now();
    let mut ops = 0usize;
    while ops < matcher_ops {
        let tif = if fok {
            TimeInForce::Fok
        } else {
            TimeInForce::Gtc
        };
        let _ = run_match(
            &level,
            1,
            Id::sequential(TAKER_ID),
            tif,
            TakerKind::Standard,
            &id_gen,
        );
        // Keep the book from running dry under a GTC matcher: replenish one
        // resting maker per op (untimed relative to the matcher's own
        // wall-clock window is not attempted here — this loop IS the timed
        // window, matching BENCH.md's contention scenario, which also folds
        // its own re-seed into the timed loop).
        let _ = level.add_order(standard_order(
            2_000_000_000 + ops as u64,
            BASE_PRICE,
            1,
            Side::Buy,
        ));
        ops += 1;
    }
    let elapsed = t0.elapsed();
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    let mut writer_samples_ns = samples.lock().expect("samples mutex poisoned").clone();
    writer_samples_ns.sort_unstable();

    ContentionReport {
        matcher_ops: ops,
        matcher_elapsed: elapsed,
        writer_samples_ns,
    }
}
