use crate::errors::{CapacityResource, PriceLevelError};
use crate::execution::list::TradeList;
use crate::execution::trade::Trade;
use crate::orders::Id;
use crate::utils::Quantity;
use crate::utils::text::{MAX_TEXT_NESTING_DEPTH, NestingError, matching_close, try_push};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The terminal classification of a single-level matching operation.
///
/// This is the explicit signal a caller uses to tell apart outcomes that all
/// look identical through `trades` / `remaining_quantity` alone — in
/// particular, a fill-or-kill *kill* and a post-only *rejection* both leave
/// zero trades and the full incoming quantity remaining, exactly like matching
/// against an empty level, yet they mean very different things.
///
/// The outcome agrees with the rest of [`MatchResult`] by construction:
/// `is_complete()` is `true` iff the outcome is [`MatchOutcome::Filled`], and
/// [`MatchOutcome::Killed`] / [`MatchOutcome::Rejected`] are only ever set when
/// no trade was emitted and the resting queue was left untouched.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MatchOutcome {
    /// The incoming order was completely filled (`remaining_quantity == 0`).
    Filled,

    /// The incoming order was partially filled: at least one trade occurred but
    /// some quantity remains. For a `Gtc` / `Gtd` / `Day` taker the order book
    /// rests the remainder; for an `Ioc` / market-to-limit taker it is
    /// discarded / converted by the caller.
    PartiallyFilled,

    /// No trade occurred and quantity remains because the level had nothing to
    /// fill the taker with (empty or fully consumed by an earlier sweep). This
    /// is the benign "no liquidity here" outcome — distinct from a kill or a
    /// rejection.
    #[default]
    NotFilled,

    /// A fill-or-kill (`Fok`) taker could not be filled in full at this level,
    /// so it was killed: zero trades, full remaining quantity, resting queue
    /// left untouched.
    Killed,

    /// A post-only taker would have taken liquidity (the level could fill some
    /// of it), so it was rejected: zero trades, full remaining quantity,
    /// resting queue left untouched.
    Rejected,
}

impl MatchOutcome {
    /// Returns `true` if the taker was killed by its fill-or-kill policy.
    #[must_use]
    #[inline]
    pub fn was_killed(self) -> bool {
        matches!(self, Self::Killed)
    }

    /// Returns `true` if the taker was rejected by its post-only policy.
    #[must_use]
    #[inline]
    pub fn was_rejected(self) -> bool {
        matches!(self, Self::Rejected)
    }
}

/// Represents the result of a matching operation.
///
/// Fields are private to enforce invariant consistency between
/// `remaining_quantity`, `is_complete`, and `trades`.
/// Use the provided accessor methods and mutation helpers.
///
/// # Failure contract (#164)
///
/// A match can stop early because a fallible step failed (result growth,
/// trade-id reservation, counter exhaustion, an arithmetic or invariant
/// violation). [`PriceLevel::match_order`](crate::PriceLevel::match_order)
/// still returns a `MatchResult` in that case (never a bare error that would
/// lose fills) and [`Self::error`] carries the typed failure:
///
/// - `error()` is `None` for a match that ran to its natural end.
/// - When `error()` is `Some`, every trade the level committed before the
///   failure is in [`Self::trades`] (in FIFO order), every maker those trades
///   fully consumed is in [`Self::filled_order_ids`], and
///   [`Self::remaining_quantity`] is the taker's true residual. The level's
///   queue and counters agree with exactly those trades. [`Self::outcome`]
///   still classifies the fills (`Filled` / `PartiallyFilled` / `NotFilled`).
/// - A fill-or-kill taker fails before any maker is touched: outcome
///   [`MatchOutcome::Killed`], no trades, full remaining, level unchanged, and
///   the error set.
///
/// Only the first failure is kept (it is the root cause; the sweep stops at
/// it). Resource failures use the allocation-free
/// [`PriceLevelError::CapacityExceeded`].
///
/// # Decode-time validation
///
/// The Rust API keeps the fields mutually consistent, but a decoder writes
/// them directly. Both `Deserialize` (via `#[serde(try_from)]` through a
/// private wire struct) and [`FromStr`] therefore route the reconstructed
/// value through a single private validator, which rejects any payload that
/// breaks the invariants a public-API-built result always upholds. Every
/// payload a valid `MatchResult` can produce still decodes unchanged; only
/// self-contradictory input is rejected — as a serde error or a
/// [`PriceLevelError`], never a panic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "MatchResultWire")]
pub struct MatchResult {
    /// The ID of the incoming order that initiated the match
    order_id: Id,

    /// List of trades that resulted from the match
    trades: TradeList,

    /// Remaining quantity of the incoming order after matching
    remaining_quantity: u64,

    /// Whether the order was completely filled
    is_complete: bool,

    /// Any orders that were completely filled and removed from the book
    filled_order_ids: Vec<Id>,

