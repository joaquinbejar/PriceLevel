use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize, Serializer, Deserializer};
use serde::ser::SerializeStruct;
use serde::de::{self, Visitor, MapAccess};
use crate::errors::PriceLevelError;

/// Tracks performance statistics for a price level
#[derive(Debug)]
pub struct PriceLevelStatistics {
    /// Number of orders added
    orders_added: AtomicUsize,

    /// Number of orders removed
    orders_removed: AtomicUsize,

    /// Number of orders executed
    pub orders_executed: AtomicUsize,

    /// Total quantity executed
    pub quantity_executed: AtomicU64,

    /// Total value executed
    pub value_executed: AtomicU64,

    /// Last execution timestamp
    pub last_execution_time: AtomicU64,

    /// First order arrival timestamp
    pub first_arrival_time: AtomicU64,

    /// Sum of waiting times for orders
    pub sum_waiting_time: AtomicU64,
}

impl PriceLevelStatistics {
    /// Create new empty statistics
    pub fn new() -> Self {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            orders_added: AtomicUsize::new(0),
            orders_removed: AtomicUsize::new(0),
            orders_executed: AtomicUsize::new(0),
            quantity_executed: AtomicU64::new(0),
            value_executed: AtomicU64::new(0),
            last_execution_time: AtomicU64::new(0),
            first_arrival_time: AtomicU64::new(current_time),
            sum_waiting_time: AtomicU64::new(0),
        }
    }

    /// Record a new order being added
    pub fn record_order_added(&self) {
        self.orders_added.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an order being removed without execution
    pub fn record_order_removed(&self) {
        self.orders_removed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an order execution
    pub fn record_execution(&self, quantity: u64, price: u64, order_timestamp: u64) {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.orders_executed.fetch_add(1, Ordering::Relaxed);
        self.quantity_executed
            .fetch_add(quantity, Ordering::Relaxed);
        self.value_executed
            .fetch_add(quantity * price, Ordering::Relaxed);
        self.last_execution_time
            .store(current_time, Ordering::Relaxed);

        // Calculate waiting time for this order
        if order_timestamp > 0 {
            let waiting_time = current_time.saturating_sub(order_timestamp);
            self.sum_waiting_time
                .fetch_add(waiting_time, Ordering::Relaxed);
        }
    }

    /// Get total number of orders added
    pub fn orders_added(&self) -> usize {
        self.orders_added.load(Ordering::Relaxed)
    }

    /// Get total number of orders removed
    pub fn orders_removed(&self) -> usize {
        self.orders_removed.load(Ordering::Relaxed)
    }

    /// Get total number of orders executed
    pub fn orders_executed(&self) -> usize {
        self.orders_executed.load(Ordering::Relaxed)
    }

    /// Get total quantity executed
    pub fn quantity_executed(&self) -> u64 {
        self.quantity_executed.load(Ordering::Relaxed)
    }

    /// Get total value executed
    pub fn value_executed(&self) -> u64 {
        self.value_executed.load(Ordering::Relaxed)
    }

    /// Get average execution price
    pub fn average_execution_price(&self) -> Option<f64> {
        let qty = self.quantity_executed.load(Ordering::Relaxed);
        let value = self.value_executed.load(Ordering::Relaxed);

        if qty == 0 {
            None
        } else {
            Some(value as f64 / qty as f64)
        }
    }

    /// Get average waiting time for executed orders (in milliseconds)
    pub fn average_waiting_time(&self) -> Option<f64> {
        let count = self.orders_executed.load(Ordering::Relaxed);
        let sum = self.sum_waiting_time.load(Ordering::Relaxed);

        if count == 0 {
            None
        } else {
            Some(sum as f64 / count as f64)
        }
    }

    /// Get time since last execution (in milliseconds)
    pub fn time_since_last_execution(&self) -> Option<u64> {
        let last = self.last_execution_time.load(Ordering::Relaxed);
        if last == 0 {
            None
        } else {
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u64;

            Some(current_time.saturating_sub(last))
        }
    }

    /// Reset all statistics
    pub fn reset(&self) {
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis() as u64;

        self.orders_added.store(0, Ordering::Relaxed);
        self.orders_removed.store(0, Ordering::Relaxed);
        self.orders_executed.store(0, Ordering::Relaxed);
        self.quantity_executed.store(0, Ordering::Relaxed);
        self.value_executed.store(0, Ordering::Relaxed);
        self.last_execution_time.store(0, Ordering::Relaxed);
        self.first_arrival_time
            .store(current_time, Ordering::Relaxed);
        self.sum_waiting_time.store(0, Ordering::Relaxed);
    }
}

impl Default for PriceLevelStatistics {
    fn default() -> Self {
        Self::new()
    }
}


impl fmt::Display for PriceLevelStatistics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PriceLevelStatistics:orders_added={};orders_removed={};orders_executed={};quantity_executed={};value_executed={};last_execution_time={};first_arrival_time={};sum_waiting_time={}",
            self.orders_added.load(Ordering::Relaxed),
            self.orders_removed.load(Ordering::Relaxed),
            self.orders_executed.load(Ordering::Relaxed),
            self.quantity_executed.load(Ordering::Relaxed),
            self.value_executed.load(Ordering::Relaxed),
            self.last_execution_time.load(Ordering::Relaxed),
            self.first_arrival_time.load(Ordering::Relaxed),
            self.sum_waiting_time.load(Ordering::Relaxed)
        )
    }
}

