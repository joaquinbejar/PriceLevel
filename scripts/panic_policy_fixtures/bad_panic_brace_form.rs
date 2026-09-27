//! Fixture (issue #173 review): `panic!` invoked with the `{...}` macro
//! delimiter. Exactly one violation.

pub fn bad() -> i32 {
    panic! { "boom" }
}
