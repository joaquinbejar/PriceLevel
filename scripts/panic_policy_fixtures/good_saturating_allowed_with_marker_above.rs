//! Fixture (issue #173): the `panic-policy-allow-saturating` marker also
//! works on a comment line directly above the flagged expression — the
//! same placement `src/utils/uuid.rs`'s `DECIMAL_RADIX` uses (a multi-line
//! doc comment above a `const`, not a trailing same-line comment).

/// Compile-time-only, provably exact.
// panic-policy-allow-saturating: see doc comment above.
pub const RADIX: std::num::NonZeroU64 = std::num::NonZeroU64::MIN.saturating_add(9);
