use crate::errors::PriceLevelError;
use crate::utils::text::{Echo, uppercases_to};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Represents the current status of an order in the system
#[repr(u8)]
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
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active | Self::PartiallyFilled)
    }

    /// Returns true if the order has been terminated
    /// (filled, canceled, rejected, or expired)
    #[allow(dead_code)]
    #[must_use]
    pub fn is_terminated(&self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Canceled | Self::Rejected | Self::Expired
        )
    }
}

/// Case-insensitive: accepts exactly the inputs whose `str::to_uppercase` is
/// one of the `Display` forms, matched without allocating (see
/// `utils::text::uppercases_to`; this includes Unicode folds such as
/// `ﬁlled`). The error message echoes at most a bounded prefix of the input.
impl FromStr for OrderStatus {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const FORMS: [(&str, OrderStatus); 7] = [
            ("NEW", OrderStatus::New),
            ("ACTIVE", OrderStatus::Active),
            ("PARTIALLYFILLED", OrderStatus::PartiallyFilled),
            ("FILLED", OrderStatus::Filled),
            ("CANCELED", OrderStatus::Canceled),
            ("REJECTED", OrderStatus::Rejected),
            ("EXPIRED", OrderStatus::Expired),
        ];
        FORMS
            .iter()
            .find(|(upper, _)| uppercases_to(s, upper))
            .map(|&(_, status)| status)
            .ok_or_else(|| PriceLevelError::ParseError {
                message: format!("Invalid OrderStatus: {}", Echo(s)),
            })
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
