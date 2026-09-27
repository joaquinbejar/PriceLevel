//! Fixture (issue #173): a visible `#[cfg(test)]` module whose name does not
//! follow the test-module convention is a production-adjacent seam and
//! stays in scope even with a visibility modifier.

#[cfg(test)]
pub(crate) mod snapshot_hook {
    pub(crate) fn fire(n: usize) -> usize {
        n.checked_sub(1).unwrap()
    }
}
