//! Snapshot functionality for price levels

use crate::orders::OrderType;
use std::sync::Arc;

/// A snapshot of the price level's state at a point in time
#[derive(Debug)]
pub struct PriceLevelSnapshot {
    /// The price of this level
    pub price: u64,
    /// Total visible quantity at this level
    pub visible_quantity: u64,
    /// Total hidden quantity at this level
    pub hidden_quantity: u64,
    /// Number of orders at this level
    pub order_count: usize,
    /// Orders at this level
    pub orders: Vec<Arc<OrderType>>,
}

impl PriceLevelSnapshot {
    /// Create a new empty snapshot
    pub fn new(price: u64) -> Self {
        Self {
            price,
            visible_quantity: 0,
            hidden_quantity: 0,
            order_count: 0,
            orders: Vec::new(),
        }
    }

    /// Get the total quantity (visible + hidden) at this price level
    pub fn total_quantity(&self) -> u64 {
        self.visible_quantity + self.hidden_quantity
    }

    /// Get an iterator over the orders in this snapshot
    pub fn iter_orders(&self) -> impl Iterator<Item = &Arc<OrderType>> {
        self.orders.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_basic() {
        let snapshot = PriceLevelSnapshot::new(10000);
        assert_eq!(snapshot.price, 10000);
        assert_eq!(snapshot.visible_quantity, 0);
        assert_eq!(snapshot.order_count, 0);
        assert_eq!(snapshot.total_quantity(), 0);
    }
}