    /// Terminal classification of the match (filled / killed / rejected / ...).
    ///
    /// Serialized as `Some(outcome)` so the emitted shape is symmetric with
    /// [`MatchResultWire`]'s `Option<MatchOutcome>`. JSON is unaffected —
    /// serde flattens `Some` away, so the payload still carries the bare
    /// value — but a non-self-describing format (bincode) decodes
    /// positionally and must find the option tag the wire struct reads
    /// back (#135).
    #[serde(serialize_with = "serialize_outcome_as_some")]
    outcome: MatchOutcome,

    /// The failure that stopped the match early, if any (#164 contract; see
    /// the type-level docs). Serialized as an `Option` on both the emit side
    /// and [`MatchResultWire`], so JSON and positional (bincode) encodings are
    /// symmetric; a JSON payload written before the field existed decodes as
    /// `None`.
    error: Option<PriceLevelError>,
}

/// Serializes `outcome` wrapped in `Some` — see the field doc on
/// [`MatchResult::outcome`] (#135). Kept as a free function because serde's
/// `serialize_with` requires this exact signature.
fn serialize_outcome_as_some<S>(outcome: &MatchOutcome, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_some(outcome)
}

/// Wire form of [`MatchResult`] used for validated deserialization.
///
/// It mirrors `MatchResult`'s serialized shape field-for-field (identical
/// names, so every previously-valid payload still decodes) but performs no
/// validation itself: `#[serde(try_from = "MatchResultWire")]` on
/// `MatchResult` deserializes this permissive struct and then runs
/// [`MatchResult::validated`] via the [`TryFrom`] impl below.
#[derive(Deserialize)]
struct MatchResultWire {
    order_id: Id,
    trades: TradeList,
    remaining_quantity: u64,
    is_complete: bool,
    filled_order_ids: Vec<Id>,
    /// Decoded as an `Option` so a legacy payload written before the field
    /// existed (key absent → `None`) is told apart from an explicit outcome.
    /// An absent outcome is *derived* from the other fields — the legacy
    /// behavior — while an explicit one is validated against them, so a
    /// payload can no longer claim `Filled` with a positive remainder or
    /// `PartiallyFilled` without trades.
    #[serde(default)]
    outcome: Option<MatchOutcome>,
    /// Absent in payloads written before the #164 contract: defaults to
    /// `None` ("no error").
    #[serde(default)]
    error: Option<PriceLevelError>,
}

impl TryFrom<MatchResultWire> for MatchResult {
    type Error = PriceLevelError;

    fn try_from(wire: MatchResultWire) -> Result<Self, Self::Error> {
        let outcome = wire.outcome.unwrap_or({
            // Legacy payload (no outcome key): re-derive the benign
            // classification exactly as the pre-outcome accessors did. A
            // killed / rejected outcome cannot be recovered — it is
            // indistinguishable from NotFilled once the trades are gone.
            if wire.remaining_quantity == 0 {
                MatchOutcome::Filled
            } else if wire.trades.is_empty() {
                MatchOutcome::NotFilled
            } else {
                MatchOutcome::PartiallyFilled
            }
        });
        MatchResult {
            order_id: wire.order_id,
            trades: wire.trades,
            remaining_quantity: wire.remaining_quantity,
            is_complete: wire.is_complete,
            filled_order_ids: wire.filled_order_ids,
            outcome,
            error: wire.error,
        }
        .validated()
    }
}

impl MatchResult {
    /// Create a new empty match result for an incoming taker of
    /// `initial_quantity` quantity units.
    #[must_use]
    pub fn new(order_id: Id, initial_quantity: Quantity) -> Self {
        // A zero-quantity result is vacuously complete (nothing to fill), so keep
        // is_complete / outcome consistent at construction — matching
        // `finalize`'s `remaining == 0 => Filled` rule. A non-zero result starts
        // incomplete / NotFilled until a trade or `finalize` updates it.
        let is_complete = initial_quantity.as_u64() == 0;
        Self {
            order_id,
            trades: TradeList::new(),
            remaining_quantity: initial_quantity.as_u64(),
            is_complete,
            filled_order_ids: Vec::new(),
            outcome: if is_complete {
                MatchOutcome::Filled
            } else {
                MatchOutcome::NotFilled
            },
            error: None,
        }
    }

    /// Create a new empty match result with the `trades` and `filled_order_ids`
    /// vectors pre-sized for up to `capacity` entries.
    ///
    /// A single match sweep at one price level produces at most one trade and
    /// at most one filled order id per maker step, so a good `capacity` is the
    /// tighter of the taker's incoming quantity and the level's resting order
    /// count (see `PriceLevel::match_order`). Pre-sizing both vectors removes
    /// the per-fill reallocations on the match hot path without over-reserving
    /// for a small taker against a deep level.
    ///
    /// Allocation is fallible (`Vec::new` + `try_reserve_exact`): an
    /// unrepresentable `capacity` such as `usize::MAX` or an allocator refusal
    /// returns a typed error instead of panicking. `capacity == 0` never
    /// allocates.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::Trades`] or [`CapacityResource::FilledOrderIds`]) if
    /// either vector cannot be reserved.
    pub fn try_with_capacity(
        order_id: Id,
        initial_quantity: Quantity,
        capacity: usize,
    ) -> Result<Self, PriceLevelError> {
        let mut result = Self::new(order_id, initial_quantity);
        result.trades = TradeList::try_with_capacity(capacity)?;
        result
            .filled_order_ids
            .try_reserve_exact(capacity)
            .map_err(|_| {
                PriceLevelError::capacity_exceeded(CapacityResource::FilledOrderIds, capacity)
            })?;
        Ok(result)
    }

