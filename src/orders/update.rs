use crate::errors::PriceLevelError;
use crate::orders::base::{OrderId, Side};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Represents a request to update an existing order
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum OrderUpdate {
    /// Update the price of an order
    UpdatePrice {
        /// ID of the order to update
        order_id: OrderId,
        /// New price for the order
        new_price: u64,
    },

    /// Update the quantity of an order
    UpdateQuantity {
        /// ID of the order to update
        order_id: OrderId,
        /// New quantity for the order
        new_quantity: u64,
    },

    /// Update both price and quantity of an order
    UpdatePriceAndQuantity {
        /// ID of the order to update
        order_id: OrderId,
        /// New price for the order
        new_price: u64,
        /// New quantity for the order
        new_quantity: u64,
    },

    /// Cancel an order
    Cancel {
        /// ID of the order to cancel
        order_id: OrderId,
    },

    /// Replace an order entirely with a new one
    Replace {
        /// ID of the order to replace
        order_id: OrderId,
        /// New price for the replacement order
        price: u64,
        /// New quantity for the replacement order
        quantity: u64,
        /// Side of the market (unchanged)
        side: Side,
    },
}

impl FromStr for OrderUpdate {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(PriceLevelError::InvalidFormat);
        }

        let update_type = parts[0];
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

        // Parse order_id field which is common to all update types
        let order_id_str = get_field("order_id")?;
        let order_id =
            OrderId::from_str(order_id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "order_id".to_string(),
                value: order_id_str.to_string(),
            })?;

        match update_type {
            "UpdatePrice" => {
                let new_price_str = get_field("new_price")?;
                let new_price = parse_u64("new_price", new_price_str)?;

                Ok(OrderUpdate::UpdatePrice {
                    order_id,
                    new_price,
                })
            }
            "UpdateQuantity" => {
                let new_quantity_str = get_field("new_quantity")?;
                let new_quantity = parse_u64("new_quantity", new_quantity_str)?;

                Ok(OrderUpdate::UpdateQuantity {
                    order_id,
                    new_quantity,
                })
            }
            "UpdatePriceAndQuantity" => {
                let new_price_str = get_field("new_price")?;
                let new_price = parse_u64("new_price", new_price_str)?;

                let new_quantity_str = get_field("new_quantity")?;
                let new_quantity = parse_u64("new_quantity", new_quantity_str)?;

                Ok(OrderUpdate::UpdatePriceAndQuantity {
                    order_id,
                    new_price,
                    new_quantity,
                })
            }
            "Cancel" => Ok(OrderUpdate::Cancel { order_id }),
            "Replace" => {
                let price_str = get_field("price")?;
                let price = parse_u64("price", price_str)?;

                let quantity_str = get_field("quantity")?;
                let quantity = parse_u64("quantity", quantity_str)?;

                let side_str = get_field("side")?;
                let side =
                    Side::from_str(side_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                        field: "side".to_string(),
                        value: side_str.to_string(),
                    })?;

                Ok(OrderUpdate::Replace {
                    order_id,
                    price,
                    quantity,
                    side,
                })
            }
            _ => Err(PriceLevelError::UnknownOrderType(update_type.to_string())),
        }
    }
}

impl std::fmt::Display for OrderUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderUpdate::UpdatePrice {
                order_id,
                new_price,
            } => {
                write!(f, "UpdatePrice:order_id={order_id};new_price={new_price}")
            }
            OrderUpdate::UpdateQuantity {
                order_id,
                new_quantity,
            } => {
                write!(
                    f,
                    "UpdateQuantity:order_id={order_id};new_quantity={new_quantity}"
                )
            }
            OrderUpdate::UpdatePriceAndQuantity {
                order_id,
                new_price,
                new_quantity,
            } => {
                write!(
                    f,
                    "UpdatePriceAndQuantity:order_id={order_id};new_price={new_price};new_quantity={new_quantity}"
                )
            }
            OrderUpdate::Cancel { order_id } => {
                write!(f, "Cancel:order_id={order_id}")
            }
            OrderUpdate::Replace {
                order_id,
                price,
                quantity,
                side,
            } => {
                write!(
                    f,
                    "Replace:order_id={order_id};price={price};quantity={quantity};side={side}"
                )
            }
        }
    }
}
