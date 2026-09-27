//! Fixture (issue #173 review): every macro delimiter form is still exempt
//! inside a real `mod tests { ... }` block — matching more delimiter shapes
//! must not narrow the test exemption.

#[cfg(test)]
mod tests {
    #[test]
    fn allows_every_delimiter_form() {
        assert!(true);
        assert! { true };
        assert![true];
        panic! { "only reached if the assertions above are broken" };
    }
}
