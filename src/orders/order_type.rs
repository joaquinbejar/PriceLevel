//! Limit order type definitions

use crate::OrderQueue;
use crate::errors::PriceLevelError;
use crate::orders::{Hash32, Id, PegReferenceType, Side, TimeInForce};
use crate::utils::{Price, Quantity, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;
use std::sync::Arc;

fn user_id_from_str(value: &str) -> Result<Hash32, PriceLevelError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    Hash32::from_hex(value).map_err(|_| PriceLevelError::InvalidFieldValue {
        field: "user_id".to_string(),
        value: value.to_string(),
    })
}

/// Default amount to replenish the reserve with, in quantity units.
///
/// A replenish amount is structurally non-zero: a zero replenish would draw an
/// empty visible tranche from hidden, silently leaving nothing visible. The
/// type therefore is [`NonZeroU64`] rather than a raw `u64`. The constant is
/// built in a `const`-evaluable form (a `match` on [`NonZeroU64::new`]) so it
/// carries no runtime `unwrap`/`expect`.
pub const DEFAULT_RESERVE_REPLENISH_AMOUNT: NonZeroU64 = match NonZeroU64::new(80) {
    Some(value) => value,
    // Unreachable: 80 is a non-zero literal, so `NonZeroU64::new` is always
    // `Some` here. The branch is resolved at compile time, so this carries no
    // runtime `unwrap`/`expect`.
    None => unreachable!(),
};

/// Represents different types of limit orders
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderType<T> {
    /// Standard limit order
    Standard {
        /// The order ID
        id: Id,
        /// The price of the order
        price: Price,
        /// The quantity of the order
        quantity: Quantity,
        /// The side of the order (buy or sell)
        side: Side,
        /// Owner identifier for fast lookup (32 bytes)
        user_id: Hash32,
        /// When the order was created
        timestamp: TimestampMs,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Additional custom fields
        extra_fields: T,
    },

    /// Iceberg order with visible and hidden quantities
    IcebergOrder {
        /// The order ID
        id: Id,
        /// The price of the order
        price: Price,
        /// The visible quantity of the order
        visible_quantity: Quantity,
        /// The hidden quantity of the order
        hidden_quantity: Quantity,
        /// The side of the order (buy or sell)
        side: Side,
        /// Owner identifier for fast lookup (32 bytes)
        user_id: Hash32,
        /// When the order was created
        timestamp: TimestampMs,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Additional custom fields
        extra_fields: T,
    },

    /// Post-only order that won't match immediately
    PostOnly {
        /// The order ID
        id: Id,
        /// The price of the order
        price: Price,
        /// The quantity of the order
        quantity: Quantity,
        /// The side of the order (buy or sell)
        side: Side,
        /// Owner identifier for fast lookup (32 bytes)
        user_id: Hash32,
        /// When the order was created
        timestamp: TimestampMs,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Additional custom fields
        extra_fields: T,
    },

    /// Trailing stop order that adjusts with market movement
    TrailingStop {
        /// The order ID
        id: Id,
        /// The price of the order
        price: Price,
        /// The quantity of the order
        quantity: Quantity,
        /// The side of the order (buy or sell)
        side: Side,
        /// Owner identifier for fast lookup (32 bytes)
        user_id: Hash32,
        /// When the order was created
        timestamp: TimestampMs,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Amount to trail the market price
        trail_amount: Quantity,
        /// Last reference price
        last_reference_price: Price,
        /// Additional custom fields
        extra_fields: T,
    },

    /// Pegged order that adjusts based on reference price
    PeggedOrder {
        /// The order ID
        id: Id,
        /// The price of the order
        price: Price,
        /// The quantity of the order
        quantity: Quantity,
        /// The side of the order (buy or sell)
        side: Side,
        /// Owner identifier for fast lookup (32 bytes)
        user_id: Hash32,
        /// When the order was created
        timestamp: TimestampMs,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Offset from the reference price
        reference_price_offset: i64,
        /// Type of reference price to track
        reference_price_type: PegReferenceType,
        /// Additional custom fields
        extra_fields: T,
    },

    /// Market-to-limit order that converts to limit after initial execution
    MarketToLimit {
        /// The order ID
        id: Id,
        /// The price of the order
        price: Price,
        /// The quantity of the order
        quantity: Quantity,
        /// The side of the order (buy or sell)
        side: Side,
        /// Owner identifier for fast lookup (32 bytes)
        user_id: Hash32,
        /// When the order was created
        timestamp: TimestampMs,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Additional custom fields
        extra_fields: T,
    },

    /// Reserve order with custom replenishment
    /// if `replenish_amount` is None, it uses DEFAULT_RESERVE_REPLENISH_AMOUNT
    /// if `auto_replenish` is false, and visible quantity is below threshold, it will not replenish
    /// if `auto_replenish` is false and visible quantity is zero it will be removed from the book
    /// if `auto_replenish` is true, and replenish_threshold is 0, it will use 1
    ReserveOrder {
        /// The order ID
        id: Id,
        /// The price of the order
        price: Price,
        /// The visible quantity of the order
        visible_quantity: Quantity,
        /// The hidden quantity of the order
        hidden_quantity: Quantity,
        /// The side of the order (buy or sell)
        side: Side,
        /// Owner identifier for fast lookup (32 bytes)
        user_id: Hash32,
        /// When the order was created
        timestamp: TimestampMs,
        /// Time-in-force policy
        time_in_force: TimeInForce,
        /// Threshold at which to replenish
        replenish_threshold: Quantity,
        /// Optional amount to replenish by, in quantity units. If `None`, uses
        /// [`DEFAULT_RESERVE_REPLENISH_AMOUNT`]. A replenish amount is
        /// structurally non-zero ([`NonZeroU64`]): a zero replenish would draw
        /// an empty visible tranche from hidden.
        replenish_amount: Option<NonZeroU64>,
        /// Whether to replenish automatically when below threshold. If false, only replenish on next match
        auto_replenish: bool,
        /// Additional custom fields
        extra_fields: T,
    },
}

