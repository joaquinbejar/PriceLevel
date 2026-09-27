//! Fixture (issue #173): the `assert!`/`assert_eq!`/`assert_ne!` family in
//! production must fail the gate. Clippy has no lint for these at all —
//! this is exactly the gap `scripts/check_panic_policy.py` closes.

pub fn bad_assert(v: i32) {
    assert!(v > 0);
}

pub fn bad_assert_eq(a: i32, b: i32) {
    assert_eq!(a, b);
}

pub fn bad_assert_ne(a: i32, b: i32) {
    assert_ne!(a, b);
}
