//! Base order definitions

use crate::errors::PriceLevelError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Represents the side of an order
#[repr(u8)]
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
    #[must_use]
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

/// A 32-byte hash value used for user identification.
///
/// This is a wrapper around `[u8; 32]` that provides convenient methods
/// for creating, displaying, and parsing hash values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Hash32(pub [u8; 32]);

impl Hash32 {
    /// Creates a new `Hash32` from a 32-byte array.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Creates a zero-filled `Hash32`.
    #[must_use]
    pub const fn zero() -> Self {
        Self([0u8; 32])
    }

    /// Returns the inner byte array.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the inner byte array as a mutable reference.
    #[must_use]
    pub fn as_bytes_mut(&mut self) -> &mut [u8; 32] {
        &mut self.0
    }

    /// Converts the hash to a hexadecimal string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Creates a `Hash32` from a hexadecimal string.
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not exactly 64 hex characters
    /// or contains invalid hex characters.
    pub fn from_hex(s: &str) -> Result<Self, PriceLevelError> {
        if s.len() != 64 {
            return Err(PriceLevelError::ParseError {
                message: format!("Hash32 hex string must be 64 characters, got {}", s.len()),
            });
        }

        let mut bytes = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hex_str = std::str::from_utf8(chunk).map_err(|_| PriceLevelError::ParseError {
                message: "Invalid UTF-8 in hex string".to_string(),
            })?;
            bytes[i] =
                u8::from_str_radix(hex_str, 16).map_err(|_| PriceLevelError::ParseError {
                    message: format!("Invalid hex character in Hash32: {hex_str}"),
                })?;
        }

        Ok(Self(bytes))
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl FromStr for Hash32 {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

impl From<[u8; 32]> for Hash32 {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<Hash32> for [u8; 32] {
    fn from(hash: Hash32) -> Self {
        hash.0
    }
}

impl Serialize for Hash32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}
