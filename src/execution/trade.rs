use crate::errors::PriceLevelError;
use crate::orders::{Id, Side};
use crate::utils::{Price, Quantity, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Represents a completed trade between two orders
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Trade {
    /// Unique trade ID
    pub trade_id: Id,

    /// ID of the aggressive order that caused the match
    pub taker_order_id: Id,

    /// ID of the passive order that was in the book
    pub maker_order_id: Id,

    /// Price at which the trade occurred
    pub price: Price,

    /// Quantity traded
    pub quantity: Quantity,

    /// Side of the taker order
    pub taker_side: Side,

    /// Timestamp when the trade occurred
    pub timestamp: TimestampMs,
}

impl Trade {
    /// Create a new trade
    #[must_use]
    pub fn new(
        trade_id: Id,
        taker_order_id: Id,
        maker_order_id: Id,
        price: Price,
        quantity: Quantity,
        taker_side: Side,
    ) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0_u64, |duration| duration.as_millis() as u64);
        let timestamp = TimestampMs::new(timestamp_ms);

        Self {
            trade_id,
            taker_order_id,
            maker_order_id,
            price,
            quantity,
            taker_side,
            timestamp,
        }
    }

    /// Returns the side of the maker order
    #[must_use]
    pub fn maker_side(&self) -> Side {
        match self.taker_side {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    /// Returns the total value of this trade
    #[must_use]
    pub fn total_value(&self) -> u128 {
        self.price.as_u128() * (self.quantity.as_u64() as u128)
    }
}

impl fmt::Display for Trade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Trade:trade_id={};taker_order_id={};maker_order_id={};price={};quantity={};taker_side={};timestamp={}",
            self.trade_id,
            self.taker_order_id,
            self.maker_order_id,
            self.price,
            self.quantity,
            self.taker_side,
            self.timestamp
        )
    }
}

impl FromStr for Trade {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 || parts[0] != "Trade" {
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

        // Parse trade_id
        let trade_id_str = get_field("trade_id")?;
        let trade_id =
            Id::from_str(trade_id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "trade_id".to_string(),
                value: trade_id_str.to_string(),
            })?;

        // Parse taker_order_id
        let taker_order_id_str = get_field("taker_order_id")?;
        let taker_order_id =
            Id::from_str(taker_order_id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "taker_order_id".to_string(),
                value: taker_order_id_str.to_string(),
            })?;

        // Parse maker_order_id
        let maker_order_id_str = get_field("maker_order_id")?;
        let maker_order_id =
            Id::from_str(maker_order_id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "maker_order_id".to_string(),
                value: maker_order_id_str.to_string(),
            })?;

        // Parse price
        let price_str = get_field("price")?;
        let price = Price::from_str(price_str).map_err(|_| PriceLevelError::InvalidFieldValue {
            field: "price".to_string(),
            value: price_str.to_string(),
        })?;

        // Parse quantity
        let quantity_str = get_field("quantity")?;
        let quantity =
            Quantity::from_str(quantity_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "quantity".to_string(),
                value: quantity_str.to_string(),
            })?;

        // Parse taker_side
        let taker_side_str = get_field("taker_side")?;
        let taker_side =
            Side::from_str(taker_side_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "taker_side".to_string(),
                value: taker_side_str.to_string(),
            })?;

        // Parse timestamp
        let timestamp_str = get_field("timestamp")?;
        let timestamp = TimestampMs::from_str(timestamp_str).map_err(|_| {
            PriceLevelError::InvalidFieldValue {
                field: "timestamp".to_string(),
                value: timestamp_str.to_string(),
            }
        })?;

        Ok(Trade {
            trade_id,
            taker_order_id,
            maker_order_id,
            price,
            quantity,
            taker_side,
            timestamp,
        })
    }
}
