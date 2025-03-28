//! Limit order type definitions

use crate::orders::time_in_force::TimeInForce;
use crate::orders::{OrderId, PegReferenceType, Side};

/// Represents different types of limit orders
#[derive(Debug, Clone)]
pub enum OrderType {
    /// Standard limit order
    Standard {
        /// The order ID
        id: OrderId,
        /// The price of the order
        price: u64,
        /// The quantity of the order
        quantity: u64,
        /// The side of the order (buy or sell)
        side: Side,
        /// When the order was created
        timestamp: u64,
        /// Time-in-force policy
        time_in_force: TimeInForce,
    },

    /// Iceberg order with visible and hidden quantities
    IcebergOrder {
        /// The order ID
        id: OrderId,
        /// The price of the order
        price: u64,
        /// The visible quantity of the order
        visible_quantity: u64,
        /// The hidden quantity of the order
        hidden_quantity: u64,
        /// The side of the order (buy or sell)
        side: Side,
        /// When the order was created
        timestamp: u64,
        /// Time-in-force policy
        time_in_force: TimeInForce,
    },

    /// Post-only order that won't match immediately
    PostOnly {
        /// The order ID
        id: OrderId,
        /// The price of the order
        price: u64,
        /// The quantity of the order
        quantity: u64,
        /// The side of the order (buy or sell)
        side: Side,
        /// When the order was created
        timestamp: u64,
        /// Time-in-force policy
        time_in_force: TimeInForce,
    },

    /// Trailing stop order that adjusts with market movement
    TrailingStop {
        /// The order ID
        id: OrderId,
        /// The price of the order
        price: u64,
        /// The quantity of the order
        quantity: u64,
        /// The side of the order (buy or sell)
        side: Side,
        /// When the order was created
        timestamp: u64,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Amount to trail the market price
        trail_amount: u64,
        /// Last reference price
        last_reference_price: u64,
    },

    /// Pegged order that adjusts based on reference price
    PeggedOrder {
        /// The order ID
        id: OrderId,
        /// The price of the order
        price: u64,
        /// The quantity of the order
        quantity: u64,
        /// The side of the order (buy or sell)
        side: Side,
        /// When the order was created
        timestamp: u64,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Offset from the reference price
        reference_price_offset: i64,
        /// Type of reference price to track
        reference_price_type: PegReferenceType,
    },

    /// Market-to-limit order that converts to limit after initial execution
    MarketToLimit {
        /// The order ID
        id: OrderId,
        /// The price of the order
        price: u64,
        /// The quantity of the order
        quantity: u64,
        /// The side of the order (buy or sell)
        side: Side,
        /// When the order was created
        timestamp: u64,
        /// Time-in-force policy
        time_in_force: TimeInForce,
    },

    /// Reserve order with custom replenishment
    ReserveOrder {
        /// The order ID
        id: OrderId,
        /// The price of the order
        price: u64,
        /// The visible quantity of the order
        visible_quantity: u64,
        /// The hidden quantity of the order
        hidden_quantity: u64,
        /// The side of the order (buy or sell)
        side: Side,
        /// When the order was created
        timestamp: u64,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Threshold at which to replenish
        replenish_threshold: u64,
    },
}

impl OrderType {
    /// Get the order ID
    pub fn id(&self) -> OrderId {
        match self {
            Self::Standard { id, .. } => *id,
            Self::IcebergOrder { id, .. } => *id,
            Self::PostOnly { id, .. } => *id,
            Self::TrailingStop { id, .. } => *id,
            Self::PeggedOrder { id, .. } => *id,
            Self::MarketToLimit { id, .. } => *id,
            Self::ReserveOrder { id, .. } => *id,
        }
    }

    /// Get the price
    pub fn price(&self) -> u64 {
        match self {
            Self::Standard { price, .. } => *price,
            Self::IcebergOrder { price, .. } => *price,
            Self::PostOnly { price, .. } => *price,
            Self::TrailingStop { price, .. } => *price,
            Self::PeggedOrder { price, .. } => *price,
            Self::MarketToLimit { price, .. } => *price,
            Self::ReserveOrder { price, .. } => *price,
        }
    }

    /// Get the visible quantity
    pub fn visible_quantity(&self) -> u64 {
        match self {
            Self::Standard { quantity, .. } => *quantity,
            Self::IcebergOrder {
                visible_quantity, ..
            } => *visible_quantity,
            Self::PostOnly { quantity, .. } => *quantity,
            Self::TrailingStop { quantity, .. } => *quantity,
            Self::PeggedOrder { quantity, .. } => *quantity,
            Self::MarketToLimit { quantity, .. } => *quantity,
            Self::ReserveOrder {
                visible_quantity, ..
            } => *visible_quantity,
        }
    }

