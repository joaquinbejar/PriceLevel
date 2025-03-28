//! Base order definitions

/// Represents the side of an order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Buy side (bids)
    Buy,
    /// Sell side (asks)
    Sell,
}

/// Represents a unique order ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderId(pub u64);

/// Represents a basic order in the limit order book
#[derive(Debug, Clone)]
pub struct Order {
    /// Unique identifier for this order
    pub id: OrderId,
    /// Price level for this order (in minor currency units, e.g. cents)
    pub price: u64,
    /// Quantity of the order (in minor units)
    pub quantity: u64,
    /// Side of the order (buy or sell)
    pub side: Side,
    /// Timestamp when the order was created (in milliseconds since epoch)
    pub timestamp: u64,
}

impl Order {
    /// Create a new order
    pub fn new(id: u64, price: u64, quantity: u64, side: Side, timestamp: u64) -> Self {
        Self {
            id: OrderId(id),
            price,
            quantity,
            side,
            timestamp,
        }
    }

    /// Create a new buy order
    pub fn buy(id: u64, price: u64, quantity: u64, timestamp: u64) -> Self {
        Self::new(id, price, quantity, Side::Buy, timestamp)
    }

    /// Create a new sell order
    pub fn sell(id: u64, price: u64, quantity: u64, timestamp: u64) -> Self {
        Self::new(id, price, quantity, Side::Sell, timestamp)
    }
}
