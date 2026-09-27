//! Fixture (issue #173 review): slice and array return types (`&[u8]`,
//! `[u8; 4]`) must not be mistaken for indexing. No actual indexing
//! happens here.

#[cfg(test)]
pub(crate) fn borrowed(v: &[u8]) -> &[u8] {
    v
}

#[cfg(test)]
pub(crate) fn owned() -> [u8; 4] {
    [0, 0, 0, 0]
}
