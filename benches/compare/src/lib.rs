//! Apples-to-apples workload source for the pricelevel 0.9.2-vs-0.10
//! comparison. See `BENCHMARKS.md` at the repo root for the results and
//! methodology; this crate is the harness that produced them.
//!
//! This crate is compiled TWICE, selected by a Cargo feature, against the
//! SAME `src/workloads.rs` source: once with `--features old` (pricelevel
//! 0.9.2 from crates.io) and once with `--features new` (the local 0.10
//! tree via a path dependency). `shim` isolates the handful of public-API
//! differences between the two versions (see `CHANGELOG.md`'s
//! `[Unreleased]` section and `src/lib.rs`'s migration guides in the main
//! crate) behind small functions so `workloads.rs` itself contains no
//! version-conditional code. Most of the hot-path surface used here
//! (`PriceLevel::new/add_order/match_order/update_order`, the `OrderType`
//! struct-variant shape, `OrderUpdate`, `Trade::with_timestamp`,
//! `MatchResult::new/add_trade`, `TradeList::from_str`,
//! `snapshot_to_json`/`snapshot_package`/`from_snapshot_json`) is
//! byte-for-byte unchanged between 0.9.2 and 0.10, which is why the shim
//! below is short.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

#[cfg(all(feature = "old", feature = "new"))]
compile_error!("enable exactly one of `--features old` / `--features new`, not both");
#[cfg(not(any(feature = "old", feature = "new")))]
compile_error!(
    "enable exactly one of `--features old` (pricelevel 0.9.2) or `--features new` (local 0.10 tree)"
);

// The version under test, re-exported under one name so `workloads.rs`
// never spells out which build it is.
#[cfg(feature = "new")]
pub use pricelevel_new as pl;
#[cfg(feature = "old")]
pub use pricelevel_old as pl;

/// Human-readable label for the active build, used in Criterion group
/// names and CSV output so both rounds land in comparable, distinct files.
pub const VERSION_LABEL: &str = if cfg!(feature = "old") {
    "0.9.2"
} else {
    "0.10.0"
};

pub mod shim;
pub mod workloads;
