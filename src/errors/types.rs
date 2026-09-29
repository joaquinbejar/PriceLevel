use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter, Result};

/// The storage or finite sequence a [`PriceLevelError::CapacityExceeded`]
/// failure could not grow or advance.
///
/// A fixed, `Copy`, payload-free tag so reporting an allocation / capacity
/// failure never needs to allocate (a `String` message built after a failed
/// allocation could itself fail). New growth paths add variants here, so the
/// enum is `#[non_exhaustive]`: match it with a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityResource {
    /// The trade vector of a [`TradeList`](crate::TradeList) /
    /// [`MatchResult`](crate::MatchResult).
    Trades,
    /// The filled-maker id vector of a [`MatchResult`](crate::MatchResult).
    FilledOrderIds,
    /// Scratch storage used while validating a decoded
    /// [`MatchResult`](crate::MatchResult) (the duplicate-id set).
    ValidationScratch,
    /// A text buffer (for example a message copied by a fallible clone).
    Text,
    /// The deterministic sequence of a [`UuidGenerator`](crate::UuidGenerator)
    /// (issue #168): the generator cannot reserve the requested number of
    /// further sequence values without wrapping, so no identifier is minted.
    /// `additional` is the number of identifiers that were requested.
    IdSequence,
    /// A materialized vector of resting orders (issue #164): the queue views
    /// (`snapshot_orders`, `snapshot_by_insertion_seq`,
    /// `snapshot_by_seq_into`, `OrderQueue::snapshot_vec` / `to_vec`), their
    /// internal `(sequence, order)` sort buffer, the fill-or-kill dry-run
    /// bulk continuation and buffer of replenished tranches, `PriceLevelData` and a snapshot's orders vector
    /// (including one decoded from JSON). `additional` is the number of
    /// orders that could not be reserved.
    OrderSnapshot,
    /// The per-sweep parked-sequence set of `PriceLevel::match_order` (issue
    /// #164): the insertion sequences of makers the sweep has set aside.
    SweepScratch,
    /// The duplicate-id set `PriceLevel::from_snapshot` builds while
    /// validating a snapshot before restoring it (issue #164).
    RestoreScratch,
    /// A serialization output buffer (issue #164): the snapshot-package JSON
    /// text, its hex checksum, or a fallibly copied checksum / message string
    /// of a snapshot package. `additional` is the number of bytes that could
    /// not be reserved.
    SerializationBuffer,
}

impl CapacityResource {
    /// Static, allocation-free name of the resource.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trades => "trades",
            Self::FilledOrderIds => "filled order ids",
            Self::ValidationScratch => "validation scratch",
            Self::Text => "text",
            Self::IdSequence => "id sequence",
            Self::OrderSnapshot => "order snapshot",
            Self::SweepScratch => "sweep scratch",
            Self::RestoreScratch => "restore scratch",
            Self::SerializationBuffer => "serialization buffer",
        }
    }
}

impl Display for CapacityResource {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(self.as_str())
    }
}

/// The internal counter a [`PriceLevelError::CounterExhausted`] failure could
/// not advance (issue #165).
///
/// Every counter listed here is monotonic and checked: it refuses to move
/// past its limit instead of wrapping to zero. The tag is fixed-size and
/// `Copy`, so reporting an exhaustion never allocates, including from inside
/// a match sweep. New counters add variants, so the enum is
/// `#[non_exhaustive]`: match it with a wildcard arm.
///
/// Only [`Self::QueueSequence`], [`Self::TopologyEpoch`] and
/// [`Self::MutationEpoch`] can refuse a match; the statistics counters
/// ([`Self::OrdersAdded`], [`Self::OrdersRemoved`],
/// [`Self::StatisticsSequence`]) never do (a dropped contribution only marks
/// the statistics degraded). A caller that must know in advance that a match
/// will not be refused, such as a multi-level fill-or-kill pre-flight, can
/// compare [`PriceLevel::match_requirements`](crate::PriceLevel::match_requirements)
/// with [`PriceLevel::counter_headroom`](crate::PriceLevel::counter_headroom)
/// under the coordination documented there (issue #218).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExhaustedCounter {
    /// `PriceLevelStatistics::orders_added` is at `usize::MAX`.
    OrdersAdded,
    /// `PriceLevelStatistics::orders_removed` is at `usize::MAX`.
    OrdersRemoved,
    /// The order queue's FIFO insertion sequence has no fresh value left.
    QueueSequence,
    /// The price level's side-topology epoch has no headroom left.
    TopologyEpoch,
    /// The price level's mutation epoch has no headroom left.
    MutationEpoch,
    /// The statistics seqlock sequence cannot open another write section.
    StatisticsSequence,
}

