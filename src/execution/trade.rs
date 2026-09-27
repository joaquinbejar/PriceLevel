use crate::errors::PriceLevelError;
use crate::orders::{Id, Side};
use crate::utils::text::{Fields, split_exactly_once};
use crate::utils::{Price, Quantity, TimestampMs, UnixClock};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Represents a completed trade between two orders.
///
/// All fields are private to enforce immutability after construction.
/// Use the provided accessor methods to read trade data.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Trade {
    /// Unique trade ID
    trade_id: Id,

    /// ID of the aggressive order that caused the match
    taker_order_id: Id,

    /// ID of the passive order that was in the book
    maker_order_id: Id,

    /// Price at which the trade occurred
    price: Price,

    /// Quantity traded
    quantity: Quantity,

    /// Side of the taker order
    taker_side: Side,

    /// Timestamp when the trade occurred in milliseconds since epoch
    timestamp: TimestampMs,
}

impl Trade {
    /// Creates a trade stamped with the current time read from a
    /// caller-supplied [`UnixClock`].
    ///
    /// The crate does not read the wall clock itself (issue #171): the clock
    /// is read exactly once, before the trade is built, and its failure is
    /// returned unchanged. No fallback timestamp (such as `0`) is ever
    /// substituted. When the time is already known (replay, deserialization,
    /// the matching engine's taker timestamp) use the infallible
    /// [`Self::with_timestamp`] instead.
    ///
    /// # Errors
    ///
    /// Returns the error reported by `clock` (for example a
    /// [`PriceLevelError::InvalidOperation`] for a pre-epoch reading or a
    /// [`PriceLevelError::InvalidFieldValue`] for a millisecond count that does
    /// not fit in `u64`, as produced by
    /// [`TimestampMs::try_from_system_time`]).
    pub fn try_new<C>(
        trade_id: Id,
        taker_order_id: Id,
        maker_order_id: Id,
        price: Price,
        quantity: Quantity,
        taker_side: Side,
        clock: &C,
    ) -> Result<Self, PriceLevelError>
    where
        C: UnixClock + ?Sized,
    {
        let timestamp = clock.try_now_ms()?;
        Ok(Self::with_timestamp(
            trade_id,
            taker_order_id,
            maker_order_id,
            price,
            quantity,
            taker_side,
            timestamp,
        ))
    }

    /// Returns the unique trade identifier.
    #[must_use]
    pub fn trade_id(&self) -> Id {
        self.trade_id
    }

    /// Returns the taker (aggressive) order identifier.
    #[must_use]
    pub fn taker_order_id(&self) -> Id {
        self.taker_order_id
    }

    /// Returns the maker (passive) order identifier.
    #[must_use]
    pub fn maker_order_id(&self) -> Id {
        self.maker_order_id
    }

    /// Returns the trade price.
    #[must_use]
    pub fn price(&self) -> Price {
        self.price
    }

    /// Returns the traded quantity.
    #[must_use]
    pub fn quantity(&self) -> Quantity {
        self.quantity
    }

    /// Returns the side of the taker order.
    #[must_use]
    pub fn taker_side(&self) -> Side {
        self.taker_side
    }

    /// Returns the trade timestamp in milliseconds since epoch.
    #[must_use]
    pub fn timestamp(&self) -> TimestampMs {
        self.timestamp
    }

    /// Creates a trade with an explicit timestamp.
    ///
    /// Intended for deserialization and testing where the timestamp is already known.
    #[must_use]
    pub fn with_timestamp(
        trade_id: Id,
        taker_order_id: Id,
        maker_order_id: Id,
        price: Price,
        quantity: Quantity,
        taker_side: Side,
        timestamp: TimestampMs,
    ) -> Self {
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

    /// Returns the side of the maker order.
    #[must_use]
    pub fn maker_side(&self) -> Side {
        match self.taker_side {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    /// Returns the total value of this trade (`price * quantity`), in
    /// price-ticks × quantity units.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `price * quantity`
    /// overflows `u128`. This mirrors the checked-arithmetic discipline of
    /// [`MatchResult::executed_value`](crate::execution::MatchResult::executed_value),
    /// which computes the same product.
    pub fn total_value(&self) -> Result<u128, PriceLevelError> {
        self.price
            .as_u128()
            .checked_mul(u128::from(self.quantity.as_u64()))
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: format!(
                    "trade total value overflow: price {} * quantity {}",
                    self.price.as_u128(),
                    self.quantity.as_u64()
                ),
            })
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
        // Exactly one `:` separates the `Trade` tag from the field list.
        let fields_str = match split_exactly_once(s, b':') {
            Some(("Trade", fields_str)) => fields_str,
            _ => return Err(PriceLevelError::InvalidFormat),
        };

        // `key=value` pairs: a pair without exactly one `=` is ignored and a
        // repeated key keeps its last value (see `utils::text::Fields`).
        const FIELD_NAMES: [&str; 7] = [
            "trade_id",
            "taker_order_id",
            "maker_order_id",
            "price",
            "quantity",
            "taker_side",
            "timestamp",
        ];
        let fields = Fields::parse(fields_str, &FIELD_NAMES);
        let get_field = |name: &str| fields.require(name);

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
