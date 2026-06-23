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
    level.add_order(standard_buy(1, 10_000, 100, 1_000));
    level.add_order(standard_buy(2, 10_000, 100, 1_001));

    // Partially fill A.
    let first = level.match_order(
        60,
        Id::from_u64(901),
        TimestampMs::new(1_716_000_000_000),
        &trade_ids,
    );
    assert_eq!(first.trades().len(), 1);
    assert_eq!(first.trades().as_vec()[0].maker_order_id(), Id::from_u64(1));

    // Second aggressor must hit A's remainder (40) first, then B (10).
    let second = level.match_order(
        50,
        Id::from_u64(902),
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