impl<T: Clone> OrderType<T> {
    /// Get the order ID
    #[must_use]
    #[inline]
    pub fn id(&self) -> Id {
        match self {
            Self::Standard { id, .. } => *id,
            Self::IcebergOrder { id, .. } => *id,
            Self::PostOnly { id, .. } => *id,
            Self::TrailingStop { id, .. } => *id,
            Self::PeggedOrder { id, .. } => *id,
            Self::MarketToLimit { id, .. } => *id,
            Self::ReserveOrder { id, .. } => *id,
        }
    }

    /// Get the user ID associated with this order
    #[must_use]
    pub fn user_id(&self) -> Hash32 {
        match self {
            Self::Standard { user_id, .. }
            | Self::IcebergOrder { user_id, .. }
            | Self::PostOnly { user_id, .. }
            | Self::TrailingStop { user_id, .. }
            | Self::PeggedOrder { user_id, .. }
            | Self::MarketToLimit { user_id, .. }
            | Self::ReserveOrder { user_id, .. } => *user_id,
        }
    }

    /// Get the price
    #[must_use]
    #[inline]
    pub fn price(&self) -> Price {
        match self {
            Self::Standard { price, .. } => *price,
            Self::IcebergOrder { price, .. } => *price,
            Self::PostOnly { price, .. } => *price,
            Self::TrailingStop { price, .. } => *price,
            Self::PeggedOrder { price, .. } => *price,
            Self::MarketToLimit { price, .. } => *price,
            Self::ReserveOrder { price, .. } => *price,
        }
    }

    /// Get the visible quantity, in quantity units.
    #[must_use]
    #[inline]
    pub fn visible_quantity(&self) -> Quantity {
        match self {
            Self::Standard { quantity, .. } => *quantity,
            Self::IcebergOrder {
                visible_quantity, ..
            } => *visible_quantity,
            Self::PostOnly { quantity, .. } => *quantity,
            Self::TrailingStop { quantity, .. } => *quantity,
            Self::PeggedOrder { quantity, .. } => *quantity,
            Self::MarketToLimit { quantity, .. } => *quantity,
            Self::ReserveOrder {
                visible_quantity, ..
            } => *visible_quantity,
        }
    }

    /// Get the hidden quantity, in quantity units.
    ///
    /// Order types without a hidden tranche return [`Quantity::ZERO`].
    #[must_use]
    #[inline]
    pub fn hidden_quantity(&self) -> Quantity {
        match self {
            Self::IcebergOrder {
                hidden_quantity, ..
            } => *hidden_quantity,
            Self::ReserveOrder {
                hidden_quantity, ..
            } => *hidden_quantity,
            _ => Quantity::ZERO,
        }
    }

    /// Returns `true` if this resting order can yield a fill for a crossing
    /// taker, i.e. a positive taker would take some quantity from it.
    ///
    /// This is the single source of truth for "matchable depth" shared by the
    /// post-only pre-check (`has_matchable_depth`) and the fill-or-kill dry run
    /// (`matchable_quantity`), so the two can never disagree about the same level
    /// (those are private price-level helpers, named here as plain spans rather
    /// than doc links):
    ///
    /// - Any order with positive **visible** quantity is matchable.
    /// - A zero-visible **iceberg** with hidden quantity is matchable: the sweep
    ///   replenishes its entire hidden into visible and then fills it.
    /// - A zero-visible **reserve** with hidden quantity is matchable **only if**
    ///   `auto_replenish` is set; without auto-replenish a drained reserve is
    ///   dropped by the sweep without ever filling, so it is *not* matchable
    ///   depth.
    /// - Every other zero-visible order (no hidden to draw on) is not matchable.
    #[must_use]
    #[inline]
    pub fn is_matchable(&self) -> bool {
        if self.visible_quantity().as_u64() > 0 {
            return true;
        }
        match self {
            Self::IcebergOrder {
                hidden_quantity, ..
            } => hidden_quantity.as_u64() > 0,
            Self::ReserveOrder {
                hidden_quantity,
                auto_replenish,
                ..
            } => *auto_replenish && hidden_quantity.as_u64() > 0,
            _ => false,
        }
    }

