// benches/latency/scenarios/tif.rs
//! Time-in-force and taker-kind coverage: GTC / IOC / DAY / GTD full
//! matches, FOK success and rejection, and a post-only rejection.
//!
//! `PriceLevel::match_order` does not itself rest an unfilled taker — the
//! doc comment on [`pricelevel::MatchOutcome::PartiallyFilled`] is explicit
//! that resting a GTC/GTD/DAY remainder, or discarding an IOC remainder, is
//! the CALLER's (order book's) job. The level also does **not** enforce a
//! resting maker's own GTD/DAY expiry (see `doc/architecture.md` and the
//! issue's reference to the TIF contract) — a maker admitted with an expired
//! `Gtd` timestamp still matches normally here. Consequently GTC, IOC, DAY
//! and a non-expired GTD taker exercise the *identical* code path inside
//! `match_order`: only the taker's `TimeInForce` enum discriminant differs.
//! This module measures that directly rather than assuming a difference; see
//! `BENCH.md` for the resulting (expected-to-be-flat) comparison.

use crate::config::Config;
use crate::fixtures::{self, EXECUTION_TIMESTAMP_MS, LEVEL_PRICE, TAKER_ID_BASE};
use crate::report::ScenarioReport;
use crate::timing::{measure, measure_with_setup, warmup};
use pricelevel::prelude::*;

/// Arbitrary far-future GTD expiry (milliseconds since epoch). Never
/// enforced by the level itself — see the module docs — but recorded to
/// document that this is not testing an already-expired order.
const GTD_EXPIRY_MS: u64 = 9_999_999_999_999;

/// Runs every TIF / taker-kind scenario and returns one report each.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    vec![
        full_match_with_tif(config, TimeInForce::Gtc, "tif_gtc_full_match"),
        full_match_with_tif(config, TimeInForce::Ioc, "tif_ioc_full_match"),
        full_match_with_tif(config, TimeInForce::Day, "tif_day_full_match"),
        full_match_with_tif(
            config,
            TimeInForce::Gtd(GTD_EXPIRY_MS),
            "tif_gtd_full_match",
        ),
        fok_success(config),
        fok_reject(config),
        post_only_reject(config),
    ]
}

/// One `match_order` call per sample, taker TIF given by `tif`, each fully
/// consuming a dedicated fresh maker of the same quantity
/// (`MatchOutcome::Filled` regardless of `tif`, including `Fok`, since an
/// exact-quantity match is a complete fill). Exactly one resting maker
/// exists at any instant, so the reported depth is `1` (issue #142 review
/// finding 2), not the cumulative warmup + sample count.
fn full_match_with_tif(config: &Config, tif: TimeInForce, name: &'static str) -> ScenarioReport {
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
            .expect("full_match_with_tif: seeding a fresh maker id must succeed");
    };
    for i in 0..config.warmup {
        seed(i);
    }
    warmup(config.warmup, |i| {
        level.match_order(
            QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            tif,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });
    // Reset so the health assertion below covers only the measured loop.
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
                tif,
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
        "{name}: every call must fully fill against its dedicated fresh maker"
    );
    fixtures::assert_stats_healthy(&level, config.samples as u64 * QTY, name);

    ScenarioReport::from_samples(
        name,
        "tif",
        1,
        "PriceLevel::match_order — full fill, taker TIF varies",
        durations_ns,
        format!("{filled}/{} Filled (taker_tif={tif:?})", config.samples),
    )
}

/// `Fok` taker whose incoming quantity exactly equals a dedicated fresh
/// maker's quantity: fills completely (`MatchOutcome::Filled`), never
/// killed.
fn fok_success(config: &Config) -> ScenarioReport {
    let mut report = full_match_with_tif(config, TimeInForce::Fok, "tif_fok_success");
    report.category = "tif";
    report
}

/// `Fok` taker whose incoming quantity exceeds the level's ENTIRE resting
/// depth. The resting depth is fixed once, up front: a `Fok` kill leaves the
/// queue untouched by construction (`MatchOutcome::Killed` — zero trades,
/// full remaining quantity), so the same fixture is valid for every sample
/// without per-iteration setup.
fn fok_reject(config: &Config) -> ScenarioReport {
    const RESTING_QTY: u64 = 5;
    const TAKER_QTY: u64 = 10;
    let level = fixtures::seeded_standard_level(1, Side::Sell, RESTING_QTY);
    let generator = fixtures::trade_id_generator();

    warmup(config.warmup, |i| {
        level.match_order(
            TAKER_QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        level.match_order(
            TAKER_QTY,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let killed = results.iter().filter(|r| r.was_killed()).count();
    assert_eq!(
        killed, config.samples,
        "fok_reject: every call must be killed (taker quantity exceeds total resting depth)"
    );
    assert_eq!(
        level.order_count(),
        1,
        "fok_reject: a killed FOK match must leave the resting queue completely untouched"
    );
    // A kill emits zero trades, so no execution is ever recorded.
    fixtures::assert_stats_healthy(&level, 0, "fok_reject");

    ScenarioReport::from_samples(
        "tif_fok_reject",
        "tif",
        1,
        "PriceLevel::match_order — Fok, taker larger than total resting depth (Killed)",
        durations_ns,
        format!("{killed}/{} Killed", config.samples),
    )
}

/// A post-only taker (`TakerKind::PostOnly`) against a non-empty level: it
/// must never take liquidity, so it is rejected on any cross
/// (`MatchOutcome::Rejected`) and the resting queue is left untouched — the
/// same "fixed fixture is valid for every sample" reasoning as
/// [`fok_reject`] applies here.
fn post_only_reject(config: &Config) -> ScenarioReport {
    const RESTING_QTY: u64 = 10;
    let level = fixtures::seeded_standard_level(1, Side::Sell, RESTING_QTY);
    let generator = fixtures::trade_id_generator();

    warmup(config.warmup, |i| {
        level.match_order(
            RESTING_QTY,
            Id::from_u64(TAKER_ID_BASE + i as u64),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        level.match_order(
            RESTING_QTY,
            Id::from_u64(TAKER_ID_BASE + config.warmup as u64 + i as u64),
            TimeInForce::Gtc,
            TakerKind::PostOnly,
            TimestampMs::new(EXECUTION_TIMESTAMP_MS),
            &generator,
        )
    });

    let rejected = results.iter().filter(|r| r.was_rejected()).count();
    assert_eq!(
        rejected, config.samples,
        "post_only_reject: every call must be rejected for crossing as a post-only taker"
    );
    assert_eq!(
        level.order_count(),
        1,
        "post_only_reject: a rejected post-only match must leave the resting queue untouched"
    );
    // A rejection emits zero trades, so no execution is ever recorded.
    fixtures::assert_stats_healthy(&level, 0, "post_only_reject");

    ScenarioReport::from_samples(
        "tif_post_only_reject",
        "tif",
        1,
        "PriceLevel::match_order — TakerKind::PostOnly crossing a non-empty level (Rejected)",
        durations_ns,
        format!("{rejected}/{} Rejected", config.samples),
    )
}
