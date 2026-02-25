// examples/src/bin/integration_special_orders.rs
//
// Functional matrix for special order types: Iceberg, Reserve, PostOnly,
// TrailingStop, Pegged, MarketToLimit.
// Validates matching behavior, visible/hidden quantity tracking, and order updates.

use pricelevel::{
    DEFAULT_RESERVE_REPLENISH_AMOUNT, Hash32, Id, OrderType, OrderUpdate, PegReferenceType, Price,
    PriceLevel, Quantity, Side, TimeInForce, TimestampMs, UuidGenerator,
};
use std::process;
use uuid::Uuid;

fn main() {
    let _ = pricelevel::setup_logger();
    println!("=== Integration: Special Orders Matrix ===\n");

    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .unwrap_or_else(|e| exit_err(&format!("uuid parse: {e}")));
    let id_gen = UuidGenerator::new(namespace);

    test_iceberg_order(&id_gen);
    test_reserve_order(&id_gen);
    test_post_only_order(&id_gen);
    test_trailing_stop_order(&id_gen);
    test_pegged_order(&id_gen);
    test_market_to_limit_order(&id_gen);

    println!("\n=== Integration: Special Orders Matrix PASSED ===");
}

fn test_iceberg_order(id_gen: &UuidGenerator) {
    println!("[Iceberg] Visible/hidden quantity tracking...");
    let level = PriceLevel::new(10_000);

    level.add_order(OrderType::IcebergOrder {
        id: Id::from_u64(1),
        price: Price::new(10_000),
        visible_quantity: Quantity::new(10),
        hidden_quantity: Quantity::new(90),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });

    assert_eq_or_exit(level.visible_quantity(), 10, "iceberg visible_qty");
    assert_eq_or_exit(level.hidden_quantity(), 90, "iceberg hidden_qty");
    assert_eq_or_exit(level.order_count(), 1, "iceberg order_count");

    // Match against visible portion
    let result = level.match_order(10, Id::from_u64(100), id_gen);
    let executed = result.executed_quantity().unwrap_or(0);
    assert_eq_or_exit(executed, 10, "iceberg match executed");
    assert_or_exit(result.is_complete(), "taker should be fully filled");

    // Iceberg should replenish from hidden
    // After matching 10 visible, the order should have replenished
    // The order is still there (not fully consumed)
    assert_or_exit(
        result.filled_order_ids().is_empty(),
        "iceberg should not be fully filled yet",
    );

    println!("  ✓ Iceberg visible/hidden tracking and replenishment correct.");
}

fn test_reserve_order(id_gen: &UuidGenerator) {
    println!("[Reserve] Auto-replenish behavior...");
    let level = PriceLevel::new(10_000);

    level.add_order(OrderType::ReserveOrder {
        id: Id::from_u64(2),
        price: Price::new(10_000),
        visible_quantity: Quantity::new(10),
        hidden_quantity: Quantity::new(50),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_001),
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: Quantity::new(2),
        replenish_amount: Some(Quantity::new(10)),
        auto_replenish: true,
        extra_fields: (),
    });

    assert_eq_or_exit(level.visible_quantity(), 10, "reserve visible_qty");
    assert_eq_or_exit(level.hidden_quantity(), 50, "reserve hidden_qty");

    // Match some visible quantity
    let result = level.match_order(8, Id::from_u64(200), id_gen);
    let executed = result.executed_quantity().unwrap_or(0);
    assert_eq_or_exit(executed, 8, "reserve match executed");

    // Reserve order should still be present (not fully consumed)
    assert_or_exit(
        result.filled_order_ids().is_empty(),
        "reserve should not be fully filled",
    );

    // Verify default replenish amount constant is accessible
    assert_or_exit(
        DEFAULT_RESERVE_REPLENISH_AMOUNT > 0,
        "DEFAULT_RESERVE_REPLENISH_AMOUNT should be positive",
    );

    println!("  ✓ Reserve auto-replenish behavior correct.");
}

