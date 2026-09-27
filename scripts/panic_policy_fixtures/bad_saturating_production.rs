//! Fixture (issue #173): `saturating_*` / `wrapping_*` on production state
//! must fail the gate — "Never `saturating_*` or `wrapping_*` on quantity /
//! value / counter state" (`rules/global_rules.md`). Clippy has no
//! restriction lint for this either.

pub fn bad(counter: u64) -> u64 {
    counter.saturating_add(1)
}
