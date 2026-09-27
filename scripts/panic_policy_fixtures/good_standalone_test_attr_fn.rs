//! Fixture (issue #173): a directly `#[test]`-attributed function, even
//! outside a `mod tests { ... }` block, is real test code and may unwrap
//! and assert freely.

#[test]
fn standalone_test_fn_allows_unwrap() {
    let v = Some(1).unwrap();
    assert_eq!(v, 1);
}
