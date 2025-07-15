use crate::errors::PriceLevelError;
use crate::orders::{OrderId, OrderType};
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use serde::de::{SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::fmt::Display;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Arc;

/// A thread-safe queue of orders with specialized operations
#[derive(Debug)]
pub struct OrderQueue {
    /// A map of order IDs to orders for quick lookups
    orders: DashMap<OrderId, Arc<OrderType>>,
    /// A queue of order IDs to maintain FIFO order
    order_ids: SegQueue<OrderId>,
}

impl OrderQueue {
    /// Create a new empty order queue
    pub fn new() -> Self {
        Self {
            orders: DashMap::new(),
            order_ids: SegQueue::new(),
        }
    }

    /// Add an order to the queue
    pub fn push(&self, order: Arc<OrderType>) {
        let order_id = order.id();
        self.orders.insert(order_id, order);
        self.order_ids.push(order_id);
    }

    /// Attempt to pop an order from the queue
    pub fn pop(&self) -> Option<Arc<OrderType>> {
        loop {
            if let Some(order_id) = self.order_ids.pop() {
                // If the order was removed, pop will return None, but the ID was in the queue.
                // In this case, we loop and try to get the next one.
                if let Some((_, order)) = self.orders.remove(&order_id) {
                    return Some(order);
                }
            } else {
                return None; // Queue is empty
            }
        }
    }

    /// Search for an order with the given ID. O(1) operation.
    pub fn find(&self, order_id: OrderId) -> Option<Arc<OrderType>> {
        self.orders.get(&order_id).map(|o| o.value().clone())
    }

    /// Remove an order with the given ID
    /// Returns the removed order if found. O(1) for the map, but the ID remains in the queue.
    pub fn remove(&self, order_id: OrderId) -> Option<Arc<OrderType>> {
        self.orders.remove(&order_id).map(|(_, order)| order)
    }

    /// Convert the queue to a vector (for snapshots)
    pub fn to_vec(&self) -> Vec<Arc<OrderType>> {
        let mut orders: Vec<Arc<OrderType>> = self.orders.iter().map(|o| o.value().clone()).collect();
        orders.sort_by_key(|o| o.timestamp());
        orders
    }

    /// Creates a new `OrderQueue` instance and populates it with orders from the provided vector.
    ///
    /// This function takes ownership of a vector of order references (wrapped in `Arc`) and constructs
    /// a new `OrderQueue` by iteratively pushing each order into the queue. The resulting queue
    /// maintains the insertion order of the original vector.
    ///
    /// # Parameters
    ///
    /// * `orders` - A vector of atomic reference counted (`Arc`) order instances representing
    ///   the orders to be added to the new queue.
    ///
    /// # Returns
    ///
    /// A new `OrderQueue` instance containing all the orders from the input vector.
    ///
    #[allow(dead_code)]
    pub fn from_vec(orders: Vec<Arc<OrderType>>) -> Self {
        let queue = OrderQueue::new();
        for order in orders {
            queue.push(order);
        }
        queue
    }

    /// Check if the queue is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Returns the number of orders currently in the queue.
    ///
    /// # Returns
    ///
    /// * `usize` - The total count of orders in the queue.
    ///
    pub fn len(&self) -> usize {
        self.orders.len()
    }
}

impl Default for OrderQueue {
    fn default() -> Self {
        Self::new()
    }
}
// Implement serialization for OrderQueue
impl Serialize for OrderQueue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for order_entry in self.orders.iter() {
            seq.serialize_element(order_entry.value().as_ref())?;
        }
        seq.end()
    }
}

impl FromStr for OrderQueue {
    type Err = PriceLevelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with("OrderQueue:orders=[") || !s.ends_with(']') {
            return Err(PriceLevelError::ParseError {
                message: "Invalid format".to_string(),
            });
        }

        let content = &s["OrderQueue:orders=[".len()..s.len() - 1];
        let queue = OrderQueue::new();

        if !content.is_empty() {
            for order_str in content.split(',') {
                let order = OrderType::from_str(order_str).map_err(|e| {
                    PriceLevelError::ParseError {
                        message: format!("Order parse error: {}", e),
                    }
                })?;
                queue.push(Arc::new(order));
            }
        }

        Ok(queue)
    }
}

impl Display for OrderQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let orders_str: Vec<String> = self.to_vec().iter().map(|o| o.to_string()).collect();
        write!(f, "OrderQueue:orders=[{}]", orders_str.join(","))
    }
}

impl From<Vec<Arc<OrderType>>> for OrderQueue {
    fn from(orders: Vec<Arc<OrderType>>) -> Self {
        let queue = OrderQueue::new();
        for order in orders {
            queue.push(order);
        }
        queue
    }
}

// Custom visitor for deserializing OrderQueue
struct OrderQueueVisitor {
    marker: PhantomData<fn() -> OrderQueue>,
}

impl OrderQueueVisitor {
    fn new() -> Self {
        OrderQueueVisitor {
            marker: PhantomData,
        }
    }
}

impl<'de> Visitor<'de> for OrderQueueVisitor {
    type Value = OrderQueue;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a sequence of orders")
    }

    fn visit_seq<V>(self, mut seq: V) -> Result<OrderQueue, V::Error>
    where
        V: SeqAccess<'de>,
    {
        let queue = OrderQueue::new();

        // Deserialize each order and add it to the queue
        while let Some(order) = seq.next_element::<OrderType>()? {
            queue.push(Arc::new(order));
        }

        Ok(queue)
    }
}

// Implement deserialization for OrderQueue
impl<'de> Deserialize<'de> for OrderQueue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize as a sequence of orders
        deserializer.deserialize_seq(OrderQueueVisitor::new())

        // Alternative approach: Deserialize as OrderQueueData first, then convert
        // let data = OrderQueueData::deserialize(deserializer)?;
        // let queue = OrderQueue::new();
        // for order in data.orders {
        //     queue.push(Arc::new(order));
        // }
        // Ok(queue)
    }
}
