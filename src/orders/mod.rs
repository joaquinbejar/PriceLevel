//! Order types, updates, and supporting domain primitives.
//!
//! This module defines the vocabulary for orders managed by a [`crate::PriceLevel`].
//!
//! # Key Types
//!
//! - [`OrderType`] — enum covering all supported order variants (Standard, Iceberg,
//!   Reserve, PostOnly, TrailingStop, PeggedOrder, MarketToLimit).
//! - [`OrderUpdate`] — enum for order mutations (update price, quantity, cancel, replace).
//! - [`Id`] — flexible identifier supporting UUID, ULID, and sequential (`u64`) formats.
//! - [`Side`] — `Buy` or `Sell`, with `#[repr(u8)]` for compact representation.
//! - [`TimeInForce`] — order duration policies (GTC, IOC, FOK, GTD, Day),
//!   with `#[repr(u8)]`.
//! - [`Hash32`] — opaque 32-byte user identifier.
//! - [`PegReferenceType`] — reference price type for pegged orders.
//!
//! # Order Lifecycle
//!
//! Orders are constructed as [`OrderType`] variants and added to a
//! [`PriceLevel`](crate::PriceLevel) via [`add_order()`](crate::PriceLevel::add_order).
//! Mutations are applied via [`update_order()`](crate::PriceLevel::update_order)
//! using [`OrderUpdate`] variants.

mod base;

mod order_type;

mod pegged;

mod status;

mod time_in_force;

mod update;

mod tests;

pub use crate::utils::Id;
pub use base::{Hash32, Side};
pub use order_type::DEFAULT_RESERVE_REPLENISH_AMOUNT;
pub use order_type::OrderType;
pub use pegged::PegReferenceType;
pub use time_in_force::TimeInForce;
pub use update::OrderUpdate;