    /// Reserves room for at least `additional` more trades AND `additional`
    /// more filled order ids (amortized growth).
    ///
    /// Never allocates when the spare capacity already suffices. The matching
    /// engine calls this before committing each maker step, so the step's
    /// [`Self::add_trade`] / [`Self::add_filled_order_id`] cannot then fail on
    /// growth.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] if either vector cannot
    /// grow. Only capacity may have changed; every observable field (trades,
    /// filled ids, remaining quantity, completion, outcome, error) is left
    /// unchanged.
    #[inline]
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), PriceLevelError> {
        self.trades.try_reserve(additional)?;
        self.try_reserve_filled(additional)
    }

    /// Exact-growth variant of [`Self::try_reserve`], used by the fill-or-kill
    /// preflight that knows the exact number of steps its sweep will take.
    pub(crate) fn try_reserve_exact(&mut self, additional: usize) -> Result<(), PriceLevelError> {
        self.trades.try_reserve_exact(additional)?;
        self.filled_order_ids
            .try_reserve_exact(additional)
            .map_err(|_| {
                PriceLevelError::capacity_exceeded(CapacityResource::FilledOrderIds, additional)
            })
    }

    /// `true` when one more trade AND one more filled id fit without growing.
    /// The matching engine's per-step fast-path check.
    #[inline]
    pub(crate) fn has_step_capacity(&self) -> bool {
        // Under an armed test limiter every step must go through the
        // (limited) reservation so the cap is exact, whatever spare capacity
        // amortized growth happened to leave.
        #[cfg(test)]
        if crate::execution::list::test_seam::armed() {
            return false;
        }
        self.trades.len() < self.trades.capacity()
            && self.filled_order_ids.len() < self.filled_order_ids.capacity()
    }

    /// Reserves `additional` filled-order-id slots (amortized growth).
    #[inline]
    fn try_reserve_filled(&mut self, additional: usize) -> Result<(), PriceLevelError> {
        self.filled_order_ids.try_reserve(additional).map_err(|_| {
            PriceLevelError::capacity_exceeded(CapacityResource::FilledOrderIds, additional)
        })
    }

    /// Add a trade to this match result.
    ///
    /// All validation and the storage reservation happen BEFORE any field is
    /// changed, so on `Err` the result is exactly as it was: trades, filled
    /// ids, remaining quantity, completion and outcome are untouched.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the trade's quantity
    /// exceeds the remaining quantity of the incoming order (the subtraction
    /// would underflow), which indicates an over-fill bug in the caller, or if
    /// the trade's taker order id differs from this result's incoming order id
    /// (a trade can only belong to the taker that initiated the match).
    /// Returns [`PriceLevelError::CapacityExceeded`] if the trade list cannot
    /// grow.
    pub fn add_trade(&mut self, trade: Trade) -> Result<(), PriceLevelError> {
        #[cfg(test)]
        test_seam::check_add_trade()?;
        if trade.taker_order_id() != self.order_id {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "trade taker order id {} does not match the result's incoming order id {}",
                    trade.taker_order_id(),
                    self.order_id
                ),
            });
        }
        let remaining_quantity = self
            .remaining_quantity
            .checked_sub(trade.quantity().as_u64())
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: format!(
                    "trade quantity {} exceeds remaining quantity {}",
                    trade.quantity().as_u64(),
                    self.remaining_quantity
                ),
            })?;
        // Reserve before committing anything (#170): a failed growth must not
        // leave remaining / completion / outcome describing a trade the list
        // does not hold.
        self.trades.try_reserve(1)?;

        // Commit: nothing below can fail.
        self.remaining_quantity = remaining_quantity;
        self.is_complete = self.remaining_quantity == 0;
        // Keep the outcome in lockstep with the fields it summarizes: a trade
        // has now occurred, so the result is at least partially filled.
        // `finalize` re-derives the terminal classification after the sweep.
        self.outcome = if self.is_complete {
            MatchOutcome::Filled
        } else {
            MatchOutcome::PartiallyFilled
        };
        self.trades.push_reserved(trade);
        Ok(())
    }

    /// Add a filled order ID to track orders removed from the book.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] with resource
    /// [`CapacityResource::FilledOrderIds`] if the id vector cannot grow; the
    /// result is left unchanged.
    pub fn add_filled_order_id(&mut self, order_id: Id) -> Result<(), PriceLevelError> {
        self.try_reserve_filled(1)?;
        // Capacity reserved: this push cannot reallocate.
        self.filled_order_ids.push(order_id);
        Ok(())
    }

    /// Returns the failure that stopped this match early, if any.
    ///
    /// `None` means the match ran to its natural end. `Some` means the sweep
    /// stopped at a failed step: the trades, filled ids and remaining quantity
    /// still describe exactly what the level committed before the failure
    /// (see the type-level "Failure contract" section).
    #[must_use]
    #[inline]
    pub fn error(&self) -> Option<&PriceLevelError> {
        self.error.as_ref()
    }

    /// Returns `true` if the match stopped early on a failure
    /// (`self.error().is_some()`).
    #[must_use]
    #[inline]
    pub fn is_failed(&self) -> bool {
        self.error.is_some()
    }

    /// Records the failure that stopped the match. Keeps the FIRST failure
    /// (the root cause); later ones are ignored. Used by the matching engine.
    #[cold]
    pub(crate) fn set_error(&mut self, error: PriceLevelError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    /// Clones the result without an infallible allocation.
    ///
    /// `Clone` is still derived; use this where a typed failure is required.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] if any buffer of the copy
    /// cannot be allocated.
    pub fn try_clone(&self) -> Result<Self, PriceLevelError> {
        let mut filled_order_ids = Vec::new();
        filled_order_ids
            .try_reserve_exact(self.filled_order_ids.len())
            .map_err(|_| {
                PriceLevelError::capacity_exceeded(
                    CapacityResource::FilledOrderIds,
                    self.filled_order_ids.len(),
                )
            })?;
        filled_order_ids.extend_from_slice(&self.filled_order_ids);
        let error = match &self.error {
            Some(error) => Some(error.try_clone()?),
            None => None,
        };
        Ok(Self {
            order_id: self.order_id,
            trades: self.trades.try_clone()?,
            remaining_quantity: self.remaining_quantity,
            is_complete: self.is_complete,
            filled_order_ids,
            outcome: self.outcome,
            error,
        })
    }

    /// Returns the ID of the incoming order that initiated the match.
    #[must_use]
    pub fn order_id(&self) -> Id {
        self.order_id
    }

    /// Returns a reference to the list of trades.
    #[must_use]
    pub fn trades(&self) -> &TradeList {
        &self.trades
    }

    /// Returns the remaining quantity of the incoming order after matching, in
    /// quantity units.
    #[must_use]
    pub fn remaining_quantity(&self) -> Quantity {
        Quantity::new(self.remaining_quantity)
    }

    /// Returns whether the order was completely filled.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    /// Returns the IDs of orders that were completely filled during matching.
    #[must_use]
    pub fn filled_order_ids(&self) -> &[Id] {
        &self.filled_order_ids
    }

    /// Returns the terminal classification of this match.
    ///
    /// See [`MatchOutcome`] for the full set of cases and how they relate to
    /// the other fields. This is the only way to distinguish a fill-or-kill
    /// *kill* and a post-only *rejection* (both zero-trade, full-remainder)
    /// from matching against an empty level.
    #[must_use]
    pub fn outcome(&self) -> MatchOutcome {
        self.outcome
    }

    /// Returns `true` if the taker was killed by its fill-or-kill policy.
    ///
    /// A killed match has zero trades, the full incoming quantity remaining,
    /// and left the resting queue untouched.
    #[must_use]
    pub fn was_killed(&self) -> bool {
        self.outcome.was_killed()
    }

    /// Returns `true` if the taker was rejected by its post-only policy.
    ///
    /// A rejected match has zero trades, the full incoming quantity remaining,
    /// and left the resting queue untouched.
    #[must_use]
    pub fn was_rejected(&self) -> bool {
        self.outcome.was_rejected()
    }

    /// Sets the final remaining quantity, completion flag, and outcome.
    ///
    /// This is used internally by the matching engine after the matching loop
    /// for outcomes that actually swept the queue (filled / partially filled /
    /// no liquidity). Kill and rejection are set by their dedicated helpers
    /// because they are decided *before* any sweep and must not be
    /// re-derived from the (deliberately untouched) fields.
    pub(crate) fn finalize(&mut self, remaining_quantity: Quantity) {
        self.remaining_quantity = remaining_quantity.as_u64();
        self.is_complete = self.remaining_quantity == 0;
        self.outcome = if self.is_complete {
            MatchOutcome::Filled
        } else if self.trades.is_empty() {
            MatchOutcome::NotFilled
        } else {
            MatchOutcome::PartiallyFilled
        };
    }

    /// Marks this result as a fill-or-kill *kill*: the taker could not be
    /// filled in full at this level, so nothing was done.
    ///
    /// Resets `trades` / `filled_order_ids` to empty and `remaining_quantity`
    /// to the full incoming quantity, asserting the "no partial state" rule.
    /// Used internally by the matching engine.
    pub(crate) fn mark_killed(&mut self, incoming_quantity: u64) {
        self.trades = TradeList::new();
        self.filled_order_ids.clear();
        self.remaining_quantity = incoming_quantity;
        self.is_complete = false;
        self.outcome = MatchOutcome::Killed;
    }

    /// Marks this result as a post-only *rejection*: the taker would have taken
    /// liquidity, so nothing was done.
    ///
    /// Resets `trades` / `filled_order_ids` to empty and `remaining_quantity`
    /// to the full incoming quantity. Used internally by the matching engine.
    pub(crate) fn mark_rejected(&mut self, incoming_quantity: u64) {
        self.trades = TradeList::new();
        self.filled_order_ids.clear();
        self.remaining_quantity = incoming_quantity;
        self.is_complete = false;
        self.outcome = MatchOutcome::Rejected;
    }

    /// Get the total executed quantity, in quantity units.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if summing the trade
    /// quantities overflows `u64`.
    pub fn executed_quantity(&self) -> Result<Quantity, PriceLevelError> {
        self.trades
            .as_vec()
            .iter()
            .try_fold(0u64, |acc, trade| {
                acc.checked_add(trade.quantity().as_u64())
                    .ok_or_else(executed_quantity_overflow)
            })
            .map(Quantity::new)
    }

    /// Get the total value executed
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any per-trade
    /// `price * quantity` product overflows `u128`, or if accumulating those
    /// products overflows `u128`.
    pub fn executed_value(&self) -> Result<u128, PriceLevelError> {
        self.trades
            .as_vec()
            .iter()
            .try_fold(0u128, |acc, trade| accumulate_value(acc, trade))
            .map_err(ValueOverflow::into_error)
    }

    /// Calculate the average execution price
    ///
    /// Returns `Ok(None)` when no quantity has been executed (no average price
    /// exists), avoiding a division by zero.
    ///
    /// Quantity and value are accumulated in a single traversal of the trades
    /// (#151), with the same checked arithmetic and the same error precedence
    /// as calling [`Self::executed_quantity`] and then
    /// [`Self::executed_value`]: a quantity overflow is reported first
    /// whatever trade it occurs at, a zero executed quantity yields
    /// `Ok(None)` before any value error can surface, and otherwise the first
    /// value overflow in trade order is reported.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the underlying
    /// [`Self::executed_quantity`] or [`Self::executed_value`] computation
    /// overflows.
    pub fn average_price(&self) -> Result<Option<f64>, PriceLevelError> {
        let mut executed_qty = 0u64;
        let mut executed_value = 0u128;
        for trade in self.trades.as_vec() {
            let quantity = trade.quantity().as_u64();
            let step = executed_qty.checked_add(quantity).zip(
                trade
                    .price()
                    .as_u128()
                    .checked_mul(u128::from(quantity))
                    .and_then(|trade_value| executed_value.checked_add(trade_value)),
            );
            match step {
                Some((qty, value)) => {
                    executed_qty = qty;
                    executed_value = value;
                }
                // Any overflow: rerun the two separate scans, which define
                // the error precedence (quantity first, `Ok(None)` for zero
                // quantity, then the first value overflow in trade order).
                None => return self.average_price_separate(),
            }
        }
        if executed_qty == 0 {
            return Ok(None);
        }
        Ok(Some(executed_value as f64 / executed_qty as f64))
    }

    /// Overflow path of [`Self::average_price`]: the quantity-then-value
    /// sequence whose error behavior the fused traversal reproduces.
    #[cold]
    fn average_price_separate(&self) -> Result<Option<f64>, PriceLevelError> {
        let executed_qty = self.executed_quantity()?.as_u64();
        if executed_qty == 0 {
            return Ok(None);
        }
        Ok(Some(self.executed_value()? as f64 / executed_qty as f64))
    }

    /// Consumes `self`, returning it only if it satisfies the invariants a
    /// public-API-built [`MatchResult`] always upholds — the single validation
    /// gate both decoders ([`FromStr`] and `Deserialize` via
    /// [`MatchResultWire`]) route through, so a decoder can never mint a
    /// self-contradictory value that the private-field Rust API forbids.
    ///
    /// The checks mirror exactly what the constructors / mutators
    /// ([`Self::new`], [`Self::add_trade`], [`Self::finalize`],
    /// [`Self::mark_killed`], [`Self::mark_rejected`]) and the matching engine
    /// guarantee — no stricter:
    ///
    /// 1. **Completion agrees with the remainder:** `is_complete` is `true` iff
    ///    `remaining_quantity == 0`.
    /// 2. **Executed quantity is representable:** the checked sum of the trade
    ///    quantities does not overflow `u64`, and that sum plus the remaining
    ///    quantity (the implied initial taker quantity, itself a `u64`) does not
    ///    overflow either.
    /// 3. **A killed / rejected result carries nothing:** a
    ///    [`MatchOutcome::Killed`] or [`MatchOutcome::Rejected`] outcome — both
    ///    decided before any sweep — has no trades and no filled order ids, as
    ///    [`Self::mark_killed`] / [`Self::mark_rejected`] enforce.
    /// 4. **Filled ids are backed by trades:** every id in `filled_order_ids`
    ///    appears as a `maker_order_id` of some trade, because the engine only
    ///    records a filled id for a maker it just traded against
    ///    (`PriceLevel::match_order`). The reverse does not hold — a partially
    ///    filled maker trades without being recorded as filled — so this is a
    ///    one-directional subset check, not equality.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] describing the first
    /// invariant the value violates.
    fn validated(self) -> Result<Self, PriceLevelError> {
        // 1. Completion agrees with the remainder.
        if self.is_complete != (self.remaining_quantity == 0) {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "is_complete ({}) disagrees with remaining_quantity ({})",
                    self.is_complete, self.remaining_quantity
                ),
            });
        }

        // 2. Executed quantity is representable, and so is the implied initial
        //    taker quantity (executed + remaining). `executed_quantity` already
        //    returns a checked-sum error on overflow.
        let executed = self.executed_quantity()?.as_u64();
        if executed.checked_add(self.remaining_quantity).is_none() {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "executed quantity ({executed}) plus remaining_quantity ({}) overflows u64",
                    self.remaining_quantity
                ),
            });
        }

        // 3. The explicit outcome agrees with the fields it classifies. Every
        //    public constructor / mutator keeps them in lockstep, so a decoded
        //    value must too (a legacy payload without an outcome key has it
        //    DERIVED from these same fields before this check, so it always
        //    passes here).
        let outcome_consistent = match self.outcome {
            // Vacuously-complete results (zero-quantity taker) are Filled with
            // no trades, so Filled constrains only the remainder.
            MatchOutcome::Filled => self.remaining_quantity == 0,
            MatchOutcome::PartiallyFilled => self.remaining_quantity > 0 && !self.trades.is_empty(),
            MatchOutcome::NotFilled => self.remaining_quantity > 0 && self.trades.is_empty(),
            // Killed / rejected takers were turned away whole: quantity
            // remains, and no trade or filled id was ever recorded.
            MatchOutcome::Killed | MatchOutcome::Rejected => {
                self.remaining_quantity > 0
                    && self.trades.is_empty()
                    && self.filled_order_ids.is_empty()
            }
        };
        if !outcome_consistent {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "{:?} outcome contradicts the decoded fields \
                     (remaining_quantity {}, {} trade(s), {} filled id(s))",
                    self.outcome,
                    self.remaining_quantity,
                    self.trades.len(),
                    self.filled_order_ids.len()
                ),
            });
        }

        // 4. Every trade belongs to this result's incoming (taker) order —
        //    mirrored by the same guard in `add_trade`, so decoded and
        //    API-built values stay aligned.
        if let Some(alien) = self
            .trades
            .as_vec()
            .iter()
            .find(|trade| trade.taker_order_id() != self.order_id)
        {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "trade taker order id {} does not match the result's incoming order id {}",
                    alien.taker_order_id(),
                    self.order_id
                ),
            });
        }

        // 5. The filled ids are a duplicate-free, order-preserving subsequence
        //    of the trade maker ids: `match_order` records each fully-consumed
        //    maker exactly once, immediately after its final trade, so trade
        //    order and filled-id order agree. The `any` on the shared iterator
        //    advances it past each match, which is exactly subsequence
        //    semantics (and implies plain membership).
        if !self.filled_order_ids.is_empty() {
            let mut seen = std::collections::HashSet::new();
            seen.try_reserve(self.filled_order_ids.len()).map_err(|_| {
                PriceLevelError::capacity_exceeded(
                    CapacityResource::ValidationScratch,
                    self.filled_order_ids.len(),
                )
            })?;
            let mut makers = self
                .trades
                .as_vec()
                .iter()
                .map(|trade| trade.maker_order_id());
            for filled in &self.filled_order_ids {
                if !seen.insert(*filled) {
                    return Err(PriceLevelError::InvalidOperation {
                        message: format!("filled order id {filled} appears more than once"),
                    });
                }
                if !makers.by_ref().any(|maker| maker == *filled) {
                    return Err(PriceLevelError::InvalidOperation {
                        message: format!(
                            "filled order id {filled} is not an in-order maker of the trades"
                        ),
                    });
                }
            }
        }

        Ok(self)
    }
}

