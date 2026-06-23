//! Taker intent classification for the matching engine.
//!
//! The kind of an incoming ("taker") order is orthogonal to its
//! [`TimeInForce`](crate::orders::TimeInForce): the TIF governs *how much* of
//! the remainder survives a match (fill-or-kill, immediate-or-cancel, rest),
//! while the [`TakerKind`] governs *whether the taker may take liquidity at
//! all* and how an unfilled remainder is interpreted by the order book. Both
//! are passed into [`PriceLevel::match_order`](crate::PriceLevel::match_order)
//! so the single-level match honors the taker's full intent.

use serde::{Deserialize, Serialize};

/// Classifies the intent of an incoming (taker) order at a single price level.
///
/// This is the taker-side discriminator that
/// [`PriceLevel::match_order`](crate::PriceLevel::match_order) consults
/// alongside the taker's [`TimeInForce`](crate::orders::TimeInForce). It is
/// deliberately distinct from the resting-maker [`OrderType`](crate::OrderType):
/// it expresses what the *incoming* order is allowed to do, not how a resting
/// order behaves.
///
/// - [`TakerKind::Standard`] — an ordinary aggressing order. Subject only to
///   its TIF.
/// - [`TakerKind::PostOnly`] — must never take liquidity. If matching at this
///   level would cross (any quantity is available to fill), the match is
///   rejected with zero trades and the queue left untouched.
/// - [`TakerKind::MarketToLimit`] — fills the available quantity and reports
///   the remainder; the order book converts/rests the residual as a limit. At
///   the single-level layer this fills like a [`TakerKind::Standard`] taker.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TakerKind {
    /// An ordinary aggressing taker. Subject only to its time-in-force.
    #[default]
    Standard,

    /// A post-only taker that must never take liquidity. Rejected on any cross.
    PostOnly,

    /// A market-to-limit taker. Fills what it can; the order book converts the
    /// unfilled remainder into a resting limit order.
    MarketToLimit,
}

impl TakerKind {
    /// Returns `true` if this taker is post-only and therefore must never take
    /// liquidity.
    #[must_use]
    #[inline]
    pub fn is_post_only(self) -> bool {
        matches!(self, Self::PostOnly)
    }

    /// Returns `true` if this taker is market-to-limit.
    #[must_use]
    #[inline]
    pub fn is_market_to_limit(self) -> bool {
        matches!(self, Self::MarketToLimit)
    }
}
