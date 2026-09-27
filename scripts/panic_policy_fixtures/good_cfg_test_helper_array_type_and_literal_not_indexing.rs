//! Fixture (issue #173 review): a standalone `#[cfg(test)]` helper that
//! uses array/slice TYPE syntax (`&[u8]`, `[u8; 4]`) and an array LITERAL
//! (`[1, 2, 3, 4]`), with no actual indexing, must NOT be flagged by the
//! indexing heuristic — proves it does not mistake type/literal `[...]`
//! for `expr[...]` indexing.

#[cfg(test)]
pub(crate) fn make(_v: &[u8]) -> [u8; 4] {
    [1, 2, 3, 4]
}