    /// Get the order side
    #[must_use]
    #[inline]
    pub fn side(&self) -> Side {
        match self {
            Self::Standard { side, .. } => *side,
            Self::IcebergOrder { side, .. } => *side,
            Self::PostOnly { side, .. } => *side,
            Self::TrailingStop { side, .. } => *side,
            Self::PeggedOrder { side, .. } => *side,
            Self::MarketToLimit { side, .. } => *side,
            Self::ReserveOrder { side, .. } => *side,
        }
    }

    /// Get the time in force
    #[must_use]
    pub fn time_in_force(&self) -> TimeInForce {
        match self {
            Self::Standard { time_in_force, .. } => *time_in_force,
            Self::IcebergOrder { time_in_force, .. } => *time_in_force,
            Self::PostOnly { time_in_force, .. } => *time_in_force,
            Self::TrailingStop { time_in_force, .. } => *time_in_force,
            Self::PeggedOrder { time_in_force, .. } => *time_in_force,
            Self::MarketToLimit { time_in_force, .. } => *time_in_force,
            Self::ReserveOrder { time_in_force, .. } => *time_in_force,
        }
    }

    /// Get the timestamp at which the order was created, in milliseconds.
    #[must_use]
    #[inline]
    pub fn timestamp(&self) -> TimestampMs {
        match self {
            Self::Standard { timestamp, .. } => *timestamp,
            Self::IcebergOrder { timestamp, .. } => *timestamp,
            Self::PostOnly { timestamp, .. } => *timestamp,
            Self::TrailingStop { timestamp, .. } => *timestamp,
            Self::PeggedOrder { timestamp, .. } => *timestamp,
            Self::MarketToLimit { timestamp, .. } => *timestamp,
            Self::ReserveOrder { timestamp, .. } => *timestamp,
        }
    }

    /// Check if the order is immediate-or-cancel
    #[must_use]
    pub fn is_immediate(&self) -> bool {
        self.time_in_force().is_immediate()
    }

    /// Check if the order is fill-or-kill
    #[must_use]
    pub fn is_fill_or_kill(&self) -> bool {
        matches!(self.time_in_force(), TimeInForce::Fok)
    }

    /// Check if this is a post-only order
    #[must_use]
    pub fn is_post_only(&self) -> bool {
        matches!(self, Self::PostOnly { .. })
    }

    /// Return a clone of this order with its resting (visible / main) quantity
    /// reset to `new_quantity`, in quantity units.
    ///
    /// Every variant is rewritten explicitly so a partial fill (via
    /// [`Self::match_against`]) or an `OrderUpdate::UpdateQuantity` leaves the
    /// residual maker at exactly `new_quantity` rather than its original size.
    /// For the single-quantity variants (`Standard`, `PostOnly`,
    /// `TrailingStop`, `PeggedOrder`, `MarketToLimit`) this rewrites the
    /// `quantity` field; for the two-tranche variants (`IcebergOrder`,
    /// `ReserveOrder`) it rewrites the *visible* quantity and preserves the
    /// hidden tranche together with every order-type-specific field (trail /
    /// peg parameters, replenishment policy).
    ///
    /// The match is exhaustive by design: a new variant must supply its own arm
    /// rather than silently retaining its original quantity, which would let a
    /// later taker execute the same depth twice and break quantity
    /// conservation.
    #[must_use]
    pub fn with_reduced_quantity(&self, new_quantity: u64) -> Self {
        let new_quantity = Quantity::new(new_quantity);
        match self {
            Self::Standard {
                id,
                price,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
                ..
            } => Self::Standard {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                user_id: *user_id,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                extra_fields: extra_fields.clone(),
            },
            Self::IcebergOrder {
                id,
                price,
                side,
                user_id,
                timestamp,
                time_in_force,
                hidden_quantity,
                extra_fields,
                ..
            } => {
                // Update visible quantity but keep hidden the same
                Self::IcebergOrder {
                    id: *id,
                    price: *price,
                    visible_quantity: new_quantity,
                    hidden_quantity: *hidden_quantity,
                    side: *side,
                    user_id: *user_id,
                    timestamp: *timestamp,
                    time_in_force: *time_in_force,
                    extra_fields: extra_fields.clone(),
                }
            }
            Self::PostOnly {
                id,
                price,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
                ..
            } => Self::PostOnly {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                user_id: *user_id,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                extra_fields: extra_fields.clone(),
            },
            Self::TrailingStop {
                id,
                price,
                side,
                user_id,
                timestamp,
                time_in_force,
                trail_amount,
                last_reference_price,
                extra_fields,
                ..
            } => Self::TrailingStop {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                user_id: *user_id,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                trail_amount: *trail_amount,
                last_reference_price: *last_reference_price,
                extra_fields: extra_fields.clone(),
            },
            Self::PeggedOrder {
                id,
                price,
                side,
                user_id,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                extra_fields,
                ..
            } => Self::PeggedOrder {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                user_id: *user_id,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                reference_price_offset: *reference_price_offset,
                reference_price_type: *reference_price_type,
                extra_fields: extra_fields.clone(),
            },
            Self::MarketToLimit {
                id,
                price,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
                ..
            } => Self::MarketToLimit {
                id: *id,
                price: *price,
                quantity: new_quantity,
                side: *side,
                user_id: *user_id,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                extra_fields: extra_fields.clone(),
            },
            // Reserve rewrites the visible tranche (mirroring `IcebergOrder`),
            // preserving the hidden quantity and the replenishment policy.
            Self::ReserveOrder {
                id,
                price,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields,
                ..
            } => Self::ReserveOrder {
                id: *id,
                price: *price,
                visible_quantity: new_quantity,
                hidden_quantity: *hidden_quantity,
                side: *side,
                user_id: *user_id,
                timestamp: *timestamp,
                time_in_force: *time_in_force,
                replenish_threshold: *replenish_threshold,
                replenish_amount: *replenish_amount,
                auto_replenish: *auto_replenish,
                extra_fields: extra_fields.clone(),
            },
        }
    }

