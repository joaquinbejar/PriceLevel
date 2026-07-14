use crate::errors::PriceLevelError;
use crate::execution::list::TradeList;
use crate::execution::trade::Trade;
use crate::orders::Id;
use crate::utils::Quantity;
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
    outcome: MatchOutcome,
}

/// Wire form of [`MatchResult`] used for validated deserialization.
///
/// It mirrors `MatchResult`'s serialized shape field-for-field (identical
/// names, so every previously-valid payload still decodes) but performs no
/// validation itself: `#[serde(try_from = "MatchResultWire")]` on
/// `MatchResult` deserializes this permissive struct and then runs
/// [`MatchResult::validated`] via the [`TryFrom`] impl below. The `outcome`
/// field keeps `#[serde(default)]` so a payload written before that field
/// existed decodes as [`MatchOutcome::NotFilled`] (the legacy default), as
/// before.
#[derive(Deserialize)]
struct MatchResultWire {
    order_id: Id,
    trades: TradeList,
    remaining_quantity: u64,
    is_complete: bool,
    filled_order_ids: Vec<Id>,
    #[serde(default)]
    outcome: MatchOutcome,
}

impl TryFrom<MatchResultWire> for MatchResult {
    type Error = PriceLevelError;

    fn try_from(wire: MatchResultWire) -> Result<Self, Self::Error> {
        MatchResult {
            order_id: wire.order_id,
            trades: wire.trades,
            remaining_quantity: wire.remaining_quantity,
            is_complete: wire.is_complete,
            filled_order_ids: wire.filled_order_ids,
            outcome: wire.outcome,
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
        }
    }

