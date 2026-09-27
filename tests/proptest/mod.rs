/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
******************************************************************************/

//! Property-test harness for the named single-price-level invariants
//! (issue #80), plus the issue #140 scaled-notional property.
//!
//! This is a dedicated integration-test target (`[[test]] name = "proptest"`)
//! kept out of the unit `tests` target so the slower, generative matching
//! properties do not weigh down the unit `cargo test` hot loop. Every property
//! drives one [`PriceLevel`](pricelevel::PriceLevel) through its public API
//! only — no crate-internal imports — building orders through the validated
//! newtypes (`Price` / `Quantity` / `TimestampMs` / `Id` / `Side` /
//! `TimeInForce`) and the taker discriminators (`TakerKind`).
//!
//! `proptest` is deterministic given its `ProptestConfig`; failing inputs (if
//! any are ever found) are persisted by `proptest` next to the test source as
//! `tests/proptest/properties.proptest-regressions` and replayed on the next
//! run.

// This crate root is entirely test code (issue #173's Production Panic
// Policy gate, `[lints.clippy]` in `Cargo.toml`, is package-wide and would
// otherwise apply here too). `clippy.toml`'s `allow-*-in-tests` keys already
// exempt `unwrap_used` / `expect_used` / `panic` / `indexing_slicing` for
// items directly under `#[cfg(test)]` or `#[test]`, but this file's helper
// functions (`build_level`, etc.) sit at plain module scope, not inside a
// `mod tests {}` block, so clippy's per-item test detection does not reach
// them — hence the explicit blanket allow. `string_slice` and
// `arithmetic_side_effects` have no "in tests" toggle at all. None of this
// reaches `src/`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

mod properties;
mod strategies;
