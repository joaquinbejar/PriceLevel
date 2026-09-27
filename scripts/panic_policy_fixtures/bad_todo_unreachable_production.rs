//! Fixture (issue #173): `todo!`/`unimplemented!`/`unreachable!` in
//! production must fail the gate — "calling a branch unreachable does not
//! exempt it" per `rules/global_rules.md`.

pub fn bad_todo() -> i32 {
    todo!()
}

pub fn bad_unimplemented() -> i32 {
    unimplemented!()
}

pub fn bad_unreachable(v: i32) -> i32 {
    match v {
        0 => 0,
        _ => unreachable!(),
    }
}
