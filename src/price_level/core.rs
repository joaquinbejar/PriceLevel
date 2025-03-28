//! Core price level implementation

use std::fmt;
use std::str::FromStr;
use crate::orders::{OrderId, OrderType};
use crate::price_level::{PriceLevelSnapshot, PriceLevelStatistics};
use crossbeam::queue::SegQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use serde::{Deserialize, Serialize};
use crate::errors::PriceLevelError;

/// A lock-free implementation of a price level in a limit order book
#[derive(Debug)]
pub struct PriceLevel {
    /// The price of this level
    price: u64,

    /// Total visible quantity at this price level
    visible_quantity: AtomicU64,

    /// Total hidden quantity at this price level
    hidden_quantity: AtomicU64,

    /// Number of orders at this price level
    order_count: AtomicUsize,

    /// Queue of orders at this price level
    orders: SegQueue<Arc<OrderType>>,

    /// Statistics for this price level
    stats: Arc<PriceLevelStatistics>,
}

impl PriceLevel {
    /// Create a new price level
    pub fn new(price: u64) -> Self {
        Self {
            price,
            visible_quantity: AtomicU64::new(0),
            hidden_quantity: AtomicU64::new(0),
            order_count: AtomicUsize::new(0),
            orders: SegQueue::new(),
            stats: Arc::new(PriceLevelStatistics::new()),
        }
    }

    /// Get the price of this level
    pub fn price(&self) -> u64 {
        self.price
    }

    /// Get the visible quantity
    pub fn visible_quantity(&self) -> u64 {
        self.visible_quantity.load(Ordering::Acquire)
    }

    /// Get the hidden quantity
    pub fn hidden_quantity(&self) -> u64 {
        self.hidden_quantity.load(Ordering::Acquire)
    }

    /// Get the total quantity (visible + hidden)
    pub fn total_quantity(&self) -> u64 {
        self.visible_quantity() + self.hidden_quantity()
    }

    /// Get the number of orders
    pub fn order_count(&self) -> usize {
        self.order_count.load(Ordering::Acquire)
    }

    /// Get the statistics for this price level
    pub fn stats(&self) -> Arc<PriceLevelStatistics> {
        self.stats.clone()
    }

    /// Add an order to this price level
    pub fn add_order(&self, order: OrderType) -> Arc<OrderType> {
        // Calculate quantities
        let visible_qty = order.visible_quantity();
        let hidden_qty = order.hidden_quantity();

        // Update atomic counters
        self.visible_quantity
            .fetch_add(visible_qty, Ordering::AcqRel);
        self.hidden_quantity.fetch_add(hidden_qty, Ordering::AcqRel);
        self.order_count.fetch_add(1, Ordering::AcqRel);

        // Update statistics
        self.stats.record_order_added();

        // Add to order queue
        let order_arc = Arc::new(order);
        self.orders.push(order_arc.clone());

        order_arc
    }

    /// Remove an order by ID
    pub fn remove_order(&self, order_id: OrderId) -> Option<Arc<OrderType>> {
        // Since SegQueue doesn't support direct removal, pop all elements and keep those we want
        let mut temp_storage = Vec::new();
        let mut removed_order = None;

        // Pop all items from the queue
        while let Some(order_arc) = self.orders.pop() {
            if order_arc.id() == order_id {
                // Found the order to remove
                let order_arc_clone = order_arc.clone();
                removed_order = Some(order_arc);

                // Update atomic counters
                let visible_qty = order_arc_clone.visible_quantity();
                let hidden_qty = order_arc_clone.hidden_quantity();

                self.visible_quantity
                    .fetch_sub(visible_qty, Ordering::AcqRel);
                self.hidden_quantity.fetch_sub(hidden_qty, Ordering::AcqRel);
                self.order_count.fetch_sub(1, Ordering::AcqRel);

                // Update statistics
                self.stats.record_order_removed();
            } else {
                // Keep this order
                temp_storage.push(order_arc);
            }
        }

        // Push back the orders we want to keep
        for order in temp_storage {
            self.orders.push(order);
        }

        removed_order
    }

    /// Creates an iterator over the orders in the price level.
    ///
    /// Note: This method temporarily drains and reconstructs the queue.
    /// In a high-concurrency environment, this might have performance implications.
    pub fn iter_orders(&self) -> Vec<Arc<OrderType>> {
        let mut temp_storage = Vec::new();

        while let Some(order) = self.orders.pop() {
            temp_storage.push(order);
        }

        for order in &temp_storage {
            self.orders.push(order.clone());
        }

        temp_storage
    }

