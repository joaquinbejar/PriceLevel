//! Fixture (issue #173 review): indexing inside a non-test-shaped
//! `#[cfg(test)] mod test_seam { ... }` (a production-adjacent seam module,
//! same shape as `src/execution/match_result.rs`'s `test_seam`) must also
//! fail the gate, not just a standalone `fn`.

#[cfg(test)]
pub(crate) mod test_seam {
    pub(crate) fn first_byte(buf: &[u8]) -> u8 {
        buf[0]
    }
}
