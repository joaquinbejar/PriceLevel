use crate::orders::base::{OrderId, Side};

/// Represents a request to update an existing order
#[derive(Debug, Clone)]
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
