//! Fixture (issue #173): `.expect()` in production must fail the gate.

pub fn bad(v: Option<i32>) -> i32 {
    v.expect("value must be present")
}
