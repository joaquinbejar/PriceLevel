use crate::errors::PriceLevelError;
use crate::orders::OrderType;
use crate::price_level::statistics::PriceLevelStatistics;
use crate::utils::{Price, Quantity};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// A snapshot of a price level in the order book. This struct provides a summary of the state of a specific price level
/// at a given point in time, including the price, visible and hidden quantities, order count, the orders
/// at that level, and the per-level execution statistics.
#[derive(Debug, Default, Clone)]
pub struct PriceLevelSnapshot {
    /// The price of this level, in price ticks.
    price: Price,
    /// Total visible quantity at this level, in quantity units.
    visible_quantity: Quantity,
    /// Total hidden quantity at this level, in quantity units.
    hidden_quantity: Quantity,
    /// Number of orders at this level.
    order_count: usize,
    /// Orders at this level.
    orders: Vec<Arc<OrderType<()>>>,
    /// Per-level execution statistics captured at snapshot time.
    ///
    /// Carries the eight counters (orders added / removed / executed, quantity
    /// and value executed, last-execution and first-arrival timestamps, and the
    /// waiting-time sum) so a restored level resumes with its recorded history
    /// rather than a zeroed set. Persisted via [`PriceLevelStatistics`]'s own
    /// serde shape and covered by the package SHA-256 checksum.
    statistics: PriceLevelStatistics,
}

impl PriceLevelSnapshot {
    /// Create a new empty snapshot at the given price.
    #[must_use]
    pub fn new(price: Price) -> Self {
        Self {
            price,
            visible_quantity: Quantity::ZERO,
            hidden_quantity: Quantity::ZERO,
            order_count: 0,
            orders: Vec::new(),
            statistics: PriceLevelStatistics::new(),
        }
    }

    /// Creates a snapshot populated with orders, computing aggregates automatically.
    ///
    /// The statistics are initialized empty; use [`Self::with_orders_and_stats`]
    /// to carry recorded execution statistics into the snapshot.
    ///
    /// The `orders` vector order is significant: it is the queue-consumption
    /// order a restore reproduces, since
    /// [`crate::price_level::PriceLevel::from_snapshot`] re-enqueues in vector
    /// order. Pass orders in ascending insertion-sequence (sweep) order to
    /// preserve price-time priority across a round-trip.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if summing the per-order
    /// visible / hidden quantities overflows `u64`.
    pub fn with_orders(
        price: Price,
        orders: Vec<Arc<OrderType<()>>>,
    ) -> Result<Self, PriceLevelError> {
        Self::with_orders_and_stats(price, orders, PriceLevelStatistics::new())
    }

    /// Creates a snapshot populated with orders and statistics, computing
    /// aggregates automatically.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if summing the per-order
    /// visible / hidden quantities overflows `u64`.
    pub fn with_orders_and_stats(
        price: Price,
        orders: Vec<Arc<OrderType<()>>>,
        statistics: PriceLevelStatistics,
    ) -> Result<Self, PriceLevelError> {
        let mut snapshot = Self {
            price,
            visible_quantity: Quantity::ZERO,
            hidden_quantity: Quantity::ZERO,
            order_count: 0,
            orders,
            statistics,
        };
        snapshot.refresh_aggregates()?;
        Ok(snapshot)
    }

    /// Returns the price of this level, in price ticks.
    #[must_use]
    pub fn price(&self) -> Price {
        self.price
    }

    /// Returns the total visible quantity, in quantity units.
    #[must_use]
    pub fn visible_quantity(&self) -> Quantity {
        self.visible_quantity
    }

    /// Returns the total hidden quantity, in quantity units.
    #[must_use]
    pub fn hidden_quantity(&self) -> Quantity {
        self.hidden_quantity
    }

    /// Returns the number of orders.
    #[must_use]
    pub fn order_count(&self) -> usize {
        self.order_count
    }

    /// Returns a reference to the per-level statistics captured in this snapshot.
    #[must_use]
    pub fn statistics(&self) -> &PriceLevelStatistics {
        &self.statistics
    }

    /// Returns a reference to the orders in this snapshot.
    ///
    /// The vector order is significant: it is the queue-consumption order a
    /// restore reproduces, since [`crate::price_level::PriceLevel::from_snapshot`]
    /// re-enqueues the orders in vector order.
    #[must_use]
    pub fn orders(&self) -> &[Arc<OrderType<()>>] {
        &self.orders
    }

