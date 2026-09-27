use crate::errors::PriceLevelError;
use crate::execution::trade::Trade;
use crate::utils::text::{
    MAX_TEXT_NESTING_DEPTH, MAX_TEXT_NESTING_DEPTH_INSIDE_LIST, NestingError, TopLevelSplit,
    try_push,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A wrapper for a vector of trades to implement custom serialization.
///
/// The inner collection is private to enforce append-only semantics
/// during matching. Use [`Self::add`] to append and [`Self::as_vec`]
/// or [`Self::into_vec`] to read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TradeList {
    /// Ordered collection of trades.
    trades: Vec<Trade>,
}

impl TradeList {
    /// Create a new empty trade list
    #[must_use]
    pub fn new() -> Self {
        Self { trades: Vec::new() }
    }

    /// Create a new empty trade list with space reserved for `n` trades.
    ///
    /// Pre-allocates the backing vector to avoid repeated reallocations when
    /// the number of trades produced by a single match sweep is known or
    /// estimable (e.g. bounded by the resting order count at a level).
    #[must_use]
    pub fn with_capacity(n: usize) -> Self {
        Self {
            trades: Vec::with_capacity(n),
        }
    }

    /// Create a trade list from an existing vector
    #[must_use]
    pub fn from_vec(trades: Vec<Trade>) -> Self {
        Self { trades }
    }

    /// Add a trade to the list
    pub fn add(&mut self, trade: Trade) {
        self.trades.push(trade);
    }

    /// Get a reference to the underlying vector
    #[must_use]
    pub fn as_vec(&self) -> &Vec<Trade> {
        &self.trades
    }

    /// Convert into a vector of trades
    #[must_use]
    pub fn into_vec(self) -> Vec<Trade> {
        self.trades
    }

    /// Returns `true` when the list does not contain any trades.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.trades.is_empty()
    }

    /// Returns the number of trades in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.trades.len()
    }
}

impl Default for TradeList {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TradeList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Trades:[")?;

        for (i, trade) in self.trades.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{trade}")?;
        }

        write!(f, "]")
    }
}

/// Parses the `Trades:[<trade>,<trade>,...]` text written by `Display`.
///
/// The list is split at top-level `,` separators and each segment is parsed
/// with [`Trade::from_str`] directly from a borrowed slice of the input — no
/// per-trade copy (issue #152). Empty segments (`,,`, a leading or trailing
/// `,`) are skipped. A `,` nested inside `[...]` belongs to its segment.
///
/// # Errors
///
/// - [`PriceLevelError::InvalidFormat`] if the `Trades:[` prefix or the final
///   `]` is missing, or a `[` / `]` inside the list is unbalanced.
/// - [`PriceLevelError::ParseError`] if brackets nest deeper than 128 levels
///   (the enclosing list bracket included).
/// - [`PriceLevelError::InvalidOperation`] if the trade vector cannot grow.
/// - Any error returned by [`Trade::from_str`] for a malformed segment,
///   reported for the first malformed segment in input order.
impl FromStr for TradeList {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let content = s
            .strip_prefix("Trades:[")
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or(PriceLevelError::InvalidFormat)?;

        if content.is_empty() {
            return Ok(TradeList::new());
        }

        // The enclosing `[` of the list counts toward the nesting limit, so
        // segments may nest one level less than the limit.
        let limit = MAX_TEXT_NESTING_DEPTH_INSIDE_LIST;
        let mut trades = Vec::new();
        for segment in TopLevelSplit::new(content, b',', b"[", b"]", limit) {
            let segment = segment.map_err(|e| match e {
                NestingError::TooDeep { .. } => {
                    NestingError::too_deep_error(MAX_TEXT_NESTING_DEPTH)
                }
                NestingError::UnmatchedClose | NestingError::Unclosed => {
                    PriceLevelError::InvalidFormat
                }
            })?;
            if !segment.text.is_empty() {
                try_push(&mut trades, Trade::from_str(segment.text)?)?;
            }
        }

        Ok(TradeList { trades })
    }
}

impl From<Vec<Trade>> for TradeList {
    fn from(trades: Vec<Trade>) -> Self {
        Self::from_vec(trades)
    }
}

impl From<TradeList> for Vec<Trade> {
    fn from(list: TradeList) -> Self {
        list.into_vec()
    }
}