    /// Process a matching order against this price level
    pub fn match_order(&self, incoming_quantity: u64) -> u64 {
        let mut remaining = incoming_quantity;
        let mut matched_orders = Vec::new();

        // Find orders to match
        while remaining > 0 {
            if let Some(order_arc) = self.orders.pop() {
                match &*order_arc {
                    OrderType::Standard {
                        quantity, price, ..
                    } => {
                        if *quantity <= remaining {
                            // Full match of the order
                            remaining -= *quantity;
                            self.visible_quantity.fetch_sub(*quantity, Ordering::AcqRel);
                            self.order_count.fetch_sub(1, Ordering::AcqRel);
                            matched_orders.push(order_arc.clone());

                            // Record execution statistics
                            self.stats
                                .record_execution(*quantity, *price, order_arc.timestamp());
                        } else {
                            // Partial match of the order
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Create an updated order with reduced quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(*quantity - executed);

                            // Record partial execution statistics
                            self.stats
                                .record_execution(executed, *price, order_arc.timestamp());

                            // Put the partially filled order back into the queue
                            self.orders.push(Arc::new(updated_order));
                            break;
                        }
                    }
                    OrderType::IcebergOrder {
                        visible_quantity,
                        hidden_quantity,
                        price,
                        ..
                    } => {
                        if *visible_quantity <= remaining {
                            // Fully match the visible portion of the iceberg order
                            remaining -= *visible_quantity;
                            self.visible_quantity
                                .fetch_sub(*visible_quantity, Ordering::AcqRel);

                            // Record execution statistics
                            self.stats.record_execution(
                                *visible_quantity,
                                *price,
                                order_arc.timestamp(),
                            );

                            if *hidden_quantity > 0 {
                                // Refresh visible portion from hidden quantity
                                let refresh_qty =
                                    std::cmp::min(*hidden_quantity, *visible_quantity);

                                // Update the order with refreshed quantities
                                let (updated_order, used_hidden) =
                                    order_arc.refresh_iceberg(refresh_qty);

                                // Update atomic counters
                                self.hidden_quantity
                                    .fetch_sub(used_hidden, Ordering::AcqRel);
                                self.visible_quantity
                                    .fetch_add(refresh_qty, Ordering::AcqRel);

                                if refresh_qty > 0 {
                                    // Put the updated order back into the queue
                                    self.orders.push(Arc::new(updated_order));
                                } else {
                                    // No more hidden quantity left
                                    self.order_count.fetch_sub(1, Ordering::AcqRel);
                                    matched_orders.push(order_arc.clone());
                                }
                            } else {
                                // No hidden quantity left
                                self.order_count.fetch_sub(1, Ordering::AcqRel);
                                matched_orders.push(order_arc.clone());
                            }
                        } else {
                            // Partially match the visible portion
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Record partial execution statistics
                            self.stats
                                .record_execution(executed, *price, order_arc.timestamp());

                            // Create an updated order with reduced visible quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(*visible_quantity - executed);

                            // Put the partially filled order back into the queue
                            self.orders.push(Arc::new(updated_order));
                            break;
                        }
                    }
                    OrderType::ReserveOrder {
                        visible_quantity,
                        hidden_quantity,
                        price,
                        replenish_threshold,
                        ..
                    } => {
                        if *visible_quantity <= remaining {
                            // Fully match the visible portion of the reserve order
                            remaining -= *visible_quantity;
                            self.visible_quantity
                                .fetch_sub(*visible_quantity, Ordering::AcqRel);

                            // Record execution statistics
                            self.stats.record_execution(
                                *visible_quantity,
                                *price,
                                order_arc.timestamp(),
                            );

                            // Check if we need to replenish
                            if *hidden_quantity > 0 && *visible_quantity <= *replenish_threshold {
                                // Replenish visible quantity from hidden
                                let refresh_qty =
                                    std::cmp::min(*hidden_quantity, *visible_quantity);

                                // Update the order
                                let (updated_order, used_hidden) =
                                    order_arc.refresh_iceberg(refresh_qty);

                                // Update atomic counters
                                self.hidden_quantity
                                    .fetch_sub(used_hidden, Ordering::AcqRel);
                                self.visible_quantity
                                    .fetch_add(refresh_qty, Ordering::AcqRel);

                                if refresh_qty > 0 {
                                    // Put the updated order back into the queue
                                    self.orders.push(Arc::new(updated_order));
                                } else {
                                    // No more quantity left
                                    self.order_count.fetch_sub(1, Ordering::AcqRel);
                                    matched_orders.push(order_arc.clone());
                                }
                            } else {
                                // Either no hidden quantity or not below threshold
                                if *hidden_quantity == 0 {
                                    // Remove the order completely
                                    self.order_count.fetch_sub(1, Ordering::AcqRel);
                                    matched_orders.push(order_arc.clone());
                                } else {
                                    // Put the order back into the queue
                                    self.orders.push(order_arc);
                                }
                            }
                        } else {
                            // Partially match the visible portion
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Record partial execution statistics
                            self.stats
                                .record_execution(executed, *price, order_arc.timestamp());

                            // Create an updated order with reduced visible quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(*visible_quantity - executed);

                            // Put the partially filled order back into the queue
                            self.orders.push(Arc::new(updated_order));
                            break;
                        }
                    }
                    // Handle other order types with a default behavior
                    _ => {
                        let visible_qty = order_arc.visible_quantity();
                        let price = order_arc.price();

                        if visible_qty <= remaining {
                            // Full match
                            remaining -= visible_qty;
                            self.visible_quantity
                                .fetch_sub(visible_qty, Ordering::AcqRel);
                            self.order_count.fetch_sub(1, Ordering::AcqRel);

                            // Record execution statistics
                            self.stats
                                .record_execution(visible_qty, price, order_arc.timestamp());

                            matched_orders.push(order_arc.clone());
                        } else {
                            // Partial match
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Record partial execution statistics
                            self.stats
                                .record_execution(executed, price, order_arc.timestamp());

                            // Create an updated order with reduced quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(visible_qty - executed);

                            // Put the partially filled order back into the queue
                            self.orders.push(Arc::new(updated_order));
                            break;
                        }
                    }
                }
            } else {
                // No more orders at this price level
                break;
            }
        }

        // Return the remaining quantity that couldn't be matched
        remaining
    }