    /// Update an iceberg or reserve order, refreshing the visible part from
    /// hidden.
    ///
    /// `refresh_amount` is the tranche size to draw from the hidden quantity,
    /// in quantity units. It is [`NonZeroU64`] because a zero refresh would
    /// draw an empty visible tranche, silently leaving nothing visible. The
    /// amount actually drawn is capped at the remaining hidden quantity.
    ///
    /// Returns the refreshed order and the quantity drawn from hidden. For a
    /// non-iceberg / non-reserve order the order is returned unchanged with a
    /// drawn quantity of `0`.
    #[must_use]
    pub fn refresh_iceberg(&self, refresh_amount: NonZeroU64) -> (Self, u64) {
        match self {
            Self::IcebergOrder {
                id,
                price,
                visible_quantity: _,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
            } => {
                let used_hidden = refresh_amount.get().min(hidden_quantity.as_u64());
                let new_hidden = hidden_quantity.as_u64() - used_hidden;

                (
                    Self::IcebergOrder {
                        id: *id,
                        price: *price,
                        visible_quantity: Quantity::new(used_hidden),
                        hidden_quantity: Quantity::new(new_hidden),
                        side: *side,
                        user_id: *user_id,
                        timestamp: *timestamp,
                        time_in_force: *time_in_force,
                        extra_fields: extra_fields.clone(),
                    },
                    used_hidden,
                )
            }
            Self::ReserveOrder {
                id,
                price,
                visible_quantity: _,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields,
            } => {
                let used_hidden = refresh_amount.get().min(hidden_quantity.as_u64());
                let new_hidden = hidden_quantity.as_u64() - used_hidden;

                (
                    Self::ReserveOrder {
                        id: *id,
                        price: *price,
                        visible_quantity: Quantity::new(used_hidden),
                        hidden_quantity: Quantity::new(new_hidden),
                        side: *side,
                        user_id: *user_id,
                        timestamp: *timestamp,
                        time_in_force: *time_in_force,
                        replenish_threshold: *replenish_threshold,
                        replenish_amount: *replenish_amount,
                        auto_replenish: *auto_replenish,
                        extra_fields: extra_fields.clone(),
                    },
                    used_hidden,
                )
            }
            // Single-tranche variants have no hidden reserve to draw from, so a
            // refresh is a no-op that draws `0`. Listed explicitly (rather than
            // via a `_` fallback) so a future variant with a hidden tranche is a
            // compile error here until it defines its own refresh behaviour.
            Self::Standard { .. }
            | Self::PostOnly { .. }
            | Self::TrailingStop { .. }
            | Self::PeggedOrder { .. }
            | Self::MarketToLimit { .. } => (self.clone(), 0),
        }
    }
}

