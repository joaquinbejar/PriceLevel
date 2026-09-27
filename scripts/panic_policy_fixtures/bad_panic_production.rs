//! Fixture (issue #173): `panic!` in production must fail the gate.

pub fn bad(v: i32) -> i32 {
    if v < 0 {
        panic!("negative value");
    }
    v
}
