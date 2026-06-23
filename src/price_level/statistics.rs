use crate::errors::PriceLevelError;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tracks performance statistics for a price level.
///
/// All counters are private atomics so that no external consumer can
/// `.store()` / `.fetch_add()` directly and desync them from the order queue
/// or from the checked-arithmetic invariants enforced in
/// [`record_execution`](Self::record_execution). Mutation happens only through
/// the `record_*` / [`reset`](Self::reset) methods; reads happen only through
/// the public accessors.
///
/// # Atomic ordering
///
/// **Every atomic access in this type uses [`Ordering::Relaxed`], deliberately.**
/// These are pure observability counters: nothing in the engine reads a
/// statistic to gate a queue mutation, and no other field's visibility is
/// published through them. They carry no happens-before relationship — each
/// counter is independent and may be read mid-update, so a multi-field read
/// (serde, [`Display`](std::fmt::Display), [`Clone`]) is an explicitly
/// best-effort, non-transactional point-in-time view (the same contract as
/// `PriceLevel::snapshot`). `Relaxed` is therefore both correct and the
/// cheapest valid choice; a stronger ordering would only add cost without
/// buying any guarantee a consumer relies on. The lone read-modify-write loop
/// in [`checked_fetch_add_u64`](Self::checked_fetch_add_u64) is a standard
/// `compare_exchange_weak` CAS retry — see the note there for why both its
/// success and failure orderings are `Relaxed`.
#[derive(Debug)]
pub struct PriceLevelStatistics {
    /// Number of orders added
    orders_added: AtomicUsize,

    /// Number of orders removed
    orders_removed: AtomicUsize,

    /// Number of orders executed
    orders_executed: AtomicUsize,

    /// Total quantity executed
    quantity_executed: AtomicU64,

    /// Total value executed
    value_executed: AtomicU64,

    /// Last execution timestamp
    last_execution_time: AtomicU64,

    /// Statistics initialization timestamp (set at construction / reset).
    /// Not updated on order arrival — see `first_arrival_time()`.
    first_arrival_time: AtomicU64,

    /// Sum of waiting times for orders
    sum_waiting_time: AtomicU64,
}

impl PriceLevelStatistics {
    fn checked_fetch_add_u64(
        target: &AtomicU64,
        value: u64,
        field: &str,
    ) -> Result<(), PriceLevelError> {
        // `Relaxed`: this is an advisory observability counter (see the
        // struct-level "Atomic ordering" note) — no happens-before rides on it.
        let mut current = target.load(Ordering::Relaxed);

        loop {
            let next =
                current
                    .checked_add(value)
                    .ok_or_else(|| PriceLevelError::InvalidOperation {
                        message: format!("{field} overflow"),
                    })?;

            // Standard lock-free CAS retry. Both the success and failure
            // orderings are `Relaxed`: the only invariant is that the stored
            // value is a checked sum of monotonic increments (the loop re-reads
            // `observed` and re-validates on contention). The counter publishes
            // nothing to another thread, so neither acquire on failure nor
            // release on success is needed. The retry body is allocation-free,
            // per the tight-CAS-loop rule.
            match target.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    #[inline]
    fn current_timestamp_milliseconds() -> Result<u64, PriceLevelError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| PriceLevelError::InvalidOperation {
                message: format!("system clock error while reading unix time: {error}"),
            })
            .map(|duration| duration.as_millis() as u64)
    }

    #[inline]
    fn current_timestamp_milliseconds_or_zero() -> u64 {
        Self::current_timestamp_milliseconds().unwrap_or(0)
    }