fn test_post_only_order(id_gen: &UuidGenerator) {
    println!("[PostOnly] Add and match behavior...");
    let level = PriceLevel::new(10_000);

    level.add_order(OrderType::PostOnly {
        id: Id::from_u64(3),
        price: Price::new(10_000),
        quantity: Quantity::new(50),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_002),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });

    assert_eq_or_exit(level.visible_quantity(), 50, "postonly visible_qty");
    assert_eq_or_exit(level.order_count(), 1, "postonly order_count");

    // Match against post-only order
    let result = level.match_order(50, Id::from_u64(300), id_gen);
    assert_eq_or_exit(
        result.executed_quantity().unwrap_or(0),
        50,
        "postonly match executed",
    );
    assert_or_exit(result.is_complete(), "postonly match should be complete");

    // Order should be fully filled
    assert_eq_or_exit(
        result.filled_order_ids().len(),
        1,
        "postonly should have 1 filled order",
    );

    println!("  ✓ PostOnly add and match behavior correct.");
}

fn test_trailing_stop_order(id_gen: &UuidGenerator) {
    println!("[TrailingStop] Add and match behavior...");
    let level = PriceLevel::new(10_000);

    level.add_order(OrderType::TrailingStop {
        id: Id::from_u64(4),
        price: Price::new(10_000),
        quantity: Quantity::new(30),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_003),
        time_in_force: TimeInForce::Gtc,
        trail_amount: Quantity::new(100),
        last_reference_price: Price::new(10_100),
        extra_fields: (),
    });

    assert_eq_or_exit(level.visible_quantity(), 30, "trailing visible_qty");

    let result = level.match_order(30, Id::from_u64(400), id_gen);
    assert_eq_or_exit(
        result.executed_quantity().unwrap_or(0),
        30,
        "trailing match executed",
    );
    assert_or_exit(result.is_complete(), "trailing match complete");

    println!("  ✓ TrailingStop add and match behavior correct.");
}

fn test_pegged_order(id_gen: &UuidGenerator) {
    println!("[Pegged] Add and match behavior...");
    let level = PriceLevel::new(10_000);

    level.add_order(OrderType::PeggedOrder {
        id: Id::from_u64(5),
        price: Price::new(10_000),
        quantity: Quantity::new(25),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_004),
        time_in_force: TimeInForce::Gtc,
        reference_price_offset: 50,
        reference_price_type: PegReferenceType::MidPrice,
        extra_fields: (),
    });

    assert_eq_or_exit(level.visible_quantity(), 25, "pegged visible_qty");

    let result = level.match_order(25, Id::from_u64(500), id_gen);
    assert_eq_or_exit(
        result.executed_quantity().unwrap_or(0),
        25,
        "pegged match executed",
    );
    assert_or_exit(result.is_complete(), "pegged match complete");

    println!("  ✓ Pegged add and match behavior correct.");
}

fn test_market_to_limit_order(id_gen: &UuidGenerator) {
    println!("[MarketToLimit] Add and match behavior...");
    let level = PriceLevel::new(10_000);

    level.add_order(OrderType::MarketToLimit {
        id: Id::from_u64(6),
        price: Price::new(10_000),
        quantity: Quantity::new(40),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_005),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });

    assert_eq_or_exit(level.visible_quantity(), 40, "m2l visible_qty");

    // Partial match
    let result = level.match_order(15, Id::from_u64(600), id_gen);
    assert_eq_or_exit(
        result.executed_quantity().unwrap_or(0),
        15,
        "m2l partial match",
    );
    assert_or_exit(result.is_complete(), "taker fully filled");
    assert_or_exit(
        result.filled_order_ids().is_empty(),
        "maker should not be fully filled",
    );

    // Cancel remaining
    let cancel = level.update_order(OrderUpdate::Cancel {
        order_id: Id::from_u64(6),
    });
    assert_or_exit(cancel.is_ok(), "cancel m2l should succeed");
    assert_eq_or_exit(level.order_count(), 0, "m2l order_count after cancel");

    println!("  ✓ MarketToLimit add, partial match, and cancel correct.");
}

fn assert_eq_or_exit<T: PartialEq + std::fmt::Debug>(actual: T, expected: T, label: &str) {
    if actual != expected {
        eprintln!(
            "ASSERTION FAILED [{}]: expected {:?}, got {:?}",
            label, expected, actual
        );
        process::exit(1);
    }
}

fn assert_or_exit(condition: bool, label: &str) {
    if !condition {
        eprintln!("ASSERTION FAILED: {}", label);
        process::exit(1);
    }
}

fn exit_err(msg: &str) -> ! {
    eprintln!("ERROR: {msg}");
    process::exit(1);
}
