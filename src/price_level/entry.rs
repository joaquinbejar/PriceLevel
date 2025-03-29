use crate::errors::PriceLevelError;
use crate::price_level::level::PriceLevel;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Represents a price level entry in the order book
#[derive(Debug)]
pub struct OrderBookEntry {
    /// The price level
    pub level: Arc<PriceLevel>,

    /// Index or position in the order book
    pub index: usize,
}

impl OrderBookEntry {
    /// Create a new order book entry
    #[allow(dead_code)]
    pub fn new(level: Arc<PriceLevel>, index: usize) -> Self {
        Self { level, index }
    }

    /// Get the price of this entry
    pub fn price(&self) -> u64 {
        self.level.price()
    }

    /// Get the visible quantity at this entry
    pub fn visible_quantity(&self) -> u64 {
        self.level.visible_quantity()
    }

    /// Get the total quantity at this entry
    pub fn total_quantity(&self) -> u64 {
        self.level.total_quantity()
    }

    /// Get the order count at this entry
    #[allow(dead_code)]
    pub fn order_count(&self) -> usize {
        self.level.order_count()
    }
}

impl PartialEq for OrderBookEntry {
    fn eq(&self, other: &Self) -> bool {
        self.price() == other.price()
    }
}

impl Eq for OrderBookEntry {}

impl PartialOrd for OrderBookEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderBookEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.price().cmp(&other.price())
    }
}

// Implement Serialize
impl Serialize for OrderBookEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("OrderBookEntry", 3)?;
        state.serialize_field("price", &self.price())?;
        state.serialize_field("visible_quantity", &self.visible_quantity())?;
        state.serialize_field("total_quantity", &self.total_quantity())?;
        state.serialize_field("index", &self.index)?;
        state.end()
    }
}

// Implement Deserialize
impl<'de> Deserialize<'de> for OrderBookEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wrapper {
            price: u64,
            index: usize,
        }

        let wrapper = Wrapper::deserialize(deserializer)?;

        // Note: This might require modifying the constructor
        // You may need to add a method to PriceLevel that allows creating from price
        let level = Arc::new(PriceLevel::new(wrapper.price));

        Ok(OrderBookEntry {
            level,
            index: wrapper.index,
        })
    }
}

// Implement Display
impl fmt::Display for OrderBookEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OrderBookEntry:price={};visible_quantity={};total_quantity={};index={}",
            self.price(),
            self.visible_quantity(),
            self.total_quantity(),
            self.index
        )
    }
}

// Implement FromStr
impl FromStr for OrderBookEntry {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 || parts[0] != "OrderBookEntry" {
            return Err(PriceLevelError::InvalidFormat);
        }

        let mut fields = std::collections::HashMap::new();
        for field_pair in parts[1].split(';') {
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

        let parse_usize = |field: &str, value: &str| -> Result<usize, PriceLevelError> {
            value
                .parse::<usize>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        let price_str = get_field("price")?;
        let price = parse_u64("price", price_str)?;

        let index_str = get_field("index")?;
        let index = parse_usize("index", index_str)?;

        // Create a new price level with the given price
        let level = Arc::new(PriceLevel::new(price));

        Ok(OrderBookEntry { level, index })
    }
}