impl FromStr for PriceLevelStatistics {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 || parts[0] != "PriceLevelStatistics" {
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

        let parse_usize = |field: &str, value: &str| -> Result<usize, PriceLevelError> {
            value
                .parse::<usize>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        let parse_u64 = |field: &str, value: &str| -> Result<u64, PriceLevelError> {
            value
                .parse::<u64>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        // Parse all fields
        let orders_added_str = get_field("orders_added")?;
        let orders_added = parse_usize("orders_added", orders_added_str)?;

        let orders_removed_str = get_field("orders_removed")?;
        let orders_removed = parse_usize("orders_removed", orders_removed_str)?;

        let orders_executed_str = get_field("orders_executed")?;
        let orders_executed = parse_usize("orders_executed", orders_executed_str)?;

        let quantity_executed_str = get_field("quantity_executed")?;
        let quantity_executed = parse_u64("quantity_executed", quantity_executed_str)?;

        let value_executed_str = get_field("value_executed")?;
        let value_executed = parse_u64("value_executed", value_executed_str)?;

        let last_execution_time_str = get_field("last_execution_time")?;
        let last_execution_time = parse_u64("last_execution_time", last_execution_time_str)?;

        let first_arrival_time_str = get_field("first_arrival_time")?;
        let first_arrival_time = parse_u64("first_arrival_time", first_arrival_time_str)?;

        let sum_waiting_time_str = get_field("sum_waiting_time")?;
        let sum_waiting_time = parse_u64("sum_waiting_time", sum_waiting_time_str)?;

        Ok(PriceLevelStatistics {
            orders_added: AtomicUsize::new(orders_added),
            orders_removed: AtomicUsize::new(orders_removed),
            orders_executed: AtomicUsize::new(orders_executed),
            quantity_executed: AtomicU64::new(quantity_executed),
            value_executed: AtomicU64::new(value_executed),
            last_execution_time: AtomicU64::new(last_execution_time),
            first_arrival_time: AtomicU64::new(first_arrival_time),
            sum_waiting_time: AtomicU64::new(sum_waiting_time),
        })
    }
}

impl Serialize for PriceLevelStatistics {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PriceLevelStatistics", 8)?;

        state.serialize_field("orders_added", &self.orders_added.load(Ordering::Relaxed))?;
        state.serialize_field("orders_removed", &self.orders_removed.load(Ordering::Relaxed))?;
        state.serialize_field("orders_executed", &self.orders_executed.load(Ordering::Relaxed))?;
        state.serialize_field("quantity_executed", &self.quantity_executed.load(Ordering::Relaxed))?;
        state.serialize_field("value_executed", &self.value_executed.load(Ordering::Relaxed))?;
        state.serialize_field("last_execution_time", &self.last_execution_time.load(Ordering::Relaxed))?;
        state.serialize_field("first_arrival_time", &self.first_arrival_time.load(Ordering::Relaxed))?;
        state.serialize_field("sum_waiting_time", &self.sum_waiting_time.load(Ordering::Relaxed))?;

        state.end()
    }
}

impl<'de> Deserialize<'de> for PriceLevelStatistics {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        enum Field {
            OrdersAdded,
            OrdersRemoved,
            OrdersExecuted,
            QuantityExecuted,
            ValueExecuted,
            LastExecutionTime,
            FirstArrivalTime,
            SumWaitingTime,
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Field, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct FieldVisitor;

