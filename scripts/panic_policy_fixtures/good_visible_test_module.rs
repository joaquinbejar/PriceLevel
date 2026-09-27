//! Fixture (issue #173): a test-support module may be visible to sibling
//! test modules (`pub(crate) mod ..._tests`, e.g. `utils::encode::
//! serde_parity_tests`). The visibility modifier must not stop it from being
//! recognized as a test module.

#[cfg(test)]
pub(crate) mod shared_harness_tests {
    pub(crate) fn assert_parity(a: u8, b: u8) {
        assert_eq!(a, b);
        let _ = Some(a).unwrap();
    }
}

#[cfg(test)]
pub mod another_tests {
    #[test]
    fn allows_assert() {
        assert!(true);
    }
}
