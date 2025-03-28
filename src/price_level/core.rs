//! Core price level implementation

use crate::orders::{OrderId, OrderType};
use crate::price_level::{PriceLevelSnapshot, PriceLevelStatistics};
use crossbeam::queue::SegQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

    /// Get an iterator over all orders (creates a copy)
    pub fn iter_orders(&self) -> impl Iterator<Item = Arc<OrderType>> {
        let mut temp_storage = Vec::new();

        // Drain the queue
        while let Some(order) = self.orders.pop() {
            temp_storage.push(order.clone());
            self.orders.push(order);
        }

        // Return an iterator over the copied orders
        temp_storage.into_iter()
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
                            // Full match
                            remaining -= *quantity;
                            self.visible_quantity.fetch_sub(*quantity, Ordering::AcqRel);
                            self.order_count.fetch_sub(1, Ordering::AcqRel);
                            matched_orders.push(order_arc.clone());

                            // Record statistics
                            self.stats
                                .record_execution(*quantity, *price, order_arc.timestamp());
                        } else {
                            // Partial match
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Create updated order with reduced quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(*quantity - executed);

                            // Record statistics
                            self.stats
                                .record_execution(executed, *price, order_arc.timestamp());

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
                            // Visible portion is fully matched
                            remaining -= *visible_quantity;
                            self.visible_quantity
                                .fetch_sub(*visible_quantity, Ordering::AcqRel);

                            // Record execution
                            self.stats.record_execution(
                                *visible_quantity,
                                *price,
                                order_arc.timestamp(),
                            );

                            if *hidden_quantity > 0 {
                                // Refresh visible portion from hidden
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
                                    self.orders.push(Arc::new(updated_order));
                                } else {
                                    // No more quantity left
                                    self.order_count.fetch_sub(1, Ordering::AcqRel);
                                    matched_orders.push(order_arc.clone());
                                }
                            } else {
                                // No hidden quantity left
                                self.order_count.fetch_sub(1, Ordering::AcqRel);
                                matched_orders.push(order_arc.clone());
                            }
                        } else {
                            // Partial match of visible portion
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Record execution
                            self.stats
                                .record_execution(executed, *price, order_arc.timestamp());

                            // Create updated order with reduced visible quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(*visible_quantity - executed);

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
                            // Visible portion is fully matched
                            remaining -= *visible_quantity;
                            self.visible_quantity
                                .fetch_sub(*visible_quantity, Ordering::AcqRel);

                            // Record execution
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
                                    self.orders.push(Arc::new(updated_order));
                                } else {
                                    // No more quantity left
                                    self.order_count.fetch_sub(1, Ordering::AcqRel);
                                    matched_orders.push(order_arc.clone());
                                }
                            } else {
                                // Either no hidden quantity or not below threshold
                                if *hidden_quantity == 0 {
                                    self.order_count.fetch_sub(1, Ordering::AcqRel);
                                    matched_orders.push(order_arc.clone());
                                } else {
                                    // Put it back in the queue
                                    self.orders.push(order_arc);
                                }
                            }
                        } else {
                            // Partial match of visible portion
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Record execution
                            self.stats
                                .record_execution(executed, *price, order_arc.timestamp());

                            // Create updated order with reduced visible quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(*visible_quantity - executed);

                            self.orders.push(Arc::new(updated_order));
                            break;
                        }
                    }
                    // Handle other order types or use a default behavior
                    _ => {
                        let visible_qty = order_arc.visible_quantity();
                        let price = order_arc.price();

                        if visible_qty <= remaining {
                            // Full match
                            remaining -= visible_qty;
                            self.visible_quantity
                                .fetch_sub(visible_qty, Ordering::AcqRel);
                            self.order_count.fetch_sub(1, Ordering::AcqRel);

                            // Record execution
                            self.stats
                                .record_execution(visible_qty, price, order_arc.timestamp());

                            matched_orders.push(order_arc.clone());
                        } else {
                            // Partial match
                            let executed = remaining;
                            remaining = 0;
                            self.visible_quantity.fetch_sub(executed, Ordering::AcqRel);

                            // Record execution
                            self.stats
                                .record_execution(executed, price, order_arc.timestamp());

                            // Create updated order with reduced quantity
                            let updated_order =
                                order_arc.with_reduced_quantity(visible_qty - executed);

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

        // Return remaining quantity
        remaining
    }

    /// Create a snapshot of the current price level state
    pub fn snapshot(&self) -> PriceLevelSnapshot {
        PriceLevelSnapshot {
            price: self.price,
            visible_quantity: self.visible_quantity(),
            hidden_quantity: self.hidden_quantity(),
            order_count: self.order_count(),
            orders: self.iter_orders().collect(),
        }
    }
}
