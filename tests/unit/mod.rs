/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 28/3/25
******************************************************************************/

//! Integration tests exercising the public `pricelevel` API across modules.

use pricelevel::prelude::*;
use uuid::Uuid;

fn standard_buy(id: u64, price: u128, quantity: u64, timestamp: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(timestamp),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

/// End-to-end repro for issue #39 through the public surface: rest A then B at
/// the same price, partially fill A, and confirm a second aggressor consumes
/// A's remainder before B (strict price-time priority across `match_order`
/// calls).
#[test]
fn partial_fill_keeps_price_time_priority_across_calls() {
    let level = PriceLevel::new(10_000);
    let namespace = match Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8") {
        Ok(ns) => ns,
        Err(e) => panic!("invalid namespace uuid: {e}"),
    };
    let trade_ids = UuidGenerator::new(namespace);

    // A (id=1) rests before B (id=2); both 100 @ 10_000.
    level
        .add_order(standard_buy(1, 10_000, 100, 1_000))
        .expect("add_order should succeed");
    level
        .add_order(standard_buy(2, 10_000, 100, 1_001))
        .expect("add_order should succeed");

    // Partially fill A.
    let first = level.match_order(
        60,
        Id::from_u64(901),
        TimeInForce::Gtc,
        TakerKind::Standard,
        TimestampMs::new(1_716_000_000_000),
        &trade_ids,
    );
    assert_eq!(first.trades().len(), 1);
    assert_eq!(first.trades().as_vec()[0].maker_order_id(), Id::from_u64(1));

    // Second aggressor must hit A's remainder (40) first, then B (10).
    let second = level.match_order(
        50,
        Id::from_u64(902),
        TimeInForce::Gtc,
        TakerKind::Standard,
        TimestampMs::new(1_716_000_000_000),
        &trade_ids,
    );
    assert_eq!(second.trades().len(), 2);
    assert_eq!(
        second.trades().as_vec()[0].maker_order_id(),
        Id::from_u64(1),
        "A's residual must be consumed before the later-arriving B"
    );
    assert_eq!(second.trades().as_vec()[0].quantity(), Quantity::new(40));
    assert_eq!(
        second.trades().as_vec()[1].maker_order_id(),
        Id::from_u64(2)
    );
    assert_eq!(second.trades().as_vec()[1].quantity(), Quantity::new(10));

    // Conservation: started 200, consumed 110, 90 remains on B.
    assert_eq!(level.visible_quantity(), 90);
    assert_eq!(level.order_count(), 1);
}

/// Fixed-point scale applied to both price and quantity in the issue #140
/// reproduction.
const ISSUE_140_SCALE: u64 = 100_000_000;

fn scaled_sell(id: u64, price: u128, quantity: u64) -> OrderType<()> {
    OrderType::Standard {
        id: Id::from_u64(id),
        price: Price::new(price),
        quantity: Quantity::new(quantity),
        side: Side::Sell,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    }
}

/// End-to-end repro for issue #140 through the public surface, case A: one
/// execution whose `quantity * price` (both scaled by 1e8) exceeds `u64::MAX`
/// is recorded in full and does not degrade the level's statistics.
#[test]
fn scaled_single_execution_above_u64_max_is_recorded() {
    let price = 49_995 * u128::from(ISSUE_140_SCALE);
    let level = PriceLevel::new(price);
    let trade_ids = UuidGenerator::new(Uuid::nil());

    level
        .add_order(scaled_sell(1, price, ISSUE_140_SCALE))
        .expect("add_order");
    let result = level.match_order(
        ISSUE_140_SCALE,
        Id::from_u64(1_000_000),
        TimeInForce::Gtc,
        TakerKind::Standard,
        TimestampMs::new(2),
        &trade_ids,
    );
    assert!(result.is_complete());

    let expected = u128::from(ISSUE_140_SCALE) * price;
    assert!(expected > u128::from(u64::MAX));
    let stats = level.stats();
    assert!(!stats.stats_degraded());
    assert_eq!(stats.value_executed(), expected);
    assert_eq!(result.executed_value().expect("executed_value"), expected);
}

/// Issue #140 case B: 1.0 @ 1.0 (both scaled by 1e8) degraded the level after
/// 1845 executions when the accumulator was `u64`. 3000 executions now record
/// exactly, and the level still round-trips through its checksummed snapshot.
#[test]
fn scaled_running_total_above_u64_max_is_recorded_and_snapshots() {
    const ROUNDS: u64 = 3_000;
    let price = u128::from(ISSUE_140_SCALE);
    let level = PriceLevel::new(price);
    let trade_ids = UuidGenerator::new(Uuid::nil());

    for i in 0..ROUNDS {
        level
            .add_order(scaled_sell(i + 1, price, ISSUE_140_SCALE))
            .expect("add_order");
        let result = level.match_order(
            ISSUE_140_SCALE,
            Id::from_u64(1_000_000 + i),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(2),
            &trade_ids,
        );
        assert!(result.is_complete());
        assert!(
            !level.stats().stats_degraded(),
            "degraded after {} executions",
            i + 1
        );
    }

    let expected = u128::from(ROUNDS) * u128::from(ISSUE_140_SCALE) * price;
    assert!(expected > u128::from(u64::MAX));
    assert_eq!(level.stats().value_executed(), expected);

    let json = level.snapshot_to_json().expect("snapshot_to_json");
    let restored = PriceLevel::from_snapshot_json(&json).expect("from_snapshot_json");
    assert_eq!(restored.stats().value_executed(), expected);
    assert!(!restored.stats().stats_degraded());
}
