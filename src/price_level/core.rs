//! Core price level implementation

use std::fmt;
use std::str::FromStr;
use crate::orders::{OrderId, OrderType};
use crate::price_level::{PriceLevelSnapshot, PriceLevelStatistics};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use serde::{Deserialize, Serialize};
use crate::errors::PriceLevelError;
use crate::price_level::order_queue::OrderQueue;

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
    orders: OrderQueue,

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
            orders: OrderQueue::new(),
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
        let removed_order = self.orders.remove(order_id);

        if let Some(ref order_arc) = removed_order {
            // Update atomic counters
            let visible_qty = order_arc.visible_quantity();
            let hidden_qty = order_arc.hidden_quantity();

            self.visible_quantity.fetch_sub(visible_qty, Ordering::AcqRel);
            self.hidden_quantity.fetch_sub(hidden_qty, Ordering::AcqRel);
            self.order_count.fetch_sub(1, Ordering::AcqRel);

            // Update statistics
            self.stats.record_order_removed();
        }

        removed_order
    }

    /// Creates an iterator over the orders in the price level.
    pub fn iter_orders(&self) -> Vec<Arc<OrderType>> {
        self.orders.to_vec()
    }

    pub fn match_order(&self, incoming_quantity: u64) -> u64 {
        let mut remaining = incoming_quantity;

        while remaining > 0 {
            if let Some(order_arc) = self.orders.pop() {
                // Obtener resultados del matching
                let (consumed, updated_order, hidden_reduced, new_remaining) =
                    order_arc.match_against(remaining);

                // Actualizar la cantidad restante
                remaining = new_remaining;

                // Actualizar contadores atómicos
                self.visible_quantity.fetch_sub(consumed, Ordering::AcqRel);

                // Actualizar estadísticas
                self.stats.record_execution(consumed, order_arc.price(), order_arc.timestamp());

                if let Some(updated) = updated_order {
                    // Si hidden_reduced > 0, se refrescó de la cantidad oculta
                    if hidden_reduced > 0 {
                        self.hidden_quantity.fetch_sub(hidden_reduced, Ordering::AcqRel);
                        self.visible_quantity.fetch_add(hidden_reduced, Ordering::AcqRel);
                    }

                    // Poner la orden actualizada de vuelta en la cola
                    self.orders.push(Arc::new(updated));
                } else {
                    // Orden completamente consumida
                    self.order_count.fetch_sub(1, Ordering::AcqRel);

                    // Si tenía cantidad oculta y no fue considerada en hidden_reduced,
                    // actualizar hidden_quantity
                    match &*order_arc {
                        OrderType::IcebergOrder { hidden_quantity, .. } => {
                            if *hidden_quantity > 0 && hidden_reduced == 0 {
                                self.hidden_quantity.fetch_sub(*hidden_quantity, Ordering::AcqRel);
                            }
                        },
                        OrderType::ReserveOrder { hidden_quantity, .. } => {
                            if *hidden_quantity > 0 && hidden_reduced == 0 {
                                self.hidden_quantity.fetch_sub(*hidden_quantity, Ordering::AcqRel);
                            }
                        },
                        _ => {}
                    }
                }

                // Si ya no hay cantidad restante, salir del bucle
                if remaining == 0 {
                    break;
                }
            } else {
                // No más órdenes en este nivel de precio
                break;
            }
        }

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
                .map(|order_arc| (*order_arc))
                .collect(),
        }
    }
}

impl TryFrom<PriceLevelData> for PriceLevel {
    type Error = PriceLevelError;

    fn try_from(data: PriceLevelData) -> Result<Self, Self::Error> {
        let price_level = PriceLevel::new(data.price);

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
        let price_level = PriceLevel::new(price);

        if let Ok(orders_str) = get_field("orders") {


            let orders_str = orders_str.trim_matches(|c| c == '[' || c == ']');
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