
/// Represents the current status of an order in the system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active | Self::PartiallyFilled)
    }

    /// Returns true if the order has been terminated 
    /// (filled, canceled, rejected, or expired)
    pub fn is_terminated(&self) -> bool {
        matches!(self, Self::Filled | Self::Canceled | Self::Rejected | Self::Expired)
    }
}