//! Fixture (issue #173 review): a `&mut [u8]` parameter type must not be
//! mistaken for indexing (`mut` immediately followed by `[` looked like
//! `INDEXING_PATTERN`'s `target[` shape before `mut` was excluded). No
//! actual indexing happens here.

#[cfg(test)]
pub(crate) fn fill(v: &mut [u8]) -> usize {
    v.len()
}
