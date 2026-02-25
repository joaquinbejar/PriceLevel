// examples/src/bin/integration_snapshot_recovery.rs
//
// Validates snapshot persistence and recovery:
// snapshot_package → JSON export → from_json → validate → from_snapshot_package.
// Also tests checksum corruption detection and aggregate consistency after restore.

use pricelevel::{
    Hash32, Id, OrderType, Price, PriceLevel, PriceLevelError, PriceLevelSnapshot,
    PriceLevelSnapshotPackage, Quantity, Side, TimeInForce, TimestampMs,
};
use std::process;

fn main() {
    let _ = pricelevel::setup_logger();
    println!("=== Integration: Snapshot & Recovery ===\n");

    // --- Phase 1: Build a price level with mixed order types ---
    println!("[Phase 1] Building price level...");
    let original = PriceLevel::new(10_000);
    let mut ts = 1_616_823_000_000_u64;

    // Standard orders
    for i in 1..=5_u64 {
        original.add_order(OrderType::Standard {
            id: Id::from_u64(i),
            price: Price::new(10_000),
            quantity: Quantity::new(100),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(ts),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        });
        ts += 1;
    }

    // Iceberg order
    original.add_order(OrderType::IcebergOrder {
        id: Id::from_u64(10),
        price: Price::new(10_000),
        visible_quantity: Quantity::new(20),
        hidden_quantity: Quantity::new(80),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(ts),
        time_in_force: TimeInForce::Gtc,
        extra_fields: (),
    });
    ts += 1;

    // Reserve order
    original.add_order(OrderType::ReserveOrder {
        id: Id::from_u64(11),
        price: Price::new(10_000),
        visible_quantity: Quantity::new(15),
        hidden_quantity: Quantity::new(60),
        side: Side::Buy,
        user_id: Hash32::zero(),
        timestamp: TimestampMs::new(ts),
        time_in_force: TimeInForce::Gtc,
        replenish_threshold: Quantity::new(5),
        replenish_amount: Some(Quantity::new(10)),
        auto_replenish: true,
        extra_fields: (),
    });

    let orig_order_count = original.order_count();
    let orig_visible = original.visible_quantity();
    let orig_hidden = original.hidden_quantity();
    let orig_price = original.price();
    println!(
        "  Built: price={}, orders={}, visible={}, hidden={}",
        orig_price, orig_order_count, orig_visible, orig_hidden
    );

    // --- Phase 2: Create snapshot package and serialize to JSON ---
    println!("[Phase 2] Creating snapshot package...");
    let package = original
        .snapshot_package()
        .unwrap_or_else(|e| exit_err(&format!("snapshot_package: {e}")));

    assert_eq_or_exit(package.version(), 1, "snapshot version");
    assert_or_exit(
        !package.checksum().is_empty(),
        "checksum should not be empty",
    );

    package
        .validate()
        .unwrap_or_else(|e| exit_err(&format!("validate: {e}")));

    let json = package
        .to_json()
        .unwrap_or_else(|e| exit_err(&format!("to_json: {e}")));
    assert_or_exit(!json.is_empty(), "JSON should not be empty");
    println!(
        "  ✓ Snapshot package created and validated. JSON length: {}",
        json.len()
    );

    // --- Phase 3: Deserialize and validate ---
    println!("[Phase 3] Restoring from JSON...");
    let restored_package = PriceLevelSnapshotPackage::from_json(&json)
        .unwrap_or_else(|e| exit_err(&format!("from_json: {e}")));

    restored_package
        .validate()
        .unwrap_or_else(|e| exit_err(&format!("restored validate: {e}")));

    // Verify snapshot accessors
    let snap: &PriceLevelSnapshot = restored_package.snapshot();
    assert_eq_or_exit(snap.price(), orig_price, "snapshot price");
    assert_eq_or_exit(snap.order_count(), orig_order_count, "snapshot order_count");
    assert_eq_or_exit(
        snap.visible_quantity(),
        orig_visible,
        "snapshot visible_qty",
    );
    assert_eq_or_exit(snap.hidden_quantity(), orig_hidden, "snapshot hidden_qty");
    assert_eq_or_exit(snap.orders().len(), orig_order_count, "snapshot orders len");
    println!("  ✓ Restored snapshot aggregates match original.");

    // --- Phase 4: Reconstruct PriceLevel from snapshot package ---
    println!("[Phase 4] Reconstructing PriceLevel...");
    let restored = PriceLevel::from_snapshot_package(restored_package)
        .unwrap_or_else(|e| exit_err(&format!("from_snapshot_package: {e}")));

    assert_eq_or_exit(restored.price(), orig_price, "restored price");
    assert_eq_or_exit(
        restored.order_count(),
        orig_order_count,
        "restored order_count",
    );
    assert_eq_or_exit(
        restored.visible_quantity(),
        orig_visible,
        "restored visible_qty",
    );
    assert_eq_or_exit(
        restored.hidden_quantity(),
        orig_hidden,
        "restored hidden_qty",
    );
    println!("  ✓ Reconstructed PriceLevel matches original.");

    // --- Phase 5: Verify order preservation ---
    println!("[Phase 5] Verifying order ID preservation...");
    let original_ids: Vec<Id> = original.snapshot_orders().iter().map(|o| o.id()).collect();
    let restored_ids: Vec<Id> = restored.snapshot_orders().iter().map(|o| o.id()).collect();
    let id_count = original_ids.len();
    assert_eq_or_exit(restored_ids, original_ids, "order IDs preserved");
    println!("  ✓ All {} order IDs preserved after restore.", id_count);

    // --- Phase 6: snapshot_to_json convenience path ---
    println!("[Phase 6] snapshot_to_json convenience roundtrip...");
    let json2 = original
        .snapshot_to_json()
        .unwrap_or_else(|e| exit_err(&format!("snapshot_to_json: {e}")));
    let restored2 = PriceLevel::from_snapshot_json(&json2)
        .unwrap_or_else(|e| exit_err(&format!("from_snapshot_json: {e}")));
    assert_eq_or_exit(
        restored2.order_count(),
        orig_order_count,
        "json roundtrip order_count",
    );
    println!("  ✓ snapshot_to_json/from_snapshot_json roundtrip correct.");

    // --- Phase 7: Checksum corruption detection ---
    println!("[Phase 7] Checksum corruption detection...");
    let mut tampered: serde_json::Value =
        serde_json::from_str(&json).unwrap_or_else(|e| exit_err(&format!("JSON parse: {e}")));
    if let Some(obj) = tampered.as_object_mut() {
        obj.insert(
            "checksum".to_string(),
            serde_json::Value::String("deadbeef_corrupted".to_string()),
        );
    }
    let tampered_json = serde_json::to_string(&tampered)
        .unwrap_or_else(|e| exit_err(&format!("tampered JSON serialize: {e}")));

    let tampered_pkg = PriceLevelSnapshotPackage::from_json(&tampered_json)
        .unwrap_or_else(|e| exit_err(&format!("tampered from_json: {e}")));

    let validate_err = tampered_pkg.validate();
    assert_or_exit(
        validate_err.is_err(),
        "tampered checksum should fail validation",
    );
    if let Err(e) = validate_err {
        match e {
            PriceLevelError::ChecksumMismatch { .. } => {
                println!("  ✓ ChecksumMismatch correctly detected.");
            }
            other => {
                exit_err(&format!("expected ChecksumMismatch, got: {other}"));
            }
        }
    }

    // Also verify from_snapshot_package rejects corrupted data
    let tampered_pkg2 = PriceLevelSnapshotPackage::from_json(&tampered_json)
        .unwrap_or_else(|e| exit_err(&format!("tampered from_json 2: {e}")));
    let restore_err = PriceLevel::from_snapshot_package(tampered_pkg2);
    assert_or_exit(
        restore_err.is_err(),
        "from_snapshot_package should reject corrupted checksum",
    );
    println!("  ✓ from_snapshot_package rejects corrupted checksum.");

    // --- Phase 8: Snapshot total_quantity ---
    println!("[Phase 8] Snapshot total_quantity...");
    let snap2 = original.snapshot();
    let total = snap2
        .total_quantity()
        .unwrap_or_else(|e| exit_err(&format!("total_quantity: {e}")));
    assert_eq_or_exit(total, orig_visible + orig_hidden, "snapshot total_quantity");
    println!(
        "  ✓ Snapshot total_quantity = {} (visible {} + hidden {}).",
        total, orig_visible, orig_hidden
    );

    println!("\n=== Integration: Snapshot & Recovery PASSED ===");
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
