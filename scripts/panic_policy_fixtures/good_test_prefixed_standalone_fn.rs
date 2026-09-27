//! Fixture (issue #173): a standalone `#[cfg(test)] fn test_*` — this
//! crate's convention for a helper called ONLY from test code (e.g.
//! `PriceLevel::test_poison_guard` in `src/price_level/level.rs`, which
//! deliberately panics under a test-only `catch_unwind` to exercise poison
//! behaviour) — is test-only and must NOT be flagged, even though it is not
//! inside a `mod tests { ... }` block.

#[cfg(test)]
pub(crate) fn test_deliberately_panics() {
    let _ = std::panic::catch_unwind(|| {
        panic!("intentional poison for a test");
    });
}