/// Which step of the executed-value computation overflowed. Kept as a
/// payload-free tag so the fused [`MatchResult::average_price`] traversal can
/// defer the error (and its message allocation) until it knows the quantity
/// sum succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueOverflow {
    /// `price * quantity` of a single trade overflowed `u128`.
    Multiplication,
    /// Summing the per-trade values overflowed `u128`.
    Accumulation,
}

impl ValueOverflow {
    /// The public error for this overflow; messages are unchanged from the
    /// pre-#151 accessors.
    #[cold]
    fn into_error(self) -> PriceLevelError {
        let message = match self {
            ValueOverflow::Multiplication => "executed value multiplication overflow",
            ValueOverflow::Accumulation => "executed value accumulation overflow",
        };
        PriceLevelError::InvalidOperation {
            message: message.to_string(),
        }
    }
}

/// Adds `trade`'s `price * quantity` to `acc` with checked arithmetic.
#[inline]
fn accumulate_value(acc: u128, trade: &Trade) -> Result<u128, ValueOverflow> {
    let trade_value = trade
        .price()
        .as_u128()
        .checked_mul(u128::from(trade.quantity().as_u64()))
        .ok_or(ValueOverflow::Multiplication)?;
    acc.checked_add(trade_value)
        .ok_or(ValueOverflow::Accumulation)
}

