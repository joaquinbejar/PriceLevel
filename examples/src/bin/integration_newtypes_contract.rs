// examples/src/bin/integration_newtypes_contract.rs
//
// Validates the domain newtype contracts: Price, Quantity, TimestampMs, Id.
// Covers construction, parsing, Display/FromStr consistency, and boundary values.

use pricelevel::{Id, Price, Quantity, TimestampMs};
use std::process;
use std::str::FromStr;

fn main() {
    let _ = pricelevel::setup_logger();
    println!("=== Integration: Newtypes Contract ===\n");

    test_price();
    test_quantity();
    test_timestamp();
    test_id();

    println!("\n=== Integration: Newtypes Contract PASSED ===");
}

fn test_price() {
    println!("[Price] Construction and roundtrip...");

    let p = Price::new(12345);
    assert_eq_or_exit(p.as_u128(), 12345, "Price::as_u128");

    // Display/FromStr roundtrip
    let s = p.to_string();
    let parsed = Price::from_str(&s).unwrap_or_else(|e| exit_err(&format!("Price::from_str: {e}")));
    assert_eq_or_exit(parsed, p, "Price roundtrip");

    // Zero price
    let zero = Price::new(0);
    assert_eq_or_exit(zero.as_u128(), 0, "Price zero");

    // Large price
    let large = Price::new(u128::MAX);
    assert_eq_or_exit(large.as_u128(), u128::MAX, "Price u128::MAX");

    // Serde roundtrip
    let json =
        serde_json::to_string(&p).unwrap_or_else(|e| exit_err(&format!("Price serialize: {e}")));
    let deser: Price = serde_json::from_str(&json)
        .unwrap_or_else(|e| exit_err(&format!("Price deserialize: {e}")));
    assert_eq_or_exit(deser, p, "Price serde roundtrip");

    println!("  ✓ Price construction, roundtrip, and boundaries correct.");
}

fn test_quantity() {
    println!("[Quantity] Construction and roundtrip...");

    let q = Quantity::new(500);
    assert_eq_or_exit(q.as_u64(), 500, "Quantity::as_u64");

    // Display/FromStr roundtrip
    let s = q.to_string();
    let parsed =
        Quantity::from_str(&s).unwrap_or_else(|e| exit_err(&format!("Quantity::from_str: {e}")));
    assert_eq_or_exit(parsed, q, "Quantity roundtrip");

    // Zero
    let zero = Quantity::new(0);
    assert_eq_or_exit(zero.as_u64(), 0, "Quantity zero");

    // Max
    let max_q = Quantity::new(u64::MAX);
    assert_eq_or_exit(max_q.as_u64(), u64::MAX, "Quantity u64::MAX");

    // Serde roundtrip
    let json =
        serde_json::to_string(&q).unwrap_or_else(|e| exit_err(&format!("Quantity serialize: {e}")));
    let deser: Quantity = serde_json::from_str(&json)
        .unwrap_or_else(|e| exit_err(&format!("Quantity deserialize: {e}")));
    assert_eq_or_exit(deser, q, "Quantity serde roundtrip");

    println!("  ✓ Quantity construction, roundtrip, and boundaries correct.");
}

fn test_timestamp() {
    println!("[TimestampMs] Construction and roundtrip...");

    let ts = TimestampMs::new(1_616_823_000_000);
    assert_eq_or_exit(ts.as_u64(), 1_616_823_000_000, "TimestampMs::as_u64");

    // Display/FromStr roundtrip
    let s = ts.to_string();
    let parsed = TimestampMs::from_str(&s)
        .unwrap_or_else(|e| exit_err(&format!("TimestampMs::from_str: {e}")));
    assert_eq_or_exit(parsed, ts, "TimestampMs roundtrip");

    // Zero
    let zero = TimestampMs::new(0);
    assert_eq_or_exit(zero.as_u64(), 0, "TimestampMs zero");

    // Serde roundtrip
    let json = serde_json::to_string(&ts)
        .unwrap_or_else(|e| exit_err(&format!("TimestampMs serialize: {e}")));
    let deser: TimestampMs = serde_json::from_str(&json)
        .unwrap_or_else(|e| exit_err(&format!("TimestampMs deserialize: {e}")));
    assert_eq_or_exit(deser, ts, "TimestampMs serde roundtrip");

    println!("  ✓ TimestampMs construction, roundtrip, and boundaries correct.");
}

fn test_id() {
    println!("[Id] Construction and roundtrip...");

    let id = Id::from_u64(42);
    let s = id.to_string();
    let parsed = Id::from_str(&s).unwrap_or_else(|e| exit_err(&format!("Id::from_str: {e}")));
    assert_eq_or_exit(parsed, id, "Id roundtrip");

    // UUID-based Id
    let uuid = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .unwrap_or_else(|e| exit_err(&format!("uuid parse: {e}")));
    let id_uuid = Id::from_uuid(uuid);
    let s2 = id_uuid.to_string();
    let parsed2 =
        Id::from_str(&s2).unwrap_or_else(|e| exit_err(&format!("Id::from_str uuid: {e}")));
    assert_eq_or_exit(parsed2, id_uuid, "Id uuid roundtrip");

    // Serde roundtrip
    let json =
        serde_json::to_string(&id).unwrap_or_else(|e| exit_err(&format!("Id serialize: {e}")));
    let deser: Id =
        serde_json::from_str(&json).unwrap_or_else(|e| exit_err(&format!("Id deserialize: {e}")));
    assert_eq_or_exit(deser, id, "Id serde roundtrip");

    // Sequential IDs are distinct
    let a = Id::from_u64(1);
    let b = Id::from_u64(2);
    assert_or_exit(a != b, "sequential IDs should differ");

    println!("  ✓ Id construction, roundtrip, and uniqueness correct.");
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