    /// Create a snapshot of the current price level state
    /// Creates a snapshot of the current price level state
    pub fn snapshot(&self) -> PriceLevelSnapshot {
        PriceLevelSnapshot {
            price: self.price,
            visible_quantity: self.visible_quantity(),
            hidden_quantity: self.hidden_quantity(),
            order_count: self.order_count(),
            orders: self.iter_orders(),
        }
    }
}

/// Serializable representation of a price level for easier data transfer and storage
#[derive(Debug, Serialize, Deserialize)]
pub struct PriceLevelData {
    /// The price of this level
    pub price: u64,
    /// Total visible quantity at this price level
    pub visible_quantity: u64,
    /// Total hidden quantity at this price level
    pub hidden_quantity: u64,
    /// Number of orders at this price level
    pub order_count: usize,
    /// Orders at this price level
    pub orders: Vec<OrderType>,
}

impl From<&PriceLevel> for PriceLevelData {
    fn from(price_level: &PriceLevel) -> Self {
        Self {
            price: price_level.price(),
            visible_quantity: price_level.visible_quantity(),
            hidden_quantity: price_level.hidden_quantity(),
            order_count: price_level.order_count(),
            orders: price_level.iter_orders()
                .into_iter()
                .map(|order_arc| (*order_arc).clone())
                .collect(),
        }
    }
}

impl TryFrom<PriceLevelData> for PriceLevel {
    type Error = PriceLevelError;

    fn try_from(data: PriceLevelData) -> Result<Self, Self::Error> {
        let mut price_level = PriceLevel::new(data.price);

        // Add orders to the price level
        for order in data.orders {
            price_level.add_order(order);
        }

        Ok(price_level)
    }
}

impl fmt::Display for PriceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data: PriceLevelData = self.into();

        write!(f, "PriceLevel:price={};visible_quantity={};hidden_quantity={};order_count={};orders=[",
               data.price, data.visible_quantity, data.hidden_quantity, data.order_count)?;

        // Write orders
        for (idx, order) in data.orders.iter().enumerate() {
            if idx > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}", order)?;
        }

        write!(f, "]")
    }
}

impl FromStr for PriceLevel {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 || parts[0] != "PriceLevel" {
            return Err(PriceLevelError::InvalidFormat);
        }

        // Parse fields
        let mut fields = std::collections::HashMap::new();
        for field_pair in parts[1].split(';') {
            let kv: Vec<&str> = field_pair.split('=').collect();
            if kv.len() == 2 {
                fields.insert(kv[0], kv[1]);
            }
        }

        // Helper function to parse a field
        let get_field = |field: &str| -> Result<&str, PriceLevelError> {
            match fields.get(field) {
                Some(result) => Ok(*result),
                None => Err(PriceLevelError::MissingField(field.to_string())),
            }
        };

        // Parse required fields
        let price_str = get_field("price")?;
        let price = price_str.parse::<u64>().map_err(|_| PriceLevelError::InvalidFieldValue {
            field: "price".to_string(),
            value: price_str.to_string(),
        })?;

        // Create price level
        let mut price_level = PriceLevel::new(price);

        // Parse orders if they exist
        if let Ok(orders_str) = get_field("orders") {
            // Remove brackets
            let orders_str = orders_str.trim_matches(|c| c == '[' || c == ']');

            // Split and parse individual orders
            if !orders_str.is_empty() {
                for order_str in orders_str.split(',') {
                    let order = OrderType::from_str(order_str.trim())?;
                    price_level.add_order(order);
                }
            }
        }

        Ok(price_level)
    }
}

// Implement custom serialization for the atomic types
impl Serialize for PriceLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Convert to a serializable representation
        let data: PriceLevelData = self.into();
        data.serialize(serializer)
    }
}

// Implement custom deserialization for the PriceLevel
impl<'de> Deserialize<'de> for PriceLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize into the data representation
        let data = PriceLevelData::deserialize(deserializer)?;

        // Convert to PriceLevel
        PriceLevel::try_from(data).map_err(serde::de::Error::custom)
    }
}