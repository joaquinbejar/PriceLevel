//! Base order definitions

use crate::errors::PriceLevelError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;
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

/// Represents a unique identifier for an order in the trading system.
///
/// This enum supports two different ID formats to provide flexibility
/// in order identification and tracking across different systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderId {
    /// UUID (Universally Unique Identifier) format
    /// A 128-bit identifier that is globally unique across space and time
    Uuid(Uuid),

    /// ULID (Universally Unique Lexicographically Sortable Identifier) format
    /// A 128-bit identifier that is lexicographically sortable and globally unique
    Ulid(Ulid),
}

impl FromStr for OrderId {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try UUID first (has hyphens), then ULID
        if let Ok(uuid) = Uuid::from_str(s) {
            Ok(OrderId::Uuid(uuid))
        } else if let Ok(ulid) = Ulid::from_string(s) {
            Ok(OrderId::Ulid(ulid))
        } else {
            Err(PriceLevelError::ParseError {
                message: format!("Failed to parse OrderId as UUID or ULID: {s}"),
            })
        }
    }
}

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderId::Uuid(uuid) => write!(f, "{}", uuid),
            OrderId::Ulid(ulid) => write!(f, "{}", ulid),
        }
    }
}

// Custom serialization to maintain backward compatibility
impl Serialize for OrderId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

// Custom deserialization to maintain backward compatibility
impl<'de> Deserialize<'de> for OrderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        OrderId::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Default for OrderId {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderId {
    /// Create a new random OrderId (defaults to ULID for better sortability)
    pub fn new() -> Self {
        OrderId::Ulid(Ulid::new())
    }

    /// Create a new UUID-based OrderId
    pub fn new_uuid() -> Self {
        OrderId::Uuid(Uuid::new_v4())
    }

    /// Create a new ULID-based OrderId
    pub fn new_ulid() -> Self {
        OrderId::Ulid(Ulid::new())
    }

    /// Create a nil OrderId (UUID format)
    pub fn nil() -> Self {
        OrderId::Uuid(Uuid::nil())
    }

    /// Create from an existing UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        OrderId::Uuid(uuid)
    }

    /// Create from an existing ULID
    pub fn from_ulid(ulid: Ulid) -> Self {
        OrderId::Ulid(ulid)
    }

    /// Get as bytes (both UUID and ULID are 16 bytes)
    pub fn as_bytes(&self) -> [u8; 16] {
        match self {
            OrderId::Uuid(uuid) => *uuid.as_bytes(),
            OrderId::Ulid(ulid) => ulid.to_bytes(),
        }
    }

    /// For backward compatibility with code still using u64 IDs
    pub fn from_u64(id: u64) -> Self {
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
        OrderId::Uuid(Uuid::from_bytes(bytes))
    }
}