impl<T: Clone> OrderType<T> {
    /// Matches this order against an incoming quantity
    ///
    /// Returns a tuple containing:
    /// - The quantity consumed from the incoming order
    /// - Optionally, an updated version of this order (if partially filled)
    /// - The quantity that was reduced from hidden portion (for iceberg/reserve orders)
    /// - The remaining quantity of the incoming order
    ///
    /// # Overflow
    ///
    /// The only quantity *addition* on any match path is a reserve order's
    /// partial-fill replenishment (`new_visible + replenish_qty`). If that sum
    /// would overflow `u64` — reachable only for a pathological reserve whose
    /// visible + hidden already exceeds `u64::MAX` — this returns the
    /// no-progress sentinel `(0, Some(self.clone()), 0, incoming_quantity)`
    /// instead of panicking or wrapping. The caller's sweep (and the
    /// fill-or-kill dry run) already treat that sentinel as "set this maker
    /// aside", so the step fails atomically: no trade, maker and taker
    /// unchanged. Every other path uses only subtraction / `min`, which cannot
    /// overflow.
    #[must_use]
    pub fn match_against(&self, incoming_quantity: u64) -> (u64, Option<Self>, u64, u64) {
        match self {
            Self::Standard {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
            } => {
                if quantity.as_u64() <= incoming_quantity {
                    // Full match
                    (
                        quantity.as_u64(),                     // consumed = full order quantity
                        None,                                  // no updated order (fully matched)
                        0,                                     // no hidden quantity reduced
                        incoming_quantity - quantity.as_u64(), // remaining = incoming - consumed
                    )
                } else {
                    // Partial match
                    (
                        incoming_quantity, // consumed = all incoming quantity
                        Some(Self::Standard {
                            id: *id,
                            price: *price,
                            quantity: Quantity::new(quantity.as_u64() - incoming_quantity),
                            side: *side,
                            user_id: *user_id,
                            timestamp: *timestamp,
                            time_in_force: *time_in_force,
                            extra_fields: extra_fields.clone(),
                        }),
                        0, // not hidden quantity reduced
                        0, // not remaining quantity
                    )
                }
            }

            // En OrderType::match_against para IcebergOrder
            Self::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
            } => {
                if visible_quantity.as_u64() <= incoming_quantity {
                    // Fully match the visible portion
                    let consumed = visible_quantity.as_u64();
                    let remaining = incoming_quantity - consumed;

                    if hidden_quantity.as_u64() > 0 {
                        // Refresh visible portion from hidden. The tranche size
                        // is the order's current visible quantity (each iceberg
                        // tranche mirrors the original visible size).
                        //
                        // Degenerate guard: a zero-visible iceberg (constructible
                        // via `add_order` with `visible_quantity: 0` or
                        // `update_order(UpdateQuantity { new_quantity: 0 })`) has
                        // a tranche size of 0, which `min(hidden, visible)` would
                        // make a no-op refresh — the order would re-queue
                        // unchanged and the sweep would re-pop it forever. Draw
                        // the entire remaining hidden into visible so the order
                        // becomes matchable, `hidden_reduced > 0`, and the sweep
                        // makes forward progress instead of looping.
                        let tranche = if visible_quantity.as_u64() == 0 {
                            hidden_quantity.as_u64()
                        } else {
                            visible_quantity.as_u64()
                        };
                        let refresh_qty = std::cmp::min(hidden_quantity.as_u64(), tranche);
                        let new_hidden = hidden_quantity.as_u64() - refresh_qty;

                        // Create updated order with refreshed quantities
                        (
                            consumed,
                            Some(Self::IcebergOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: Quantity::new(refresh_qty),
                                hidden_quantity: Quantity::new(new_hidden),
                                side: *side,
                                user_id: *user_id,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                extra_fields: extra_fields.clone(),
                            }),
                            refresh_qty,
                            remaining,
                        )
                    } else {
                        // No hidden quantity left
                        (consumed, None, 0, remaining)
                    }
                } else {
                    // Partial match of visible quantity
                    let executed = incoming_quantity;

                    (
                        executed,
                        Some(Self::IcebergOrder {
                            id: *id,
                            price: *price,
                            visible_quantity: Quantity::new(visible_quantity.as_u64() - executed),
                            hidden_quantity: *hidden_quantity,
                            side: *side,
                            user_id: *user_id,
                            timestamp: *timestamp,
                            time_in_force: *time_in_force,
                            extra_fields: extra_fields.clone(),
                        }),
                        0,
                        0,
                    )
                }
            }

