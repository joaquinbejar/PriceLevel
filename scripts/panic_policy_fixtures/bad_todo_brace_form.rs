//! Fixture (issue #173 review): `todo!` invoked with the `{...}` macro
//! delimiter. Exactly one violation.

pub fn bad() -> i32 {
    todo! {}
}
