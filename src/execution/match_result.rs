use crate::errors::PriceLevelError;
use crate::execution::list::TradeList;
use crate::execution::trade::Trade;
use crate::orders::Id;
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// `#[serde(default)]` keeps snapshots produced before this field was added
    /// deserializable: an older JSON payload restores as
    /// [`MatchOutcome::NotFilled`] and is corrected by the accessors derived
    /// from the other fields where it matters.
    #[serde(default)]
    outcome: MatchOutcome,
}

impl MatchResult {
    /// Create a new empty match result
    #[must_use]
    pub fn new(order_id: Id, initial_quantity: u64) -> Self {
        // A zero-quantity result is vacuously complete (nothing to fill), so keep
        // is_complete / outcome consistent at construction — matching
        // `finalize`'s `remaining == 0 => Filled` rule. A non-zero result starts
        // incomplete / NotFilled until a trade or `finalize` updates it.
        let is_complete = initial_quantity == 0;
        Self {
            order_id,
            trades: TradeList::new(),
            remaining_quantity: initial_quantity,
            is_complete,
            filled_order_ids: Vec::new(),
            outcome: if is_complete {
                MatchOutcome::Filled
            } else {
                MatchOutcome::NotFilled
            },
        }
    }

    /// Create a new empty match result with the `trades` and `filled_order_ids`
    /// vectors pre-sized for up to `capacity` entries.
    ///
    /// A single match sweep at one price level produces at most one trade and
    /// at most one filled order id per resting order, so `capacity` is normally
    /// the level's resting order count. Pre-sizing both vectors removes the
    /// per-fill reallocations on the match hot path.
    #[must_use]
    pub fn with_capacity(order_id: Id, initial_quantity: u64, capacity: usize) -> Self {
        // Same zero-quantity consistency as `new` (see there).
        let is_complete = initial_quantity == 0;
        Self {
            order_id,
            trades: TradeList::with_capacity(capacity),
            remaining_quantity: initial_quantity,
            is_complete,
            filled_order_ids: Vec::with_capacity(capacity),
            outcome: if is_complete {
                MatchOutcome::Filled
            } else {
                MatchOutcome::NotFilled
            },
        }
    }

