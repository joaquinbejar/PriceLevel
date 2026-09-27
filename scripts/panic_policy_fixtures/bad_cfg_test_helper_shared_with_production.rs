//! Fixture (issue #173): a standalone `#[cfg(test)]` production helper —
//! NOT a `mod tests { ... }` block — must still fail the gate. This is
//! exactly the shape `src/execution/match_result.rs`'s `test_seam` has
//! (called from the production `add_trade` under `cfg(test)`), and exactly
//! the shape clippy's own `allow-unwrap-in-tests` / `allow-panic-in-tests`
//! (`clippy.toml`) wrongly exempts, because clippy treats ANY
//! `#[cfg(test)]`-attributed item as test code regardless of whether it is
//! a `mod tests` block. `rules/global_rules.md`'s Testing section is
//! explicit that this permission "does not extend to production functions,
//! including their `cfg(test)` branches, or to helpers shared with
//! production."

#[cfg(test)]
pub(crate) fn helper_shared_with_production(n: usize) -> usize {
    debug_assert!(n > 0, "n must be positive");
    n.checked_sub(1).unwrap()
}