    /// Consumes the snapshot and returns the inner orders vector.
    #[must_use]
    pub fn into_orders(self) -> Vec<Arc<OrderType<()>>> {
        self.orders
    }

    /// Constructs a snapshot with pre-computed aggregates and empty statistics.
    ///
    /// This is intended for internal crate use where the caller has already
    /// computed the aggregate values (e.g., from atomic counters) and does not
    /// carry execution statistics. Use [`Self::from_raw_parts_with_stats`] to
    /// persist recorded statistics alongside the aggregates.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_raw_parts(
        price: Price,
        visible_quantity: Quantity,
        hidden_quantity: Quantity,
        order_count: usize,
        orders: Vec<Arc<OrderType<()>>>,
    ) -> Self {
        Self::from_raw_parts_with_stats(
            price,
            visible_quantity,
            hidden_quantity,
            order_count,
            orders,
            PriceLevelStatistics::new(),
        )
    }

    /// Constructs a snapshot with pre-computed aggregates and recorded statistics.
    ///
    /// This is intended for internal crate use where the caller has already
    /// computed the aggregate values (e.g., from atomic counters) and wants to
    /// preserve the level's execution statistics through the snapshot.
    #[must_use]
    pub(crate) fn from_raw_parts_with_stats(
        price: Price,
        visible_quantity: Quantity,
        hidden_quantity: Quantity,
        order_count: usize,
        orders: Vec<Arc<OrderType<()>>>,
        statistics: PriceLevelStatistics,
    ) -> Self {
        Self {
            price,
            visible_quantity,
            hidden_quantity,
            order_count,
            orders,
            statistics,
        }
    }

    /// Get the total quantity (visible + hidden) at this price level.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `visible + hidden`
    /// overflows `u64`.
    pub fn total_quantity(&self) -> Result<Quantity, PriceLevelError> {
        self.visible_quantity
            .as_u64()
            .checked_add(self.hidden_quantity.as_u64())
            .map(Quantity::new)
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: "snapshot total quantity overflow".to_string(),
            })
    }

    /// Get an iterator over the orders in this snapshot
    pub fn iter_orders(&self) -> impl Iterator<Item = &Arc<OrderType<()>>> {
        self.orders.iter()
    }

    /// Recomputes aggregate fields (`visible_quantity`, `hidden_quantity`, and `order_count`) based on current orders.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any single order's own
    /// visible + hidden total overflows `u64`, or if summing the per-order
    /// visible or hidden quantities across the level overflows `u64`.
    pub fn refresh_aggregates(&mut self) -> Result<(), PriceLevelError> {
        self.order_count = self.orders.len();

        let mut visible_total: u64 = 0;
        let mut hidden_total: u64 = 0;

        for order in &self.orders {
            // Reject any order whose OWN visible + hidden total is not
            // representable in `u64`. `PriceLevel::add_order` enforces this same
            // per-order invariant at admission, and the match sweep's reserve
            // replenishment relies on it (a refreshed tranche is
            // `new_visible + drawn_hidden <= visible + hidden`, which overflows
            // only if the order's own total already does). Restoring such an
            // order would smuggle in a state admission rejects, so the restore
            // path validates it too rather than trusting the serialized bytes.
            order
                .visible_quantity()
                .as_u64()
                .checked_add(order.hidden_quantity().as_u64())
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "order total quantity overflows u64".to_string(),
                })?;

            visible_total = visible_total
                .checked_add(order.visible_quantity().as_u64())
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "snapshot visible quantity overflow".to_string(),
                })?;

            hidden_total = hidden_total
                .checked_add(order.hidden_quantity().as_u64())
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "snapshot hidden quantity overflow".to_string(),
                })?;
        }

        self.visible_quantity = Quantity::new(visible_total);
        self.hidden_quantity = Quantity::new(hidden_total);

        Ok(())
    }
}

/// Format version for checksum-enabled price level snapshots.
///
/// Version 2 (issue #63) persists per-level [`PriceLevelStatistics`] inside the
/// snapshot. Version 1 packages carried no statistics and are rejected by
/// [`PriceLevelSnapshotPackage::validate`] with a version mismatch.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 2;

