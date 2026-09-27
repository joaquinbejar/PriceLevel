//! Fixture (issue #173): a real co-located `mod tests { ... }` block may
//! unwrap, expect, assert, debug_assert, panic and index freely — this is
//! the shape every co-located test module in this crate uses.

#[cfg(test)]
mod tests {
    #[test]
    fn it_unwraps_asserts_and_panics() {
        let v = Some(1);
        let x = v.unwrap();
        assert_eq!(x, 1);
        debug_assert!(x == 1);
        let s = "hello";
        let _ = &s[0..2];
        if x != 1 {
            panic!("unreachable in this fixture, but allowed to say so");
        }
    }
}
