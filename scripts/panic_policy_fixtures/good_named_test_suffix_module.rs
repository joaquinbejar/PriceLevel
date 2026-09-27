//! Fixture (issue #173): the crate's secondary co-located test modules use
//! names like `tests_eq`, `tests_order_status` or
//! `transaction_serialization_tests` — a `tests_` prefix or `_tests` suffix
//! must be recognized as a test module too, not just the literal name
//! `tests`.

#[cfg(test)]
mod some_behavior_tests {
    #[test]
    fn allows_unwrap_and_assert() {
        let v = Some(1).unwrap();
        assert_eq!(v, 1);
    }
}

#[cfg(test)]
mod tests_another_case {
    #[test]
    fn also_allows_unwrap() {
        let _ = Some(1).unwrap();
    }
}