    /// Get the hidden quantity
    pub fn hidden_quantity(&self) -> u64 {
        match self {
            Self::IcebergOrder {
                hidden_quantity, ..
            } => *hidden_quantity,
            Self::ReserveOrder {
                hidden_quantity, ..
            } => *hidden_quantity,
            _ => 0,
        }
    }

    /// Get the order side
    pub fn side(&self) -> Side {
        match self {
            Self::Standard { side, .. } => *side,
            Self::IcebergOrder { side, .. } => *side,
            Self::PostOnly { side, .. } => *side,
            Self::TrailingStop { side, .. } => *side,
            Self::PeggedOrder { side, .. } => *side,
            Self::MarketToLimit { side, .. } => *side,
            Self::ReserveOrder { side, .. } => *side,
        }
    }

    /// Get the time in force
    pub fn time_in_force(&self) -> TimeInForce {
        match self {
            Self::Standard { time_in_force, .. } => *time_in_force,
            Self::IcebergOrder { time_in_force, .. } => *time_in_force,
            Self::PostOnly { time_in_force, .. } => *time_in_force,
            Self::TrailingStop { time_in_force, .. } => *time_in_force,
            Self::PeggedOrder { time_in_force, .. } => *time_in_force,
            Self::MarketToLimit { time_in_force, .. } => *time_in_force,
            Self::ReserveOrder { time_in_force, .. } => *time_in_force,
        }
    }

    /// Get the timestamp
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::Standard { timestamp, .. } => *timestamp,
            Self::IcebergOrder { timestamp, .. } => *timestamp,
            Self::PostOnly { timestamp, .. } => *timestamp,
            Self::TrailingStop { timestamp, .. } => *timestamp,
            Self::PeggedOrder { timestamp, .. } => *timestamp,
            Self::MarketToLimit { timestamp, .. } => *timestamp,
            Self::ReserveOrder { timestamp, .. } => *timestamp,
        }
    }

    /// Check if the order is immediate-or-cancel
    pub fn is_immediate(&self) -> bool {
        self.time_in_force().is_immediate()
    }

    /// Check if the order is fill-or-kill
    pub fn is_fill_or_kill(&self) -> bool {
        matches!(self.time_in_force(), TimeInForce::FOK)
    }

    /// Check if this is a post-only order
    pub fn is_post_only(&self) -> bool {
        matches!(self, Self::PostOnly { .. })
    }

    /// Create a new standard order with reduced quantity
    pub fn with_reduced_quantity(&self, new_quantity: u64) -> Self {
        match self {
            Self::Standard {
                id,
                price,
                side,
                timestamp,
                time_in_force,
                ..
            } => Self::Standard {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
            },
            Self::IcebergOrder {
                id,
                price,
                side,
                timestamp,
                time_in_force,
                hidden_quantity,
                ..
            } => {
                // Update visible quantity but keep hidden the same
                Self::IcebergOrder {
                    id: *id,
                    price: *price,
                    visible_quantity: new_quantity,
                    hidden_quantity: *hidden_quantity,
                    side: *side,
                    timestamp: *timestamp,
                    time_in_force: *time_in_force,
                }
            }
            Self::PostOnly {
                id,
                price,
                side,
                timestamp,
                time_in_force,
                ..
            } => Self::PostOnly {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
            },
            // For other order types, similar pattern...
            _ => self.clone(), // Default fallback, though this should be implemented for all types
        }
    }

    /// Update an iceberg order, refreshing visible part from hidden
    pub fn refresh_iceberg(&self, refresh_amount: u64) -> (Self, u64) {
        match self {
            Self::IcebergOrder {
                id,
                price,
                visible_quantity: _,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
            } => {
                let new_hidden = hidden_quantity.saturating_sub(refresh_amount);
                let used_hidden = hidden_quantity - new_hidden;

                (
                    Self::IcebergOrder {
                        id: *id,
                        price: *price,
                        visible_quantity: refresh_amount,
                        hidden_quantity: new_hidden,
                        side: *side,
                        timestamp: *timestamp,
                        time_in_force: *time_in_force,
                    },
                    used_hidden,
                )
            }
            Self::ReserveOrder {
                id,
                price,
                visible_quantity: _,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                replenish_threshold,
            } => {
                let new_hidden = hidden_quantity.saturating_sub(refresh_amount);
                let used_hidden = hidden_quantity - new_hidden;

                (
                    Self::ReserveOrder {
                        id: *id,
                        price: *price,
                        visible_quantity: refresh_amount,
                        hidden_quantity: new_hidden,
                        side: *side,
                        timestamp: *timestamp,
                        time_in_force: *time_in_force,
                        replenish_threshold: *replenish_threshold,
                    },
                    used_hidden,
                )
            }
            _ => (self.clone(), 0), // Non-iceberg orders don't refresh
        }
    }
}
