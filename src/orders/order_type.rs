//! Limit order type definitions

use crate::OrderQueue;
use crate::errors::PriceLevelError;
use crate::orders::{OrderId, PegReferenceType, Side, TimeInForce};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Default amount to replenish the reserve with.
pub const DEFAULT_RESERVE_REPLENISH_AMOUNT: u64 = 80;

/// Represents different types of limit orders
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderType<T> {
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
        /// Additional custom fields
        extra_fields: T,
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
        /// Additional custom fields
        extra_fields: T,
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
        /// Additional custom fields
        extra_fields: T,
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
        /// Additional custom fields
        extra_fields: T,
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
        /// Additional custom fields
        extra_fields: T,
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
        /// Additional custom fields
        extra_fields: T,
    },

    /// Reserve order with custom replenishment
    /// if `replenish_amount` is None, it uses DEFAULT_RESERVE_REPLENISH_AMOUNT
    /// if `auto_replenish` is false, and visible quantity is below threshold, it will not replenish
    /// if `auto_replenish` is false and visible quantity is zero it will be removed from the book
    /// if `auto_replenish` is true, and replenish_threshold is 0, it will use 1
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
        /// Optional amount to replenish by. If None, uses DEFAULT_RESERVE_REPLENISH_AMOUNT
        replenish_amount: Option<u64>,
        /// Whether to replenish automatically when below threshold. If false, only replenish on next match
        auto_replenish: bool,
        /// Additional custom fields
        extra_fields: T,
    },
}

impl<T: Clone> OrderType<T> {
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
        matches!(self.time_in_force(), TimeInForce::Fok)
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
                extra_fields,
                ..
            } => Self::Standard {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                extra_fields: extra_fields.clone(),
            },
            Self::IcebergOrder {
                id,
                price,
                side,
                timestamp,
                time_in_force,
                hidden_quantity,
                extra_fields,
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
                    extra_fields: extra_fields.clone(),
                }
            }
            Self::PostOnly {
                id,
                price,
                side,
                timestamp,
                time_in_force,
                extra_fields,
                ..
            } => Self::PostOnly {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                extra_fields: extra_fields.clone(),
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
                extra_fields,
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
                        extra_fields: extra_fields.clone(),
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
                replenish_amount,
                auto_replenish,
                extra_fields,
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
                        replenish_amount: *replenish_amount,
                        auto_replenish: *auto_replenish,
                        extra_fields: extra_fields.clone(),
                    },
                    used_hidden,
                )
            }
            _ => (self.clone(), 0), // Non-iceberg orders don't refresh
        }
    }
}

impl<T: Clone> OrderType<T> {
    /// Matches this order against an incoming quantity
    ///
    /// Returns a tuple containing:
    /// - The quantity consumed from the incoming order
    /// - Optionally, an updated version of this order (if partially filled)
    /// - The quantity that was reduced from hidden portion (for iceberg/reserve orders)
    /// - The remaining quantity of the incoming order
    pub fn match_against(&self, incoming_quantity: u64) -> (u64, Option<Self>, u64, u64) {
        match self {
            Self::Standard {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields,
            } => {
                if *quantity <= incoming_quantity {
                    // Full match
                    (
                        *quantity,                     // consumed = full order quantity
                        None,                          // no updated order (fully matched)
                        0,                             // no hidden quantity reduced
                        incoming_quantity - *quantity, // remaining = incoming - consumed
                    )
                } else {
                    // Partial match
                    (
                        incoming_quantity, // consumed = all incoming quantity
                        Some(Self::Standard {
                            id: *id,
                            price: *price,
                            quantity: *quantity - incoming_quantity, // reduce quantity
                            side: *side,
                            timestamp: *timestamp,
                            time_in_force: *time_in_force,
                            extra_fields: extra_fields.clone(),
                        }),
                        0, // not hidden quantity reduced
                        0, // not remaining quantity
                    )
                }
            }

            // En OrderType::match_against para IcebergOrder
            Self::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields,
            } => {
                if *visible_quantity <= incoming_quantity {
                    // Fully match the visible portion
                    let consumed = *visible_quantity;
                    let remaining = incoming_quantity - consumed;

                    if *hidden_quantity > 0 {
                        // Refresh visible portion from hidden
                        let refresh_qty = std::cmp::min(*hidden_quantity, *visible_quantity);
                        let new_hidden = *hidden_quantity - refresh_qty;

                        // Create updated order with refreshed quantities
                        (
                            consumed,
                            Some(Self::IcebergOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: refresh_qty,
                                hidden_quantity: new_hidden,
                                side: *side,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                extra_fields: extra_fields.clone(),
                            }),
                            refresh_qty,
                            remaining,
                        )
                    } else {
                        // No hidden quantity left
                        (consumed, None, 0, remaining)
                    }
                } else {
                    // Partial match of visible quantity
                    let executed = incoming_quantity;

                    (
                        executed,
                        Some(Self::IcebergOrder {
                            id: *id,
                            price: *price,
                            visible_quantity: *visible_quantity - executed,
                            hidden_quantity: *hidden_quantity,
                            side: *side,
                            timestamp: *timestamp,
                            time_in_force: *time_in_force,
                            extra_fields: extra_fields.clone(),
                        }),
                        0,
                        0,
                    )
                }
            }

