//! Fixture (issue #173 review): lifetimes and a loop label (`'a`,
//! `'static`, `'outer:`) have no closing quote and must be left untouched
//! by the char-literal lexer (not treated as an unterminated char literal
//! that swallows the rest of the file), so scanning continues normally.

pub fn bad<'a>(v: &'a bool) -> bool {
    'outer: loop {
        if *v {
            break 'outer;
        }
    }
    let _s: &'static str = "ok";
    assert!(*v);
    *v
}