/// The error [`MatchResult::executed_quantity`] reports when the trade
/// quantities do not sum within `u64`.
#[cold]
fn executed_quantity_overflow() -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: "executed quantity overflow".to_string(),
    }
}

impl fmt::Display for MatchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MatchResult:order_id={};remaining_quantity={};is_complete={}",
            self.order_id, self.remaining_quantity, self.is_complete
        )?;
        write!(f, ";trades={}", self.trades)?;
        write!(f, ";filled_order_ids=[")?;
        for (i, order_id) in self.filled_order_ids.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{order_id}")?;
        }
        write!(f, "]")
    }
}

/// Parses the text written by `Display`:
/// `MatchResult:order_id=..;remaining_quantity=..;is_complete=..;trades=Trades:[..];filled_order_ids=[..]`.
///
/// Fields may appear in any order; a repeated field keeps its last value.
/// The `trades` and `filled_order_ids` values are bracketed sections located
/// by balanced-bracket scanning, so they may contain `;` and nested brackets.
/// Every slice is taken at an ASCII delimiter through checked access, so
/// multibyte text in any field yields a typed error, never a panic.
///
/// # Errors
///
/// - [`PriceLevelError::InvalidFormat`] for a missing `MatchResult:` prefix,
///   an unknown field name, a field without `=`, an unclosed bracketed
///   section or trailing text after one.
/// - [`PriceLevelError::ParseError`] if a bracketed section nests deeper than
///   128 levels (its own bracket included).
/// - [`PriceLevelError::MissingField`] for an absent field.
/// - [`PriceLevelError::InvalidFieldValue`] for an unparsable value.
/// - Errors from [`TradeList::from_str`], and the field-agreement errors of
///   the same validation `Deserialize` applies.
impl FromStr for MatchResult {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        /// Splits off a `[...]`-balanced section: `rest` starts at the bytes
        /// after the section's opening `[`. Returns the section body (without
        /// its closing `]`) and the text after the section, which must be
        /// empty or start with the `;` field separator (consumed here).
        fn bracketed_section(rest: &str) -> Result<(&str, &str), PriceLevelError> {
            let close =
                matching_close(rest, b'[', b']', MAX_TEXT_NESTING_DEPTH).map_err(|e| match e {
                    NestingError::TooDeep { limit } => NestingError::too_deep_error(limit),
                    NestingError::UnmatchedClose | NestingError::Unclosed => {
                        PriceLevelError::InvalidFormat
                    }
                })?;
            // `close` is the offset of an ASCII `]`, hence a char boundary.
            let (body, after) = rest
                .split_at_checked(close)
                .ok_or(PriceLevelError::InvalidFormat)?;
            let after = after
                .strip_prefix(']')
                .ok_or(PriceLevelError::InvalidFormat)?;
            // After the closing `]` the only thing allowed is a `;` field
            // separator or the end of the string; any other trailing content
            // is malformed and rejected rather than silently ignored.
            let next = match after.strip_prefix(';') {
                Some(next) => next,
                None if after.is_empty() => after,
                None => return Err(PriceLevelError::InvalidFormat),
            };
            Ok((body, next))
        }

