//! Fixture (issue #173 review): `return [...]` / `break [...]` introduce an
//! array literal, not indexing — the `return`/`break`/`in` keyword
//! exclusion in `INDEXING_PATTERN`'s caller must not flag these.

#[cfg(test)]
pub(crate) fn make(early: bool) -> [u8; 2] {
    if early {
        return [0, 0];
    }
    [1, 1]
}
