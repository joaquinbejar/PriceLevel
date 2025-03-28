/// Specifies how long an order remains active before it is executed or expires
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeInForce {
    /// Good Till Canceled - remains active until explicitly canceled
    GTC,

    /// Immediate Or Cancel - must be filled immediately (partially or completely) or canceled
    IOC,

    /// Fill Or Kill - must be filled completely immediately or canceled entirely
    FOK,

    /// Good Till Date - remains active until a specified date/time
    GTD(u64), // timestamp in milliseconds

    /// Day Order - valid only for the current trading day
    Day,
}

impl TimeInForce {
    /// Returns true if the order should be canceled after attempting to match
    pub fn is_immediate(&self) -> bool {
        matches!(self, Self::IOC | Self::FOK)
    }

    /// Returns true if the order has a specific expiration time
    pub fn has_expiry(&self) -> bool {
        matches!(self, Self::GTD(_) | Self::Day)
    }

    /// Checks if an order with this time in force has expired
    pub fn is_expired(&self, current_timestamp: u64, market_close_timestamp: Option<u64>) -> bool {
        match self {
            Self::GTD(expiry) => current_timestamp >= *expiry,
            Self::Day => {
                if let Some(close) = market_close_timestamp {
                    current_timestamp >= close
                } else {
                    false
                }
            },
            _ => false,
        }
    }
}