        /// A plain value runs to the next `;` (consumed) or the end.
        fn plain_value(rest: &str) -> (&str, &str) {
            rest.split_once(';').unwrap_or((rest, ""))
        }

        let mut rest = s
            .strip_prefix("MatchResult:")
            .ok_or(PriceLevelError::InvalidFormat)?;

        let mut order_id_str = None;
        let mut remaining_quantity_str = None;
        let mut is_complete_str = None;
        let mut trades_str = None;
        let mut filled_order_ids_content = None;

        // Every structural delimiter (`=`, `;`, `[`, `]`) and literal prefix
        // (`Trades:[`) is ASCII, so each split below happens at a char
        // boundary and a field carrying malformed / multibyte text yields a
        // deterministic `Err`.
        while !rest.is_empty() {
            let (field_name, after_eq) =
                rest.split_once('=').ok_or(PriceLevelError::InvalidFormat)?;
            match field_name {
                "order_id" => {
                    let (value, next) = plain_value(after_eq);
                    order_id_str = Some(value);
                    rest = next;
                }
                "remaining_quantity" => {
                    let (value, next) = plain_value(after_eq);
                    remaining_quantity_str = Some(value);
                    rest = next;
                }
                "is_complete" => {
                    let (value, next) = plain_value(after_eq);
                    is_complete_str = Some(value);
                    rest = next;
                }
                "trades" => {
                    let body = after_eq
                        .strip_prefix("Trades:[")
                        .ok_or(PriceLevelError::InvalidFormat)?;
                    let (content, next) = bracketed_section(body)?;
                    // Re-borrow the full `Trades:[...]` text for
                    // `TradeList::from_str`: it spans from the start of the
                    // value to just past the closing `]`.
                    let full_len = "Trades:[]"
                        .len()
                        .checked_add(content.len())
                        .ok_or(PriceLevelError::InvalidFormat)?;
                    trades_str = Some(
                        after_eq
                            .get(..full_len)
                            .ok_or(PriceLevelError::InvalidFormat)?,
                    );
                    rest = next;
                }
                "filled_order_ids" => {
                    let body = after_eq
                        .strip_prefix('[')
                        .ok_or(PriceLevelError::InvalidFormat)?;
                    let (content, next) = bracketed_section(body)?;
                    filled_order_ids_content = Some(content);
                    rest = next;
                }
                _ => return Err(PriceLevelError::InvalidFormat),
            }
        }

