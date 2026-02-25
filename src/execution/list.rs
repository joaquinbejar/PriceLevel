use crate::errors::PriceLevelError;
use crate::execution::trade::Trade;
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

impl FromStr for TradeList {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with("Trades:[") || !s.ends_with("]") {
            return Err(PriceLevelError::InvalidFormat);
        }

        let content_start = s.find('[').ok_or(PriceLevelError::InvalidFormat)?;
        let content_end = s.rfind(']').ok_or(PriceLevelError::InvalidFormat)?;

        if content_start >= content_end {
            return Err(PriceLevelError::InvalidFormat);
        }

        let content = &s[content_start + 1..content_end];

        if content.is_empty() {
            return Ok(TradeList::new());
        }

        let mut trades = Vec::new();
        let mut current_trade = String::new();
        let mut bracket_depth = 0;

        for c in content.chars() {
            match c {
                ',' if bracket_depth == 0 => {
                    if !current_trade.is_empty() {
                        let trade = Trade::from_str(&current_trade)?;
                        trades.push(trade);
                        current_trade.clear();
                    }
                }
                '[' => {
                    bracket_depth += 1;
                    current_trade.push(c);
                }
                ']' => {
                    bracket_depth -= 1;
                    current_trade.push(c);
                }
                _ => current_trade.push(c),
            }
        }

        if !current_trade.is_empty() {
            let trade = Trade::from_str(&current_trade)?;
            trades.push(trade);
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
