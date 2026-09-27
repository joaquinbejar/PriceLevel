//! Fixture (issue #173): `debug_assert!` and friends in production must fail
//! the gate even though they compile out of release builds — the policy
//! forbids them "including checks intended only for development builds"
//! (`rules/global_rules.md`).

pub fn bad_debug_assert(v: i32) {
    debug_assert!(v > 0);
}

pub fn bad_debug_assert_eq(a: i32, b: i32) {
    debug_assert_eq!(a, b);
}

pub fn bad_debug_assert_ne(a: i32, b: i32) {
    debug_assert_ne!(a, b);
}
