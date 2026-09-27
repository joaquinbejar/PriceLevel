//! Criterion entry point for the 0.9.2-vs-0.10 comparison. Compiled twice:
//!
//! ```sh
//! cargo bench --manifest-path benches/compare/Cargo.toml --bench compare --features old
//! cargo bench --manifest-path benches/compare/Cargo.toml --bench compare --features new
//! ```
//!
//! Each build writes its own Criterion `target/criterion/<group>/<version
//! label>/...` tree (group names below embed `pricelevel_compare::VERSION_LABEL`),
//! so both versions' estimates land side by side under
//! `benches/compare/target/criterion/`. See `BENCHMARKS.md` at the repo root
//! for the run protocol (interleaving, rounds, noise policy) and the
//! resulting tables; `make bench-compare-0.9` in the main `Makefile` drives
//! both builds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use criterion::{BatchSize, BenchmarkGroup, Criterion, criterion_group, measurement::WallTime};
use pricelevel_compare::VERSION_LABEL;
use pricelevel_compare::pl::{self, TakerKind, TimeInForce};
use pricelevel_compare::workloads as w;
use std::time::Duration;

fn group_name(base: &str) -> String {
    format!("{base}/{VERSION_LABEL}")
}

/// Uniform, deliberately short group configuration so the full ~35-scenario
/// suite finishes in a reasonable wall-clock window on a shared,
/// non-dedicated host (see `BENCHMARKS.md`'s methodology section for why:
/// Criterion's own defaults, 100 samples / 3s warm-up / 5s measurement per
/// benchmark, would make a 2-version x 2-round interleaved run take hours).
/// `sample_size(10)` is Criterion's own minimum. Point estimates from a
/// short run are noisier than Criterion's defaults would give — this is
/// exactly why `BENCHMARKS.md`'s comparison protocol treats any case where
/// two interleaved rounds disagree in sign or differ by more than 10% as
/// noise, not as a real regression or speedup.
fn fast(g: &mut BenchmarkGroup<'_, WallTime>) {
    g.sample_size(10);
    g.warm_up_time(Duration::from_millis(300));
    g.measurement_time(Duration::from_millis(700));
}

fn bench_add_batches(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("add_orders_batch_100"));
    fast(&mut g);
    g.bench_function("standard", |b| b.iter(|| w::add_standard_batch(100)));
    g.bench_function("iceberg", |b| b.iter(|| w::add_iceberg_batch(100)));
    g.bench_function("reserve", |b| b.iter(|| w::add_reserve_batch(100)));
    g.finish();
}

