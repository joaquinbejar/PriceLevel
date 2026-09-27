//! Fixture (issue #173 review): a char literal containing a `"` must be
//! lexed as a char literal, not mistaken for the start of a string — else
//! everything up to the next unrelated `"` (arbitrarily far away, possibly
//! never) gets masked out, hiding the real violation right after it.

pub fn bad(v: bool) {
    let _quote_char = '"';
    assert!(v);
}
