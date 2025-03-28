use crate::errors::PriceLevelError;
use crate::orders::{OrderId, Side};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Represents a completed transaction between two orders
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Transaction {
    /// Unique transaction ID
    pub transaction_id: u64,

    /// ID of the aggressive order that caused the match
    pub taker_order_id: OrderId,

    /// ID of the passive order that was in the book
    pub maker_order_id: OrderId,

    /// Price at which the transaction occurred
    pub price: u64,

    /// Quantity that was traded
    pub quantity: u64,

    /// Side of the taker order
    pub taker_side: Side,

    /// Timestamp when the transaction occurred
    pub timestamp: u64,
}

impl Transaction {
    /// Create a new transaction
    pub fn new(
        transaction_id: u64,
        taker_order_id: OrderId,
        maker_order_id: OrderId,
        price: u64,
        quantity: u64,
        taker_side: Side,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis() as u64;

        Self {
            transaction_id,
            taker_order_id,
            maker_order_id,
            price,
            quantity,
            taker_side,
            timestamp,
        }
    }

    /// Returns the side of the maker order
    pub fn maker_side(&self) -> Side {
        match self.taker_side {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    /// Returns the total value of this transaction
    pub fn total_value(&self) -> u64 {
        self.price * self.quantity
    }
}

impl fmt::Display for Transaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Transaction:transaction_id={};taker_order_id={};maker_order_id={};price={};quantity={};taker_side={};timestamp={}",
            self.transaction_id,
            self.taker_order_id,
            self.maker_order_id,
            self.price,
            self.quantity,
            self.taker_side,
            self.timestamp
        )
    }
}

impl FromStr for Transaction {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 || parts[0] != "Transaction" {
            return Err(PriceLevelError::InvalidFormat);
        }

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

        // Parse transaction_id
        let transaction_id_str = get_field("transaction_id")?;
        let transaction_id = parse_u64("transaction_id", transaction_id_str)?;

        // Parse taker_order_id
        let taker_order_id_str = get_field("taker_order_id")?;
        let taker_order_id = OrderId::from_str(taker_order_id_str).map_err(|_| {
            PriceLevelError::InvalidFieldValue {
                field: "taker_order_id".to_string(),
                value: taker_order_id_str.to_string(),
            }
        })?;

        // Parse maker_order_id
        let maker_order_id_str = get_field("maker_order_id")?;
        let maker_order_id = OrderId::from_str(maker_order_id_str).map_err(|_| {
            PriceLevelError::InvalidFieldValue {
                field: "maker_order_id".to_string(),
                value: maker_order_id_str.to_string(),
            }
        })?;

        // Parse price
        let price_str = get_field("price")?;
        let price = parse_u64("price", price_str)?;

        // Parse quantity
        let quantity_str = get_field("quantity")?;
        let quantity = parse_u64("quantity", quantity_str)?;

        // Parse taker_side
        let taker_side_str = get_field("taker_side")?;
        let taker_side =
            Side::from_str(taker_side_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "taker_side".to_string(),
                value: taker_side_str.to_string(),
            })?;

        // Parse timestamp
        let timestamp_str = get_field("timestamp")?;
        let timestamp = parse_u64("timestamp", timestamp_str)?;

        Ok(Transaction {
            transaction_id,
            taker_order_id,
            maker_order_id,
            price,
            quantity,
            taker_side,
            timestamp,
        })
    }
}
