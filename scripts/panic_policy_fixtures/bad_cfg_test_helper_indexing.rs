//! Fixture (issue #173 review): a standalone `#[cfg(test)]` production
//! helper — not a `mod tests { ... }` block — that indexes must fail the
//! gate. `clippy.toml`'s `allow-indexing-slicing-in-tests` exempts this
//! exact shape from clippy's `indexing_slicing` lint (clippy cannot tell it
//! apart from a real test), so `INDEXING_PATTERN` re-checks it.

#[cfg(test)]
pub(crate) fn check(v: &[u8]) -> u8 {
    v[1]
}