impl ExhaustedCounter {
    /// Static, allocation-free name of the counter.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OrdersAdded => "orders added",
            Self::OrdersRemoved => "orders removed",
            Self::QueueSequence => "queue sequence",
            Self::TopologyEpoch => "topology epoch",
            Self::MutationEpoch => "mutation epoch",
            Self::StatisticsSequence => "statistics sequence",
        }
    }
}

impl Display for ExhaustedCounter {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(self.as_str())
    }
}

/// Represents errors that can occur when processing price levels in trading operations.
///
/// This enum encapsulates various error conditions that might arise during order book
/// management, price validation, and other trading-related operations.
///
/// # Examples
///
/// ```
/// use pricelevel::PriceLevelError;
///
/// // Creating a parse error
/// let error = PriceLevelError::ParseError {
///     message: "Failed to parse price: invalid decimal format".to_string()
/// };
///
/// // Creating a missing field error
/// let missing_field_error = PriceLevelError::MissingField("price".to_string());
/// ```
///
/// `Clone`, `PartialEq`, `Eq`, `Serialize` and `Deserialize` are derived so an
/// error can travel inside a [`MatchResult`](crate::MatchResult) (see
/// [`MatchResult::error`](crate::MatchResult::error)) and round-trip with it.
/// New variants are appended at the end so the positional (bincode) variant
/// indices of existing variants never move.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PriceLevelError {
    /// Error that occurs when parsing fails with a specific message.
    ///
    /// This variant is used when string conversion or data parsing operations fail.
    ParseError {
        /// Descriptive message explaining the parsing failure
        message: String,
    },

    /// Error indicating that the input is in an invalid format.
    ///
    /// This is a general error for when the input data doesn't conform to expected patterns
    /// but doesn't fit into more specific error categories.
    InvalidFormat,

    /// Error indicating an unrecognized order type was provided.
    ///
    /// Used when the system encounters an order type string that isn't in the supported set.
    /// The string parameter contains the unrecognized order type.
    UnknownOrderType(String),

    /// Error indicating a required field is missing.
    ///
    /// Used when a mandatory field is absent in the input data.
    /// The string parameter specifies which field is missing.
    MissingField(String),

    /// Error indicating an order id already rests at the price level.
    ///
    /// Admission (or a duplicate-bearing restore) is rejected atomically rather
    /// than overwriting the live order, which would leave the level's id-keyed
    /// map and its ordered index disagreeing (two sequences for one id) and its
    /// counters double-counted. The string parameter is the offending id.
    DuplicateOrderId(String),

    /// Error indicating a field has an invalid value.
    ///
    /// This error occurs when a field's value is present but doesn't meet validation criteria.
    InvalidFieldValue {
        /// The name of the field with the invalid value
        field: String,
        /// The invalid value as a string representation
        value: String,
    },

    /// Error indicating an operation cannot be performed for the specified reason.
    ///
    /// Used when an action is prevented due to business rules or system constraints.
    InvalidOperation {
        /// Explanation of why the operation is invalid
        message: String,
    },

    /// Error raised when serialization of internal data structures fails.
    SerializationError {
        /// Descriptive message with the serialization failure details
        message: String,
    },

    /// Error raised when deserialization of external data into internal structures fails.
    DeserializationError {
        /// Descriptive message with the deserialization failure details
        message: String,
    },

    /// Error raised when a checksum validation fails while restoring a snapshot.
    ChecksumMismatch {
        /// The checksum that was expected according to the serialized payload
        expected: String,
        /// The checksum that was computed from the provided payload
        actual: String,
    },

    /// Error raised when a caller-supplied
    /// [`EntropySource`](crate::EntropySource) cannot produce random bytes
    /// (for example an OS entropy call failing, or an RNG failing to seed or
    /// reseed).
    ///
    /// Random [`Id`](crate::Id) constructors propagate it unchanged; no
    /// identifier is produced and no fallback value is substituted.
    EntropyUnavailable {
        /// Descriptive message with the entropy failure details
        message: String,
    },

    /// Error raised when a collection could not grow: the requested capacity
    /// exceeds what the allocator / `isize::MAX` byte limit allows, or the
    /// allocator refused the request.
    ///
    /// The payload is fixed-size (no `String`), so reporting the failure never
    /// allocates after an allocation has just failed. Raised by the fallible
    /// execution-result constructors and growth paths
    /// ([`TradeList::try_with_capacity`](crate::TradeList::try_with_capacity),
    /// [`MatchResult::try_with_capacity`](crate::MatchResult::try_with_capacity),
    /// [`MatchResult::add_trade`](crate::MatchResult::add_trade), ...) and
    /// reported by [`MatchResult::error`](crate::MatchResult::error) when a
    /// match stops early.
    CapacityExceeded {
        /// The storage that could not grow.
        resource: CapacityResource,
        /// The number of additional elements that were requested.
        additional: usize,
    },

    /// Error raised when a monotonic internal counter has reached its limit
    /// and advancing it would wrap (issue #165).
    ///
    /// The counter is left at its current value; the operation that needed a
    /// fresh value is refused (or, for an advisory statistics counter whose
    /// operation already committed, its contribution is dropped and the
    /// statistics are marked degraded). The payload is fixed-size, so the
    /// error is built without allocating.
    CounterExhausted {
        /// The counter that could not advance.
        counter: ExhaustedCounter,
    },
}

