//! Fixture (issue #173): a standalone `#[cfg(test)]` production helper (the
//! same shape as `bad_cfg_test_helper_shared_with_production.rs`) that
//! already follows the Production Panic Policy must NOT be flagged — the
//! gate checks for forbidden forms, it does not forbid `#[cfg(test)]`
//! helpers outright.

#[cfg(test)]
pub(crate) fn helper_shared_with_production(n: usize) -> Option<usize> {
    n.checked_sub(1)
}
