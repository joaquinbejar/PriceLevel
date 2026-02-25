// examples/src/bin/integration_trade_roundtrip.rs
//
// Validates Trade, TradeList, and MatchResult serialization roundtrips:
// Display/FromStr and serde JSON for all execution types.

use pricelevel::{
    Hash32, Id, MatchResult, OrderType, Price, PriceLevel, Quantity, Side, TimeInForce,
    TimestampMs, Trade, TradeList, UuidGenerator,
};
use std::process;
use std::str::FromStr;
use uuid::Uuid;

fn main() {
    let _ = pricelevel::setup_logger();
    println!("=== Integration: Trade & MatchResult Roundtrip ===\n");

    // --- Phase 1: Trade Display/FromStr roundtrip ---
    println!("[Phase 1] Trade Display/FromStr roundtrip...");
    let trade = Trade::with_timestamp(
        Id::from_u64(100),
        Id::from_u64(1),
        Id::from_u64(2),
        Price::new(9500),
        Quantity::new(42),
        Side::Buy,
        TimestampMs::new(1_616_823_000_000),
    );

    let display_str = trade.to_string();
    assert_or_exit(
        display_str.contains("trade_id="),
        "Trade display should contain trade_id",
    );
    assert_or_exit(
        display_str.contains("price=9500"),
        "Trade display should contain price",
    );

    let parsed = Trade::from_str(&display_str)
        .unwrap_or_else(|e| exit_err(&format!("Trade::from_str: {e}")));
    assert_eq_or_exit(parsed.trade_id(), trade.trade_id(), "trade_id roundtrip");
    assert_eq_or_exit(
        parsed.taker_order_id(),
        trade.taker_order_id(),
        "taker_order_id roundtrip",
    );
    assert_eq_or_exit(
        parsed.maker_order_id(),
        trade.maker_order_id(),
        "maker_order_id roundtrip",
    );
    assert_eq_or_exit(parsed.price(), trade.price(), "price roundtrip");
    assert_eq_or_exit(parsed.quantity(), trade.quantity(), "quantity roundtrip");
    assert_eq_or_exit(
        parsed.taker_side(),
        trade.taker_side(),
        "taker_side roundtrip",
    );
    assert_eq_or_exit(parsed.timestamp(), trade.timestamp(), "timestamp roundtrip");
    println!("  ✓ Trade Display/FromStr roundtrip correct.");

    // --- Phase 2: Trade serde JSON roundtrip ---
    println!("[Phase 2] Trade serde JSON roundtrip...");
    let json = serde_json::to_string(&trade)
        .unwrap_or_else(|e| exit_err(&format!("Trade serialize: {e}")));
    let deserialized: Trade = serde_json::from_str(&json)
        .unwrap_or_else(|e| exit_err(&format!("Trade deserialize: {e}")));
    assert_eq_or_exit(deserialized, trade, "Trade serde roundtrip");
    println!("  ✓ Trade serde JSON roundtrip correct.");

    // --- Phase 3: Trade accessor validation ---
    println!("[Phase 3] Trade accessor validation...");
    assert_eq_or_exit(
        trade.maker_side(),
        Side::Sell,
        "maker_side is opposite of taker",
    );
    let trade_buy = Trade::with_timestamp(
        Id::from_u64(200),
        Id::from_u64(10),
        Id::from_u64(20),
        Price::new(5000),
        Quantity::new(1),
        Side::Sell,
        TimestampMs::new(1_616_823_001_000),
    );
    assert_eq_or_exit(
        trade_buy.maker_side(),
        Side::Buy,
        "maker_side for sell taker",
    );
    println!("  ✓ Trade accessors correct.");

    // --- Phase 4: TradeList roundtrip ---
    println!("[Phase 4] TradeList Display/FromStr roundtrip...");
    let mut trade_list = TradeList::new();
    trade_list.add(trade);
    trade_list.add(trade_buy);

    assert_eq_or_exit(trade_list.len(), 2, "TradeList len");
    assert_or_exit(!trade_list.is_empty(), "TradeList should not be empty");

    let tl_str = trade_list.to_string();
    let tl_parsed = TradeList::from_str(&tl_str)
        .unwrap_or_else(|e| exit_err(&format!("TradeList::from_str: {e}")));
    assert_eq_or_exit(tl_parsed.len(), 2, "TradeList roundtrip len");

    // Verify first trade preserved
    let first = &tl_parsed.as_vec()[0];
    assert_eq_or_exit(
        first.price(),
        Price::new(9500),
        "first trade price after roundtrip",
    );
    println!("  ✓ TradeList Display/FromStr roundtrip correct.");

    // --- Phase 5: MatchResult from real matching ---
    println!("[Phase 5] MatchResult from real matching...");
    let price_level = PriceLevel::new(10_000);
    for i in 1..=5_u64 {
        price_level.add_order(OrderType::Standard {
            id: Id::from_u64(i),
            price: Price::new(10_000),
            quantity: Quantity::new(20),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + i),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        });
    }

    let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .unwrap_or_else(|e| exit_err(&format!("uuid: {e}")));
    let id_gen = UuidGenerator::new(namespace);
    let result: MatchResult = price_level.match_order(50, Id::from_u64(999), &id_gen);

    assert_eq_or_exit(
        result.executed_quantity().unwrap_or(0),
        50,
        "executed_quantity",
    );
    assert_or_exit(result.is_complete(), "match should be complete");
    assert_or_exit(
        !result.filled_order_ids().is_empty(),
        "should have filled orders",
    );

    // --- Phase 6: MatchResult Display/FromStr roundtrip ---
    println!("[Phase 6] MatchResult Display/FromStr roundtrip...");
    let mr_str = result.to_string();
    assert_or_exit(
        mr_str.contains("MatchResult:"),
        "MatchResult display prefix",
    );
    let mr_parsed = MatchResult::from_str(&mr_str)
        .unwrap_or_else(|e| exit_err(&format!("MatchResult::from_str: {e}")));
    assert_eq_or_exit(
        mr_parsed.order_id(),
        result.order_id(),
        "MatchResult order_id roundtrip",
    );
    assert_eq_or_exit(
        mr_parsed.remaining_quantity(),
        result.remaining_quantity(),
        "MatchResult remaining_quantity roundtrip",
    );
    assert_eq_or_exit(
        mr_parsed.is_complete(),
        result.is_complete(),
        "MatchResult is_complete roundtrip",
    );
    assert_eq_or_exit(
        mr_parsed.trades().len(),
        result.trades().len(),
        "MatchResult trades len roundtrip",
    );
    println!("  ✓ MatchResult Display/FromStr roundtrip correct.");

    // --- Phase 7: MatchResult serde JSON roundtrip ---
    println!("[Phase 7] MatchResult serde JSON roundtrip...");
    let mr_json = serde_json::to_string(&result)
        .unwrap_or_else(|e| exit_err(&format!("MatchResult serialize: {e}")));
    let mr_deser: MatchResult = serde_json::from_str(&mr_json)
        .unwrap_or_else(|e| exit_err(&format!("MatchResult deserialize: {e}")));
    assert_eq_or_exit(
        mr_deser.order_id(),
        result.order_id(),
        "MatchResult serde order_id",
    );
    assert_eq_or_exit(
        mr_deser.trades().len(),
        result.trades().len(),
        "MatchResult serde trades len",
    );

    // Verify average_price computation
    let avg = result
        .average_price()
        .unwrap_or_else(|e| exit_err(&format!("average_price: {e}")));
    assert_or_exit(avg.is_some(), "average_price should be Some");
    let avg_val = avg.unwrap_or(0.0);
    assert_or_exit(
        (avg_val - 10_000.0).abs() < 0.01,
        "average_price should be ~10000",
    );
    println!("  ✓ MatchResult serde JSON roundtrip and average_price correct.");

    println!("\n=== Integration: Trade & MatchResult Roundtrip PASSED ===");
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
