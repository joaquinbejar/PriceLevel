//! Valid only while the caller excludes every mutator of every involved level
//! for the whole interval from the query through the last `match_order`: the
//! level cannot hold its guard across calls, so that exclusion is the
//! caller's alone.
//!
//! Read-only counter headroom and match requirements for a caller's
//! multi-level fill-or-kill pre-flight (issue #218). A single-level
//! fill-or-kill taker is already all-or-nothing inside
//! [`PriceLevel::match_order`]; an order book sweeping several levels for one
//! fill-or-kill taker uses [`PriceLevel::counter_headroom`] and
//! [`PriceLevel::match_requirements`] to prove, before the first level
//! mutates, that no per-level counter will refuse any of the matches. The
//! full contract, the multi-level usage and what is not covered are
//! documented on [`PriceLevel::match_requirements`].
//!
//! Quantities ([`MatchRequirements::fillable`],
//! [`MatchRequirements::incoming_quantity`]) are raw `u64`, like
//! [`PriceLevel::matchable_quantity`] and the `incoming_quantity` argument of
//! [`PriceLevel::match_order`], so the three compare without conversion.

use crate::errors::{ExhaustedCounter, PriceLevelError};
#[cfg(doc)]
use crate::price_level::PriceLevel;
use crate::price_level::level::DryRun;

/// Headroom of the per-level counters that can refuse a match (issue #218).
/// Returned by [`PriceLevel::counter_headroom`]; see
/// [`PriceLevel::match_requirements`] for the contract under which it is
/// valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterHeadroom {
    queue_sequence: u64,
    closed_epoch: Option<ExhaustedCounter>,
}

impl CounterHeadroom {
    /// Built by [`PriceLevel::counter_headroom`].
    pub(crate) fn new(queue_sequence: u64, closed_epoch: Option<ExhaustedCounter>) -> Self {
        Self {
            queue_sequence,
            closed_epoch,
        }
    }

    /// FIFO sequences that can still be reserved: the number of iceberg /
    /// reserve replenishments the level can still perform.
    #[must_use]
    #[inline]
    pub fn queue_sequence(&self) -> u64 {
        self.queue_sequence
    }

    /// `true` when both epochs are below their limit, the same test
    /// [`PriceLevel::match_order`] applies before a sweep.
    #[must_use]
    #[inline]
    pub fn epochs_open(&self) -> bool {
        self.closed_epoch.is_none()
    }

    /// The epoch that has reached its limit, if any
    /// ([`ExhaustedCounter::TopologyEpoch`] is reported first, as
    /// [`PriceLevel::match_order`] does).
    #[must_use]
    #[inline]
    pub fn closed_epoch(&self) -> Option<ExhaustedCounter> {
        self.closed_epoch
    }
}

/// What one [`PriceLevel::match_order`] call would consume at this level, from
/// a read-only dry run (issue #218). Returned by
/// [`PriceLevel::match_requirements`]; see there for the contract under which
/// it is valid, the multi-level usage and what it does not cover.
///
/// Describes a sweeping taker (`Standard`, or `MarketToLimit`, which matches
/// like it at this level). It is not meaningful for a `PostOnly` taker, which
/// never sweeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchRequirements {
    incoming_quantity: u64,
    self_match_rejected: bool,
    fillable: u64,
    trades: usize,
    replenishes: u64,
    parks: usize,
    stop_error: Option<PriceLevelError>,
    abort_reserves_sequence: bool,
    abort_reserves_trade_id: bool,
    replenish_overflow_stop: bool,
}

impl MatchRequirements {
    /// The requirement of a positive taker whose id rests at the level:
    /// [`PriceLevel::match_order`] rejects it whole, so nothing is consumed.
    pub(crate) fn self_match(incoming_quantity: u64) -> Self {
        Self {
            incoming_quantity,
            self_match_rejected: true,
            fillable: 0,
            trades: 0,
            replenishes: 0,
            parks: 0,
            stop_error: None,
            abort_reserves_sequence: false,
            abort_reserves_trade_id: false,
            replenish_overflow_stop: false,
        }
    }

    /// The requirement a dry run predicts.
    pub(crate) fn from_dry_run(incoming_quantity: u64, dry: DryRun) -> Self {
        Self {
            incoming_quantity,
            self_match_rejected: false,
            fillable: dry.filled,
            trades: dry.trades,
            replenishes: dry.replenishes,
            parks: dry.parks,
            stop_error: dry.error,
            abort_reserves_sequence: dry.abort_reserves_sequence,
            abort_reserves_trade_id: dry.abort_reserves_trade_id,
            replenish_overflow_stop: dry.replenish_overflow_stop,
        }
    }

    /// The incoming quantity the requirement was computed for.
    #[must_use]
    #[inline]
    pub fn incoming_quantity(&self) -> u64 {
        self.incoming_quantity
    }

    /// `true` when the taker's id already rests at the level: a positive
    /// [`PriceLevel::match_order`] then rejects the whole taker (no trades,
    /// whatever the time in force; issue #126), and every count here is zero.
    #[must_use]
    #[inline]
    pub fn self_match_rejected(&self) -> bool {
        self.self_match_rejected
    }

