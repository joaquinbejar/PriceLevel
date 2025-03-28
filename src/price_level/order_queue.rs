use std::sync::Arc;
use crossbeam::queue::SegQueue;
use crate::orders::{OrderType, OrderId};

/// A thread-safe queue of orders with specialized operations
pub struct OrderQueue {
    /// The underlying lock-free queue from crossbeam
    queue: SegQueue<Arc<OrderType>>,
}

impl OrderQueue {
    /// Create a new empty order queue
    pub fn new() -> Self {
        Self {
            queue: SegQueue::new(),
        }
    }

    /// Add an order to the queue
    pub fn push(&self, order: Arc<OrderType>) {
        self.queue.push(order);
    }

    /// Attempt to pop an order from the queue
    pub fn pop(&self) -> Option<Arc<OrderType>> {
        self.queue.pop()
    }

    /// Search for an order with the given ID
    /// Note: This is an O(n) operation that will recreate the queue
    pub fn find(&self, order_id: OrderId) -> Option<Arc<OrderType>> {
        let mut temp_storage = Vec::new();
        let mut found_order = None;

        // Pop all items from the queue
        while let Some(order) = self.queue.pop() {
            if order.id() == order_id {
                found_order = Some(order.clone());
            }
            temp_storage.push(order);
        }

        // Push back all orders
        for order in temp_storage {
            self.queue.push(order);
        }

        found_order
    }

    /// Remove an order with the given ID
    /// Returns the removed order if found
    pub fn remove(&self, order_id: OrderId) -> Option<Arc<OrderType>> {
        let mut temp_storage = Vec::new();
        let mut removed_order = None;

        // Pop all items from the queue
        while let Some(order) = self.queue.pop() {
            if order.id() == order_id {
                removed_order = Some(order);
            } else {
                temp_storage.push(order);
            }
        }

        // Push back the orders we want to keep
        for order in temp_storage {
            self.queue.push(order);
        }

        removed_order
    }

    /// Convert the queue to a vector (for snapshots)
    /// Note: This is a destructive operation that will recreate the queue
    pub fn to_vec(&self) -> Vec<Arc<OrderType>> {
        let mut result = Vec::new();

        // Pop all items from the queue
        while let Some(order) = self.queue.pop() {
            result.push(order.clone());
            self.queue.push(order);
        }

        result
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        // This is a heuristic and not guaranteed to be accurate in a concurrent environment
        let mut is_empty = true;
        if let Some(order) = self.queue.pop() {
            self.queue.push(order);
            is_empty = false;
        }
        is_empty
    }
}

impl Default for OrderQueue {
    fn default() -> Self {
        Self::new()
    }
}