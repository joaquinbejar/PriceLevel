use crate::execution::transaction::Transaction;
use crate::orders::OrderId;

/// Represents the result of a matching operation
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// The ID of the incoming order that initiated the match
    pub order_id: OrderId,

    /// List of transactions that resulted from the match
    pub transactions: Vec<Transaction>,

    /// Remaining quantity of the incoming order after matching
    pub remaining_quantity: u64,

    /// Whether the order was completely filled
    pub is_complete: bool,

    /// Any orders that were completely filled and removed from the book
    pub filled_order_ids: Vec<OrderId>,
}

impl MatchResult {
    /// Create a new empty match result
    pub fn new(order_id: OrderId, initial_quantity: u64) -> Self {
        Self {
            order_id,
            transactions: Vec::new(),
            remaining_quantity: initial_quantity,
            is_complete: false,
            filled_order_ids: Vec::new(),
        }
    }

    /// Add a transaction to this match result
    pub fn add_transaction(&mut self, transaction: Transaction) {
        self.remaining_quantity = self.remaining_quantity.saturating_sub(transaction.quantity);
        self.is_complete = self.remaining_quantity == 0;
        self.transactions.push(transaction);
    }

    /// Add a filled order ID to track orders removed from the book
    pub fn add_filled_order_id(&mut self, order_id: OrderId) {
        self.filled_order_ids.push(order_id);
    }

    /// Get the total executed quantity
    pub fn executed_quantity(&self) -> u64 {
        self.transactions.iter().map(|t| t.quantity).sum()
    }

    /// Get the total value executed
    pub fn executed_value(&self) -> u64 {
        self.transactions.iter().map(|t| t.price * t.quantity).sum()
    }

    /// Calculate the average execution price
    pub fn average_price(&self) -> Option<f64> {
        let executed_qty = self.executed_quantity();
        if executed_qty == 0 {
            None
        } else {
            Some(self.executed_value() as f64 / executed_qty as f64)
        }
    }
}
