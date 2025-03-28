use crate::errors::PriceLevelError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Specifies how long an order remains active before it is executed or expires
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    #[serde(rename(serialize = "GTC"))]
    #[serde(alias = "gtc", alias = "Gtc", alias = "GTC")]
    Gtc,

    #[serde(rename(serialize = "IOC"))]
    #[serde(alias = "ioc", alias = "Ioc", alias = "IOC")]
    Ioc,

    #[serde(rename(serialize = "FOK"))]
    #[serde(alias = "fok", alias = "Fok", alias = "FOK")]
    Fok,

    #[serde(rename(serialize = "GTD"))]
    #[serde(alias = "gtd", alias = "Gtd", alias = "GTD")]
    Gtd(u64),

    #[serde(rename(serialize = "DAY"))]
    #[serde(alias = "day", alias = "Day", alias = "DAY")]
    Day,
}

impl TimeInForce {
    /// Returns true if the order should be canceled after attempting to match
    pub fn is_immediate(&self) -> bool {
        matches!(self, Self::Ioc | Self::Fok)
    }

    /// Returns true if the order has a specific expiration time
    pub fn has_expiry(&self) -> bool {
        matches!(self, Self::Gtd(_) | Self::Day)
    }

    /// Checks if an order with this time in force has expired
    pub fn is_expired(&self, current_timestamp: u64, market_close_timestamp: Option<u64>) -> bool {
        match self {
            Self::Gtd(expiry) => current_timestamp >= *expiry,
            Self::Day => {
                if let Some(close) = market_close_timestamp {
                    current_timestamp >= close
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

impl fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeInForce::Gtc => write!(f, "GTC"),
            TimeInForce::Ioc => write!(f, "IOC"),
            TimeInForce::Fok => write!(f, "FOK"),
            TimeInForce::Gtd(expiry) => write!(f, "GTD-{}", expiry),
            TimeInForce::Day => write!(f, "DAY"),
        }
    }
}

impl FromStr for TimeInForce {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GTC" => Ok(TimeInForce::Gtc),
            "IOC" => Ok(TimeInForce::Ioc),
            "FOK" => Ok(TimeInForce::Fok),
            "DAY" => Ok(TimeInForce::Day),
            s if s.starts_with("GTD-") => {
                let parts: Vec<&str> = s.split('-').collect();
                if parts.len() != 2 {
                    return Err(PriceLevelError::ParseError {
                        message: format!("Invalid GTD format: {}", s),
                    });
                }

                match parts[1].parse::<u64>() {
                    Ok(expiry) => Ok(TimeInForce::Gtd(expiry)),
                    Err(_) => Err(PriceLevelError::ParseError {
                        message: format!("Invalid expiry timestamp in GTD: {}", parts[1]),
                    }),
                }
            }
            _ => Err(PriceLevelError::ParseError {
                message: format!("Invalid TimeInForce: {}", s),
            }),
        }
    }
}