/// Serialized representation of a price level snapshot including checksum validation metadata.
///
/// All fields are private to protect checksum integrity.
/// Use the provided accessor methods to read package data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevelSnapshotPackage {
    /// Version of the serialized snapshot schema to support future migrations.
    version: u32,
    /// Captured snapshot data.
    snapshot: PriceLevelSnapshot,
    /// Hex-encoded checksum used to validate the snapshot integrity.
    checksum: String,
}

impl PriceLevelSnapshotPackage {
    /// Returns the schema version of this package.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Returns a reference to the contained snapshot.
    #[must_use]
    pub fn snapshot(&self) -> &PriceLevelSnapshot {
        &self.snapshot
    }

    /// Returns the hex-encoded checksum.
    #[must_use]
    pub fn checksum(&self) -> &str {
        &self.checksum
    }
}

impl PriceLevelSnapshotPackage {
    /// Creates a new snapshot package computing the checksum for the provided snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if refreshing the snapshot
    /// aggregates overflows a quantity, or [`PriceLevelError::SerializationError`]
    /// if the snapshot payload cannot be encoded while computing its SHA-256
    /// checksum.
    pub fn new(mut snapshot: PriceLevelSnapshot) -> Result<Self, PriceLevelError> {
        snapshot.refresh_aggregates()?;

        let checksum = Self::compute_checksum(&snapshot)?;

        Ok(Self {
            version: SNAPSHOT_FORMAT_VERSION,
            snapshot,
            checksum,
        })
    }

    /// Serializes the package to JSON.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::SerializationError`] if the package cannot be
    /// encoded to a JSON string.
    pub fn to_json(&self) -> Result<String, PriceLevelError> {
        serde_json::to_string(self).map_err(|error| PriceLevelError::SerializationError {
            message: error.to_string(),
        })
    }

    /// Deserializes a package from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DeserializationError`] if `data` is not a
    /// valid JSON representation of a snapshot package. The returned package is
    /// not yet checksum-validated; call [`Self::validate`] or
    /// [`Self::into_snapshot`] to verify integrity.
    pub fn from_json(data: &str) -> Result<Self, PriceLevelError> {
        serde_json::from_str(data).map_err(|error| PriceLevelError::DeserializationError {
            message: error.to_string(),
        })
    }

