use crate::errors::PriceLevelError;
use crate::orders::{Id, OrderType};
use crossbeam_skiplist::SkipMap;
use dashmap::DashMap;
use serde::de::{SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::fmt::Display;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A thread-safe queue of orders with specialized operations.
///
/// Time priority (price-time / FIFO within the level) is maintained by an
/// ordered index keyed by a monotonic insertion sequence rather than a plain
/// tail-only FIFO. This lets a partially-filled maker keep its place at the
/// front of the queue: the residual is re-inserted at its *original* sequence,
/// instead of being appended to the tail.
#[derive(Debug)]
pub struct OrderQueue {
    /// A map of order IDs to `(insertion sequence, order)` for O(1) lookups.
    /// The sequence travels with the value so it can be recovered on pop and
    /// reused when re-inserting a partial-fill residual.
    orders: DashMap<Id, (u64, Arc<OrderType<()>>)>,
    /// Ordered index `sequence -> Id`. The lowest sequence is the front
    /// (oldest) order, so iteration / pop honours strict time priority.
    index: SkipMap<u64, Id>,
    /// Monotonic source of insertion sequences.
    next_seq: AtomicU64,
}

impl OrderQueue {
    /// Create a new empty order queue
    #[must_use]
    pub fn new() -> Self {
        Self {
            orders: DashMap::new(),
            index: SkipMap::new(),
            next_seq: AtomicU64::new(0),
        }
    }

    /// Add an order to the tail of the queue (newest time priority).
    pub fn push(&self, order: Arc<OrderType<()>>) {
        // `Relaxed` is sufficient: only the uniqueness and monotonicity of the
        // counter matter. The happens-before ordering between concurrent
        // producers/consumers is provided by the lock-free `index`/`orders`
        // structures, not by this counter, so no synchronization rides on it.
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let order_id = order.id();
        self.orders.insert(order_id, (seq, order));
        self.index.insert(seq, order_id);
    }

    /// Pop the front (oldest) order together with its insertion sequence.
    ///
    /// The sequence is returned so the caller can re-insert a partial-fill
    /// residual at the *same* position via [`OrderQueue::reinsert`], preserving
    /// strict price-time priority.
    #[must_use]
    pub(crate) fn pop_entry(&self) -> Option<(u64, Arc<OrderType<()>>)> {
        loop {
            // `pop_front` atomically removes the lowest-sequence index entry.
            let entry = self.index.pop_front()?;
            let order_id = *entry.value();
            // The id may have been concurrently cancelled via `remove`; in that
            // case the map no longer holds it, so skip it and try the next one.
            if let Some((_, (seq, order))) = self.orders.remove(&order_id) {
                return Some((seq, order));
            }
        }
    }

    /// Attempt to pop an order from the queue (front / oldest first).
    #[must_use]
    pub fn pop(&self) -> Option<Arc<OrderType<()>>> {
        self.pop_entry().map(|(_, order)| order)
    }

    /// Re-insert an order at a given (previously assigned) insertion sequence.
    ///
    /// Used for a partial-fill residual: re-inserting at the maker's original
    /// sequence returns it to the front of the queue, keeping its time priority
    /// ahead of orders that arrived later.
    pub(crate) fn reinsert(&self, seq: u64, order: Arc<OrderType<()>>) {
        let order_id = order.id();
        self.orders.insert(order_id, (seq, order));
        self.index.insert(seq, order_id);
    }

    /// Search for an order with the given ID. O(1) operation.
    #[must_use]
    pub fn find(&self, order_id: Id) -> Option<Arc<OrderType<()>>> {
        self.orders.get(&order_id).map(|o| o.value().1.clone())
    }

    /// Remove an order with the given ID.
    /// Returns the removed order if found. Cleans both the map and the index.
    #[must_use]
    pub fn remove(&self, order_id: Id) -> Option<Arc<OrderType<()>>> {
        let (_, (seq, order)) = self.orders.remove(&order_id)?;
        self.index.remove(&seq);
        Some(order)
    }

    /// Iterate through current orders without materializing an intermediate vector.
    pub fn iter_orders(&self) -> impl Iterator<Item = Arc<OrderType<()>>> + '_ {
        self.orders.iter().map(|entry| entry.value().1.clone())
    }

    /// Materialize a stable snapshot vector sorted by `(timestamp, sequence)`.
    ///
    /// The insertion sequence is used as a deterministic tiebreak so orders
    /// sharing a millisecond timestamp are still ordered exactly as matching
    /// would consume them. Note the sequence itself is not serialized: a
    /// snapshot round-trip reconstructs queue order from `(timestamp,
    /// sequence)`, so exact price-time priority survives a round-trip only when
    /// timestamps are monotonic with insertion order (the normal case).
    #[must_use]
    pub fn snapshot_vec(&self) -> Vec<Arc<OrderType<()>>> {
        let mut orders: Vec<(u64, Arc<OrderType<()>>)> =
            self.orders.iter().map(|o| o.value().clone()).collect();
        orders.sort_by_key(|(seq, o)| (o.timestamp(), *seq));
        orders.into_iter().map(|(_, o)| o).collect()
    }

    /// Convert the queue to a vector (for compatibility and snapshots).
    #[must_use]
    pub fn to_vec(&self) -> Vec<Arc<OrderType<()>>> {
        self.snapshot_vec()
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
    #[must_use]
    pub fn from_vec(orders: Vec<Arc<OrderType<()>>>) -> Self {
        let queue = OrderQueue::new();
        for order in orders {
            queue.push(order);
        }
        queue
    }

    /// Check if the queue is empty
    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Returns the number of orders currently in the queue.
    ///
    /// # Returns
    ///
    /// * `usize` - The total count of orders in the queue.
    ///
    #[must_use]
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
        // Emit in insertion-sequence order so the round-trip preserves time
        // priority (the DashMap alone has no deterministic iteration order).
        for index_entry in self.index.iter() {
            if let Some(order_entry) = self.orders.get(index_entry.value()) {
                seq.serialize_element(order_entry.value().1.as_ref())?;
            }
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
                let order =
                    OrderType::from_str(order_str).map_err(|e| PriceLevelError::ParseError {
                        message: format!("Order parse error: {e}"),
                    })?;
                queue.push(Arc::new(order));
            }
        }

        Ok(queue)
    }
}

impl Display for OrderQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OrderQueue:orders=[")?;
        let mut first = true;
        for order in self.snapshot_vec() {
            if !first {
                write!(f, ",")?;
            }
            write!(f, "{order}")?;
            first = false;
        }
        write!(f, "]")
    }
}

impl From<Vec<Arc<OrderType<()>>>> for OrderQueue {
    fn from(orders: Vec<Arc<OrderType<()>>>) -> Self {
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
        while let Some(order) = seq.next_element::<OrderType<()>>()? {
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