    /// Quantity the sweep would fill: what
    /// [`PriceLevel::matchable_quantity`] returns, or zero for a self-match
    /// rejection.
    #[must_use]
    #[inline]
    pub fn fillable(&self) -> u64 {
        self.fillable
    }

    /// Trades the sweep would emit: an exact reservation for the result.
    /// For the ids it takes from the shared trade-id generator, use
    /// [`Self::trade_ids_required`], which can be one more.
    #[must_use]
    #[inline]
    pub fn trades(&self) -> usize {
        self.trades
    }

    /// Iceberg / reserve replenishments the sweep would commit, each taking
    /// one fresh FIFO sequence. Not bounded by [`Self::trades`].
    #[must_use]
    #[inline]
    pub fn replenishes(&self) -> u64 {
        self.replenishes
    }

    /// Makers the sweep would set aside (self-trade skip or no-progress
    /// guard).
    #[must_use]
    #[inline]
    pub fn parks(&self) -> usize {
        self.parks
    }

    /// `true` when the sweep would stop at a replenish whose visible net
    /// change overflows the level's visible counter (the maker is set aside
    /// untouched and the sweep ends without an error, so the taker is not
    /// filled in full). Before aborting, that step reserves the maker's FIFO
    /// sequence, which [`Self::check`] requires on top of
    /// [`Self::replenishes`], and, when it would have traded, one trade id,
    /// which [`Self::trade_ids_required`] counts on top of [`Self::trades`].
    #[must_use]
    #[inline]
    pub fn stops_at_replenish_overflow(&self) -> bool {
        self.replenish_overflow_stop
    }

    /// Trade ids the sweep takes from the shared generator: one per trade,
    /// plus the id a replenish-overflow stop reserves and skips when that
    /// step would have traded. Compare the sum over every level of a sweep
    /// with [`UuidGenerator::remaining`](crate::UuidGenerator::remaining).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] if the count does not fit
    /// `u64` (unreachable in practice: every trade fills at least one unit
    /// of a `u64` quantity).
    pub fn trade_ids_required(&self) -> Result<u64, PriceLevelError> {
        u64::try_from(self.trades)
            .ok()
            .and_then(|trades| trades.checked_add(u64::from(self.abort_reserves_trade_id)))
            .ok_or_else(trade_ids_overflow)
    }

    /// The error of the maker step the sweep would stop at, if any (matching
    /// arithmetic or the resting-order count). [`Self::fillable`] and
    /// [`Self::trades`] are then the prefix it would commit, and a
    /// fill-or-kill taker would be killed.
    #[must_use]
    #[inline]
    pub fn stop_error(&self) -> Option<&PriceLevelError> {
        self.stop_error.as_ref()
    }

    /// Checks the requirement against `headroom` taken from the same level
    /// under the same exclusion (see [`PriceLevel::match_requirements`]).
    ///
    /// `Ok` only rules out [`PriceLevelError::CounterExhausted`]: it does
    /// **not** mean the taker fills. Use [`Self::fills_completely`] for that.
    /// A self-match rejection and a zero-quantity match consume nothing and
    /// always pass.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CounterExhausted`] with
    /// [`ExhaustedCounter::TopologyEpoch`] or
    /// [`ExhaustedCounter::MutationEpoch`] when an epoch is closed, or with
    /// [`ExhaustedCounter::QueueSequence`] when the sequences the sweep
    /// reserves exceed [`CounterHeadroom::queue_sequence`].
    pub fn check(&self, headroom: &CounterHeadroom) -> Result<(), PriceLevelError> {
        if self.self_match_rejected || self.incoming_quantity == 0 {
            return Ok(());
        }
        if let Some(counter) = headroom.closed_epoch {
            return Err(PriceLevelError::counter_exhausted(counter));
        }
        // Sequences reserved: every committed replenishment, plus one for a
        // step that reserves its sequence and then aborts. `>=` expresses
        // `replenishes + 1 > headroom` without the addition.
        let short = if self.abort_reserves_sequence {
            self.replenishes >= headroom.queue_sequence
        } else {
            self.replenishes > headroom.queue_sequence
        };
        if short {
            return Err(PriceLevelError::counter_exhausted(
                ExhaustedCounter::QueueSequence,
            ));
        }
        Ok(())
    }

    /// `true` when the match would fill the whole incoming quantity at this
    /// level: no self-match rejection, no [`Self::stop_error`], and
    /// [`Self::fillable`] equal to [`Self::incoming_quantity`], with the
    /// counters checked by [`Self::check`].
    ///
    /// # Errors
    ///
    /// The [`PriceLevelError::CounterExhausted`] of [`Self::check`].
    pub fn fills_completely(&self, headroom: &CounterHeadroom) -> Result<bool, PriceLevelError> {
        self.check(headroom)?;
        Ok(!self.self_match_rejected
            && self.stop_error.is_none()
            && self.fillable == self.incoming_quantity)
    }
}

/// The error of [`MatchRequirements::trade_ids_required`] when the count does
/// not fit `u64`.
#[cold]
fn trade_ids_overflow() -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: "trade id requirement overflows u64".to_string(),
    }
}