    /// Validates the checksum contained in the package against the serialized snapshot data.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the package's format
    /// version is not `SNAPSHOT_FORMAT_VERSION`, [`PriceLevelError::SerializationError`]
    /// if the snapshot payload cannot be re-encoded to recompute the checksum,
    /// and [`PriceLevelError::ChecksumMismatch`] if the recomputed SHA-256
    /// checksum does not match the stored one (tampered or corrupted snapshot).
    // Snapshot restoration / validation is a cold path: keep it out of line.
    #[inline(never)]
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
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate`]:
    /// [`PriceLevelError::InvalidOperation`] on an unsupported format version,
    /// [`PriceLevelError::SerializationError`] if the payload cannot be
    /// re-encoded, and [`PriceLevelError::ChecksumMismatch`] if the stored
    /// checksum does not match the recomputed one.
    pub fn into_snapshot(self) -> Result<PriceLevelSnapshot, PriceLevelError> {
        self.validate()?;
        Ok(self.snapshot)
    }

    #[inline(never)]
    fn compute_checksum(snapshot: &PriceLevelSnapshot) -> Result<String, PriceLevelError> {
        use std::fmt::Write as _;

        let payload =
            serde_json::to_vec(snapshot).map_err(|error| PriceLevelError::SerializationError {
                message: error.to_string(),
            })?;

        let mut hasher = Sha256::new();
        hasher.update(payload);

        // `digest` 0.11 returns the digest as a `hybrid_array::Array`, which —
        // unlike the `generic_array::GenericArray` from 0.10 — does not
        // implement `LowerHex`. Encode the raw SHA-256 bytes to lowercase hex
        // by hand. The bytes are defined by the algorithm and are unchanged, so
        // the produced checksum string is byte-identical to the 0.10 output.
        let checksum_bytes = hasher.finalize();
        let mut checksum = String::with_capacity(checksum_bytes.len() * 2);
        for byte in checksum_bytes {
            // Writing to a `String` is infallible; `{byte:02x}` is the same
            // lowercase, zero-padded, two-hex-digits-per-byte encoding the
            // previous `format!("{:x}", checksum_bytes)` produced.
            let _ = write!(checksum, "{byte:02x}");
        }
        Ok(checksum)
    }
}

impl Serialize for PriceLevelSnapshot {
    // Snapshot serialization is a cold path (taken/restored, not per-match):
    // keep it out of line.
    #[inline(never)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PriceLevelSnapshot", 6)?;

        state.serialize_field("price", &self.price)?;
        state.serialize_field("visible_quantity", &self.visible_quantity)?;
        state.serialize_field("hidden_quantity", &self.hidden_quantity)?;
        state.serialize_field("order_count", &self.order_count)?;

        // Serialize the borrowed orders rather than deep-copying every
        // `OrderType<()>` by value (issue #72). `Serialize for &T` forwards to
        // `T`'s impl, so a sequence of `&OrderType<()>` produces byte-identical
        // output to the previous `Vec<OrderType<()>>` — the checksum and
        // round-trip are unchanged — while only copying `Arc` pointers, not the
        // whole order payload.
        let borrowed_orders: Vec<&OrderType<()>> = self.orders.iter().map(Arc::as_ref).collect();

        state.serialize_field("orders", &borrowed_orders)?;
        state.serialize_field("statistics", &self.statistics)?;

        state.end()
    }
}

impl<'de> Deserialize<'de> for PriceLevelSnapshot {
    // Snapshot restoration is a cold path (taken/restored, not per-match):
    // keep it out of line.
    #[inline(never)]
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
            Statistics,
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
                        formatter.write_str("`price`, `visible_quantity`, `hidden_quantity`, `order_count`, `orders`, or `statistics`")
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
                            "statistics" => Ok(Field::Statistics),
                            _ => Err(de::Error::unknown_field(
                                value,
                                &[
                                    "price",
                                    "visible_quantity",
                                    "hidden_quantity",
                                    "order_count",
                                    "orders",
                                    "statistics",
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
                let mut statistics = None;

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
                        Field::Statistics => {
                            if statistics.is_some() {
                                return Err(de::Error::duplicate_field("statistics"));
                            }
                            statistics = Some(map.next_value()?);
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
                // `statistics` is optional on deserialize so a payload that omits
                // it (e.g. a hand-built fixture) restores with empty statistics
                // rather than failing — i.e. tolerant of a *missing* field, not
                // forward-compatible with future *added* fields (this visitor
                // still rejects unknown fields). A genuine v1 *package* is
                // rejected up-front by `validate()`'s version check regardless.
                let statistics = statistics.unwrap_or_default();

                Ok(PriceLevelSnapshot {
                    price,
                    visible_quantity,
                    hidden_quantity,
                    order_count,
                    orders,
                    statistics,
                })
            }
        }

        const FIELDS: &[&str] = &[
            "price",
            "visible_quantity",
            "hidden_quantity",
            "order_count",
            "orders",
            "statistics",
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

        let parse_u128 = |field: &str, value: &str| -> Result<u128, PriceLevelError> {
            value
                .parse::<u128>()
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
        let price = parse_u128("price", price_str)?;

        let visible_quantity_str = get_field("visible_quantity")?;
        let visible_quantity = parse_u64("visible_quantity", visible_quantity_str)?;

        let hidden_quantity_str = get_field("hidden_quantity")?;
        let hidden_quantity = parse_u64("hidden_quantity", hidden_quantity_str)?;

        let order_count_str = get_field("order_count")?;
        let order_count = parse_usize("order_count", order_count_str)?;

        // Create a new snapshot - note that orders and statistics cannot be
        // serialized/deserialized in this simple, human-readable format. Use the
        // JSON snapshot package for a lossless round-trip.
        Ok(PriceLevelSnapshot {
            price: Price::new(price),
            visible_quantity: Quantity::new(visible_quantity),
            hidden_quantity: Quantity::new(hidden_quantity),
            order_count,
            orders: Vec::new(),
            statistics: PriceLevelStatistics::new(),
        })
    }
}
