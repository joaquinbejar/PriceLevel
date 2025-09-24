use crate::errors::PriceLevelError;
use crate::orders::OrderType;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// A snapshot of a price level in the order book. This struct provides a summary of the state of a specific price level
/// at a given point in time, including the price, visible and hidden quantities, order count, and a vector of the orders
/// at that level.
#[derive(Debug, Default, Clone)]
pub struct PriceLevelSnapshot {
    /// The price of this level.
    pub price: u64,
    /// Total visible quantity at this level. This represents the sum of the visible quantities of all orders at this price level.
    pub visible_quantity: u64,
    /// Total hidden quantity at this level. This represents the sum of the hidden quantities of all orders at this price level.
    pub hidden_quantity: u64,
    /// Number of orders at this level.
    pub order_count: usize,
    /// Orders at this level.  This is a vector of `Arc<OrderType<()>>` representing each individual order at this price level.
    pub orders: Vec<Arc<OrderType<()>>>,
}

impl PriceLevelSnapshot {
    /// Create a new empty snapshot
    pub fn new(price: u64) -> Self {
        Self {
            price,
            visible_quantity: 0,
            hidden_quantity: 0,
            order_count: 0,
            orders: Vec::new(),
        }
    }

    /// Get the total quantity (visible + hidden) at this price level
    pub fn total_quantity(&self) -> u64 {
        self.visible_quantity + self.hidden_quantity
    }

    /// Get an iterator over the orders in this snapshot
    pub fn iter_orders(&self) -> impl Iterator<Item = &Arc<OrderType<()>>> {
        self.orders.iter()
    }

    /// Recomputes aggregate fields (`visible_quantity`, `hidden_quantity`, and `order_count`) based on current orders.
    pub fn refresh_aggregates(&mut self) {
        self.order_count = self.orders.len();

        let mut visible_total: u64 = 0;
        let mut hidden_total: u64 = 0;

        for order in &self.orders {
            visible_total = visible_total.saturating_add(order.visible_quantity());
            hidden_total = hidden_total.saturating_add(order.hidden_quantity());
        }

        self.visible_quantity = visible_total;
        self.hidden_quantity = hidden_total;
    }
}

/// Format version for checksum-enabled price level snapshots.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Serialized representation of a price level snapshot including checksum validation metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevelSnapshotPackage {
    /// Version of the serialized snapshot schema to support future migrations.
    pub version: u32,
    /// Captured snapshot data.
    pub snapshot: PriceLevelSnapshot,
    /// Hex-encoded checksum used to validate the snapshot integrity.
    pub checksum: String,
}

impl PriceLevelSnapshotPackage {
    /// Creates a new snapshot package computing the checksum for the provided snapshot.
    pub fn new(mut snapshot: PriceLevelSnapshot) -> Result<Self, PriceLevelError> {
        snapshot.refresh_aggregates();

        let checksum = Self::compute_checksum(&snapshot)?;

        Ok(Self {
            version: SNAPSHOT_FORMAT_VERSION,
            snapshot,
            checksum,
        })
    }

    /// Serializes the package to JSON.
    pub fn to_json(&self) -> Result<String, PriceLevelError> {
        serde_json::to_string(self).map_err(|error| PriceLevelError::SerializationError {
            message: error.to_string(),
        })
    }

    /// Deserializes a package from JSON.
    pub fn from_json(data: &str) -> Result<Self, PriceLevelError> {
        serde_json::from_str(data).map_err(|error| PriceLevelError::DeserializationError {
            message: error.to_string(),
        })
    }

    /// Validates the checksum contained in the package against the serialized snapshot data.
    pub fn validate(&self) -> Result<(), PriceLevelError> {
        if self.version != SNAPSHOT_FORMAT_VERSION {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "Unsupported snapshot version: {} (expected {})",
                    self.version, SNAPSHOT_FORMAT_VERSION
                ),
            });
        }

        let computed = Self::compute_checksum(&self.snapshot)?;
        if computed != self.checksum {
            return Err(PriceLevelError::ChecksumMismatch {
                expected: self.checksum.clone(),
                actual: computed,
            });
        }

        Ok(())
    }

    /// Consumes the package after validating the checksum and returns the contained snapshot.
    pub fn into_snapshot(self) -> Result<PriceLevelSnapshot, PriceLevelError> {
        self.validate()?;
        Ok(self.snapshot)
    }

    fn compute_checksum(snapshot: &PriceLevelSnapshot) -> Result<String, PriceLevelError> {
        let payload =
            serde_json::to_vec(snapshot).map_err(|error| PriceLevelError::SerializationError {
                message: error.to_string(),
            })?;

        let mut hasher = Sha256::new();
        hasher.update(payload);

        let checksum_bytes = hasher.finalize();
        Ok(format!("{:x}", checksum_bytes))
    }
}

