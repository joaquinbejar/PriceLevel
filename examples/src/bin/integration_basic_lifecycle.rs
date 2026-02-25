// examples/src/bin/integration_basic_lifecycle.rs
//
// End-to-end order lifecycle: add → update → cancel → match.
// Validates stats deltas, queue/order count consistency, and matching correctness.

use pricelevel::{
    Hash32, Id, MatchResult, OrderType, OrderUpdate, Price, PriceLevel, Quantity, Side,
    TimeInForce, TimestampMs, UuidGenerator,
};
use std::process;
use uuid::Uuid;

fn main() {
    let _ = pricelevel::setup_logger();
    println!("=== Integration: Basic Lifecycle ===\n");

    let price_level = PriceLevel::new(10_000);
    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .unwrap_or_else(|e| exit_err(&format!("uuid parse: {e}")));
    let trade_id_gen = UuidGenerator::new(namespace);

    // --- Phase 1: Add orders ---
    println!("[Phase 1] Adding orders...");
    let mut ts = 1_616_823_000_000_u64;

    for i in 1..=10_u64 {
        let order = OrderType::Standard {
            id: Id::from_u64(i),
            price: Price::new(10_000),
            quantity: Quantity::new(100),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(ts),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        price_level.add_order(order);
        ts += 1;
    }

    assert_eq_or_exit(price_level.order_count(), 10, "order_count after add");
    assert_eq_or_exit(
        price_level.visible_quantity(),
        1_000,
        "visible_quantity after add",
    );
    assert_eq_or_exit(
        price_level.hidden_quantity(),
        0,
        "hidden_quantity after add",
    );
    assert_eq_or_exit(price_level.stats().orders_added(), 10, "stats.orders_added");
    println!("  ✓ 10 orders added, counters consistent.");

    // --- Phase 2: Update quantity ---
    println!("[Phase 2] Updating order quantity...");
    let update_result = price_level.update_order(OrderUpdate::UpdateQuantity {
        order_id: Id::from_u64(1),
        new_quantity: Quantity::new(50),
    });
    assert_or_exit(update_result.is_ok(), "update_order should succeed");
    assert_or_exit(
        update_result
            .as_ref()
            .ok()
            .and_then(|o| o.as_ref())
            .is_some(),
        "updated order should be returned",
    );
    // Visible went from 1000 → 950 (order 1: 100 → 50)
    assert_eq_or_exit(
        price_level.visible_quantity(),
        950,
        "visible_quantity after update",
    );
    assert_eq_or_exit(
        price_level.order_count(),
        10,
        "order_count unchanged after update",
    );
    println!("  ✓ Order 1 quantity updated from 100 to 50.");

    // --- Phase 3: Cancel order ---
    println!("[Phase 3] Cancelling order...");
    let cancel_result = price_level.update_order(OrderUpdate::Cancel {
        order_id: Id::from_u64(2),
    });
    assert_or_exit(cancel_result.is_ok(), "cancel should succeed");
    assert_or_exit(
        cancel_result
            .as_ref()
            .ok()
            .and_then(|o| o.as_ref())
            .is_some(),
        "cancelled order should be returned",
    );
    assert_eq_or_exit(price_level.order_count(), 9, "order_count after cancel");
    assert_eq_or_exit(
        price_level.visible_quantity(),
        850,
        "visible_quantity after cancel",
    );
    assert_eq_or_exit(
        price_level.stats().orders_removed(),
        1,
        "stats.orders_removed",
    );
    println!("  ✓ Order 2 cancelled, counters updated.");

    // --- Phase 4: Cancel non-existent order ---
    println!("[Phase 4] Cancelling non-existent order...");
    let cancel_missing = price_level.update_order(OrderUpdate::Cancel {
        order_id: Id::from_u64(999),
    });
    assert_or_exit(
        cancel_missing.is_ok(),
        "cancel non-existent should not error",
    );
    assert_or_exit(
        cancel_missing.ok().and_then(|o| o).is_none(),
        "cancel non-existent should return None",
    );
    println!("  ✓ Non-existent order cancel returns None.");

    // --- Phase 5: Match orders ---
    println!("[Phase 5] Matching orders...");
    let taker_id = Id::from_u64(1000);
    let match_result: MatchResult = price_level.match_order(200, taker_id, &trade_id_gen);

    let executed_qty = match_result
        .executed_quantity()
        .unwrap_or_else(|e| exit_err(&format!("executed_quantity: {e}")));
    assert_eq_or_exit(executed_qty, 200, "executed_quantity");
    assert_eq_or_exit(match_result.remaining_quantity(), 0, "remaining_quantity");
    assert_or_exit(match_result.is_complete(), "match should be complete");
    assert_or_exit(
        !match_result.trades().as_vec().is_empty(),
        "should have trades",
    );

    // Verify order_id accessor
    assert_eq_or_exit(match_result.order_id(), taker_id, "match_result.order_id");

    println!(
        "  ✓ Matched 200 units across {} trades.",
        match_result.trades().len()
    );

    // --- Phase 6: Partial match ---
    println!("[Phase 6] Partial match (exceeds available)...");
    let taker_id2 = Id::from_u64(1001);
    let remaining_visible = price_level.visible_quantity();
    let partial = price_level.match_order(remaining_visible + 500, taker_id2, &trade_id_gen);

    assert_or_exit(
        !partial.is_complete(),
        "partial match should not be complete",
    );
    assert_or_exit(
        partial.remaining_quantity() > 0,
        "should have remaining quantity",
    );
    println!(
        "  ✓ Partial match: {} remaining out of {} requested.",
        partial.remaining_quantity(),
        remaining_visible + 500,
    );

    // --- Phase 7: Stats consistency ---
    println!("[Phase 7] Verifying statistics...");
    let stats = price_level.stats();
    assert_or_exit(stats.orders_executed() > 0, "should have executed orders");
    assert_or_exit(
        stats.quantity_executed() > 0,
        "should have executed quantity",
    );
    println!(
        "  ✓ Stats: added={}, removed={}, executed={}, qty_executed={}",
        stats.orders_added(),
        stats.orders_removed(),
        stats.orders_executed(),
        stats.quantity_executed(),
    );

    println!("\n=== Integration: Basic Lifecycle PASSED ===");
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
