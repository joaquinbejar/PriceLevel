//! Fixture (issue #173 review): `unreachable!` invoked with the `[...]`
//! macro delimiter. Exactly one violation.

pub fn bad(v: i32) -> i32 {
    match v {
        0 => 0,
        _ => unreachable![],
    }
}
