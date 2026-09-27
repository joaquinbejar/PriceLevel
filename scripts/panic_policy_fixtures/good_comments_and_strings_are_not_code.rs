//! Fixture (issue #173): forbidden tokens inside doc comments, line
//! comments, block comments and string literals must NOT trip the gate —
//! only real code does.
//!
//! Do not call `.unwrap()`, `.expect()`, `panic!()`, `assert!()`,
//! `debug_assert!()`, `todo!()`, `unreachable!()` or `std::process::exit()`
//! in production.

/* Block comment mentioning panic!(), unwrap(), assert!(x), debug_assert!(x)
   and std::process::exit(1) on purpose — must be ignored. */

/// Example (not run): `value.unwrap()` would panic; use `?` instead.
pub fn safe(v: Option<i32>) -> Result<i32, &'static str> {
    // Trailing comment: assert_eq!(1, 2) must not be flagged either.
    let message = "do not call .unwrap() or panic!() or assert!(x) here";
    let _ = message;
    v.ok_or("missing value")
}
