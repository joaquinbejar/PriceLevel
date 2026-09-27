use crate::errors::PriceLevelError;
use crate::price_level::level::PriceLevel;
use crate::utils::text::{Fields, split_exactly_once};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Literal text of the full representation, excluding the four numeric values.
const FULL_TEXT_LITERAL_LEN: usize =
    "OrderBookEntry:price=;visible_quantity=;total_quantity=;index=".len();

/// Maximum decimal digits of a `u128` (`price`).
const MAX_U128_DECIMAL_DIGITS: usize = 39;

/// Maximum decimal digits of a `u64` (`visible_quantity`, `total_quantity`).
const MAX_U64_DECIMAL_DIGITS: usize = 20;

/// Maximum decimal digits of a `usize` (`index`) on every supported target
/// (at most 64-bit).
const MAX_USIZE_DECIMAL_DIGITS: usize = 20;

/// Represents a price level entry in the order book
///
/// Text forms:
///
/// - [`fmt::Display`] writes only the infallibly readable fields
///   (`price`, `visible_quantity`, `index`) and never produces a
///   crate-originated [`fmt::Error`].
/// - [`OrderBookEntry::to_full_string`] additionally includes
///   `total_quantity`, which can fail with a typed error when the level's
///   `visible + hidden` quantity overflows `u64`.
///
/// Both forms are accepted by [`FromStr`], which reads `price` and `index`.
///
/// The type lives in a private module and is not part of the public API.
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
    #[must_use]
    pub fn new(level: Arc<PriceLevel>, index: usize) -> Self {
        Self { level, index }
    }

    /// Get the price of this entry
    #[must_use]
    pub fn price(&self) -> u128 {
        self.level.price()
    }

    /// Get the visible quantity at this entry
    #[must_use]
    pub fn visible_quantity(&self) -> u64 {
        self.level.visible_quantity()
    }

    /// Get the total quantity at this entry
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the underlying level's
    /// `visible + hidden` quantity overflows `u64`.
    pub fn total_quantity(&self) -> Result<u64, PriceLevelError> {
        self.level.total_quantity()
    }

    /// Full text representation including `total_quantity`:
    /// `OrderBookEntry:price=P;visible_quantity=V;total_quantity=T;index=I`.
    ///
    /// Unlike [`fmt::Display`], this form needs the level's checked
    /// `visible + hidden` sum, so it is fallible. It never substitutes a
    /// placeholder for an unrepresentable total. The level is only read.
    ///
    /// # Errors
    ///
    /// - [`PriceLevelError::InvalidOperation`] if the level's
    ///   `visible + hidden` quantity overflows `u64`.
    /// - [`PriceLevelError::SerializationError`] if the output buffer cannot
    ///   be allocated or the text cannot be written.
    #[allow(dead_code)]
    pub fn to_full_string(&self) -> Result<String, PriceLevelError> {
        use std::fmt::Write as _;

        let total_quantity = self.total_quantity()?;
        let price = self.price();
        let visible_quantity = self.visible_quantity();

        let capacity = full_text_capacity_bound()?;
        let mut out = String::new();
        out.try_reserve_exact(capacity)
            .map_err(|err| full_text_error(format_args!("buffer reservation failed: {err}")))?;
        write!(
            out,
            "OrderBookEntry:price={price};visible_quantity={visible_quantity};\
             total_quantity={total_quantity};index={}",
            self.index
        )
        .map_err(|err| full_text_error(format_args!("write failed: {err}")))?;
        Ok(out)
    }

    /// Get the order count at this entry
    #[allow(dead_code)]
    #[must_use]
    pub fn order_count(&self) -> usize {
        self.level.order_count()
    }
}

/// Upper bound on the byte length of [`OrderBookEntry::to_full_string`], so
/// the buffer is reserved once and never grows while writing.
fn full_text_capacity_bound() -> Result<usize, PriceLevelError> {
    FULL_TEXT_LITERAL_LEN
        .checked_add(MAX_U128_DECIMAL_DIGITS)
        .and_then(|n| n.checked_add(MAX_U64_DECIMAL_DIGITS))
        .and_then(|n| n.checked_add(MAX_U64_DECIMAL_DIGITS))
        .and_then(|n| n.checked_add(MAX_USIZE_DECIMAL_DIGITS))
        .ok_or_else(|| full_text_error(format_args!("capacity bound overflow")))
}

/// Typed failure for the fallible full text conversion.
#[cold]
fn full_text_error(detail: fmt::Arguments<'_>) -> PriceLevelError {
    PriceLevelError::SerializationError {
        message: format!("OrderBookEntry full text conversion: {detail}"),
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

        // Four fields: price, visible_quantity, total_quantity, index.
        let mut state = serializer.serialize_struct("OrderBookEntry", 4)?;
        state.serialize_field("price", &self.price())?;
        state.serialize_field("visible_quantity", &self.visible_quantity())?;
        let total_quantity = self.total_quantity().map_err(serde::ser::Error::custom)?;
        state.serialize_field("total_quantity", &total_quantity)?;
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
            price: u128,
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
//
// Writes only fields that are read infallibly. `total_quantity` is omitted on
// purpose: it is a checked sum that can overflow, and a crate-originated
// `fmt::Error` would make `to_string()` / `format!` panic. Use
// `OrderBookEntry::to_full_string` for the representation that includes it.
// Only genuine sink errors from the formatter are propagated.
impl fmt::Display for OrderBookEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OrderBookEntry:price={};visible_quantity={};index={}",
            self.price(),
            self.visible_quantity(),
            self.index
        )
    }
}

// Implement FromStr
impl FromStr for OrderBookEntry {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Exactly one `:` separates the `OrderBookEntry` tag from the field list.
        let fields_str = match split_exactly_once(s, b':') {
            Some(("OrderBookEntry", fields_str)) => fields_str,
            _ => return Err(PriceLevelError::InvalidFormat),
        };

        // `key=value` pairs: a pair without exactly one `=` is ignored and a
        // repeated key keeps its last value (see `utils::text::Fields`).
        const FIELD_NAMES: [&str; 2] = ["price", "index"];
        let fields = Fields::parse(fields_str, &FIELD_NAMES);
        let get_field = |name: &str| fields.require(name);

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

        let price_str = get_field("price")?;
        let price = parse_u128("price", price_str)?;

        let index_str = get_field("index")?;
        let index = parse_usize("index", index_str)?;

        // Create a new price level with the given price
        let level = Arc::new(PriceLevel::new(price));

        Ok(OrderBookEntry { level, index })
    }
}