            Self::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields,
            } => {
                // Ensure the threshold is never 0 if auto_replenish is true
                let safe_threshold = if *auto_replenish && replenish_threshold.as_u64() == 0 {
                    1
                } else {
                    replenish_threshold.as_u64()
                };

                let replenish_qty = replenish_amount
                    .unwrap_or(DEFAULT_RESERVE_REPLENISH_AMOUNT)
                    .get()
                    .min(hidden_quantity.as_u64());

                if visible_quantity.as_u64() <= incoming_quantity {
                    // Full match of the visible part
                    let consumed = visible_quantity.as_u64();
                    let remaining = incoming_quantity - consumed;

                    // Verify if we need and can replenish
                    if hidden_quantity.as_u64() > 0 && *auto_replenish {
                        // Restore from the hidden quantity
                        let new_hidden = hidden_quantity.as_u64() - replenish_qty;

                        (
                            consumed,
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: Quantity::new(replenish_qty),
                                hidden_quantity: Quantity::new(new_hidden),
                                side: *side,
                                user_id: *user_id,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                                replenish_amount: *replenish_amount,
                                auto_replenish: *auto_replenish,
                                extra_fields: extra_fields.clone(),
                            }),
                            replenish_qty,
                            remaining,
                        )
                    } else {
                        // If there is no auto-replenishment or no hidden quantity, delete the order
                        (consumed, None, 0, remaining)
                    }
                } else {
                    // Partial match of the visible part
                    let consumed = incoming_quantity;
                    let new_visible = visible_quantity.as_u64() - consumed;

                    // Check if we need to replenish (we fell below the threshold)
                    if new_visible < safe_threshold
                        && hidden_quantity.as_u64() > 0
                        && *auto_replenish
                    {
                        // Refreshed visible is `new_visible + replenish_qty`.
                        // This sum is provably `<= u64::MAX` for any order the
                        // level admits: `new_visible <= visible_quantity`,
                        // `replenish_qty` is capped by `.min(hidden_quantity)`
                        // above, and `PriceLevel::add_order` /
                        // `PriceLevelSnapshot::refresh_aggregates` reject any
                        // order whose own `visible + hidden` overflows `u64`,
                        // so `new_visible + replenish_qty <= visible + hidden
                        // <= u64::MAX`. The `checked_add` is therefore kept as
                        // defense-in-depth against an unadmitted / internally
                        // constructed order (e.g. a direct `match_against` unit
                        // test): on the unreachable overflow it makes NO progress
                        // — the maker is handed back unchanged with
                        // `consumed == 0` and `remaining` untouched, the
                        // no-progress sentinel both the real sweep and the
                        // fill-or-kill dry run detect and set aside — never a
                        // partial or a manufactured (wrapped) fill.
                        let Some(refreshed_visible) = new_visible.checked_add(replenish_qty) else {
                            return (0, Some(self.clone()), 0, incoming_quantity);
                        };
                        // Restore from the hidden quantity.
                        let new_hidden = hidden_quantity.as_u64() - replenish_qty;

                        (
                            consumed,
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: Quantity::new(refreshed_visible),
                                hidden_quantity: Quantity::new(new_hidden),
                                side: *side,
                                user_id: *user_id,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                                replenish_amount: *replenish_amount,
                                auto_replenish: *auto_replenish,
                                extra_fields: extra_fields.clone(),
                            }),
                            replenish_qty,
                            0,
                        )
                    } else {
                        // We don't need to replenish or it is not automatic
                        (
                            consumed,
                            Some(Self::ReserveOrder {
                                id: *id,
                                price: *price,
                                visible_quantity: Quantity::new(new_visible),
                                hidden_quantity: *hidden_quantity,
                                side: *side,
                                user_id: *user_id,
                                timestamp: *timestamp,
                                time_in_force: *time_in_force,
                                replenish_threshold: *replenish_threshold,
                                replenish_amount: *replenish_amount,
                                auto_replenish: *auto_replenish,
                                extra_fields: extra_fields.clone(),
                            }),
                            0,
                            0,
                        )
                    }
                }
            }

            // Single-quantity variants with no hidden tranche: match against
            // the whole (visible) quantity and, on a partial fill, rewrite the
            // residual to exactly the untaken remainder via
            // `with_reduced_quantity`. Listed explicitly rather than via a `_`
            // fallback so a future variant must decide its own matching path
            // instead of silently inheriting this single-quantity logic (which
            // would keep its full size after a partial fill and let a later
            // taker execute the same depth twice).
            Self::PostOnly { .. }
            | Self::TrailingStop { .. }
            | Self::PeggedOrder { .. }
            | Self::MarketToLimit { .. } => {
                let visible_qty = self.visible_quantity().as_u64();

                if visible_qty <= incoming_quantity {
                    // Full match
                    (
                        visible_qty,                     // consumed full visible quantity
                        None,                            // fully matched
                        0,                               // no hidden reduced
                        incoming_quantity - visible_qty, // remaining quantity
                    )
                } else {
                    // Partial match
                    (
                        incoming_quantity, // consumed all incoming
                        Some(self.with_reduced_quantity(visible_qty - incoming_quantity)),
                        0, // not hidden reduced
                        0, // not remaining quantity
                    )
                }
            }
        }
    }
}

impl<T> OrderType<T> {
    /// Get the extra fields
    #[must_use]
    pub fn extra_fields(&self) -> &T {
        match self {
            Self::Standard { extra_fields, .. } => extra_fields,
            Self::IcebergOrder { extra_fields, .. } => extra_fields,
            Self::PostOnly { extra_fields, .. } => extra_fields,
            Self::TrailingStop { extra_fields, .. } => extra_fields,
            Self::PeggedOrder { extra_fields, .. } => extra_fields,
            Self::MarketToLimit { extra_fields, .. } => extra_fields,
            Self::ReserveOrder { extra_fields, .. } => extra_fields,
        }
    }

    /// Get mutable reference to extra fields
    pub fn extra_fields_mut(&mut self) -> &mut T {
        match self {
            Self::Standard { extra_fields, .. } => extra_fields,
            Self::IcebergOrder { extra_fields, .. } => extra_fields,
            Self::PostOnly { extra_fields, .. } => extra_fields,
            Self::TrailingStop { extra_fields, .. } => extra_fields,
            Self::PeggedOrder { extra_fields, .. } => extra_fields,
            Self::MarketToLimit { extra_fields, .. } => extra_fields,
            Self::ReserveOrder { extra_fields, .. } => extra_fields,
        }
    }

