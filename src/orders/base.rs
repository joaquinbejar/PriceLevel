//! Base order definitions

use crate::errors::PriceLevelError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

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

impl FromStr for Side {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "BUY" => Ok(Side::Buy),
            "SELL" => Ok(Side::Sell),
            _ => Err(PriceLevelError::ParseError{
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

/// Represents a unique order ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrderId(pub u64);

impl FromStr for OrderId {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse::<u64>() {
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

/// Represents a basic order in the limit order book
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// Unique identifier for this order
    pub id: OrderId,
    /// Price level for this order (in minor currency units, e.g. cents)
    pub price: u64,
    /// Quantity of the order (in minor units)
    pub quantity: u64,
    /// Side of the order (buy or sell)
    pub side: Side,
    /// Timestamp when the order was created (in milliseconds since epoch)
    pub timestamp: u64,
}

impl Order {
    /// Create a new order
    pub fn new(id: u64, price: u64, quantity: u64, side: Side, timestamp: u64) -> Self {
        Self {
            id: OrderId(id),
            price,
            quantity,
            side,
            timestamp,
        }
    }

    /// Create a new buy order
    pub fn buy(id: u64, price: u64, quantity: u64, timestamp: u64) -> Self {
        Self::new(id, price, quantity, Side::Buy, timestamp)
    }

    /// Create a new sell order
    pub fn sell(id: u64, price: u64, quantity: u64, timestamp: u64) -> Self {
        Self::new(id, price, quantity, Side::Sell, timestamp)
    }
}

impl FromStr for Order {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() != 5 {
            return Err(PriceLevelError::ParseError {
                message: format!("Expected 5 parts separated by ':', got {}", parts.len()),
            });
        }

        // Parse each part
        let id = parts[0]
            .parse::<OrderId>()
            .map_err(|e| PriceLevelError::ParseError {
                message: format!("Failed to parse id: {}", e),
            })?;

        let price = parts[1]
            .parse::<u64>()
            .map_err(|e| PriceLevelError::ParseError {
                message: format!("Failed to parse price: {}", e),
            })?;

        let quantity = parts[2]
            .parse::<u64>()
            .map_err(|e| PriceLevelError::ParseError {
                message: format!("Failed to parse quantity: {}", e),
            })?;

        let side = parts[3]
            .parse::<Side>()
            .map_err(|e| PriceLevelError::ParseError {
                message: format!("Failed to parse side: {}", e),
            })?;

        let timestamp = parts[4]
            .parse::<u64>()
            .map_err(|e| PriceLevelError::ParseError {
                message: format!("Failed to parse timestamp: {}", e),
            })?;

        Ok(Order {
            id,
            price,
            quantity,
            side,
            timestamp,
        })
    }
}

impl fmt::Display for Order {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}:{}:{}",
            self.id.0,
            self.price,
            self.quantity,
            match self.side {
                Side::Buy => "BUY",
                Side::Sell => "SELL",
            },
            self.timestamp
        )
    }
}