    /// Create a new empty match result with the `trades` and `filled_order_ids`
    /// vectors pre-sized for up to `capacity` entries.
    ///
    /// A single match sweep at one price level produces at most one trade and
    /// at most one filled order id per resting order it consumes, so a good
    /// `capacity` is the tighter of the taker's incoming quantity and the
    /// level's resting order count (see `PriceLevel::match_order`). Pre-sizing
    /// both vectors removes the per-fill reallocations on the match hot path
    /// without over-reserving for a small taker against a deep level.
    #[must_use]
    pub fn with_capacity(order_id: Id, initial_quantity: Quantity, capacity: usize) -> Self {
        // Same zero-quantity consistency as `new` (see there).
        let is_complete = initial_quantity.as_u64() == 0;
        Self {
            order_id,
            trades: TradeList::with_capacity(capacity),
            remaining_quantity: initial_quantity.as_u64(),
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
                acc.checked_add(trade.quantity().as_u64()).ok_or_else(|| {
                    PriceLevelError::InvalidOperation {
                        message: "executed quantity overflow".to_string(),
                    }
                })
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
        let executed_qty = self.executed_quantity()?.as_u64();
        if executed_qty == 0 {
            Ok(None)
        } else {
            Ok(Some(self.executed_value()? as f64 / executed_qty as f64))
        }
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

        // 3. A killed / rejected result carries no trades and no filled ids.
        if matches!(self.outcome, MatchOutcome::Killed | MatchOutcome::Rejected)
            && (!self.trades.is_empty() || !self.filled_order_ids.is_empty())
        {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "{:?} outcome must carry no trades and no filled_order_ids \
                     (found {} trade(s), {} filled id(s))",
                    self.outcome,
                    self.trades.len(),
                    self.filled_order_ids.len()
                ),
            });
        }

        // 4. Every filled id is backed by a trade whose maker it is.
        if !self.filled_order_ids.is_empty() {
            let makers: std::collections::HashSet<Id> = self
                .trades
                .as_vec()
                .iter()
                .map(|trade| trade.maker_order_id())
                .collect();
            if let Some(missing) = self.filled_order_ids.iter().find(|id| !makers.contains(id)) {
                return Err(PriceLevelError::InvalidOperation {
                    message: format!(
                        "filled order id {missing} does not appear as a maker in any trade"
                    ),
                });
            }
        }

        Ok(self)
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
            // Scan raw bytes for the ASCII ';' delimiter rather than advancing a
            // `&str` slice one byte at a time. Every byte of a multibyte scalar
            // is >= 0x80, so it can never equal `b';'` (0x3B): a byte comparison
            // never mistakes an interior byte for the delimiter, and we only
            // ever form a `&str` slice at a `;` position or the end of the
            // string — both guaranteed char boundaries — so slicing cannot panic
            // on malformed UTF-8. `start_pos` is always the boundary just after
            // an ASCII `=`.
            let bytes = s.as_bytes();
            let mut pos = start_pos;

            while let Some(&byte) = bytes.get(pos) {
                if byte == b';' {
                    let value = s
                        .get(start_pos..pos)
                        .ok_or(PriceLevelError::InvalidFormat)?;
                    return Ok((value, pos + 1));
                }
                pos += 1;
            }

            // No ';' before the end: the value runs to the end of the string.
            let value = s.get(start_pos..).ok_or(PriceLevelError::InvalidFormat)?;
            Ok((value, bytes.len()))
        }
        if !s.starts_with("MatchResult:") {
            return Err(PriceLevelError::InvalidFormat);
        }

        let mut order_id_str = None;
        let mut remaining_quantity_str = None;
        let mut is_complete_str = None;
        let mut trades_str = None;
        let mut filled_order_ids_str = None;

        // Scan over the raw bytes. Every structural delimiter in the format
        // (`=`, `;`, `[`, `]`) and every literal prefix (`Trades:[`) is ASCII,
        // so a byte comparison locates them without ever splitting a multibyte
        // scalar, and every `&str` slice below is taken at an ASCII delimiter
        // position (or the string end) — all guaranteed char boundaries — so a
        // field carrying malformed / multibyte text yields a deterministic
        // `Err`, never a slice-on-non-boundary panic.
        let bytes = s.as_bytes();
        let mut pos = "MatchResult:".len();

        while pos < bytes.len() {
            let field_end = match bytes
                .get(pos..)
                .and_then(|rest| rest.iter().position(|&b| b == b'='))
            {
                Some(idx) => pos + idx,
                None => return Err(PriceLevelError::InvalidFormat),
            };

            let field_name = s
                .get(pos..field_end)
                .ok_or(PriceLevelError::InvalidFormat)?;
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
                    if !bytes
                        .get(pos..)
                        .is_some_and(|rest| rest.starts_with(b"Trades:["))
                    {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    let mut bracket_depth = 1;
                    let mut i = pos + "Trades:[".len();

                    while bracket_depth > 0 {
                        match bytes.get(i) {
                            Some(b']') => {
                                bracket_depth -= 1;
                                if bracket_depth == 0 {
                                    break;
                                }
                                i += 1;
                            }
                            Some(b'[') => {
                                bracket_depth += 1;
                                i += 1;
                            }
                            Some(_) => {
                                i += 1;
                            }
                            None => break,
                        }
                    }

                    if bracket_depth > 0 {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    // `i` is the byte index of the closing ASCII `]`, so the
                    // inclusive slice ends on a char boundary.
                    trades_str = Some(s.get(pos..=i).ok_or(PriceLevelError::InvalidFormat)?);
                    pos = i + 1;
                    if bytes.get(pos) == Some(&b';') {
                        pos += 1;
                    } else if pos < bytes.len() {
                        return Err(PriceLevelError::InvalidFormat);
                    }
                }
                "filled_order_ids" => {
                    if bytes.get(pos) != Some(&b'[') {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    let mut bracket_depth = 1;
                    let mut i = pos + 1;

                    while bracket_depth > 0 {
                        match bytes.get(i) {
                            Some(b']') => {
                                bracket_depth -= 1;
                                if bracket_depth == 0 {
                                    break;
                                }
                                i += 1;
                            }
                            Some(b'[') => {
                                bracket_depth += 1;
                                i += 1;
                            }
                            Some(_) => {
                                i += 1;
                            }
                            None => break,
                        }
                    }

                    if bracket_depth > 0 {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    // `i` is the byte index of the closing ASCII `]`, so the
                    // inclusive slice ends on a char boundary.
                    filled_order_ids_str =
                        Some(s.get(pos..=i).ok_or(PriceLevelError::InvalidFormat)?);

                    pos = i + 1;
                    // Symmetric with the `trades` branch: after the closing `]`
                    // the only thing allowed is a `;` field separator or the end
                    // of the string. Any other trailing content is malformed and
                    // rejected rather than silently ignored.
                    if bytes.get(pos) == Some(&b';') {
                        pos += 1;
                    } else if pos < bytes.len() {
                        return Err(PriceLevelError::InvalidFormat);
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
        }
        .validated()
    }
}