fn bench_isolated_updates(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("isolated_updates"));
    fast(&mut g);
    let depth = 350u64;

    g.bench_function("cancel", |b| {
        b.iter_batched(
            || w::cancel_fixture(depth),
            |level| level.update_order(w::cancel_update(depth)),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("update_quantity_increase", |b| {
        b.iter_batched(
            || w::update_quantity_fixture(depth),
            |level| level.update_order(w::increase_quantity_update(depth)),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("update_quantity_decrease", |b| {
        b.iter_batched(
            || w::update_quantity_fixture(depth),
            |level| level.update_order(w::decrease_quantity_update(depth)),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("replace_same_price", |b| {
        b.iter_batched(
            || w::update_quantity_fixture(depth),
            |level| level.update_order(w::replace_same_price_update(depth)),
            BatchSize::SmallInput,
        );
    });

    g.bench_function("replace_diff_price", |b| {
        b.iter_batched(
            || w::update_quantity_fixture(depth),
            |level| level.update_order(w::replace_diff_price_update(depth)),
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

const TAKER: fn(u64) -> pl::Id = pl::Id::sequential;

fn bench_matching(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("matching"));
    fast(&mut g);
    let id_gen = w::trade_id_generator();

    g.bench_function("standard_full", |b| {
        b.iter_batched(
            w::match_standard_full_fixture,
            |level| {
                w::run_match(
                    &level,
                    100,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("iceberg", |b| {
        b.iter_batched(
            w::match_iceberg_fixture,
            |level| {
                w::run_match(
                    &level,
                    10,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("reserve", |b| {
        b.iter_batched(
            w::match_reserve_fixture,
            |level| {
                w::run_match(
                    &level,
                    10,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("mixed_100", |b| {
        b.iter_batched(
            || w::match_mixed_fixture(100),
            |level| {
                w::run_match(
                    &level,
                    500,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("sweep_100_makers", |b| {
        b.iter_batched(
            w::match_sweep_fixture,
            |level| {
                w::run_match(
                    &level,
                    100,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("partial_fill_reinsert", |b| {
        b.iter_batched(
            w::partial_fill_reinsert_fixture,
            |level| {
                let result = w::run_match(
                    &level,
                    10,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Gtc,
                    TakerKind::Standard,
                    &id_gen,
                );
                let remaining = result.remaining_quantity();
                if remaining != pl::Quantity::ZERO {
                    let remaining_raw = remaining.to_f64_lossy() as u64;
                    let _ = level.add_order(w::standard_order(
                        u64::MAX - 3,
                        w::BASE_PRICE,
                        remaining_raw,
                        pl::Side::Buy,
                    ));
                }
                result
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("partial_fill_churn_10x10", |b| {
        b.iter_batched(
            || w::match_maker_partial_fixture(0),
            |level| {
                let mut last = None;
                for i in 0..10u64 {
                    last = Some(w::run_match(
                        &level,
                        10,
                        TAKER(u64::MAX - 10 - i),
                        TimeInForce::Gtc,
                        TakerKind::Standard,
                        &id_gen,
                    ));
                }
                last
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("ioc_partial", |b| {
        b.iter_batched(
            || w::match_maker_partial_fixture(0),
            |level| {
                w::run_match(
                    &level,
                    10_000_000,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Ioc,
                    TakerKind::Standard,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("post_only_reject", |b| {
        b.iter_batched(
            w::post_only_reject_fixture,
            |level| {
                w::run_match(
                    &level,
                    10,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Gtc,
                    TakerKind::PostOnly,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

fn bench_fok_depth(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("fok_depth"));
    fast(&mut g);
    let id_gen = w::trade_id_generator();

    for depth in [1u64, 100, 10_000] {
        g.bench_function(format!("success_depth_{depth}"), |b| {
            b.iter_batched(
                || w::fok_fixture(depth),
                |level| {
                    w::run_match(
                        &level,
                        1,
                        TAKER(u64::MAX - 2),
                        TimeInForce::Fok,
                        TakerKind::Standard,
                        &id_gen,
                    )
                },
                BatchSize::SmallInput,
            );
        });
    }

    for depth in [100u64, 10_000] {
        g.bench_function(format!("reject_depth_{depth}"), |b| {
            b.iter_batched(
                || w::fok_fixture(depth),
                |level| {
                    w::run_match(
                        &level,
                        depth + 1,
                        TAKER(u64::MAX - 2),
                        TimeInForce::Fok,
                        TakerKind::Standard,
                        &id_gen,
                    )
                },
                BatchSize::SmallInput,
            );
        });
    }

    g.bench_function("replenish_depth_10000", |b| {
        b.iter_batched(
            || w::fok_replenish_fixture(10_000),
            |level| {
                w::run_match(
                    &level,
                    10_005,
                    TAKER(u64::MAX - 2),
                    TimeInForce::Fok,
                    TakerKind::Standard,
                    &id_gen,
                )
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

fn bench_iter_orders(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("iter_orders"));
    fast(&mut g);
    for depth in [100u64, 10_000] {
        g.bench_function(format!("depth_{depth}"), |b| {
            b.iter_batched_ref(
                || w::iter_orders_fixture(depth),
                |level| w::count_iter_orders(level),
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

fn bench_snapshot(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("snapshot"));
    fast(&mut g);

    for depth in [100u64, 10_000] {
        let fixture = || w::iter_orders_fixture(depth);

        g.bench_function(format!("capture_depth_{depth}"), |b| {
            b.iter_batched_ref(
                fixture,
                |level| w::snapshot_capture(level),
                BatchSize::SmallInput,
            );
        });

        g.bench_function(format!("package_depth_{depth}"), |b| {
            b.iter_batched_ref(
                fixture,
                |level| w::snapshot_package(level),
                BatchSize::SmallInput,
            );
        });

        g.bench_function(format!("to_json_depth_{depth}"), |b| {
            b.iter_batched_ref(
                fixture,
                |level| w::snapshot_json(level),
                BatchSize::SmallInput,
            );
        });

        g.bench_function(format!("restore_depth_{depth}"), |b| {
            b.iter_batched(
                || w::snapshot_json(&fixture()),
                |json| w::restore_from_json(&json),
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

fn bench_match_result_analytics(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("match_result_analytics"));
    fast(&mut g);
    for n in [256usize, 4096] {
        let result = w::match_result_with_trades(n);
        g.bench_function(format!("n_{n}"), |b| {
            b.iter(|| {
                let _ = result.executed_quantity();
                let _ = result.executed_value();
                let _ = result.average_price();
            });
        });
    }
    g.finish();
}

fn bench_trade_list_parse(c: &mut Criterion) {
    let mut g = c.benchmark_group(group_name("trade_list_parse"));
    fast(&mut g);
    for n in [32usize, 1024] {
        let text = w::trade_list_text(n);
        g.bench_function(format!("n_{n}"), |b| {
            b.iter(|| w::parse_trade_list(&text));
        });
    }
    g.finish();
}

/// Validates every scenario's outcome ONCE, outside any timed Criterion
/// closure, before the suite runs (rules/global_rules.md's "outcomes are
/// validated outside timed regions"). A wrong outcome panics — bench-only
/// code, never part of the published crate.
fn self_check() {
    use pl::MatchOutcome;

    let level = w::cancel_fixture(10);
    assert!(matches!(
        level.update_order(w::cancel_update(10)),
        Ok(Some(_))
    ));

    let level = w::update_quantity_fixture(10);
    assert!(matches!(
        level.update_order(w::increase_quantity_update(10)),
        Ok(Some(_))
    ));
    let level = w::update_quantity_fixture(10);
    assert!(matches!(
        level.update_order(w::decrease_quantity_update(10)),
        Ok(Some(_))
    ));
    let level = w::update_quantity_fixture(10);
    assert!(matches!(
        level.update_order(w::replace_same_price_update(10)),
        Ok(Some(_))
    ));
    let level = w::update_quantity_fixture(10);
    assert!(matches!(
        level.update_order(w::replace_diff_price_update(10)),
        Ok(Some(_))
    ));

    let id_gen = w::trade_id_generator();
    // Taker ids far above any fixture's resting-maker id range (0..depth,
    // TARGET_ID = u64::MAX/2): a taker whose id collides with a resting
    // maker id is a self-match, terminally rejected regardless of TIF/kind
    // (see `src/lib.rs`'s "level topology invariants" migration guide) — an
    // easy way to accidentally test the wrong thing here.
    let self_check_taker = |n: u64| pl::Id::sequential(u64::MAX - 200 - n);

    let level = w::match_standard_full_fixture();
    let r = w::run_match(
        &level,
        100,
        self_check_taker(1),
        TimeInForce::Gtc,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::Filled);

    let level = w::match_iceberg_fixture();
    let r = w::run_match(
        &level,
        10,
        self_check_taker(2),
        TimeInForce::Gtc,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::Filled);

    let level = w::match_reserve_fixture();
    let r = w::run_match(
        &level,
        10,
        self_check_taker(3),
        TimeInForce::Gtc,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::Filled);

    let level = w::match_sweep_fixture();
    let r = w::run_match(
        &level,
        100,
        self_check_taker(4),
        TimeInForce::Gtc,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::Filled);
    assert_eq!(r.trades().len(), 100);

    let level = w::partial_fill_reinsert_fixture();
    let r = w::run_match(
        &level,
        10,
        self_check_taker(5),
        TimeInForce::Gtc,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::PartiallyFilled);

    let level = w::match_maker_partial_fixture(0);
    let r = w::run_match(
        &level,
        10_000_000,
        self_check_taker(6),
        TimeInForce::Ioc,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::PartiallyFilled);

    let level = w::post_only_reject_fixture();
    let r = w::run_match(
        &level,
        10,
        self_check_taker(7),
        TimeInForce::Gtc,
        TakerKind::PostOnly,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::Rejected);

    let level = w::fok_fixture(100);
    let r = w::run_match(
        &level,
        1,
        self_check_taker(8),
        TimeInForce::Fok,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::Filled);

    let level = w::fok_fixture(100);
    let r = w::run_match(
        &level,
        101,
        self_check_taker(9),
        TimeInForce::Fok,
        TakerKind::Standard,
        &id_gen,
    );
    assert!(r.was_killed() || r.was_rejected());

    let level = w::fok_replenish_fixture(10_000);
    let r = w::run_match(
        &level,
        10_005,
        self_check_taker(10),
        TimeInForce::Fok,
        TakerKind::Standard,
        &id_gen,
    );
    assert_eq!(r.outcome(), MatchOutcome::Filled);

    let level = w::iter_orders_fixture(100);
    assert_eq!(w::count_iter_orders(&level), 100);

    let level = w::iter_orders_fixture(100);
    let snap = w::snapshot_capture(&level);
    assert_eq!(snap.orders().len(), 100);
    let json = w::snapshot_json(&level);
    let restored = w::restore_from_json(&json);
    assert_eq!(w::count_iter_orders(&restored), 100);

    let mr = w::match_result_with_trades(256);
    assert_eq!(
        mr.executed_quantity().expect("executed_quantity failed"),
        pl::Quantity::new(256)
    );

    let text = w::trade_list_text(32);
    let parsed = w::parse_trade_list(&text);
    assert_eq!(parsed.len(), 32);

    eprintln!("pricelevel-compare[{}]: self-check passed", VERSION_LABEL);
}

criterion_group!(
    benches,
    bench_add_batches,
    bench_isolated_updates,
    bench_matching,
    bench_fok_depth,
    bench_iter_orders,
    bench_snapshot,
    bench_match_result_analytics,
    bench_trade_list_parse,
);

fn main() {
    self_check();
    benches();
    Criterion::default().configure_from_args().final_summary();
}