        let order_id_str =
            order_id_str.ok_or_else(|| PriceLevelError::MissingField("order_id".to_string()))?;
        let remaining_quantity_str = remaining_quantity_str
            .ok_or_else(|| PriceLevelError::MissingField("remaining_quantity".to_string()))?;
        let is_complete_str = is_complete_str
            .ok_or_else(|| PriceLevelError::MissingField("is_complete".to_string()))?;
        let trades_str =
            trades_str.ok_or_else(|| PriceLevelError::MissingField("trades".to_string()))?;
        let filled_order_ids_content = filled_order_ids_content
            .ok_or_else(|| PriceLevelError::MissingField("filled_order_ids".to_string()))?;

        let order_id =
            Id::from_str(order_id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "order_id".to_string(),
                value: order_id_str.to_string(),
            })?;

        let remaining_quantity = remaining_quantity_str.parse::<u64>().map_err(|_| {
            PriceLevelError::InvalidFieldValue {
                field: "remaining_quantity".to_string(),
                value: remaining_quantity_str.to_string(),
            }
        })?;

        let is_complete =
            is_complete_str
                .parse::<bool>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: "is_complete".to_string(),
                    value: is_complete_str.to_string(),
                })?;

        let trades = TradeList::from_str(trades_str)?;

        let mut filled_order_ids = Vec::new();
        if !filled_order_ids_content.is_empty() {
            for id_str in filled_order_ids_content.split(',') {
                let id = Id::from_str(id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: "filled_order_ids".to_string(),
                    value: id_str.to_string(),
                })?;
                try_push(&mut filled_order_ids, id)?;
            }
        }

        // The text format predates the explicit outcome signal and does not
        // carry it, so re-derive the benign classification from the parsed
        // fields. A `Killed` / `Rejected` outcome cannot be recovered from text
        // (it is indistinguishable from `NotFilled` once the trades are gone);
        // callers that need that distinction must use the in-memory result or
        // the JSON (serde) representation, which preserves `outcome`.
        let outcome = if is_complete {
            MatchOutcome::Filled
        } else if trades.is_empty() {
            MatchOutcome::NotFilled
        } else {
            MatchOutcome::PartiallyFilled
        };

        // Route the structurally-parsed value through the same invariant gate
        // as `Deserialize`, so text input that is well-formed but
        // self-contradictory (e.g. `is_complete=true` with a positive
        // remainder, trade quantities that overflow, or a filled id absent from
        // the trades) is rejected rather than accepted.
        MatchResult {
            order_id,
            trades,
            remaining_quantity,
            is_complete,
            filled_order_ids,
            outcome,
            // The text format does not carry the failure slot (like `outcome`
            // before it, it is lossy); decode as "no error".
            error: None,
        }
        .validated()
    }
}