            Self::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields,
            } => {
                // Ensure the threshold is never 0 if auto_replenish is true
                let safe_threshold = if *auto_replenish && *replenish_threshold == 0 {
                    1
                } else {
                    *replenish_threshold
                };

                let replenish_qty = replenish_amount
                    .unwrap_or(DEFAULT_RESERVE_REPLENISH_AMOUNT)
                    .min(*hidden_quantity);

                if *visible_quantity <= incoming_quantity {
                    // Full match of the visible part
                    let consumed = *visible_quantity;
                    let remaining = incoming_quantity - consumed;

                    // Verify if we need and can replenish
                    if *hidden_quantity > 0 && *auto_replenish {
                        // Restore from the hidden quantity
                        let new_hidden = *hidden_quantity - replenish_qty;

                        (
                            consumed,
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: replenish_qty,
                                hidden_quantity: new_hidden,
                                side: *side,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                                replenish_amount: *replenish_amount,
                                auto_replenish: *auto_replenish,
                                extra_fields: extra_fields.clone(),
                            }),
                            replenish_qty,
                            remaining,
                        )
                    } else {
                        // If there is no auto-replenishment or no hidden quantity, delete the order
                        (consumed, None, 0, remaining)
                    }
                } else {
                    // Partial match of the visible part
                    let consumed = incoming_quantity;
                    let new_visible = *visible_quantity - consumed;

                    // Check if we need to replenish (we fell below the threshold)
                    if new_visible < safe_threshold && *hidden_quantity > 0 && *auto_replenish {
                        // Restore from the hidden quantity
                        let new_hidden = *hidden_quantity - replenish_qty;

                        (
                            consumed,
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: new_visible + replenish_qty,
                                hidden_quantity: new_hidden,
                                side: *side,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                                replenish_amount: *replenish_amount,
                                auto_replenish: *auto_replenish,
                                extra_fields: extra_fields.clone(),
                            }),
                            replenish_qty,
                            0,
                        )
                    } else {
                        // We don't need to replenish or it is not automatic
                        (
                            consumed,
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: new_visible,
                                hidden_quantity: *hidden_quantity,
                                side: *side,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                                replenish_amount: *replenish_amount,
                                auto_replenish: *auto_replenish,
                                extra_fields: extra_fields.clone(),
                            }),
                            0,
                            0,
                        )
                    }
                }
            }

            // For all other order types, use standard matching logic
            _ => {
                let visible_qty = self.visible_quantity();

                if visible_qty <= incoming_quantity {
                    // Full match
                    (
                        visible_qty,                     // consumed full visible quantity
                        None,                            // fully matched
                        0,                               // no hidden reduced
                        incoming_quantity - visible_qty, // remaining quantity
                    )
                } else {
                    // Partial match
                    (
                        incoming_quantity, // consumed all incoming
                        Some(self.with_reduced_quantity(visible_qty - incoming_quantity)),
                        0, // not hidden reduced
                        0, // not remaining quantity
                    )
                }
            }
        }
    }
}

impl<T> OrderType<T> {
    /// Get the extra fields
    pub fn extra_fields(&self) -> &T {
        match self {
            Self::Standard { extra_fields, .. } => extra_fields,
            Self::IcebergOrder { extra_fields, .. } => extra_fields,
            Self::PostOnly { extra_fields, .. } => extra_fields,
            Self::TrailingStop { extra_fields, .. } => extra_fields,
            Self::PeggedOrder { extra_fields, .. } => extra_fields,
            Self::MarketToLimit { extra_fields, .. } => extra_fields,
            Self::ReserveOrder { extra_fields, .. } => extra_fields,
        }
    }

