use crate::errors::PriceLevelError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Domain value type representing a price.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Price(u128);

impl Price {
    /// Zero price value.
    pub const ZERO: Self = Self(0);

    /// Creates a new price from a raw integer value.
    #[must_use]
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Creates a validated price from a raw integer value.
    pub fn try_new(value: u128) -> Result<Self, PriceLevelError> {
        Ok(Self(value))
    }

    /// Creates a price from an `f64` value.
    pub fn from_f64(value: f64) -> Result<Self, PriceLevelError> {
        if !value.is_finite() || value < 0.0 {
            return Err(PriceLevelError::InvalidOperation {
                message: format!("invalid price from f64: {value}"),
            });
        }

        Ok(Self(value.round() as u128))
    }

    /// Converts the price to `f64` with potential precision loss.
    #[must_use]
    pub fn to_f64_lossy(self) -> f64 {
        self.0 as f64
    }

    /// Returns the inner raw value.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Price {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u128>()
            .map(Self)
            .map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "price".to_string(),
                value: s.to_string(),
            })
    }
}

/// Domain value type representing a quantity.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Quantity(u64);

impl Quantity {
    /// Zero quantity value.
    pub const ZERO: Self = Self(0);

    /// Creates a new quantity from a raw integer value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Creates a validated quantity from a raw integer value.
    pub fn try_new(value: u64) -> Result<Self, PriceLevelError> {
        Ok(Self(value))
    }

    /// Creates a quantity from an `f64` value.
    pub fn from_f64(value: f64) -> Result<Self, PriceLevelError> {
        if !value.is_finite() || value < 0.0 {
            return Err(PriceLevelError::InvalidOperation {
                message: format!("invalid quantity from f64: {value}"),
            });
        }

        Ok(Self(value.round() as u64))
    }

    /// Converts the quantity to `f64` with potential precision loss.
    #[must_use]
    pub fn to_f64_lossy(self) -> f64 {
        self.0 as f64
    }

    /// Returns the inner raw value.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Quantity {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "quantity".to_string(),
                value: s.to_string(),
            })
    }
}

/// Domain value type representing a timestamp in milliseconds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TimestampMs(u64);

impl TimestampMs {
    /// Zero timestamp value.
    pub const ZERO: Self = Self(0);

    /// Creates a new timestamp from milliseconds.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Creates a validated timestamp from milliseconds.
    pub fn try_new(value: u64) -> Result<Self, PriceLevelError> {
        Ok(Self(value))
    }

    /// Returns the inner raw milliseconds value.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TimestampMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TimestampMs {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "timestamp".to_string(),
                value: s.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Price, Quantity, TimestampMs};
    use std::str::FromStr;

    #[test]
    fn price_roundtrip() {
        let value = Price::new(1_000);
        let parsed = Price::from_str(&value.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap_or_default(), value);
    }

    #[test]
    fn quantity_roundtrip() {
        let value = Quantity::new(42);
        let parsed = Quantity::from_str(&value.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap_or_default(), value);
    }

    #[test]
    fn timestamp_roundtrip() {
        let value = TimestampMs::new(1_716_000_000_000);
        let parsed = TimestampMs::from_str(&value.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap_or_default(), value);
    }

    #[test]
    fn from_f64_rejects_negative() {
        assert!(Price::from_f64(-1.0).is_err());
        assert!(Quantity::from_f64(-1.0).is_err());
    }
}
