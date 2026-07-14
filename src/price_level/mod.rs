/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 28/3/25
******************************************************************************/

//! Core price level module: order queue, matching, snapshots, and statistics,
//! lock-free on the match path.
//!
//! This module provides the central [`PriceLevel`] type that represents a single price
//! point in a limit order book. It manages a queue of orders, performs matching (the
//! `Gtc` / `Ioc` / `Day` match path is lock-free), tracks statistics, and supports
//! snapshot persistence with checksum protection. Admissions and updates (cancel / resize)
//! take the shared side of a per-level guard — normally uncontended, but they can block
//! behind an `O(depth)` fill-or-kill match that holds the exclusive side across its
//! feasibility check and sweep so it stays all-or-nothing against concurrent mutation
//! (issue #112).
//!
//! # Key Types
//!
//! - [`PriceLevel`] — the main price level implementation supporting concurrent add, match,
//!   update, and cancel operations via atomic counters and crossbeam queues; the
//!   `Gtc` / `Ioc` / `Day` match is lock-free, while admissions / updates take a
//!   normally-uncontended shared lock that can block behind an `O(depth)` fill-or-kill.
//! - [`PriceLevelData`] — a serializable representation for data transfer and storage.
//! - [`PriceLevelSnapshot`] — a point-in-time snapshot of all orders at a price level.
//! - [`PriceLevelSnapshotPackage`] — a checksum-protected wrapper around a snapshot for
//!   safe persistence and recovery via JSON.
//! - [`PriceLevelStatistics`] — real-time execution statistics (orders added/removed/executed,
//!   quantity/value executed, average price, waiting times).
//! - [`OrderQueue`] — the underlying lock-free order queue based on crossbeam.
//!
//! # Snapshot Persistence
//!
//! Snapshots can be serialized to JSON with SHA-256 checksum protection:
//!
//! ```rust
//! use pricelevel::PriceLevel;
//!
//! let level = PriceLevel::new(10_000);
//! let json = level.snapshot_to_json().unwrap();
//! let restored = PriceLevel::from_snapshot_json(&json).unwrap();
//! ```

mod level;

mod snapshot;

mod entry;

mod order_queue;

mod statistics;
mod tests;

pub use level::{PriceLevel, PriceLevelData};
pub use order_queue::OrderQueue;
pub use snapshot::{PriceLevelSnapshot, PriceLevelSnapshotPackage};
pub use statistics::PriceLevelStatistics;
