//! Fixture (issue #173 review): `assert!` with the `(...)` delimiter, the
//! plain, no-space form. Exactly one violation, so a missed macro-delimiter
//! form elsewhere cannot hide behind this one passing.

pub fn bad(v: i32) {
    assert!(v > 0);
}
