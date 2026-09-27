//! Fixture (issue #173 review): an array-TYPE parameter (`&[u8; 2]`)
//! contains a `;` that is not the signature's terminator. A naive
//! `[^{;]*` (or `masked.find(";", pos)`) scope scan stops there instead of
//! at the real `{`, so the function body — and the real indexing inside
//! it — escaped the check entirely. Must fail the gate.

#[cfg(test)]
pub(crate) fn check(v: &[u8; 2], i: usize) -> u8 {
    v[i]
}
