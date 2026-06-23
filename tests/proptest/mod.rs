/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
******************************************************************************/

//! Property-test harness for the nine named single-price-level invariants
//! (issue #80).
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
//! any are ever found) are persisted by `proptest` under
//! `proptest-regressions/` and replayed on the next run.

mod properties;
mod strategies;
