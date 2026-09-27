// benches/latency/scenarios/matching.rs
//! Isolated `match_order` outcome scenarios: empty book, full fill, partial
//! fill, a many-fill sweep in one call, and iceberg / reserve replenishment.
//! All takers use `TimeInForce::Gtc` and `TakerKind::Standard`; TIF-specific
//! and post-only behavior is covered separately in `tif.rs`.
//!
//! Every scenario that expects trades to occur uses
//! [`fixtures::EXECUTION_TIMESTAMP_MS`] as the match's execution timestamp
//! and asserts [`fixtures::assert_stats_healthy`] after its measured loop
//! (issue #142 review finding 1) — see that constant's docs for why an
//! earlier version of this harness silently measured the degraded/error
//! path instead.

use crate::alloc_measurements::Retention;
use crate::config::Config;
use crate::fixtures::{self, EXECUTION_TIMESTAMP_MS, LEVEL_PRICE, TAKER_ID_BASE};
use crate::report::ScenarioReport;
use crate::timing::{measure, measure_with_setup, warmup};
use pricelevel::prelude::*;

/// Runs every match-outcome scenario and returns one report each.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    vec![
        match_empty(config),
        match_full(config),
        match_partial(config),
        match_maker_partial(config),
        many_fill_sweep(config),
        iceberg_replenish(config),
        reserve_replenish(config),
        single_maker_partial(config, Retention::None),
        single_maker_partial(config, Retention::Admission),
        single_maker_partial(config, Retention::View),
    ]
}