                impl Visitor<'_> for FieldVisitor {
                    type Value = Field;

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str("field name")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Field, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            "orders_added" => Ok(Field::OrdersAdded),
                            "orders_removed" => Ok(Field::OrdersRemoved),
                            "orders_executed" => Ok(Field::OrdersExecuted),
                            "quantity_executed" => Ok(Field::QuantityExecuted),
                            "value_executed" => Ok(Field::ValueExecuted),
                            "last_execution_time" => Ok(Field::LastExecutionTime),
                            "first_arrival_time" => Ok(Field::FirstArrivalTime),
                            "sum_waiting_time" => Ok(Field::SumWaitingTime),
                            _ => Err(de::Error::unknown_field(value, FIELDS)),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct StatisticsVisitor;

        impl<'de> Visitor<'de> for StatisticsVisitor {
            type Value = PriceLevelStatistics;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct PriceLevelStatistics")
            }

            fn visit_map<V>(self, mut map: V) -> Result<PriceLevelStatistics, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut orders_added = None;
                let mut orders_removed = None;
                let mut orders_executed = None;
                let mut quantity_executed = None;
                let mut value_executed = None;
                let mut last_execution_time = None;
                let mut first_arrival_time = None;
                let mut sum_waiting_time = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::OrdersAdded => {
                            if orders_added.is_some() {
                                return Err(de::Error::duplicate_field("orders_added"));
                            }
                            orders_added = Some(map.next_value()?);
                        }
                        Field::OrdersRemoved => {
                            if orders_removed.is_some() {
                                return Err(de::Error::duplicate_field("orders_removed"));
                            }
                            orders_removed = Some(map.next_value()?);
                        }
                        Field::OrdersExecuted => {
                            if orders_executed.is_some() {
                                return Err(de::Error::duplicate_field("orders_executed"));
                            }
                            orders_executed = Some(map.next_value()?);
                        }
                        Field::QuantityExecuted => {
                            if quantity_executed.is_some() {
                                return Err(de::Error::duplicate_field("quantity_executed"));
                            }
                            quantity_executed = Some(map.next_value()?);
                        }
                        Field::ValueExecuted => {
                            if value_executed.is_some() {
                                return Err(de::Error::duplicate_field("value_executed"));
                            }
                            value_executed = Some(map.next_value()?);
                        }
                        Field::LastExecutionTime => {
                            if last_execution_time.is_some() {
                                return Err(de::Error::duplicate_field("last_execution_time"));
                            }
                            last_execution_time = Some(map.next_value()?);
                        }
                        Field::FirstArrivalTime => {
                            if first_arrival_time.is_some() {
                                return Err(de::Error::duplicate_field("first_arrival_time"));
                            }
                            first_arrival_time = Some(map.next_value()?);
                        }
                        Field::SumWaitingTime => {
                            if sum_waiting_time.is_some() {
                                return Err(de::Error::duplicate_field("sum_waiting_time"));
                            }
                            sum_waiting_time = Some(map.next_value()?);
                        }
                    }
                }

                let orders_added = orders_added.unwrap_or(0);
                let orders_removed = orders_removed.unwrap_or(0);
                let orders_executed = orders_executed.unwrap_or(0);
                let quantity_executed = quantity_executed.unwrap_or(0);
                let value_executed = value_executed.unwrap_or(0);
                let last_execution_time = last_execution_time.unwrap_or(0);

                let first_arrival_time = first_arrival_time.unwrap_or_else(|| {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64
                });

                let sum_waiting_time = sum_waiting_time.unwrap_or(0);

                Ok(PriceLevelStatistics {
                    orders_added: AtomicUsize::new(orders_added),
                    orders_removed: AtomicUsize::new(orders_removed),
                    orders_executed: AtomicUsize::new(orders_executed),
                    quantity_executed: AtomicU64::new(quantity_executed),
                    value_executed: AtomicU64::new(value_executed),
                    last_execution_time: AtomicU64::new(last_execution_time),
                    first_arrival_time: AtomicU64::new(first_arrival_time),
                    sum_waiting_time: AtomicU64::new(sum_waiting_time),
                })
            }
        }

        const FIELDS: &[&str] = &[
            "orders_added", "orders_removed", "orders_executed", "quantity_executed",
            "value_executed", "last_execution_time", "first_arrival_time", "sum_waiting_time"
        ];

        deserializer.deserialize_struct("PriceLevelStatistics", FIELDS, StatisticsVisitor)
    }
}