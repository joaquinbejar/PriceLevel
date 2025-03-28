use crate::errors::PriceLevelError;
use crate::orders::{OrderId, OrderType};
use crossbeam::queue::SegQueue;
use serde::de::{SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Arc;

/// A thread-safe queue of orders with specialized operations
#[derive(Debug)]
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
        let mut temp_storage = Vec::new();

        while let Some(order) = self.queue.pop() {
            temp_storage.push(order);
        }

        for order in &temp_storage {
            self.queue.push(order.clone());
        }

        temp_storage
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

/// Serializable representation of OrderQueue for easier serialization/deserialization
#[derive(Debug, Serialize, Deserialize)]
pub struct OrderQueueData {
    /// Orders in the queue
    pub orders: Vec<OrderType>,
}

impl From<&OrderQueue> for OrderQueueData {
    fn from(queue: &OrderQueue) -> Self {
        Self {
            orders: queue
                .to_vec()
                .into_iter()
                .map(|order_arc| (*order_arc))
                .collect(),
        }
    }
}

impl fmt::Display for OrderQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let orders = self.to_vec();

        write!(f, "OrderQueue:orders=[")?;

        for (idx, order) in orders.iter().enumerate() {
            if idx > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}", order)?;
        }

        write!(f, "]")
    }
}

impl FromStr for OrderQueue {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check if the string starts with "OrderQueue:orders=["
        if !s.starts_with("OrderQueue:orders=[") {
            return Err(PriceLevelError::InvalidFormat);
        }

        // Extract the orders content between the brackets
        let orders_start = s.find('[').ok_or(PriceLevelError::InvalidFormat)?;
        let orders_end = s.rfind(']').ok_or(PriceLevelError::InvalidFormat)?;

        if orders_start >= orders_end {
            return Err(PriceLevelError::InvalidFormat);
        }

        let orders_content = &s[orders_start + 1..orders_end];

        // Create a new queue
        let queue = OrderQueue::new();

        // Split the orders content by commas and parse each order
        if !orders_content.is_empty() {
            // We need to be careful with splitting because order strings might contain commas inside
            // For simplicity, we'll use a basic split but in a more complex scenario
            // you might need a more sophisticated parser
            let mut current_order = String::new();
            let mut bracket_depth = 0;

            for c in orders_content.chars() {
                match c {
                    '[' => {
                        bracket_depth += 1;
                        current_order.push(c);
                    }
                    ']' => {
                        bracket_depth -= 1;
                        current_order.push(c);
                    }
                    ',' if bracket_depth == 0 => {
                        // End of an order
                        if !current_order.is_empty() {
                            let order = OrderType::from_str(&current_order)
                                .map_err(|_| PriceLevelError::InvalidFormat)?;
                            queue.push(Arc::new(order));
                            current_order.clear();
                        }
                    }
                    _ => current_order.push(c),
                }
            }

            // Don't forget to process the last order
            if !current_order.is_empty() {
                let order = OrderType::from_str(&current_order)
                    .map_err(|_| PriceLevelError::InvalidFormat)?;
                queue.push(Arc::new(order));
            }
        }

        Ok(queue)
    }
}

// Implement serialization for OrderQueue
impl Serialize for OrderQueue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Convert to a serializable representation
        let data: OrderQueueData = self.into();

        // Option 1: Serialize as a sequence of orders
        let mut seq = serializer.serialize_seq(Some(data.orders.len()))?;
        for order in data.orders {
            seq.serialize_element(&order)?;
        }
        seq.end()

        // Option 2: Serialize as a structure with an orders field
        // data.serialize(serializer)
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
