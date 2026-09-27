//! Fixture (issue #173 review): a borrowed mutable array literal
//! (`&mut [1u8, 2]`) must not be mistaken for indexing — `mut` immediately
//! followed by `[` looks like `INDEXING_PATTERN`'s `target[` shape. No
//! actual indexing happens here.

#[cfg(test)]
pub(crate) fn make() -> usize {
    let v: &mut [u8] = &mut [1u8, 2];
    v.len()
}
