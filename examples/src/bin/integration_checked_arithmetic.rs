// examples/src/bin/integration_checked_arithmetic.rs
//
// Validates checked arithmetic APIs and error propagation:
// total_quantity(), executed_quantity(), executed_value(), average_price().
// Verifies PriceLevelError variants and panic-free overflow handling.

use pricelevel::{
    Hash32, Id, OrderType, Price, PriceLevel, PriceLevelError, Quantity, Side, TimeInForce,
    TimestampMs, UuidGenerator,
};
use std::process;
use uuid::Uuid;

fn main() {
    let _ = pricelevel::setup_logger();
    println!("=== Integration: Checked Arithmetic & Errors ===\n");

    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .unwrap_or_else(|e| exit_err(&format!("uuid parse: {e}")));
    let id_gen = UuidGenerator::new(namespace);

    test_total_quantity_checked();
    test_executed_quantity_and_value(&id_gen);
    test_average_price(&id_gen);
    test_empty_match_result(&id_gen);
    test_error_variants();
    test_match_result_display_fromstr(&id_gen);

    println!("\n=== Integration: Checked Arithmetic & Errors PASSED ===");
}

fn test_total_quantity_checked() {
    println!("[total_quantity] Checked addition...");

    let level = PriceLevel::new(10_000);
    level.add_order(OrderType::Standard {
        id: Id::from_u64(1),
        price: Price::new(10_000),
        quantity: Quantity::new(500),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });

    level.add_order(OrderType::IcebergOrder {
        id: Id::from_u64(2),
        price: Price::new(10_000),
        visible_quantity: Quantity::new(100),
        hidden_quantity: Quantity::new(400),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(1_000_001),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });

    let total = level
        .total_quantity()
        .unwrap_or_else(|e| exit_err(&format!("total_quantity: {e}")));
    // visible: 500 + 100 = 600, hidden: 0 + 400 = 400, total = 1000
    assert_eq_or_exit(total, 1000, "total_quantity = visible + hidden");
    assert_eq_or_exit(level.visible_quantity(), 600, "visible_quantity");
    assert_eq_or_exit(level.hidden_quantity(), 400, "hidden_quantity");

    println!("  ✓ total_quantity checked addition correct.");
}

fn test_executed_quantity_and_value(id_gen: &UuidGenerator) {
    println!("[executed_quantity/value] Checked aggregation...");

    let level = PriceLevel::new(5_000);

    for i in 1..=3_u64 {
        level.add_order(OrderType::Standard {
            id: Id::from_u64(i),
            price: Price::new(5_000),
            quantity: Quantity::new(100),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_000_000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        });
    }

    let result = level.match_order(250, Id::from_u64(999), id_gen);

    let executed_qty = result
        .executed_quantity()
        .unwrap_or_else(|e| exit_err(&format!("executed_quantity: {e}")));
    assert_eq_or_exit(executed_qty, 250, "executed_quantity");

    let executed_val = result
        .executed_value()
        .unwrap_or_else(|e| exit_err(&format!("executed_value: {e}")));
    // 250 units at price 5000 = 1,250,000
    assert_eq_or_exit(executed_val, 1_250_000, "executed_value");

    // Verify remaining
    assert_eq_or_exit(result.remaining_quantity(), 0, "remaining_quantity");
    assert_or_exit(result.is_complete(), "should be complete");

    // Verify filled_order_ids: first 2 orders fully filled (100 each), third partially filled
    assert_eq_or_exit(result.filled_order_ids().len(), 2, "filled_order_ids count");

    // Verify individual trade accessors
    for trade in result.trades().as_vec() {
        assert_eq_or_exit(trade.price(), Price::new(5_000), "trade price");
        assert_or_exit(trade.quantity().as_u64() > 0, "trade quantity > 0");
        assert_eq_or_exit(trade.taker_order_id(), Id::from_u64(999), "trade taker_id");
    }

    println!("  ✓ executed_quantity and executed_value checked aggregation correct.");
}

