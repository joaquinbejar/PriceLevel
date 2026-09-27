//! Fixture (issue #173): a `saturating_*` use with an inline
//! `panic-policy-allow-saturating` marker on the same line is a reviewed,
//! narrow exception (the same shape as `src/utils/uuid.rs`'s
//! `DECIMAL_RADIX`) and must NOT fail the gate.

pub const RADIX: std::num::NonZeroU64 = std::num::NonZeroU64::MIN.saturating_add(9); // panic-policy-allow-saturating: compile-time-only, provably exact
