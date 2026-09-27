// benches/latency/scenarios/isolated.rs
//! Isolated single-mutation scenarios: one `add_order`, one `update_order`
//! call per sample, fixture construction and result handling outside the
//! timed window (issue #142).

use crate::config::Config;
use crate::fixtures::{self, LEVEL_PRICE};
use crate::report::ScenarioReport;
use crate::timing::{measure, warmup};
use pricelevel::prelude::*;

/// Fixed resting depth every isolated scenario seeds before measuring, large
/// enough that a single extra order is a negligible fraction of the level.
const SEED_DEPTH: u64 = 1_000;

/// Runs every isolated scenario and returns one report each.
#[must_use]
pub fn run(config: &Config) -> Vec<ScenarioReport> {
    vec![
        add_gtc(config),
        cancel_success(config),
        cancel_missing(config),
        quantity_decrease(config),
        quantity_increase(config),
        replace(config),
    ]
}

/// Isolated `add_order` of a fresh standard GTC order into a level that
/// already has [`SEED_DEPTH`] resting orders on the same side.
fn add_gtc(config: &Config) -> ScenarioReport {
    let total = config.warmup + config.samples;
    let level = fixtures::seeded_standard_level(SEED_DEPTH, Side::Buy, 10);

    // Pre-build every order outside the timed loop; `measure` only ever
    // calls `add_order` on an already-constructed value.
    let mut orders: Vec<Option<OrderType<()>>> = (0..total)
        .map(|i| {
            Some(fixtures::standard_order(
                SEED_DEPTH + i as u64,
                Side::Buy,
                10,
                TimeInForce::Gtc,
            ))
        })
        .collect();

    warmup(config.warmup, |i| {
        level
            .add_order(orders[i].take().expect("warmup order slot must be present"))
            .expect("warmup add_order must succeed for a fresh sequential id")
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        let order = orders[config.warmup + i]
            .take()
            .expect("measured order slot must be present");
        level.add_order(order)
    });

    let succeeded = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        succeeded, config.samples,
        "add_gtc: every add_order call on a fresh sequential id must succeed"
    );

    ScenarioReport::from_samples(
        "isolated_add_gtc",
        "isolated",
        SEED_DEPTH,
        "PriceLevel::add_order(Standard, Gtc)",
        durations_ns,
        format!("{succeeded}/{} succeeded", config.samples),
    )
}

/// Isolated successful `update_order(Cancel)` on an id that is present.
fn cancel_success(config: &Config) -> ScenarioReport {
    let total = (config.warmup + config.samples) as u64;
    let level = fixtures::seeded_standard_level(total, Side::Buy, 10);

    warmup(config.warmup, |i| {
        level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(i as u64),
            })
            .expect("warmup cancel must not error")
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64((config.warmup + i) as u64),
        })
    });

    let found = results.iter().filter(|r| matches!(r, Ok(Some(_)))).count();
    assert_eq!(
        found, config.samples,
        "cancel_success: every id in the seeded range must be found and cancelled exactly once"
    );

    ScenarioReport::from_samples(
        "isolated_cancel_success",
        "isolated",
        total,
        "PriceLevel::update_order(Cancel) — id present",
        durations_ns,
        format!("{found}/{} found and cancelled", config.samples),
    )
}

/// Isolated `update_order(Cancel)` on an id that was never admitted —
/// exercises the "not found" (`Ok(None)`) path, never a failure.
fn cancel_missing(config: &Config) -> ScenarioReport {
    let level = fixtures::seeded_standard_level(SEED_DEPTH, Side::Buy, 10);
    let miss_base = SEED_DEPTH;

    warmup(config.warmup, |i| {
        level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(miss_base + i as u64),
            })
            .expect("warmup missing-cancel must not error")
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(miss_base + config.warmup as u64 + i as u64),
        })
    });

    let missing = results.iter().filter(|r| matches!(r, Ok(None))).count();
    assert_eq!(
        missing, config.samples,
        "cancel_missing: every id in the never-admitted range must report Ok(None)"
    );

    ScenarioReport::from_samples(
        "isolated_cancel_missing",
        "isolated",
        SEED_DEPTH,
        "PriceLevel::update_order(Cancel) — id absent",
        durations_ns,
        format!("{missing}/{} reported missing (Ok(None))", config.samples),
    )
}

/// Isolated `update_order(UpdateQuantity)` decreasing a fresh order's
/// quantity.
fn quantity_decrease(config: &Config) -> ScenarioReport {
    resize_scenario(config, 100, 40, "isolated_quantity_decrease")
}

/// Isolated `update_order(UpdateQuantity)` increasing a fresh order's
/// quantity.
fn quantity_increase(config: &Config) -> ScenarioReport {
    resize_scenario(config, 40, 100, "isolated_quantity_increase")
}

fn resize_scenario(
    config: &Config,
    from_qty: u64,
    to_qty: u64,
    name: &'static str,
) -> ScenarioReport {
    let total = (config.warmup + config.samples) as u64;
    let level = PriceLevel::new(LEVEL_PRICE);
    for i in 0..total {
        level
            .add_order(fixtures::standard_order(
                i,
                Side::Buy,
                from_qty,
                TimeInForce::Gtc,
            ))
            .expect("fixture seeding: add_order must succeed for a fresh sequential id");
    }

    warmup(config.warmup, |i| {
        level
            .update_order(OrderUpdate::UpdateQuantity {
                order_id: Id::from_u64(i as u64),
                new_quantity: Quantity::new(to_qty),
            })
            .expect("warmup resize must not error")
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        level.update_order(OrderUpdate::UpdateQuantity {
            order_id: Id::from_u64((config.warmup + i) as u64),
            new_quantity: Quantity::new(to_qty),
        })
    });

    let resized = results.iter().filter(|r| matches!(r, Ok(Some(_)))).count();
    assert_eq!(
        resized, config.samples,
        "{name}: every seeded id must resize exactly once"
    );

    ScenarioReport::from_samples(
        name,
        "isolated",
        total,
        "PriceLevel::update_order(UpdateQuantity)",
        durations_ns,
        format!(
            "{resized}/{} resized ({from_qty} -> {to_qty})",
            config.samples
        ),
    )
}

/// Isolated `update_order(Replace)` of a fresh order with a new price
/// (same-price level, so `Replace` here changes quantity while price is
/// re-affirmed — matching the level's own price is mandatory, see
/// `PriceLevel::add_order`'s admission check).
fn replace(config: &Config) -> ScenarioReport {
    let total = (config.warmup + config.samples) as u64;
    let level = fixtures::seeded_standard_level(total, Side::Buy, 10);

    warmup(config.warmup, |i| {
        level
            .update_order(OrderUpdate::Replace {
                order_id: Id::from_u64(i as u64),
                price: Price::new(LEVEL_PRICE),
                quantity: Quantity::new(25),
                side: Side::Buy,
            })
            .expect("warmup replace must not error")
    });

    let (durations_ns, results) = measure(config.samples, |i| {
        level.update_order(OrderUpdate::Replace {
            order_id: Id::from_u64((config.warmup + i) as u64),
            price: Price::new(LEVEL_PRICE),
            quantity: Quantity::new(25),
            side: Side::Buy,
        })
    });

    let replaced = results.iter().filter(|r| matches!(r, Ok(Some(_)))).count();
    assert_eq!(
        replaced, config.samples,
        "replace: every seeded id must be replaced exactly once"
    );

    ScenarioReport::from_samples(
        "isolated_replace",
        "isolated",
        total,
        "PriceLevel::update_order(Replace)",
        durations_ns,
        format!("{replaced}/{} replaced", config.samples),
    )
}