impl PriceLevelError {
    /// Builds a [`PriceLevelError::CapacityExceeded`]. Cold: only reached when
    /// an allocation / capacity request fails.
    #[cold]
    #[inline(never)]
    #[must_use]
    pub(crate) fn capacity_exceeded(resource: CapacityResource, additional: usize) -> Self {
        Self::CapacityExceeded {
            resource,
            additional,
        }
    }

    /// Builds a [`PriceLevelError::CounterExhausted`]. Cold: only reached when
    /// a monotonic counter has no value left.
    #[cold]
    #[inline(never)]
    #[must_use]
    pub(crate) fn counter_exhausted(counter: ExhaustedCounter) -> Self {
        Self::CounterExhausted { counter }
    }

    /// Clones the error without an infallible allocation: every `String`
    /// payload is copied through `try_reserve_exact`, and a failure is reported
    /// as the allocation-free [`PriceLevelError::CapacityExceeded`].
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::Text`]) if a message buffer cannot be allocated.
    pub(crate) fn try_clone(&self) -> std::result::Result<Self, PriceLevelError> {
        fn copy(text: &str) -> std::result::Result<String, PriceLevelError> {
            let mut out = String::new();
            out.try_reserve_exact(text.len()).map_err(|_| {
                PriceLevelError::capacity_exceeded(CapacityResource::Text, text.len())
            })?;
            out.push_str(text);
            Ok(out)
        }
        Ok(match self {
            Self::ParseError { message } => Self::ParseError {
                message: copy(message)?,
            },
            Self::InvalidFormat => Self::InvalidFormat,
            Self::UnknownOrderType(value) => Self::UnknownOrderType(copy(value)?),
            Self::MissingField(value) => Self::MissingField(copy(value)?),
            Self::DuplicateOrderId(value) => Self::DuplicateOrderId(copy(value)?),
            Self::InvalidFieldValue { field, value } => Self::InvalidFieldValue {
                field: copy(field)?,
                value: copy(value)?,
            },
            Self::InvalidOperation { message } => Self::InvalidOperation {
                message: copy(message)?,
            },
            Self::SerializationError { message } => Self::SerializationError {
                message: copy(message)?,
            },
            Self::DeserializationError { message } => Self::DeserializationError {
                message: copy(message)?,
            },
            Self::ChecksumMismatch { expected, actual } => Self::ChecksumMismatch {
                expected: copy(expected)?,
                actual: copy(actual)?,
            },
            Self::EntropyUnavailable { message } => Self::EntropyUnavailable {
                message: copy(message)?,
            },
            Self::CapacityExceeded {
                resource,
                additional,
            } => Self::CapacityExceeded {
                resource: *resource,
                additional: *additional,
            },
            Self::CounterExhausted { counter } => Self::CounterExhausted { counter: *counter },
        })
    }
}
impl Display for PriceLevelError {
    // Error formatting is off the hot match path: keep it out of line and hint
    // the optimizer that it is rarely reached.
    #[cold]
    #[inline(never)]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            PriceLevelError::ParseError { message } => write!(f, "{message}"),
            PriceLevelError::InvalidFormat => write!(f, "Invalid format"),
            PriceLevelError::UnknownOrderType(order_type) => {
                write!(f, "Unknown order type: {order_type}")
            }
            PriceLevelError::MissingField(field) => write!(f, "Missing field: {field}"),
            PriceLevelError::DuplicateOrderId(id) => write!(f, "Duplicate order id: {id}"),
            PriceLevelError::InvalidFieldValue { field, value } => {
                write!(f, "Invalid value for field {field}: {value}")
            }
            PriceLevelError::InvalidOperation { message } => {
                write!(f, "Invalid operation: {message}")
            }
            PriceLevelError::SerializationError { message } => {
                write!(f, "Serialization error: {message}")
            }
            PriceLevelError::DeserializationError { message } => {
                write!(f, "Deserialization error: {message}")
            }
            PriceLevelError::ChecksumMismatch { expected, actual } => {
                write!(f, "Checksum mismatch: expected {expected}, got {actual}")
            }
            PriceLevelError::EntropyUnavailable { message } => {
                write!(f, "Entropy unavailable: {message}")
            }
            PriceLevelError::CapacityExceeded {
                resource,
                additional,
            } => write!(
                f,
                "Capacity exceeded: could not reserve {additional} more {resource} entries"
            ),
            PriceLevelError::CounterExhausted { counter } => {
                write!(
                    f,
                    "Counter exhausted: {counter} cannot advance without wrapping"
                )
            }
        }
    }
}

impl Debug for PriceLevelError {
    // Error formatting is off the hot match path: keep it out of line and hint
    // the optimizer that it is rarely reached.
    #[cold]
    #[inline(never)]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            PriceLevelError::ParseError { message } => write!(f, "{message}"),
            PriceLevelError::InvalidFormat => write!(f, "Invalid format"),
            PriceLevelError::UnknownOrderType(order_type) => {
                write!(f, "Unknown order type: {order_type}")
            }
            PriceLevelError::MissingField(field) => write!(f, "Missing field: {field}"),
            PriceLevelError::DuplicateOrderId(id) => write!(f, "Duplicate order id: {id}"),
            PriceLevelError::InvalidFieldValue { field, value } => {
                write!(f, "Invalid value for field {field}: {value}")
            }
            PriceLevelError::InvalidOperation { message } => {
                write!(f, "Invalid operation: {message}")
            }
            PriceLevelError::SerializationError { message } => {
                write!(f, "Serialization error: {message}")
            }
            PriceLevelError::DeserializationError { message } => {
                write!(f, "Deserialization error: {message}")
            }
            PriceLevelError::ChecksumMismatch { expected, actual } => {
                write!(f, "Checksum mismatch: expected {expected}, got {actual}")
            }
            PriceLevelError::EntropyUnavailable { message } => {
                write!(f, "Entropy unavailable: {message}")
            }
            PriceLevelError::CapacityExceeded {
                resource,
                additional,
            } => write!(
                f,
                "Capacity exceeded: could not reserve {additional} more {resource} entries"
            ),
            PriceLevelError::CounterExhausted { counter } => {
                write!(
                    f,
                    "Counter exhausted: {counter} cannot advance without wrapping"
                )
            }
        }
    }
}

impl std::error::Error for PriceLevelError {}