impl Serialize for PriceLevelSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PriceLevelSnapshot", 5)?;

        state.serialize_field("price", &self.price)?;
        state.serialize_field("visible_quantity", &self.visible_quantity)?;
        state.serialize_field("hidden_quantity", &self.hidden_quantity)?;
        state.serialize_field("order_count", &self.order_count)?;

        let plain_orders: Vec<OrderType<()>> =
            self.orders.iter().map(|arc_order| **arc_order).collect();

        state.serialize_field("orders", &plain_orders)?;

        state.end()
    }
}

impl<'de> Deserialize<'de> for PriceLevelSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        enum Field {
            Price,
            VisibleQuantity,
            HiddenQuantity,
            OrderCount,
            Orders,
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
                        formatter.write_str("`price`, `visible_quantity`, `hidden_quantity`, `order_count`, or `orders`")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Field, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            "price" => Ok(Field::Price),
                            "visible_quantity" => Ok(Field::VisibleQuantity),
                            "hidden_quantity" => Ok(Field::HiddenQuantity),
                            "order_count" => Ok(Field::OrderCount),
                            "orders" => Ok(Field::Orders),
                            _ => Err(de::Error::unknown_field(
                                value,
                                &[
                                    "price",
                                    "visible_quantity",
                                    "hidden_quantity",
                                    "order_count",
                                    "orders",
                                ],
                            )),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct PriceLevelSnapshotVisitor;

        impl<'de> Visitor<'de> for PriceLevelSnapshotVisitor {
            type Value = PriceLevelSnapshot;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct PriceLevelSnapshot")
            }

            fn visit_map<V>(self, mut map: V) -> Result<PriceLevelSnapshot, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut price = None;
                let mut visible_quantity = None;
                let mut hidden_quantity = None;
                let mut order_count = None;
                let mut orders = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Price => {
                            if price.is_some() {
                                return Err(de::Error::duplicate_field("price"));
                            }
                            price = Some(map.next_value()?);
                        }
                        Field::VisibleQuantity => {
                            if visible_quantity.is_some() {
                                return Err(de::Error::duplicate_field("visible_quantity"));
                            }
                            visible_quantity = Some(map.next_value()?);
                        }
                        Field::HiddenQuantity => {
                            if hidden_quantity.is_some() {
                                return Err(de::Error::duplicate_field("hidden_quantity"));
                            }
                            hidden_quantity = Some(map.next_value()?);
                        }
                        Field::OrderCount => {
                            if order_count.is_some() {
                                return Err(de::Error::duplicate_field("order_count"));
                            }
                            order_count = Some(map.next_value()?);
                        }
                        Field::Orders => {
                            if orders.is_some() {
                                return Err(de::Error::duplicate_field("orders"));
                            }
                            let plain_orders: Vec<OrderType<()>> = map.next_value()?;
                            orders = Some(plain_orders.into_iter().map(Arc::new).collect());
                        }
                    }
                }

                let price = price.ok_or_else(|| de::Error::missing_field("price"))?;
                let visible_quantity =
                    visible_quantity.ok_or_else(|| de::Error::missing_field("visible_quantity"))?;
                let hidden_quantity =
                    hidden_quantity.ok_or_else(|| de::Error::missing_field("hidden_quantity"))?;
                let order_count =
                    order_count.ok_or_else(|| de::Error::missing_field("order_count"))?;
                let orders = orders.unwrap_or_default();

                Ok(PriceLevelSnapshot {
                    price,
                    visible_quantity,
                    hidden_quantity,
                    order_count,
                    orders,
                })
            }
        }

        const FIELDS: &[&str] = &[
            "price",
            "visible_quantity",
            "hidden_quantity",
            "order_count",
            "orders",
        ];
        deserializer.deserialize_struct("PriceLevelSnapshot", FIELDS, PriceLevelSnapshotVisitor)
    }
}

impl fmt::Display for PriceLevelSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PriceLevelSnapshot:price={};visible_quantity={};hidden_quantity={};order_count={}",
            self.price, self.visible_quantity, self.hidden_quantity, self.order_count
        )
    }
}

impl FromStr for PriceLevelSnapshot {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 || parts[0] != "PriceLevelSnapshot" {
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

        let parse_usize = |field: &str, value: &str| -> Result<usize, PriceLevelError> {
            value
                .parse::<usize>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        // Parse fields
        let price_str = get_field("price")?;
        let price = parse_u64("price", price_str)?;

        let visible_quantity_str = get_field("visible_quantity")?;
        let visible_quantity = parse_u64("visible_quantity", visible_quantity_str)?;

        let hidden_quantity_str = get_field("hidden_quantity")?;
        let hidden_quantity = parse_u64("hidden_quantity", hidden_quantity_str)?;

        let order_count_str = get_field("order_count")?;
        let order_count = parse_usize("order_count", order_count_str)?;

        // Create a new snapshot - note that orders cannot be serialized/deserialized in this simple format
        Ok(PriceLevelSnapshot {
            price,
            visible_quantity,
            hidden_quantity,
            order_count,
            orders: Vec::new(),
        })
    }
}