    /// Create new empty statistics
    #[must_use]
    pub fn new() -> Self {
        let current_time = Self::current_timestamp_milliseconds_or_zero();

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

    /// Record an order execution.
    ///
    /// The `execution_timestamp` is the taker timestamp threaded in from the
    /// caller (the same value stamped onto the emitted [`Trade`]s). It is used
    /// both as the level's `last_execution_time` and as the reference time for
    /// the per-maker waiting-time accumulation. This keeps the match path
    /// clock-free and deterministic: no wall-clock read happens during a match.
    ///
    /// [`Trade`]: crate::execution::Trade
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any of the counter
    /// accumulations overflow, if the value (`quantity * price`) overflows
    /// `u128`/`u64`, or if `order_timestamp` is strictly greater than
    /// `execution_timestamp` (a maker arriving in the future of execution).
    pub fn record_execution(
        &self,
        quantity: u64,
        price: u128,
        order_timestamp: u64,
        execution_timestamp: u64,
    ) -> Result<(), PriceLevelError> {
        let current_time = execution_timestamp;

        // Validate everything that can fail BEFORE mutating any counter, so a
        // rejected record leaves the statistics untouched. `match_order` ignores
        // the returned `Result`, so any partial side effect would silently
        // corrupt the stats.
        let waiting_time = if order_timestamp > 0 {
            Some(current_time.checked_sub(order_timestamp).ok_or_else(|| {
                PriceLevelError::InvalidOperation {
                    message: format!(
                        "order timestamp {} is in the future of current time {}",
                        order_timestamp, current_time
                    ),
                }
            })?)
        } else {
            None
        };

        let value_u128 = u128::from(quantity).checked_mul(price).ok_or_else(|| {
            PriceLevelError::InvalidOperation {
                message: "value_executed multiplication overflow".to_string(),
            }
        })?;

        let value_u64 =
            u64::try_from(value_u128).map_err(|_| PriceLevelError::InvalidOperation {
                message: "value_executed exceeds u64 storage".to_string(),
            })?;

        // All fallible validation passed — now apply the mutations. (The only
        // residual failure mode is a counter nearing `u64::MAX`, inherent to
        // independent lock-free atomics.)
        self.orders_executed.fetch_add(1, Ordering::Relaxed);
        Self::checked_fetch_add_u64(&self.quantity_executed, quantity, "quantity_executed")?;
        Self::checked_fetch_add_u64(&self.value_executed, value_u64, "value_executed")?;
        self.last_execution_time
            .store(current_time, Ordering::Relaxed);
        if let Some(waiting_time) = waiting_time {
            Self::checked_fetch_add_u64(&self.sum_waiting_time, waiting_time, "sum_waiting_time")?;
        }

        Ok(())
    }

    /// Get total number of orders added
    #[must_use]
    pub fn orders_added(&self) -> usize {
        self.orders_added.load(Ordering::Relaxed)
    }

    /// Get total number of orders removed
    #[must_use]
    pub fn orders_removed(&self) -> usize {
        self.orders_removed.load(Ordering::Relaxed)
    }

    /// Get total number of orders executed
    #[must_use]
    pub fn orders_executed(&self) -> usize {
        self.orders_executed.load(Ordering::Relaxed)
    }

    /// Get total quantity executed
    #[must_use]
    pub fn quantity_executed(&self) -> u64 {
        self.quantity_executed.load(Ordering::Relaxed)
    }

    /// Get total value executed
    #[must_use]
    pub fn value_executed(&self) -> u64 {
        self.value_executed.load(Ordering::Relaxed)
    }

    /// Get the timestamp of the most recent execution, in milliseconds since
    /// the Unix epoch.
    ///
    /// Returns `0` when no execution has been recorded yet.
    #[must_use]
    pub fn last_execution_time(&self) -> u64 {
        self.last_execution_time.load(Ordering::Relaxed)
    }

    /// Get the statistics initialization timestamp, in milliseconds since the
    /// Unix epoch.
    ///
    /// Set when the statistics are created and on [`reset`](Self::reset) with the
    /// current wall-clock time (`0` if the system clock could not be read). It is
    /// **not** updated on order arrival, so it marks when statistics tracking
    /// began for this level, not the first order's actual arrival time.
    #[must_use]
    pub fn first_arrival_time(&self) -> u64 {
        self.first_arrival_time.load(Ordering::Relaxed)
    }

    /// Get the accumulated waiting time across all executed orders, in
    /// milliseconds.
    ///
    /// This is the sum of `execution_timestamp - order_timestamp` over every
    /// recorded execution that carried a non-zero maker timestamp. Divide by
    /// [`orders_executed`](Self::orders_executed) for the average; see
    /// [`average_waiting_time`](Self::average_waiting_time).
    #[must_use]
    pub fn sum_waiting_time(&self) -> u64 {
        self.sum_waiting_time.load(Ordering::Relaxed)
    }

    /// Get average execution price
    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn time_since_last_execution(&self) -> Option<u64> {
        let last = self.last_execution_time.load(Ordering::Relaxed);
        if last == 0 {
            None
        } else {
            let current_time = Self::current_timestamp_milliseconds_or_zero();
            current_time.checked_sub(last)
        }
    }

    /// Reset all statistics
    pub fn reset(&self) {
        let current_time = Self::current_timestamp_milliseconds_or_zero();

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

impl Clone for PriceLevelStatistics {
    /// Clones the statistics by snapshotting each atomic counter.
    ///
    /// Each counter is read with [`Ordering::Relaxed`]: the clone is a
    /// best-effort point-in-time copy of independent counters (the same
    /// contract as the serde path and `snapshot()`), not a globally consistent
    /// transactional read across all eight fields. This is the representation
    /// persisted in [`PriceLevelSnapshot`](crate::price_level::PriceLevelSnapshot),
    /// so a restored level carries the recorded statistics rather than a fresh,
    /// zeroed set.
    fn clone(&self) -> Self {
        Self {
            orders_added: AtomicUsize::new(self.orders_added.load(Ordering::Relaxed)),
            orders_removed: AtomicUsize::new(self.orders_removed.load(Ordering::Relaxed)),
            orders_executed: AtomicUsize::new(self.orders_executed.load(Ordering::Relaxed)),
            quantity_executed: AtomicU64::new(self.quantity_executed.load(Ordering::Relaxed)),
            value_executed: AtomicU64::new(self.value_executed.load(Ordering::Relaxed)),
            last_execution_time: AtomicU64::new(self.last_execution_time.load(Ordering::Relaxed)),
            first_arrival_time: AtomicU64::new(self.first_arrival_time.load(Ordering::Relaxed)),
            sum_waiting_time: AtomicU64::new(self.sum_waiting_time.load(Ordering::Relaxed)),
        }
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
        state.serialize_field(
            "orders_removed",
            &self.orders_removed.load(Ordering::Relaxed),
        )?;
        state.serialize_field(
            "orders_executed",
            &self.orders_executed.load(Ordering::Relaxed),
        )?;
        state.serialize_field(
            "quantity_executed",
            &self.quantity_executed.load(Ordering::Relaxed),
        )?;
        state.serialize_field(
            "value_executed",
            &self.value_executed.load(Ordering::Relaxed),
        )?;
        state.serialize_field(
            "last_execution_time",
            &self.last_execution_time.load(Ordering::Relaxed),
        )?;
        state.serialize_field(
            "first_arrival_time",
            &self.first_arrival_time.load(Ordering::Relaxed),
        )?;
        state.serialize_field(
            "sum_waiting_time",
            &self.sum_waiting_time.load(Ordering::Relaxed),
        )?;

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
                    PriceLevelStatistics::current_timestamp_milliseconds_or_zero()
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
            "orders_added",
            "orders_removed",
            "orders_executed",
            "quantity_executed",
            "value_executed",
            "last_execution_time",
            "first_arrival_time",
            "sum_waiting_time",
        ];

        deserializer.deserialize_struct("PriceLevelStatistics", FIELDS, StatisticsVisitor)
    }
}
