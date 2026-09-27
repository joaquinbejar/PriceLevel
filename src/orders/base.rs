//! Base order definitions

use crate::errors::PriceLevelError;
use crate::utils::encode::{HASH32_HEX_LEN, encode_hash32_hex};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Represents the side of an order
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Buy side (bids)
    #[serde(rename(serialize = "BUY"))]
    #[serde(alias = "buy", alias = "BUY")]
    Buy,
    /// Sell side (asks)
    #[serde(rename(serialize = "SELL"))]
    #[serde(alias = "sell", alias = "SELL")]
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

    /// Converts the hash to a lowercase hexadecimal string (64 characters).
    ///
    /// Allocates exactly the returned `String`; the digits are produced in a
    /// stack buffer first (issue #201).
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.to_string()
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

        // Exactly 64 input bytes pair up with the 32 output bytes, so zipping
        // the byte pairs with the output slots visits every slot once without
        // an index.
        let mut bytes = [0u8; 32];
        let (pairs, _) = s.as_bytes().as_chunks::<2>();
        for (slot, pair) in bytes.iter_mut().zip(pairs) {
            let hex_str =
                std::str::from_utf8(pair.as_slice()).map_err(|_| PriceLevelError::ParseError {
                    message: "Invalid UTF-8 in hex string".to_string(),
                })?;
            *slot = u8::from_str_radix(hex_str, 16).map_err(|_| PriceLevelError::ParseError {
                message: format!("Invalid hex character in Hash32: {hex_str}"),
            })?;
        }

        Ok(Self(bytes))
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; HASH32_HEX_LEN];
        match encode_hash32_hex(&self.0, &mut buf) {
            Ok(hex) => f.write_str(hex),
            // Unreachable in practice; write byte by byte so `to_string`
            // never observes an encoder error.
            Err(fmt::Error) => self.0.iter().try_for_each(|b| write!(f, "{b:02x}")),
        }
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
    /// Serializes the lowercase hex text through a stack buffer: no per-hash
    /// heap allocation (issue #201). The wire form is unchanged.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut buf = [0u8; HASH32_HEX_LEN];
        let hex = encode_hash32_hex(&self.0, &mut buf)
            .map_err(|_| serde::ser::Error::custom("failed to encode Hash32 hex"))?;
        serializer.serialize_str(hex)
    }
}

/// Serde visitor for [`Hash32`]: parses the borrowed string through
/// [`Hash32::from_hex`] without copying it.
struct Hash32Visitor;

impl serde::de::Visitor<'_> for Hash32Visitor {
    type Value = Hash32;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Hash32, E>
    where
        E: serde::de::Error,
    {
        Hash32::from_hex(v).map_err(E::custom)
    }

    /// Owned-string fallback for deserializers that only hand out `String`s;
    /// parses in place without a further copy.
    fn visit_string<E>(self, v: String) -> Result<Hash32, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&v)
    }

    /// UTF-8 byte input, accepted exactly as `String`'s visitor accepts it
    /// (the pre-#201 `String::deserialize` path): invalid UTF-8 is an
    /// `invalid_value` error naming the bytes, valid text goes through
    /// [`Hash32::from_hex`]. `char` input needs no override: the default
    /// `visit_char` forwards to `visit_str`.
    fn visit_bytes<E>(self, v: &[u8]) -> Result<Hash32, E>
    where
        E: serde::de::Error,
    {
        match std::str::from_utf8(v) {
            Ok(s) => self.visit_str(s),
            Err(_) => Err(E::invalid_value(serde::de::Unexpected::Bytes(v), &self)),
        }
    }

    /// Borrowed UTF-8 byte input; same rules as `visit_bytes`.
    fn visit_borrowed_bytes<E>(self, v: &[u8]) -> Result<Hash32, E>
    where
        E: serde::de::Error,
    {
        self.visit_bytes(v)
    }

    /// Owned UTF-8 byte input; same rules as `visit_bytes`.
    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Hash32, E>
    where
        E: serde::de::Error,
    {
        self.visit_bytes(&v)
    }
}

impl<'de> Deserialize<'de> for Hash32 {
    /// Deserializes through a borrowing `str` visitor: input is parsed in
    /// place (issue #201) with the exact [`Hash32::from_hex`] grammar.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(Hash32Visitor)
    }
}
