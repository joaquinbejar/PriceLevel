//! Fixture (issue #173 review): `debug_assert!` invoked with the `[...]`
//! macro delimiter. Exactly one violation.

pub fn bad(v: i32) {
    debug_assert![v > 0];
}
