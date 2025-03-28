use crate::orders::{OrderId, Side};
use std::time::{SystemTime, UNIX_EPOCH};

/// Represents a completed transaction between two orders
#[derive(Debug, Clone)]
pub struct Transaction {
    /// Unique transaction ID
    pub transaction_id: u64,

    /// ID of the aggressive order that caused the match
    pub taker_order_id: OrderId,

    /// ID of the passive order that was in the book
    pub maker_order_id: OrderId,

    /// Price at which the transaction occurred
    pub price: u64,

    /// Quantity that was traded
    pub quantity: u64,

    /// Side of the taker order
    pub taker_side: Side,

    /// Timestamp when the transaction occurred
    pub timestamp: u64,
}

impl Transaction {
    /// Create a new transaction
    pub fn new(
        transaction_id: u64,
        taker_order_id: OrderId,
        maker_order_id: OrderId,
        price: u64,
        quantity: u64,
        taker_side: Side,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis() as u64;

        Self {
            transaction_id,
            taker_order_id,
            maker_order_id,
            price,
            quantity,
            taker_side,
            timestamp,
        }
    }

    /// Returns the side of the maker order
    pub fn maker_side(&self) -> Side {
        match self.taker_side {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    /// Returns the total value of this transaction
    pub fn total_value(&self) -> u64 {
        self.price * self.quantity
    }
}
