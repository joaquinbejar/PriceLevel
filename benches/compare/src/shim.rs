//! The complete set of API differences `workloads.rs` needs to paper over
//! between pricelevel 0.9.2 and 0.10, so a workload does IDENTICAL work on
//! both builds. Each function below has an `old` arm and a `new` arm; the
//! two arms produce the same observable state, differing only in how many
//! `Result`s the 0.10 API makes the caller handle. `workloads.rs` never
//! matches on `cfg(feature = ...)` itself — it only calls into this module.
//!
//! Everything NOT listed here (`PriceLevel::new/add_order/match_order/
//! update_order`, the `OrderType` struct-variant shape, `OrderUpdate`,
//! `Trade::with_timestamp`, `MatchResult::new`/`add_trade`,
//! `TradeList::from_str`/`Display`, `Price::new`/`Quantity::new`/
//! `TimestampMs::new`, `Id::sequential`/`from_u64`,
//! `UuidGenerator::new`, `snapshot_to_json`/`snapshot_package`/
//! `from_snapshot_json`) is byte-for-byte identical between the two
//! versions and is called directly from `workloads.rs` with no shim.

use crate::pl;

/// `PriceLevel::snapshot()`: infallible in 0.9.2, `Result` in 0.10 (issue
/// #162 — a same-side quantity transfer between shards during the shard
/// walk could otherwise overflow a `u64` aggregate). `.expect` here is
/// bench-only code (never part of the published crate); a failure would be
/// a harness bug, not a measured outcome.
#[cfg(feature = "old")]
pub fn take_snapshot(level: &pl::PriceLevel) -> pl::PriceLevelSnapshot {
    level.snapshot()
}
#[cfg(feature = "new")]
pub fn take_snapshot(level: &pl::PriceLevel) -> pl::PriceLevelSnapshot {
    level
        .snapshot()
        .expect("snapshot capture failed in harness")
}

/// `MatchResult::with_capacity` (0.9.2, panics on capacity overflow) vs
/// `MatchResult::try_with_capacity` (0.10, issue #170, `Result`). Used only
/// in untimed setup (`iter_batched` fixture construction), never inside a
/// measured closure.
#[cfg(feature = "old")]
pub fn match_result_with_capacity(
    order_id: pl::Id,
    initial_quantity: pl::Quantity,
    capacity: usize,
) -> pl::MatchResult {
    pl::MatchResult::with_capacity(order_id, initial_quantity, capacity)
}
#[cfg(feature = "new")]
pub fn match_result_with_capacity(
    order_id: pl::Id,
    initial_quantity: pl::Quantity,
    capacity: usize,
) -> pl::MatchResult {
    pl::MatchResult::try_with_capacity(order_id, initial_quantity, capacity)
        .expect("MatchResult capacity reservation failed in harness")
}

/// `TradeList::with_capacity` (0.9.2, panics) vs `TradeList::try_with_capacity`
/// (0.10, issue #170, `Result`). Untimed setup only.
#[cfg(feature = "old")]
pub fn trade_list_with_capacity(capacity: usize) -> pl::TradeList {
    pl::TradeList::with_capacity(capacity)
}
#[cfg(feature = "new")]
pub fn trade_list_with_capacity(capacity: usize) -> pl::TradeList {
    pl::TradeList::try_with_capacity(capacity)
        .expect("TradeList capacity reservation failed in harness")
}

/// `TradeList::add`: infallible `()` in 0.9.2, `Result` in 0.10 (issue
/// #170). Both arms perform the identical mutation.
#[cfg(feature = "old")]
pub fn trade_list_add(list: &mut pl::TradeList, trade: pl::Trade) {
    list.add(trade);
}
#[cfg(feature = "new")]
pub fn trade_list_add(list: &mut pl::TradeList, trade: pl::Trade) {
    list.add(trade).expect("TradeList::add failed in harness");
}

/// `MatchResult::add_trade`: `Result<(), PriceLevelError>` on both versions
/// already (no shim needed for the signature) — listed here only so this
/// module documents every construction difference in one place; callers use
/// `pl::MatchResult::add_trade` directly.
///
/// `MatchResult::add_filled_order_id`: infallible `()` in 0.9.2, `Result` in
/// 0.10 (issue #170). Both arms perform the identical mutation.
#[cfg(feature = "old")]
pub fn add_filled_id(result: &mut pl::MatchResult, id: pl::Id) {
    result.add_filled_order_id(id);
}
#[cfg(feature = "new")]
pub fn add_filled_id(result: &mut pl::MatchResult, id: pl::Id) {
    result
        .add_filled_order_id(id)
        .expect("add_filled_order_id failed in harness");
}
