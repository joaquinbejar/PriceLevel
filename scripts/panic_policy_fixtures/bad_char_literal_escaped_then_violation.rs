//! Fixture (issue #173 review): escaped char literals (`'\''`, `'\\'`,
//! `'\u{2764}'`) and a byte-char literal (`b'"'`) must each be lexed as one
//! literal (escape sequence, or `u{...}` unicode escape, then the closing
//! quote) without disturbing what follows.

pub fn bad(v: bool) {
    let _escaped_quote = '\'';
    let _escaped_backslash = '\\';
    let _unicode_escape = '\u{2764}';
    let _byte_quote = b'"';
    assert!(v);
}