    /// Transform the extra fields type using a function
    #[must_use]
    pub fn map_extra_fields<U, F>(self, f: F) -> OrderType<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            Self::Standard {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::Standard {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::PostOnly {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::PostOnly {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::TrailingStop {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                trail_amount,
                last_reference_price,
                extra_fields,
            } => OrderType::TrailingStop {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                trail_amount,
                last_reference_price,
                extra_fields: f(extra_fields),
            },
            Self::PeggedOrder {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                extra_fields,
            } => OrderType::PeggedOrder {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                extra_fields: f(extra_fields),
            },
            Self::MarketToLimit {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields,
            } => OrderType::MarketToLimit {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: f(extra_fields),
            },
            Self::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields,
            } => OrderType::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields: f(extra_fields),
            },
        }
    }
}

/// Expected string format:
/// ORDER_TYPE:id=`<id>`;price=`<price>`;quantity=`<qty>`;side=<BUY|SELL>;timestamp=`<ts>`;time_in_force=`<tif>`;[additional fields]
///
/// Examples:
/// - Standard:id=123;price=10000;quantity=5;side=BUY;timestamp=1616823000000;time_in_force=GTC
/// - IcebergOrder:id=124;price=10000;visible_quantity=1;hidden_quantity=4;side=SELL;timestamp=1616823000000;time_in_force=GTC
impl<T: Default> FromStr for OrderType<T> {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(PriceLevelError::InvalidFormat);
        }

        let order_type = parts[0];
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

        let parse_quantity = |field: &str, value: &str| -> Result<Quantity, PriceLevelError> {
            Quantity::from_str(value).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: field.to_string(),
                value: value.to_string(),
            })
        };

        let parse_price = |field: &str, value: &str| -> Result<Price, PriceLevelError> {
            Price::from_str(value).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: field.to_string(),
                value: value.to_string(),
            })
        };

        let parse_timestamp = |field: &str, value: &str| -> Result<TimestampMs, PriceLevelError> {
            TimestampMs::from_str(value).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: field.to_string(),
                value: value.to_string(),
            })
        };

        let parse_i64 = |field: &str, value: &str| -> Result<i64, PriceLevelError> {
            value
                .parse::<i64>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        // Parse common fields
        let id_str = get_field("id")?;
        let id = Id::from_str(id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
            field: "id".to_string(),
            value: id_str.to_string(),
        })?;

        let price_str = get_field("price")?;
        let price = parse_price("price", price_str)?;

        let side_str = get_field("side")?;
        let side: Side = Side::from_str(side_str)?;

        let timestamp_str = get_field("timestamp")?;
        let timestamp = parse_timestamp("timestamp", timestamp_str)?;

        let tif_str = get_field("time_in_force")?;
        let time_in_force = TimeInForce::from_str(tif_str)?;

        let user_id = match fields.get("user_id") {
            Some(value) => user_id_from_str(value)?,
            None => Hash32::zero(),
        };

        // Parse specific order types
        match order_type {
            "Standard" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_quantity("quantity", quantity_str)?;

                Ok(OrderType::Standard {
                    id,
                    price,
                    quantity,
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "IcebergOrder" => {
                let visible_quantity_str = get_field("visible_quantity")?;
                let visible_quantity = parse_quantity("visible_quantity", visible_quantity_str)?;

                let hidden_quantity_str = get_field("hidden_quantity")?;
                let hidden_quantity = parse_quantity("hidden_quantity", hidden_quantity_str)?;

                Ok(OrderType::IcebergOrder {
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "PostOnly" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_quantity("quantity", quantity_str)?;

                Ok(OrderType::PostOnly {
                    id,
                    price,
                    quantity,
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "TrailingStop" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_quantity("quantity", quantity_str)?;

                let trail_amount_str = get_field("trail_amount")?;
                let trail_amount = parse_quantity("trail_amount", trail_amount_str)?;

                let last_reference_price_str = get_field("last_reference_price")?;
                let last_reference_price =
                    parse_price("last_reference_price", last_reference_price_str)?;

                Ok(OrderType::TrailingStop {
                    id,
                    price,
                    quantity,
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    trail_amount,
                    last_reference_price,
                    extra_fields: T::default(),
                })
            }
            "PeggedOrder" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_quantity("quantity", quantity_str)?;

                let reference_price_offset_str = get_field("reference_price_offset")?;
                let reference_price_offset =
                    parse_i64("reference_price_offset", reference_price_offset_str)?;

                let reference_price_type_str = get_field("reference_price_type")?;
                let reference_price_type = match reference_price_type_str {
                    "BestBid" => PegReferenceType::BestBid,
                    "BestAsk" => PegReferenceType::BestAsk,
                    "MidPrice" => PegReferenceType::MidPrice,
                    "LastTrade" => PegReferenceType::LastTrade,
                    _ => {
                        return Err(PriceLevelError::InvalidFieldValue {
                            field: "reference_price_type".to_string(),
                            value: reference_price_type_str.to_string(),
                        });
                    }
                };

                Ok(OrderType::PeggedOrder {
                    id,
                    price,
                    quantity,
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    reference_price_offset,
                    reference_price_type,
                    extra_fields: T::default(),
                })
            }
            "MarketToLimit" => {
                let quantity_str = get_field("quantity")?;
                let quantity = parse_quantity("quantity", quantity_str)?;

                Ok(OrderType::MarketToLimit {
                    id,
                    price,
                    quantity,
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    extra_fields: T::default(),
                })
            }
            "ReserveOrder" => {
                let visible_quantity_str = get_field("visible_quantity")?;
                let visible_quantity = parse_quantity("visible_quantity", visible_quantity_str)?;

                let hidden_quantity_str = get_field("hidden_quantity")?;
                let hidden_quantity = parse_quantity("hidden_quantity", hidden_quantity_str)?;

                let replenish_threshold_str = get_field("replenish_threshold")?;
                let replenish_threshold =
                    parse_quantity("replenish_threshold", replenish_threshold_str)?;
                let replenish_amount_str = get_field("replenish_amount")?;
                let replenish_amount = if replenish_amount_str == "None" {
                    None
                } else {
                    // A replenish amount is structurally non-zero. Parse into
                    // `NonZeroU64` so a literal `0` (or a non-numeric value) is
                    // surfaced as a typed `InvalidFieldValue` rather than a
                    // silently-accepted empty tranche.
                    let value = replenish_amount_str.parse::<NonZeroU64>().map_err(|_| {
                        PriceLevelError::InvalidFieldValue {
                            field: "replenish_amount".to_string(),
                            value: replenish_amount_str.to_string(),
                        }
                    })?;
                    Some(value)
                };
                let auto_replenish_str = get_field("auto_replenish")?;
                let auto_replenish = match auto_replenish_str {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(PriceLevelError::InvalidFieldValue {
                            field: "auto_replenish".to_string(),
                            value: auto_replenish_str.to_string(),
                        });
                    }
                };

                Ok(OrderType::ReserveOrder {
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    side,
                    user_id,
                    timestamp,
                    time_in_force,
                    replenish_threshold,
                    replenish_amount,
                    auto_replenish,
                    extra_fields: T::default(),
                })
            }
            _ => Err(PriceLevelError::UnknownOrderType(order_type.to_string())),
        }
    }
}

impl<T> fmt::Display for OrderType<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::Standard {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "Standard:id={};price={};quantity={};side={};user_id={};timestamp={};time_in_force={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    user_id,
                    timestamp,
                    time_in_force
                )
            }
            OrderType::IcebergOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "IcebergOrder:id={};price={};visible_quantity={};hidden_quantity={};side={};user_id={};timestamp={};time_in_force={}",
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    format!("{side:?}").to_uppercase(),
                    user_id,
                    timestamp,
                    time_in_force
                )
            }
            OrderType::PostOnly {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "PostOnly:id={};price={};quantity={};side={};user_id={};timestamp={};time_in_force={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    user_id,
                    timestamp,
                    time_in_force
                )
            }
            OrderType::TrailingStop {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                trail_amount,
                last_reference_price,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "TrailingStop:id={};price={};quantity={};side={};user_id={};timestamp={};time_in_force={};trail_amount={};last_reference_price={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    user_id,
                    timestamp,
                    time_in_force,
                    trail_amount,
                    last_reference_price
                )
            }
            OrderType::PeggedOrder {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                reference_price_offset,
                reference_price_type,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "PeggedOrder:id={};price={};quantity={};side={};user_id={};timestamp={};time_in_force={};reference_price_offset={};reference_price_type={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    user_id,
                    timestamp,
                    time_in_force,
                    reference_price_offset,
                    reference_price_type
                )
            }
            OrderType::MarketToLimit {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "MarketToLimit:id={};price={};quantity={};side={};user_id={};timestamp={};time_in_force={}",
                    id,
                    price,
                    quantity,
                    format!("{side:?}").to_uppercase(),
                    user_id,
                    timestamp,
                    time_in_force
                )
            }
            OrderType::ReserveOrder {
                id,
                price,
                visible_quantity,
                hidden_quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                replenish_threshold,
                replenish_amount,
                auto_replenish,
                extra_fields: _,
            } => {
                write!(
                    f,
                    "ReserveOrder:id={};price={};visible_quantity={};hidden_quantity={};side={};user_id={};timestamp={};time_in_force={};replenish_threshold={};replenish_amount={};auto_replenish={}",
                    id,
                    price,
                    visible_quantity,
                    hidden_quantity,
                    format!("{side:?}").to_uppercase(),
                    user_id,
                    timestamp,
                    time_in_force,
                    replenish_threshold,
                    replenish_amount.map_or("None".to_string(), |v| v.to_string()),
                    auto_replenish
                )
            }
        }
    }
}

impl From<OrderQueue> for Vec<Arc<OrderType<()>>> {
    fn from(queue: OrderQueue) -> Self {
        queue.to_vec()
    }
}

// Type aliases for common use cases
#[allow(dead_code)]
pub type SimpleOrderType = OrderType<()>;
#[allow(dead_code)]
pub type OrderTypeWithMetadata = OrderType<OrderMetadata>;

// Example of what the extra fields could contain
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OrderMetadata {
    pub client_id: Option<u64>,
    pub user_id: Option<u64>,
    pub exchange_id: Option<u8>,
    pub priority: u8,
}