/// Test-only fault injection for [`MatchResult::add_trade`] (issue #170).
///
/// While armed, `add_trade` succeeds `after` more times on this thread and then
/// fails (before mutating anything) with an `InvalidOperation`. It exists so the
/// engine's "a committed fill could not be recorded" path can be exercised; in
/// production that failure is ruled out structurally. Compiled only under
/// `cfg(test)`.
#[cfg(test)]
pub(crate) mod test_seam {
    use crate::errors::PriceLevelError;
    use std::cell::Cell;

    thread_local! {
        static FAIL_ADD_TRADE_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
    }

    /// Disarms the injection on drop.
    pub(crate) struct AddTradeFailGuard;

    impl Drop for AddTradeFailGuard {
        fn drop(&mut self) {
            FAIL_ADD_TRADE_AFTER.with(|cell| cell.set(None));
        }
    }

    /// Makes the `(after + 1)`-th `add_trade` on this thread fail.
    pub(crate) fn fail_add_trade_after(after: usize) -> AddTradeFailGuard {
        FAIL_ADD_TRADE_AFTER.with(|cell| cell.set(Some(after)));
        AddTradeFailGuard
    }

    pub(super) fn check_add_trade() -> Result<(), PriceLevelError> {
        FAIL_ADD_TRADE_AFTER.with(|cell| match cell.get() {
            Some(0) => Err(PriceLevelError::InvalidOperation {
                message: "injected add_trade failure".to_string(),
            }),
            Some(n) => {
                cell.set(Some(n - 1));
                Ok(())
            }
            None => Ok(()),
        })
    }
}
