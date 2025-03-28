//! Limit order type definitions

use crate::errors::PriceLevelError;
use crate::orders::{OrderId, PegReferenceType, Side, TimeInForce};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Represents different types of limit orders
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
            _ => *self, // Default fallback, though this should be implemented for all types
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
            _ => (*self, 0), // Non-iceberg orders don't refresh
        }
    }
}

impl OrderType {
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
                        }),
                        0, // not hidden quantity reduced
                        0, // not remaining quantity
                    )
                }
            }

            Self::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                timestamp,
                time_in_force,
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
            } => {
                if *visible_quantity <= incoming_quantity {
                    // Full match of visible portion
                    let consumed = *visible_quantity;
                    let remaining = incoming_quantity - consumed;

                    // Check if we need to replenish
                    if *hidden_quantity > 0 && *visible_quantity <= *replenish_threshold {
                        // Replenish visible quantity from hidden
                        let refresh_qty = std::cmp::min(*hidden_quantity, *visible_quantity);
                        let new_hidden = *hidden_quantity - refresh_qty;
                        let hidden_reduced = refresh_qty; // Amount reduced from hidden

                        // Return updated order with refreshed quantities
                        (
                            consumed, // consumed full visible quantity
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: refresh_qty,
                                hidden_quantity: new_hidden,
                                side: *side,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                            }),
                            hidden_reduced, // amount reduced from hidden
                            remaining,      // remaining quantity
                        )
                    } else if *hidden_quantity == 0 {
                        // No hidden quantity, order is fully matched
                        (
                            consumed,  // consumed full visible quantity
                            None,      // fully matched
                            0,         // no hidden reduced
                            remaining, // remaining quantity
                        )
                    } else {
                        // Has hidden quantity but not below threshold
                        (
                            consumed, // consumed full visible quantity
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: *visible_quantity, // keep as is
                                hidden_quantity: *hidden_quantity,
                                side: *side,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                            }),
                            0,         // no hidden reduced
                            remaining, // remaining quantity
                        )
                    }
                } else {
                    // Partial match of visible portion
                    (
                        incoming_quantity, // consumed all incoming
                        Some(Self::ReserveOrder {
                            id: *id,
                            price: *price,
                            visible_quantity: *visible_quantity - incoming_quantity,
                            hidden_quantity: *hidden_quantity,
                            side: *side,
                            timestamp: *timestamp,
                            time_in_force: *time_in_force,
                            replenish_threshold: *replenish_threshold,
                        }),
                        0, // no hidden reduced
                        0, // no remaining quantity
                    )
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

/// Expected string format:
/// ORDER_TYPE:id=<id>;price=<price>;quantity=<qty>;side=<BUY|SELL>;timestamp=<ts>;time_in_force=<tif>;[additional fields]
///
/// Examples:
/// - Standard:id=123;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC
/// - IcebergOrder:id=124;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC
impl FromStr for OrderType {
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
        let id = OrderId(parse_u64("id", id_str)?);

        let price_str = get_field("price")?;
        let price = parse_u64("price", price_str)?;

        let side_str = get_field("side")?;
        let side = match side_str {
            "BUY" => Side::Buy,
            "SELL" => Side::Sell,
            _ => {
                return Err(PriceLevelError::InvalidFieldValue {
                    field: "side".to_string(),
                    value: side_str.to_string(),
                });
            }
        };

        let timestamp_str = get_field("timestamp")?;
        let timestamp = parse_u64("timestamp", timestamp_str)?;

        let tif_str = get_field("time_in_force")?;
        let time_in_force = match tif_str {
            "GTC" => TimeInForce::Gtc,
            "IOC" => TimeInForce::Ioc,
            "FOK" => TimeInForce::Fok,
            "DAY" => TimeInForce::Day,
            _ if tif_str.starts_with("GTD-") => {
                let date_str = &tif_str[4..];
                let expiry = parse_u64("time_in_force", date_str)?;
                TimeInForce::Gtd(expiry)
            }
            _ => {
                return Err(PriceLevelError::InvalidFieldValue {
                    field: "time_in_force".to_string(),
                    value: tif_str.to_string(),
                });
            }
        };

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

                Ok(OrderType::ReserveOrder {
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    side,
                    timestamp,
                    time_in_force,
                    replenish_threshold,
                })
            }
            _ => Err(PriceLevelError::UnknownOrderType(order_type.to_string())),
        }
    }
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::Standard {
                id,
                price,
                quantity,
                side,
                timestamp,
                time_in_force,
            } => {
                write!(
                    f,
                    "Standard:id={};price={};quantity={};side={};timestamp={};time_in_force={}",
                    id.0,
                    price,
                    quantity,
                    format!("{:?}", side).to_uppercase(),
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
            } => {
                write!(
                    f,
                    "IcebergOrder:id={};price={};visible_quantity={};hidden_quantity={};side={};timestamp={};time_in_force={}",
                    id.0,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    format!("{:?}", side).to_uppercase(),
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
            } => {
                write!(
                    f,
                    "PostOnly:id={};price={};quantity={};side={};timestamp={};time_in_force={}",
                    id.0,
                    price,
                    quantity,
                    format!("{:?}", side).to_uppercase(),
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
            } => {
                write!(
                    f,
                    "TrailingStop:id={};price={};quantity={};side={};timestamp={};time_in_force={};trail_amount={};last_reference_price={}",
                    id.0,
                    price,
                    quantity,
                    format!("{:?}", side).to_uppercase(),
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
            } => {
                write!(
                    f,
                    "PeggedOrder:id={};price={};quantity={};side={};timestamp={};time_in_force={};reference_price_offset={};reference_price_type={}",
                    id.0,
                    price,
                    quantity,
                    format!("{:?}", side).to_uppercase(),
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
            } => {
                write!(
                    f,
                    "MarketToLimit:id={};price={};quantity={};side={};timestamp={};time_in_force={}",
                    id.0,
                    price,
                    quantity,
                    format!("{:?}", side).to_uppercase(),
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
            } => {
                write!(
                    f,
                    "ReserveOrder:id={};price={};visible_quantity={};hidden_quantity={};side={};timestamp={};time_in_force={};replenish_threshold={}",
                    id.0,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    format!("{:?}", side).to_uppercase(),
                    timestamp,
                    time_in_force,
                    replenish_threshold
                )
            }
        }
    }
}
