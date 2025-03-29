use crate::errors::PriceLevelError;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Represents the current status of an order in the system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order has been created but not yet processed
    New,

    /// Order is active in the order book
    Active,

    /// Order has been partially filled
    PartiallyFilled,

    /// Order has been completely filled
    Filled,

    /// Order has been canceled by the user
    Canceled,

    /// Order has been rejected by the system
    Rejected,

    /// Order has expired (for time-bounded orders)
    Expired,
}

impl OrderStatus {
    /// Returns true if the order is still active in the book
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active | Self::PartiallyFilled)
    }

    /// Returns true if the order has been terminated
    /// (filled, canceled, rejected, or expired)
    #[allow(dead_code)]
    pub fn is_terminated(&self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Canceled | Self::Rejected | Self::Expired
        )
    }
}

impl FromStr for OrderStatus {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "NEW" => Ok(OrderStatus::New),
            "ACTIVE" => Ok(OrderStatus::Active),
            "PARTIALLYFILLED" => Ok(OrderStatus::PartiallyFilled),
            "FILLED" => Ok(OrderStatus::Filled),
            "CANCELED" => Ok(OrderStatus::Canceled),
            "REJECTED" => Ok(OrderStatus::Rejected),
            "EXPIRED" => Ok(OrderStatus::Expired),
            _ => Err(PriceLevelError::ParseError {
                message: format!("Invalid OrderStatus: {}", s),
            }),
        }
    }
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderStatus::New => write!(f, "NEW"),
            OrderStatus::Active => write!(f, "ACTIVE"),
            OrderStatus::PartiallyFilled => write!(f, "PARTIALLYFILLED"),
            OrderStatus::Filled => write!(f, "FILLED"),
            OrderStatus::Canceled => write!(f, "CANCELED"),
            OrderStatus::Rejected => write!(f, "REJECTED"),
            OrderStatus::Expired => write!(f, "EXPIRED"),
        }
    }
}
