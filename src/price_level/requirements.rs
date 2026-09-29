//! Read-only counter headroom and match requirements for a caller's
//! multi-level fill-or-kill pre-flight (issue #218).
//!
//! A single-level fill-or-kill taker is already all-or-nothing inside
//! [`PriceLevel::match_order`]. An order book that sweeps several levels for
//! one fill-or-kill taker cannot get that guarantee from the levels alone: a
//! counter exhausted at level `k` would refuse the match after levels
//! `1..k` committed. [`PriceLevel::counter_headroom`] and
//! [`PriceLevel::match_requirements`] let the caller prove, before the first
//! level mutates, that no per-level counter will refuse any of the matches.
//!
//! The contract is documented on [`PriceLevel::match_requirements`].

use crate::errors::{ExhaustedCounter, PriceLevelError};
#[cfg(doc)]
use crate::price_level::PriceLevel;

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
/// [`PriceLevel::match_requirements`]; see
/// [`PriceLevel::match_requirements`] for the contract under which it is
/// valid.
///
/// Computed for a taker that sweeps (`Standard` or `MarketToLimit`, any time
/// in force). A positive `PostOnly` taker never sweeps and consumes nothing;
/// [`Self::check`] is then conservative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchRequirements {
    incoming_quantity: u64,
    fillable: u64,
    trades: usize,
    replenishes: u64,
    parks: usize,
    stop_error: Option<PriceLevelError>,
}

impl MatchRequirements {
    /// Built by [`PriceLevel::match_requirements`].
    pub(crate) fn new(
        incoming_quantity: u64,
        fillable: u64,
        trades: usize,
        replenishes: u64,
        parks: usize,
        stop_error: Option<PriceLevelError>,
    ) -> Self {
        Self {
            incoming_quantity,
            fillable,
            trades,
            replenishes,
            parks,
            stop_error,
        }
    }

    /// The incoming quantity the requirement was computed for.
    #[must_use]
    #[inline]
    pub fn incoming_quantity(&self) -> u64 {
        self.incoming_quantity
    }

    /// Quantity the sweep would fill (what
    /// [`PriceLevel::matchable_quantity`] returns). A fill-or-kill taker is
    /// filled in full only if this equals [`Self::incoming_quantity`].
    #[must_use]
    #[inline]
    pub fn fillable(&self) -> u64 {
        self.fillable
    }

    /// Trades the sweep would emit: an exact reservation for the result and
    /// the trade ids it takes from the shared generator.
    #[must_use]
    #[inline]
    pub fn trades(&self) -> usize {
        self.trades
    }

    /// Iceberg / reserve replenishments the sweep would perform, each taking
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
    /// under the same exclusion (see the contract on
    /// [`PriceLevel::match_requirements`]).
    ///
    /// `Ok` means the match does not fail with
    /// [`PriceLevelError::CounterExhausted`]. It says nothing about the
    /// refusals listed as not covered (poisoning, allocation, shared trade
    /// ids, [`Self::stop_error`]).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CounterExhausted`] with
    /// [`ExhaustedCounter::TopologyEpoch`] or
    /// [`ExhaustedCounter::MutationEpoch`] when an epoch is closed and the
    /// incoming quantity is positive (a zero-quantity match never sweeps),
    /// or with [`ExhaustedCounter::QueueSequence`] when
    /// [`Self::replenishes`] exceeds [`CounterHeadroom::queue_sequence`].
    pub fn check(&self, headroom: &CounterHeadroom) -> Result<(), PriceLevelError> {
        if self.incoming_quantity > 0
            && let Some(counter) = headroom.closed_epoch
        {
            return Err(PriceLevelError::counter_exhausted(counter));
        }
        if self.replenishes > headroom.queue_sequence {
            return Err(PriceLevelError::counter_exhausted(
                ExhaustedCounter::QueueSequence,
            ));
        }
        Ok(())
    }
}
