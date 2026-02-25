//! Execution module: trade generation, result tracking, and trade list management.
//!
//! This module contains the types produced by the matching engine when an incoming
//! order is matched against resting orders in a [`crate::PriceLevel`].
//!
//! # Key Types
//!
//! - [`Trade`] — a single completed trade between a taker and a maker order.
//! - [`TradeList`] — an append-only, ordered collection of trades.
//! - [`MatchResult`] — the full outcome of a matching operation, including trades,
//!   remaining quantity, completion status, and filled order IDs.
//!
//! # Checked Arithmetic
//!
//! Aggregate methods on [`MatchResult`] ([`executed_quantity`](MatchResult::executed_quantity),
//! [`executed_value`](MatchResult::executed_value),
//! [`average_price`](MatchResult::average_price)) use checked arithmetic and return
//! `Result<T, PriceLevelError>` to prevent silent overflow.
//!
//! # Serialization
//!
//! All types implement `Display`, `FromStr`, `Serialize`, and `Deserialize` for
//! text and JSON roundtrip support.

mod trade;

mod list;
mod match_result;
mod tests;

pub use list::TradeList;
pub use match_result::MatchResult;
pub use trade::Trade;
