//! Allocation-free building blocks shared by the crate's text (`FromStr`)
//! parsers (issue #174).
//!
//! Every helper works on borrowed `&str` slices taken at ASCII delimiter
//! positions (always UTF-8 char boundaries) through checked access — no
//! indexing, no slicing expressions and no unchecked offset arithmetic.
//!
//! This module is a leaf: it depends only on `crate::errors`.

use crate::errors::PriceLevelError;

/// Splits `s` at its **only** occurrence of the ASCII byte `sep`.
///
/// Returns `None` when `sep` is absent or occurs more than once, which is
/// exactly the grammar of the former `s.split(sep).collect::<Vec<_>>()`
/// followed by a `len() == 2` check — without the temporary vector. An ASCII
/// byte never occurs inside a multibyte scalar, so both halves are split at
/// a char boundary.
#[inline]
#[must_use]
pub(crate) fn split_exactly_once(s: &str, sep: u8) -> Option<(&str, &str)> {
    let pos = s.as_bytes().iter().position(|&b| b == sep)?;
    let (head, with_sep) = s.split_at_checked(pos)?;
    let tail = with_sep.get(1..)?;
    if tail.as_bytes().contains(&sep) {
        None
    } else {
        Some((head, tail))
    }
}

/// The values of a fixed set of known keys, read in one pass over a
/// `;`-separated list of `key=value` pairs.
///
/// Semantics are those of the `HashMap` the parsers used to build: a pair
/// that does not contain exactly one `=` is ignored, keys outside `names`
/// are ignored, and when a key repeats the **last** occurrence wins. No
/// allocation: values are borrowed slices of the input held in a fixed array.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fields<'a, const N: usize> {
    names: &'static [&'static str; N],
    values: [Option<&'a str>; N],
}

impl<'a, const N: usize> Fields<'a, N> {
    /// Scans `fields` once, recording the last value of every key in `names`.
    #[must_use]
    pub(crate) fn parse(fields: &'a str, names: &'static [&'static str; N]) -> Self {
        let mut values = [None; N];
        for pair in fields.split(';') {
            if let Some((key, value)) = split_exactly_once(pair, b'=')
                && let Some((_, slot)) = names
                    .iter()
                    .zip(values.iter_mut())
                    .find(|(name, _)| **name == key)
            {
                *slot = Some(value);
            }
        }
        Self { names, values }
    }

    /// The value of `name`, if present. A `name` outside the parsed set is
    /// reported as absent.
    #[must_use]
    pub(crate) fn get(&self, name: &str) -> Option<&'a str> {
        self.names
            .iter()
            .zip(self.values.iter())
            .find(|(n, _)| **n == name)
            .and_then(|(_, value)| *value)
    }

    /// The value of `name`.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::MissingField`] naming `name` if it is absent.
    pub(crate) fn require(&self, name: &str) -> Result<&'a str, PriceLevelError> {
        self.get(name)
            .ok_or_else(|| PriceLevelError::MissingField(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_exactly_once_rejects_missing_and_extra_separators() {
        assert_eq!(split_exactly_once("a:b", b':'), Some(("a", "b")));
        assert_eq!(split_exactly_once(":", b':'), Some(("", "")));
        assert_eq!(split_exactly_once("ab", b':'), None);
        assert_eq!(split_exactly_once("a:b:c", b':'), None);
        assert_eq!(split_exactly_once("é:日", b':'), Some(("é", "日")));
    }

    #[test]
    fn test_fields_last_wins_and_ignores_malformed_pairs() {
        static NAMES: [&str; 6] = ["a", "b", "c", "d", "", "e"];
        let f = Fields::parse("a=1;b=2;a=3;c=4=5;d;=6;e=;z=9", &NAMES);
        assert_eq!(f.get("a"), Some("3"));
        assert_eq!(f.get("b"), Some("2"));
        assert_eq!(f.get("c"), None);
        assert_eq!(f.get("d"), None);
        assert_eq!(f.get(""), Some("6"));
        assert_eq!(f.get("e"), Some(""));
        // Keys outside the parsed set are never reported.
        assert_eq!(f.get("z"), None);
        assert!(matches!(
            f.require("c"),
            Err(PriceLevelError::MissingField(ref name)) if name == "c"
        ));
        assert_eq!(Fields::parse("", &NAMES).get("a"), None);
        static MB: [&str; 1] = ["日"];
        assert_eq!(Fields::parse("é=日;日=é", &MB).get("日"), Some("é"));
    }
}
