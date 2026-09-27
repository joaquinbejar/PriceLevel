//! Fixture (issue #173 review): raw strings and raw byte strings (`r"..."`,
//! `r#"..."#`, `br"..."`) must be lexed to their real closing delimiter
//! without disturbing what follows, including one that itself contains a
//! quote-like sequence a naive scan could misparse.

pub fn bad(v: bool) {
    let _raw = r"contains a \ backslash and no escape processing";
    let _raw_hashed = r#"contains a "quoted" word"#;
    let _raw_bytes: &[u8] = br"raw bytes, no escapes";
    assert!(v);
}
