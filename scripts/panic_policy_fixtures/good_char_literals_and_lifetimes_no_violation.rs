//! Fixture (issue #173 review): char literals, byte-char literals,
//! lifetimes and raw strings with NO forbidden form anywhere in the file
//! must not be flagged — proves the new lexing does not itself introduce a
//! false positive.

pub fn safe<'a>(v: &'a [u8]) -> Option<u8> {
    let _quote_char = '"';
    let _brace_char = '{';
    let _escaped = '\'';
    let _byte = b'\\';
    let _raw = r#"a "quoted" word"#;
    let _label = 'outer: loop {
        break 'outer 0u8;
    };
    v.first().copied()
}