/// `match_order` against a level with no resting orders at all
/// (`MatchOutcome::NotFilled`). No trades are ever emitted, so there is
/// nothing for `PriceLevelStatistics` to record.
fn match_empty(config: &Config) -> ScenarioReport {
    let level = PriceLevel::new(LEVEL_PRICE);
    let generator = fixtures::trade_id_generator();

    warmup(config.warmup, |i| {
        level.match_order(
            10,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        level.match_order(
            10,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let not_filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::NotFilled)
        .count();
    assert_eq!(
        not_filled, config.samples,
        "match_empty: every call against an empty level must be NotFilled"
    );
    fixtures::assert_stats_healthy(&level, 0, "match_empty");

    ScenarioReport::from_samples(
        "match_empty",
        "match",
        0,
        "PriceLevel::match_order — empty level",
        durations_ns,
        format!("{not_filled}/{} NotFilled", config.samples),
    )
}

/// `match_order` where each call fully consumes exactly one fresh maker of
/// the same quantity as the taker (`MatchOutcome::Filled`). Exactly one
/// resting maker exists at any instant (added just before the timed call,
/// fully consumed by it), so the reported depth is `1`, not the cumulative
/// warmup + sample count (issue #142 review finding 2).
fn match_full(config: &Config) -> ScenarioReport {
    const QTY: u64 = 10;
    let level = PriceLevel::new(LEVEL_PRICE);
    let generator = fixtures::trade_id_generator();

    let seed = |i: usize| {
        level
            .add_order(fixtures::standard_order(
                i as u64,
                Side::Sell,
                QTY,
                TimeInForce::Gtc,
            ))
            .expect("match_full: seeding a fresh maker id must succeed");
    };
    for i in 0..config.warmup {
        seed(i);
    }
    warmup(config.warmup, |i| {
        level.match_order(
            QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    // Reset so the assertion below covers only the measured loop, not the
    // warmup fills that also executed against this same level.
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure_with_setup(
        config.samples,
        |i| seed(config.warmup + i),
        |i| {
            level.match_order(
                QTY,
                Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
                TimeInForce::Gtc,
                TakerKind::Standard,
                TimestampMs::new(EXECUTION_TIMESTAMP_MS),
                &generator,
            )
        },
    );

    let filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::Filled)
        .count();
    assert_eq!(
        filled, config.samples,
        "match_full: every call must fully fill against its dedicated fresh maker"
    );
    assert_eq!(
        level.order_count(),
        0,
        "match_full: every seeded maker must have been fully consumed"
    );
    fixtures::assert_stats_healthy(&level, config.samples as u64 * QTY, "match_full");

    ScenarioReport::from_samples(
        "match_full",
        "match",
        1,
        "PriceLevel::match_order — full fill of one dedicated maker",
        durations_ns,
        format!("{filled}/{} Filled", config.samples),
    )
}

/// `match_order` where each call fully consumes a small fresh maker but the
/// taker's own quantity is larger, so the taker's remainder is unfilled
/// (`MatchOutcome::PartiallyFilled`). Same single-maker-in-flight reasoning
/// as [`match_full`] — reported depth is `1`.
fn match_partial(config: &Config) -> ScenarioReport {
    const MAKER_QTY: u64 = 5;
    const TAKER_QTY: u64 = 10;
    let level = PriceLevel::new(LEVEL_PRICE);
    let generator = fixtures::trade_id_generator();

    let seed = |i: usize| {
        level
            .add_order(fixtures::standard_order(
                i as u64,
                Side::Sell,
                MAKER_QTY,
                TimeInForce::Gtc,
            ))
            .expect("match_partial: seeding a fresh maker id must succeed");
    };
    for i in 0..config.warmup {
        seed(i);
        level.match_order(
            TAKER_QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        );
    }
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure_with_setup(
        config.samples,
        |i| seed(config.warmup + i),
        |i| {
            level.match_order(
                TAKER_QTY,
                Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
                TimeInForce::Gtc,
                TakerKind::Standard,
                TimestampMs::new(EXECUTION_TIMESTAMP_MS),
                &generator,
            )
        },
    );

    let partial = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::PartiallyFilled)
        .count();
    assert_eq!(
        partial, config.samples,
        "match_partial: every call must partially fill (maker smaller than taker)"
    );
    assert_eq!(
        level.order_count(),
        0,
        "match_partial: every seeded maker must have been fully consumed by the larger taker"
    );
    fixtures::assert_stats_healthy(&level, config.samples as u64 * MAKER_QTY, "match_partial");

    ScenarioReport::from_samples(
        "match_partial",
        "match",
        1,
        "PriceLevel::match_order — taker larger than one dedicated maker",
        durations_ns,
        format!("{partial}/{} PartiallyFilled", config.samples),
    )
}

/// `match_order` where each call partially fills the same huge front maker of
/// a 1,000-deep level (issue #148): one trade per call, no maker fully
/// consumed, so the result records no filled order id. The level's depth
/// never changes, so the reported depth is exact for every sample.
fn match_maker_partial(config: &Config) -> ScenarioReport {
    const DEPTH: u64 = 1_000;
    const TAKER_QTY: u64 = 10;
    let level = fixtures::deep_level_with_large_front(DEPTH);
    let generator = fixtures::trade_id_generator();

    warmup(config.warmup, |i| {
        level.match_order(
            TAKER_QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure(config.samples, |i| {
        level.match_order(
            TAKER_QTY,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let exact = results
        .iter()
        .filter(|r| {
            r.outcome() == MatchOutcome::Filled
                && r.trades().len() == 1
                && r.filled_order_ids().is_empty()
        })
        .count();
    assert_eq!(
        exact, config.samples,
        "match_maker_partial: every call must fill with one trade and no filled maker"
    );
    assert_eq!(
        level.order_count(),
        DEPTH as usize,
        "match_maker_partial: the front maker must never be fully consumed"
    );
    fixtures::assert_stats_healthy(
        &level,
        config.samples as u64 * TAKER_QTY,
        "match_maker_partial",
    );

    ScenarioReport::from_samples(
        "match_maker_partial",
        "match",
        DEPTH,
        "PriceLevel::match_order — partial fill of one large front maker, deep level",
        durations_ns,
        format!("{exact}/{} Filled (1 trade, 0 filled ids)", config.samples),
    )
}

/// One `match_order` call per sample, each consuming several resting makers
/// in a single sweep (`FILLS_PER_CALL` trades per call) — the "many-fill
/// sweep" workload the issue asks for. Sample count is capped independently
/// of `config.samples` (see `SWEEP_SAMPLES_CAP`) because its fixture is
/// `SWEEP_SAMPLES_CAP * FILLS_PER_CALL` resting orders, built entirely
/// up-front outside timing.
fn many_fill_sweep(config: &Config) -> ScenarioReport {
    const FILLS_PER_CALL: u64 = 20;
    const MAKER_QTY: u64 = 1;
    /// Bounds the fixture (and its build time / memory) for this scenario
    /// independently of `config.samples`; a full-size run still exercises
    /// thousands of many-fill sweeps.
    const SWEEP_SAMPLES_CAP: usize = 2_000;

    let samples = config.samples.min(SWEEP_SAMPLES_CAP);
    let warmup_count = config.warmup.clamp(1, SWEEP_SAMPLES_CAP / 4);
    let total_calls = warmup_count + samples;
    let total_makers = total_calls as u64 * FILLS_PER_CALL;

    let level = PriceLevel::new(LEVEL_PRICE);
    for i in 0..total_makers {
        level
            .add_order(fixtures::standard_order(
                i,
                Side::Sell,
                MAKER_QTY,
                TimeInForce::Gtc,
            ))
            .expect("many_fill_sweep: seeding a fresh maker id must succeed");
    }
    let generator = fixtures::trade_id_generator();

    warmup(warmup_count, |i| {
        level.match_order(
            FILLS_PER_CALL,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure(samples, |i| {
        level.match_order(
            FILLS_PER_CALL,
            Id::from_u64(TAKER_ID_BASE + warmup_count as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::Filled)
        .count();
    let total_trades: usize = results.iter().map(|r| r.trades().len()).sum();
    assert_eq!(
        filled, samples,
        "many_fill_sweep: every call must fully fill by sweeping FILLS_PER_CALL 1-qty makers"
    );
    assert_eq!(
        total_trades as u64,
        samples as u64 * FILLS_PER_CALL,
        "many_fill_sweep: total trade count must equal samples * FILLS_PER_CALL exactly"
    );
    fixtures::assert_stats_healthy(
        &level,
        samples as u64 * FILLS_PER_CALL * MAKER_QTY,
        "many_fill_sweep",
    );

    ScenarioReport::from_samples(
        "many_fill_sweep",
        "match",
        total_makers,
        "PriceLevel::match_order — one call sweeps 20 makers",
        durations_ns,
        format!("{filled}/{samples} Filled, {total_trades} total trades"),
    )
}

/// One `match_order` call per sample against a single iceberg maker whose
/// visible tranche is fully consumed each time, forcing a replenish from its
/// hidden tranche before the next sample.
fn iceberg_replenish(config: &Config) -> ScenarioReport {
    const VISIBLE: u64 = 10;
    let total = (config.warmup + config.samples) as u64;
    // Hidden tranche sized so it never runs dry across every warmup and
    // measured call.
    let hidden = total * VISIBLE;
    let level = fixtures::seeded_iceberg_level(1, Side::Sell, VISIBLE, hidden);
    let generator = fixtures::trade_id_generator();

    warmup(config.warmup, |i| {
        level.match_order(
            VISIBLE,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure(config.samples, |i| {
        level.match_order(
            VISIBLE,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::Filled)
        .count();
    assert_eq!(
        filled, config.samples,
        "iceberg_replenish: every call must fully consume the visible tranche and trigger a replenish"
    );
    fixtures::assert_stats_healthy(&level, config.samples as u64 * VISIBLE, "iceberg_replenish");

    ScenarioReport::from_samples(
        "iceberg_replenish",
        "match",
        1,
        "PriceLevel::match_order — consumes iceberg visible tranche, triggers replenish",
        durations_ns,
        format!("{filled}/{} Filled", config.samples),
    )
}

/// Same shape as [`iceberg_replenish`] but against a `ReserveOrder`, whose
/// replenishment policy (`replenish_threshold` / `replenish_amount`) is
/// distinct code from the iceberg path.
fn reserve_replenish(config: &Config) -> ScenarioReport {
    const VISIBLE: u64 = 10;
    const REPLENISH_THRESHOLD: u64 = 1;
    const REPLENISH_AMOUNT: u64 = 10;
    let total = (config.warmup + config.samples) as u64;
    let hidden = total * VISIBLE;
    let level = fixtures::seeded_reserve_level(
        1,
        Side::Sell,
        VISIBLE,
        hidden,
        REPLENISH_THRESHOLD,
        REPLENISH_AMOUNT,
    );
    let generator = fixtures::trade_id_generator();

    warmup(config.warmup, |i| {
        level.match_order(
            VISIBLE,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure(config.samples, |i| {
        level.match_order(
            VISIBLE,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let filled = results
        .iter()
        .filter(|r| r.outcome() == MatchOutcome::Filled)
        .count();
    assert_eq!(
        filled, config.samples,
        "reserve_replenish: every call must fully consume the visible tranche and trigger a replenish"
    );
    fixtures::assert_stats_healthy(&level, config.samples as u64 * VISIBLE, "reserve_replenish");

    ScenarioReport::from_samples(
        "reserve_replenish",
        "match",
        1,
        "PriceLevel::match_order — consumes reserve visible tranche, triggers replenish",
        durations_ns,
        format!("{filled}/{} Filled", config.samples),
    )
}

/// Repeated 10-unit fills of one huge standard maker resting alone on the
/// level (issue #147), with and without an externally retained `Arc` of the
/// maker (see [`Retention`]). For `Retention::View` the untimed setup takes a
/// fresh `iter_orders().next()` handle before every timed call and drops the
/// previous one, so every fill sees a shared maker; the held handle's drop
/// is never timed.
fn single_maker_partial(config: &Config, retention: Retention) -> ScenarioReport {
    const TAKER_QTY: u64 = 10;
    let name = match retention {
        Retention::None => "single_partial",
        Retention::Admission => "single_partial_adm",
        Retention::View => "single_partial_view",
    };
    let level = PriceLevel::new(LEVEL_PRICE);
    let admission = level
        .add_order(fixtures::standard_order(
            0,
            Side::Sell,
            1_000_000_000_000,
            TimeInForce::Gtc,
        ))
        .expect("single_maker_partial: the large maker must be admitted");
    let admission = (retention == Retention::Admission).then_some(admission);
    let generator = fixtures::trade_id_generator();
    let mut view: Option<std::sync::Arc<OrderType<()>>> = None;

    warmup(config.warmup, |i| {
        view = (retention == Retention::View)
            .then(|| level.iter_orders().next())
            .flatten();
        level.match_order(
            TAKER_QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    level
        .stats()
        .reset_at(TimestampMs::new(0))
        .expect("fresh statistics sequence has headroom");

    let (durations_ns, results) = measure_with_setup(
        config.samples,
        |_| {
            view = (retention == Retention::View)
                .then(|| level.iter_orders().next())
                .flatten();
        },
        |i| {
            level.match_order(
                TAKER_QTY,
                Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
                TimeInForce::Gtc,
                TakerKind::Standard,
                TimestampMs::new(EXECUTION_TIMESTAMP_MS),
                &generator,
            )
        },
    );
    drop(view);
    drop(admission);

    let exact = results
        .iter()
        .filter(|r| {
            r.outcome() == MatchOutcome::Filled
                && r.trades().len() == 1
                && r.filled_order_ids().is_empty()
        })
        .count();
    assert_eq!(
        exact, config.samples,
        "{name}: every call must fill with one trade and no filled maker"
    );
    assert_eq!(
        level.order_count(),
        1,
        "{name}: the maker must never be fully consumed"
    );
    fixtures::assert_stats_healthy(&level, config.samples as u64 * TAKER_QTY, name);

    ScenarioReport::from_samples(
        name,
        "match",
        1,
        "PriceLevel::match_order — repeated partial fill of one large standard maker (#147)",
        durations_ns,
        format!("{exact}/{} Filled (1 trade, 0 filled ids)", config.samples),
    )
}
