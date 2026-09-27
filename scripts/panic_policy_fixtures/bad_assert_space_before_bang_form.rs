//! Fixture (issue #173 review): whitespace between the macro name and `!`
//! (`assert !(v)`) is valid Rust — token spacing does not change what the
//! macro invocation means — and must still be caught. Exactly one
//! violation.

pub fn bad(v: i32) {
    assert !(v > 0);
}
