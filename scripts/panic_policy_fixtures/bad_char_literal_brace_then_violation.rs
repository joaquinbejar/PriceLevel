//! Fixture (issue #173 review): a char literal containing `{` or `}` must
//! not be miscounted as real brace syntax by the matching-brace scan the
//! test-module / test-seam span detection relies on, and must not confuse
//! the macro-delimiter matching either.

pub fn bad(v: bool) {
    let _open_brace_char = '{';
    let _close_brace_char = '}';
    assert!(v);
}
