/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 28/3/25
******************************************************************************/

//! Core price level module: order queue, matching, snapshots, and statistics.
//!
//! This module provides the central [`PriceLevel`] type that represents a single price
//! point in a limit order book. It manages a queue of orders, performs matching, tracks
//! statistics, and supports snapshot persistence with checksum protection.
//!
//! The ordered index and the atomic counters are lock-free; the complete public methods
//! are not. Matching commits each fill under the maker's `DashMap` shard write lock (the
//! serialization point with a cancel or resize of that order) and assumes one logical
//! matcher per level. Admissions and updates take their target's shard write lock and the
//! shared side of a per-level guard, which can block behind a fill-or-kill
//! match holding the exclusive side across its feasibility check and sweep (issue #112),
//! a section proportional to the makers the fill visits within the dry run's lazy
//! budget and `O(depth log depth)` past it (issue #143).
//! See the crate-level "Concurrency Model" section for the full table.
//!
//! # Key Types
//!
//! - [`PriceLevel`] — the main price level implementation supporting concurrent add,
//!   update, and cancel alongside one logical matcher per level; built on atomic
//!   counters, a lock-free ordered index and sharded `DashMap` storage.
//! - [`PriceLevelData`] — a serializable representation for data transfer and storage.
//! - [`PriceLevelSnapshot`] — a point-in-time snapshot of all orders at a price level.
//! - [`PriceLevelSnapshotPackage`] — a checksum-protected wrapper around a snapshot for
//!   safe persistence and recovery via JSON.
//! - [`PriceLevelStatistics`] — real-time execution statistics (orders added/removed/executed,
//!   quantity/value executed, average price, waiting times).
//! - [`OrderQueue`] — the underlying order queue: a lock-free `crossbeam-skiplist` ordered
//!   index over sharded `DashMap` order storage.
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

mod fok_guard;

/// The synchronization primitives behind `fok_guard` (issue #206). The loom
/// model `tests/loom/fok_handoff.rs` compiles `fok_guard.rs` against its own
/// `fok_sync` with loom's instrumented equivalents.
mod fok_sync {
    pub(crate) use std::hint::spin_loop;
    pub(crate) use std::sync::atomic::{AtomicUsize, Ordering};
    pub(crate) use std::sync::{
        LockResult, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError,
    };
    pub(crate) use std::thread::yield_now;

    /// Whether `lock` carries an unrecovered poison (issue #217). A relaxed
    /// load in `std`.
    #[inline]
    pub(crate) fn rwlock_is_poisoned(lock: &RwLock<()>) -> bool {
        lock.is_poisoned()
    }
}

mod order_queue;

mod statistics;
mod tests;

pub use level::{PriceLevel, PriceLevelData};
pub use order_queue::OrderQueue;
pub use snapshot::{PriceLevelSnapshot, PriceLevelSnapshotPackage};
pub use statistics::PriceLevelStatistics;
