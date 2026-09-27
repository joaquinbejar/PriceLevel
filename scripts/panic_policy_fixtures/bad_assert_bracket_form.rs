//! Fixture (issue #173 review): `assert!` invoked with the `[...]` macro
//! delimiter — valid Rust, and NOT caught by a scanner that only matches a
//! `(` after the `!`. Exactly one violation.

pub fn bad(v: i32) {
    assert![v > 0];
}