    /// Get mutable reference to extra fields
    pub fn extra_fields_mut(&mut self) -> &mut T {
        match self {
            Self::Standard { extra_fields, .. } => extra_fields,
            Self::IcebergOrder { extra_fields, .. } => extra_fields,
            Self::PostOnly { extra_fields, .. } => extra_fields,
            Self::TrailingStop { extra_fields, .. } => extra_fields,
            Self::PeggedOrder { extra_fields, .. } => extra_fields,
            Self::MarketToLimit { extra_fields, .. } => extra_fields,
            Self::ReserveOrder { extra_fields, .. } => extra_fields,
        }
    }

    /// Transform the extra fields type using a function
    pub fn map_extra_fields<U, F>(self, f: F) -> OrderType<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            Self::Standard {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::Standard {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::PostOnly {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::PostOnly {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::TrailingStop {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                trail_amount,
                last_reference_price,
                extra_fields,
            } => OrderType::TrailingStop {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                trail_amount,
                last_reference_price,
                extra_fields: f(extra_fields),
            },
            Self::PeggedOrder {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                extra_fields,
            } => OrderType::PeggedOrder {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                extra_fields: f(extra_fields),
            },
            Self::MarketToLimit {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::MarketToLimit {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields,
            } => OrderType::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields: f(extra_fields),
            },
        }
    }
}

/// Expected string format:
/// ORDER_TYPE:id=`<id>`;price=`<price>`;quantity=`<qty>`;side=<BUY|SELL>;timestamp=`<ts>`;time_in_force=`<tif>`;[additional fields]
///
/// Examples:
/// - Standard:id=123;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC
/// - IcebergOrder:id=124;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC
impl<T: Default> FromStr for OrderType<T> {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(PriceLevelError::InvalidFormat);
        }

        let order_type = parts[0];
        let fields_str = parts[1];

        let mut fields = std::collections::HashMap::new();
        for field_pair in fields_str.split(';') {
            let kv: Vec<&str> = field_pair.split('=').collect();
            if kv.len() == 2 {
                fields.insert(kv[0], kv[1]);
            }
        }

        let get_field = |field: &str| -> Result<&str, PriceLevelError> {
            match fields.get(field) {
                Some(result) => Ok(*result),
                None => Err(PriceLevelError::MissingField(field.to_string())),
            }
        };

        let parse_u64 = |field: &str, value: &str| -> Result<u64, PriceLevelError> {
            value
                .parse::<u64>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        let parse_i64 = |field: &str, value: &str| -> Result<i64, PriceLevelError> {
            value
                .parse::<i64>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        // Parse common fields
        let id_str = get_field("id")?;
        let id = OrderId::from_str(id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
            field: "id".to_string(),
            value: id_str.to_string(),
        })?;

        let price_str = get_field("price")?;
        let price = parse_u64("price", price_str)?;

        let side_str = get_field("side")?;
        let side: Side = Side::from_str(side_str)?;

        let timestamp_str = get_field("timestamp")?;
        let timestamp = parse_u64("timestamp", timestamp_str)?;

        let tif_str = get_field("time_in_force")?;
        let time_in_force = TimeInForce::from_str(tif_str)?;

        // Parse specific order types
        match order_type {
            "Standard" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_u64("quantity", quantity_str)?;

                Ok(OrderType::Standard {
                    id,
                    price,
                    quantity,
                    side,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "IcebergOrder" => {
                let visible_quantity_str = get_field("visible_quantity")?;
                let visible_quantity = parse_u64("visible_quantity", visible_quantity_str)?;

                let hidden_quantity_str = get_field("hidden_quantity")?;
                let hidden_quantity = parse_u64("hidden_quantity", hidden_quantity_str)?;

                Ok(OrderType::IcebergOrder {
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    side,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "PostOnly" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_u64("quantity", quantity_str)?;

                Ok(OrderType::PostOnly {
                    id,
                    price,
                    quantity,
                    side,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "TrailingStop" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_u64("quantity", quantity_str)?;

                let trail_amount_str = get_field("trail_amount")?;
                let trail_amount = parse_u64("trail_amount", trail_amount_str)?;

                let last_reference_price_str = get_field("last_reference_price")?;
                let last_reference_price =
                    parse_u64("last_reference_price", last_reference_price_str)?;

                Ok(OrderType::TrailingStop {
                    id,
                    price,
                    quantity,
                    side,
                    timestamp,
                    time_in_force,
                    trail_amount,
                    last_reference_price,
                    extra_fields: T::default(),
                })
            }
            "PeggedOrder" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_u64("quantity", quantity_str)?;

                let reference_price_offset_str = get_field("reference_price_offset")?;
                let reference_price_offset =
                    parse_i64("reference_price_offset", reference_price_offset_str)?;

                let reference_price_type_str = get_field("reference_price_type")?;
                let reference_price_type = match reference_price_type_str {
                    "BestBid" => PegReferenceType::BestBid,
                    "BestAsk" => PegReferenceType::BestAsk,
                    "MidPrice" => PegReferenceType::MidPrice,
                    "LastTrade" => PegReferenceType::LastTrade,
                    _ => {
                        return Err(PriceLevelError::InvalidFieldValue {
                            field: "reference_price_type".to_string(),
                            value: reference_price_type_str.to_string(),
                        });
                    }
                };

                Ok(OrderType::PeggedOrder {
                    id,
                    price,
                    quantity,
                    side,
                    timestamp,
                    time_in_force,
                    reference_price_offset,
                    reference_price_type,
                    extra_fields: T::default(),
                })
            }
            "MarketToLimit" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_u64("quantity", quantity_str)?;

                Ok(OrderType::MarketToLimit {
                    id,
                    price,
                    quantity,
                    side,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "ReserveOrder" => {
                let visible_quantity_str = get_field("visible_quantity")?;
                let visible_quantity = parse_u64("visible_quantity", visible_quantity_str)?;

                let hidden_quantity_str = get_field("hidden_quantity")?;
                let hidden_quantity = parse_u64("hidden_quantity", hidden_quantity_str)?;

                let replenish_threshold_str = get_field("replenish_threshold")?;
                let replenish_threshold =
                    parse_u64("replenish_threshold", replenish_threshold_str)?;
                let replenish_amount_str = get_field("replenish_amount")?;
                let replenish_amount = if replenish_amount_str == "None" {
                    None
                } else {
                    Some(parse_u64("replenish_amount", replenish_amount_str)?)
                };
                let auto_replenish_str = get_field("auto_replenish")?;
                let auto_replenish = match auto_replenish_str {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(PriceLevelError::InvalidFieldValue {
                            field: "auto_replenish".to_string(),
                            value: auto_replenish_str.to_string(),
                        });
                    }
                };

                Ok(OrderType::ReserveOrder {
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    side,
                    timestamp,
                    time_in_force,
                    replenish_threshold,
                    replenish_amount,
                    auto_replenish,
                    extra_fields: T::default(),
                })
            }
            _ => Err(PriceLevelError::UnknownOrderType(order_type.to_string())),
        }
    }
}

impl<T> fmt::Display for OrderType<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::Standard {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "Standard:id={};price={};quantity={};side={};timestamp={};time_in_force={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    timestamp,
                    time_in_force
                )
            }
            OrderType::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "IcebergOrder:id={};price={};visible_quantity={};hidden_quantity={};side={};timestamp={};time_in_force={}",
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    format!("{side:?}").to_uppercase(),
                    timestamp,
                    time_in_force
                )
            }
            OrderType::PostOnly {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "PostOnly:id={};price={};quantity={};side={};timestamp={};time_in_force={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    timestamp,
                    time_in_force
                )
            }
            OrderType::TrailingStop {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                trail_amount,
                last_reference_price,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "TrailingStop:id={};price={};quantity={};side={};timestamp={};time_in_force={};trail_amount={};last_reference_price={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    timestamp,
                    time_in_force,
                    trail_amount,
                    last_reference_price
                )
            }
            OrderType::PeggedOrder {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "PeggedOrder:id={};price={};quantity={};side={};timestamp={};time_in_force={};reference_price_offset={};reference_price_type={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    timestamp,
                    time_in_force,
                    reference_price_offset,
                    reference_price_type
                )
            }
            OrderType::MarketToLimit {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "MarketToLimit:id={};price={};quantity={};side={};timestamp={};time_in_force={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    timestamp,
                    time_in_force
                )
            }
            OrderType::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "ReserveOrder:id={};price={};visible_quantity={};hidden_quantity={};side={};timestamp={};time_in_force={};replenish_threshold={};replenish_amount={};auto_replenish={}",
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    format!("{side:?}").to_uppercase(),
                    timestamp,
                    time_in_force,
                    replenish_threshold,
                    replenish_amount.map_or("None".to_string(), |v| v.to_string()),
                    auto_replenish
                )
            }
        }
    }
}

impl From<OrderQueue> for Vec<Arc<OrderType<()>>> {
    fn from(queue: OrderQueue) -> Self {
        queue.to_vec()
    }
}

// Type aliases for common use cases
#[allow(dead_code)]
pub type SimpleOrderType = OrderType<()>;
#[allow(dead_code)]
pub type OrderTypeWithMetadata = OrderType<OrderMetadata>;

// Example of what the extra fields could contain
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OrderMetadata {
    pub client_id: Option<u64>,
    pub user_id: Option<u64>,
    pub exchange_id: Option<u8>,
    pub priority: u8,
}
