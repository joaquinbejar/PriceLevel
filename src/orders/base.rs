//! Base order definitions

use crate::errors::PriceLevelError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Represents the side of an order
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Buy side (bids)
    #[serde(rename(serialize = "BUY"))]
    #[serde(alias = "buy", alias = "Buy", alias = "BUY")]
    Buy,
    /// Sell side (asks)
    #[serde(rename(serialize = "SELL"))]
    #[serde(alias = "sell", alias = "Sell", alias = "SELL")]
    Sell,
}

impl Side {
    /// Returns the opposite side of the order.
    ///
    /// # Examples
    ///
    /// ```
    /// use pricelevel::Side;
    /// let buy_side = Side::Buy;
    /// let sell_side = buy_side.opposite();
    /// assert_eq!(sell_side, Side::Sell);
    ///
    /// let sell_side = Side::Sell;
    /// let buy_side = sell_side.opposite();
    /// assert_eq!(buy_side, Side::Buy);
    /// ```
    pub fn opposite(&self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

impl FromStr for Side {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "BUY" => Ok(Side::Buy),
            "SELL" => Ok(Side::Sell),
            _ => Err(PriceLevelError::ParseError {
                message: "Failed to parse Side".to_string(),
            }),
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Represents a unique order ID using UUID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrderId(pub Uuid);

impl FromStr for OrderId {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Uuid::from_str(s) {
            Ok(id) => Ok(OrderId(id)),
            Err(e) => Err(PriceLevelError::ParseError {
                message: format!("Failed to parse OrderId: {}", e),
            }),
        }
    }
}

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for OrderId {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderId {
    /// Create a new random OrderId
    pub fn new() -> Self {
        OrderId(Uuid::new_v4())
    }

    /// Create a nil UUID (all zeros)
    pub fn nil() -> Self {
        OrderId(Uuid::nil())
    }

    /// Create from an existing UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        OrderId(uuid)
    }

    /// For backward compatibility with code still using u64 IDs
    pub fn from_u64(id: u64) -> Self {
        // Create a deterministic UUID from the u64 by using it in the most significant bytes
        let bytes = [
            ((id >> 56) & 0xFF) as u8,
            ((id >> 48) & 0xFF) as u8,
            ((id >> 40) & 0xFF) as u8,
            ((id >> 32) & 0xFF) as u8,
            ((id >> 24) & 0xFF) as u8,
            ((id >> 16) & 0xFF) as u8,
            ((id >> 8) & 0xFF) as u8,
            (id & 0xFF) as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        OrderId(Uuid::from_bytes(bytes))
    }
}
