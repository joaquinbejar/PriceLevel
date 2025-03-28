use serde::{Deserialize, Serialize};

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
