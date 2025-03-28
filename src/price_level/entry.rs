use std::sync::Arc;
use crate::price_level::core::PriceLevel;

/// Represents a price level entry in the order book
#[derive(Debug)]
pub struct OrderBookEntry {
    /// The price level
    pub level: Arc<PriceLevel>,

    /// Index or position in the order book
    pub index: usize,
}

impl OrderBookEntry {
    /// Create a new order book entry
    pub fn new(level: Arc<PriceLevel>, index: usize) -> Self {
        Self { level, index }
    }

    /// Get the price of this entry
    pub fn price(&self) -> u64 {
        self.level.price()
    }

    /// Get the visible quantity at this entry
    pub fn visible_quantity(&self) -> u64 {
        self.level.visible_quantity()
    }

    /// Get the total quantity at this entry
    pub fn total_quantity(&self) -> u64 {
        self.level.total_quantity()
    }

    /// Get the order count at this entry
    pub fn order_count(&self) -> usize {
        self.level.order_count()
    }
}

impl PartialEq for OrderBookEntry {
    fn eq(&self, other: &Self) -> bool {
        self.price() == other.price()
    }
}

impl Eq for OrderBookEntry {}

impl PartialOrd for OrderBookEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderBookEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.price().cmp(&other.price())
    }
}