    /// Add a trade to this match result.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the trade's quantity
    /// exceeds the remaining quantity of the incoming order (the subtraction
    /// would underflow), which indicates an over-fill bug in the caller.
    pub fn add_trade(&mut self, trade: Trade) -> Result<(), PriceLevelError> {
        self.remaining_quantity = self
            .remaining_quantity
            .checked_sub(trade.quantity().as_u64())
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: format!(
                    "trade quantity {} exceeds remaining quantity {}",
                    trade.quantity().as_u64(),
                    self.remaining_quantity
                ),
            })?;
        self.is_complete = self.remaining_quantity == 0;
        // Keep the outcome in lockstep with the fields it summarizes: a trade
        // has now occurred, so the result is at least partially filled.
        // `finalize` re-derives the terminal classification after the sweep.
        self.outcome = if self.is_complete {
            MatchOutcome::Filled
        } else {
            MatchOutcome::PartiallyFilled
        };
        self.trades.add(trade);
        Ok(())
    }

    /// Add a filled order ID to track orders removed from the book
    pub fn add_filled_order_id(&mut self, order_id: Id) {
        self.filled_order_ids.push(order_id);
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

    /// Returns the remaining quantity of the incoming order after matching.
    #[must_use]
    pub fn remaining_quantity(&self) -> u64 {
        self.remaining_quantity
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
    pub(crate) fn finalize(&mut self, remaining_quantity: u64) {
        self.remaining_quantity = remaining_quantity;
        self.is_complete = remaining_quantity == 0;
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

    /// Get the total executed quantity
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if summing the trade
    /// quantities overflows `u64`.
    pub fn executed_quantity(&self) -> Result<u64, PriceLevelError> {
        self.trades.as_vec().iter().try_fold(0u64, |acc, trade| {
            acc.checked_add(trade.quantity().as_u64()).ok_or_else(|| {
                PriceLevelError::InvalidOperation {
                    message: "executed quantity overflow".to_string(),
                }
            })
        })
    }

    /// Get the total value executed
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any per-trade
    /// `price * quantity` product overflows `u128`, or if accumulating those
    /// products overflows `u128`.
    pub fn executed_value(&self) -> Result<u128, PriceLevelError> {
        self.trades.as_vec().iter().try_fold(0u128, |acc, trade| {
            let trade_value = trade
                .price()
                .as_u128()
                .checked_mul(u128::from(trade.quantity().as_u64()))
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "executed value multiplication overflow".to_string(),
                })?;

            acc.checked_add(trade_value)
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "executed value accumulation overflow".to_string(),
                })
        })
    }

    /// Calculate the average execution price
    ///
    /// Returns `Ok(None)` when no quantity has been executed (no average price
    /// exists), avoiding a division by zero.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the underlying
    /// [`Self::executed_quantity`] or [`Self::executed_value`] computation
    /// overflows.
    pub fn average_price(&self) -> Result<Option<f64>, PriceLevelError> {
        let executed_qty = self.executed_quantity()?;
        if executed_qty == 0 {
            Ok(None)
        } else {
            Ok(Some(self.executed_value()? as f64 / executed_qty as f64))
        }
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

impl FromStr for MatchResult {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn find_next_field(s: &str, start_pos: usize) -> Result<(&str, usize), PriceLevelError> {
            let mut pos = start_pos;

            while pos < s.len() {
                if s[pos..].starts_with(';') {
                    let value = &s[start_pos..pos];
                    return Ok((value, pos + 1));
                }
                pos += 1;
            }

            if pos == s.len() {
                let value = &s[start_pos..pos];
                return Ok((value, pos));
            }

            Err(PriceLevelError::InvalidFormat)
        }
        if !s.starts_with("MatchResult:") {
            return Err(PriceLevelError::InvalidFormat);
        }

        let mut order_id_str = None;
        let mut remaining_quantity_str = None;
        let mut is_complete_str = None;
        let mut trades_str = None;
        let mut filled_order_ids_str = None;

        let mut pos = "MatchResult:".len();

        while pos < s.len() {
            let field_end = match s[pos..].find('=') {
                Some(idx) => pos + idx,
                None => return Err(PriceLevelError::InvalidFormat),
            };

            let field_name = &s[pos..field_end];
            pos = field_end + 1;
            match field_name {
                "order_id" => {
                    let (value, next_pos) = find_next_field(s, pos)?;
                    order_id_str = Some(value);
                    pos = next_pos;
                }
                "remaining_quantity" => {
                    let (value, next_pos) = find_next_field(s, pos)?;
                    remaining_quantity_str = Some(value);
                    pos = next_pos;
                }
                "is_complete" => {
                    let (value, next_pos) = find_next_field(s, pos)?;
                    is_complete_str = Some(value);
                    pos = next_pos;
                }
                "trades" => {
                    if !s[pos..].starts_with("Trades:[") {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    let mut bracket_depth = 1;
                    let mut i = pos + "Trades:[".len();

                    while i < s.len() && bracket_depth > 0 {
                        if s[i..].starts_with(']') {
                            bracket_depth -= 1;
                            if bracket_depth == 0 {
                                break;
                            }
                            i += 1;
                        } else if s[i..].starts_with('[') {
                            bracket_depth += 1;
                            i += 1;
                        } else {
                            i += 1;
                        }
                    }

                    if bracket_depth > 0 {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    trades_str = Some(&s[pos..=i]);
                    pos = i + 1;
                    if pos < s.len() && s[pos..].starts_with(';') {
                        pos += 1;
                    } else if pos < s.len() {
                        return Err(PriceLevelError::InvalidFormat);
                    }
                }
                "filled_order_ids" => {
                    if !s[pos..].starts_with('[') {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    let mut bracket_depth = 1;
                    let mut i = pos + 1;

                    while i < s.len() && bracket_depth > 0 {
                        if s[i..].starts_with(']') {
                            bracket_depth -= 1;
                            if bracket_depth == 0 {
                                break;
                            }
                            i += 1;
                        } else if s[i..].starts_with('[') {
                            bracket_depth += 1;
                            i += 1;
                        } else {
                            i += 1;
                        }
                    }

                    if bracket_depth > 0 {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    filled_order_ids_str = Some(&s[pos..=i]);

                    pos = i + 1;
                    if pos < s.len() && s[pos..].starts_with(';') {
                        pos += 1;
                    }
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
        let filled_order_ids_str = filled_order_ids_str
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

        let filled_order_ids = if filled_order_ids_str == "[]" {
            Vec::new()
        } else {
            let content = &filled_order_ids_str[1..filled_order_ids_str.len() - 1];

            if content.is_empty() {
                Vec::new()
            } else {
                content
                    .split(',')
                    .map(|id_str| {
                        Id::from_str(id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                            field: "filled_order_ids".to_string(),
                            value: id_str.to_string(),
                        })
                    })
                    .collect::<Result<Vec<Id>, PriceLevelError>>()?
            }
        };

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

        Ok(MatchResult {
            order_id,
            trades,
            remaining_quantity,
            is_complete,
            filled_order_ids,
            outcome,
        })
    }
}