fn test_average_price(id_gen: &UuidGenerator) {
    println!("[average_price] Computation...");

    let level = PriceLevel::new(7_500);
    level.add_order(OrderType::Standard {
        id: Id::from_u64(50),
        price: Price::new(7_500),
        quantity: Quantity::new(200),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(2_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });

    let result = level.match_order(100, Id::from_u64(800), id_gen);
    let avg = result
        .average_price()
        .unwrap_or_else(|e| exit_err(&format!("average_price: {e}")));

    assert_or_exit(avg.is_some(), "average_price should be Some");
    let avg_val = avg.unwrap_or(0.0);
    assert_or_exit(
        (avg_val - 7_500.0).abs() < 0.01,
        "average_price should be 7500",
    );

    println!("  ✓ average_price computation correct: {avg_val:.2}");
}

fn test_empty_match_result(id_gen: &UuidGenerator) {
    println!("[empty match] Zero quantity and empty level...");

    // Match on empty level
    let empty_level = PriceLevel::new(10_000);
    let result = empty_level.match_order(100, Id::from_u64(900), id_gen);

    let executed = result
        .executed_quantity()
        .unwrap_or_else(|e| exit_err(&format!("executed_quantity: {e}")));
    assert_eq_or_exit(executed, 0, "empty level executed_quantity");
    assert_eq_or_exit(result.remaining_quantity(), 100, "empty level remaining");
    assert_or_exit(!result.is_complete(), "empty level should not be complete");
    assert_or_exit(
        result.trades().is_empty(),
        "empty level should have no trades",
    );
    assert_or_exit(
        result.filled_order_ids().is_empty(),
        "empty level should have no filled orders",
    );

    // average_price on empty result should be None
    let avg = result
        .average_price()
        .unwrap_or_else(|e| exit_err(&format!("average_price empty: {e}")));
    assert_or_exit(avg.is_none(), "average_price on empty should be None");

    // executed_value on empty result should be 0
    let val = result
        .executed_value()
        .unwrap_or_else(|e| exit_err(&format!("executed_value empty: {e}")));
    assert_eq_or_exit(val, 0, "empty executed_value");

    println!("  ✓ Empty match result arithmetic correct.");
}

fn test_error_variants() {
    println!("[error variants] PriceLevelError matching...");

    // InvalidFormat
    let err1 = "bad input".parse::<pricelevel::Trade>();
    assert_or_exit(err1.is_err(), "bad Trade parse should fail");
    match err1.unwrap_err() {
        PriceLevelError::InvalidFormat => {}
        other => exit_err(&format!("expected InvalidFormat, got: {other}")),
    }

    // InvalidFieldValue
    let err2 = "Trade:trade_id=not-a-uuid;taker_order_id=x;maker_order_id=y;price=z;quantity=1;taker_side=BUY;timestamp=0"
        .parse::<pricelevel::Trade>();
    assert_or_exit(err2.is_err(), "bad field value should fail");

    // InvalidOperation via update on same price
    let level = PriceLevel::new(10_000);
    let err3 = level.update_order(pricelevel::OrderUpdate::UpdatePrice {
        order_id: Id::from_u64(1),
        new_price: Price::new(10_000), // same price
    });
    assert_or_exit(err3.is_err(), "update to same price should error");
    match err3.unwrap_err() {
        PriceLevelError::InvalidOperation { .. } => {}
        other => exit_err(&format!("expected InvalidOperation, got: {other}")),
    }

    println!("  ✓ PriceLevelError variants correctly propagated.");
}

fn test_match_result_display_fromstr(id_gen: &UuidGenerator) {
    println!("[MatchResult] Display/FromStr with checked fields...");

    let level = PriceLevel::new(3_000);
    level.add_order(OrderType::Standard {
        id: Id::from_u64(77),
        price: Price::new(3_000),
        quantity: Quantity::new(50),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(3_000_000),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });

    let result = level.match_order(30, Id::from_u64(888), id_gen);
    let display = result.to_string();

    // Verify the display contains expected fields
    assert_or_exit(
        display.contains("remaining_quantity=0"),
        "display remaining_quantity",
    );
    assert_or_exit(display.contains("is_complete=true"), "display is_complete");

    // Parse back
    let parsed = display
        .parse::<pricelevel::MatchResult>()
        .unwrap_or_else(|e| exit_err(&format!("MatchResult parse: {e}")));
    assert_eq_or_exit(
        parsed.remaining_quantity(),
        result.remaining_quantity(),
        "parsed remaining_quantity",
    );
    assert_eq_or_exit(
        parsed.is_complete(),
        result.is_complete(),
        "parsed is_complete",
    );

    println!("  ✓ MatchResult Display/FromStr with checked fields correct.");
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
