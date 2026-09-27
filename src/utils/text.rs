//! Allocation-free building blocks shared by the crate's text (`FromStr`)
//! parsers (issue #174).
//!
//! Every helper works on borrowed `&str` slices taken at ASCII delimiter
//! positions (always UTF-8 char boundaries) through checked access — no
//! indexing, no slicing expressions and no unchecked offset or depth
//! arithmetic. Nesting is tracked by [`NestingDepth`], a checked counter
//! bounded by [`MAX_TEXT_NESTING_DEPTH`] in both directions, so hostile input
//! is rejected with a typed error long before the counter could overflow.
//!
//! This module is a leaf: it depends only on `crate::errors`.

use crate::errors::{CapacityResource, PriceLevelError};
#[cfg(test)]
use crate::utils::alloc::try_reserve_vec;
use crate::utils::alloc::{try_push_vec, try_reserve_string};

/// Maximum bracket / parenthesis nesting depth the text parsers accept.
///
/// The depth counts every open delimiter, including the list bracket that
/// encloses a `Trades:[...]` or `filled_order_ids=[...]` section. The crate's
/// own `Display` output never nests deeper than 2; the limit exists so that
/// adversarial input is rejected with a typed error long before any counter
/// could overflow.
pub(crate) const MAX_TEXT_NESTING_DEPTH: u32 = 128;

/// Nesting budget left for the contents of a list once its enclosing bracket
/// is counted toward [`MAX_TEXT_NESTING_DEPTH`].
pub(crate) const MAX_TEXT_NESTING_DEPTH_INSIDE_LIST: u32 = MAX_TEXT_NESTING_DEPTH - 1;

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

/// Returns `true` when `s.to_uppercase() == upper`, without allocating.
///
/// `str::to_uppercase` is the concatenation of every scalar's full Unicode
/// uppercase mapping (`char::to_uppercase`; unlike lowercasing it has no
/// context-sensitive rule), so streaming that mapping and comparing it with
/// `upper` accepts exactly the inputs the former
/// `match s.to_uppercase().as_str()` accepted. That includes the handful of
/// non-ASCII scalars whose uppercase form is ASCII: `ſ` (U+017F) → `S`,
/// dotless `ı` (U+0131) → `I`, `ß` → `SS` and the Latin ligatures
/// `ﬀ ﬁ ﬂ ﬃ ﬄ ﬅ ﬆ`. The comparison stops at the first mismatch or as soon as
/// the stream outruns `upper`, so the work is bounded by `upper.len()`, not by
/// the input length.
#[inline]
#[must_use]
pub(crate) fn uppercases_to(s: &str, upper: &str) -> bool {
    s.chars().flat_map(char::to_uppercase).eq(upper.chars())
}

/// Maximum number of input scalars an error message echoes back.
pub(crate) const MAX_ECHOED_INPUT_CHARS: usize = 128;

/// A bounded, allocation-free `Display` view of untrusted input for error
/// messages.
///
/// Inputs of at most [`MAX_ECHOED_INPUT_CHARS`] scalars are written verbatim.
/// Longer inputs are cut at the char boundary after the first
/// [`MAX_ECHOED_INPUT_CHARS`] scalars (found through checked access) and
/// followed by `... (<n> bytes total)`, so an error message built from it is
/// bounded in size no matter how large the input is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Echo<'a>(pub(crate) &'a str);

impl std::fmt::Display for Echo<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cut = self
            .0
            .char_indices()
            .nth(MAX_ECHOED_INPUT_CHARS)
            .map(|(pos, _)| pos);
        match cut.and_then(|pos| self.0.get(..pos)) {
            Some(prefix) => write!(f, "{prefix}... ({} bytes total)", self.0.len()),
            None => f.write_str(self.0),
        }
    }
}

/// The bounded echo of `s` ([`Echo`]) as an owned `String` of at most
/// [`MAX_ECHOED_INPUT_CHARS`] scalars plus a fixed-size suffix.
#[inline]
#[must_use]
pub(crate) fn echo(s: &str) -> String {
    Echo(s).to_string()
}

#[cfg(test)]
mod bounded_text_tests {
    use super::{MAX_ECHOED_INPUT_CHARS, echo, uppercases_to};
    use proptest::prelude::*;

    #[test]
    fn test_echo_short_input_is_verbatim() {
        assert_eq!(echo(""), "");
        assert_eq!(echo("abc"), "abc");
        let exact = "é".repeat(MAX_ECHOED_INPUT_CHARS);
        assert_eq!(echo(&exact), exact);
    }

    #[test]
    fn test_echo_long_input_is_truncated_at_char_boundary() {
        let long = "é".repeat(MAX_ECHOED_INPUT_CHARS + 1);
        let expected = format!(
            "{}... ({} bytes total)",
            "é".repeat(MAX_ECHOED_INPUT_CHARS),
            long.len()
        );
        assert_eq!(echo(&long), expected);
        let huge = "x".repeat(1 << 20);
        assert!(echo(&huge).len() < 2 * MAX_ECHOED_INPUT_CHARS);
    }

    #[test]
    fn test_uppercases_to_matches_non_ascii_folds() {
        assert!(uppercases_to("ſell", "SELL"));
        assert!(uppercases_to("ﬁlled", "FILLED"));
        assert!(uppercases_to("ıoc", "IOC"));
        assert!(!uppercases_to("sel", "SELL"));
        assert!(!uppercases_to("sells", "SELL"));
    }

    proptest! {
        #[test]
        fn prop_uppercases_to_equals_to_uppercase(
            s in prop_oneof![".{0,8}", "[sSſıiﬁﬂﬀßeEllL]{0,6}"],
            upper in prop_oneof![Just("SELL"), Just("SS"), Just("FILLED"), Just("IOC"), Just("")],
        ) {
            prop_assert_eq!(uppercases_to(&s, upper), s.to_uppercase() == upper);
        }
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

/// A structural nesting failure detected while scanning text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NestingError {
    /// A closing delimiter appeared with no matching opening delimiter.
    UnmatchedClose,
    /// The input ended while at least one delimiter was still open.
    Unclosed,
    /// The nesting depth (or the excess of unmatched closing delimiters)
    /// would exceed the limit.
    TooDeep {
        /// The limit that was exceeded.
        limit: u32,
    },
}

impl NestingError {
    /// The typed error for a depth-limit violation; structural (unmatched /
    /// unclosed) failures are mapped by each parser onto its established
    /// error contract.
    #[cold]
    #[inline(never)]
    #[must_use]
    pub(crate) fn too_deep_error(limit: u32) -> PriceLevelError {
        PriceLevelError::ParseError {
            message: format!("nesting depth exceeds the limit of {limit}"),
        }
    }
}

/// Checked, bounded bracket-nesting counter.
///
/// The depth is signed so that a scan can continue past an unmatched closing
/// delimiter exactly as the pre-#174 parsers did (separators are only
/// top-level at depth zero), while remembering the underflow so the caller
/// can reject the input once the scan is complete. `|depth|` is bounded by
/// `limit`, so the counter can never overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NestingDepth {
    depth: i64,
    limit: u32,
    underflowed: bool,
}

impl NestingDepth {
    /// A counter at depth zero bounded by `limit` in both directions.
    #[inline]
    #[must_use]
    pub(crate) const fn new(limit: u32) -> Self {
        Self {
            depth: 0,
            limit,
            underflowed: false,
        }
    }

    /// `true` at depth zero, where separators are top-level.
    #[inline]
    #[must_use]
    pub(crate) const fn is_top_level(&self) -> bool {
        self.depth == 0
    }

    /// Records an opening delimiter.
    ///
    /// # Errors
    ///
    /// [`NestingError::TooDeep`] if the new depth would exceed the limit; the
    /// depth is left unchanged.
    #[inline]
    pub(crate) fn open(&mut self) -> Result<(), NestingError> {
        match self.depth.checked_add(1) {
            Some(next) if next <= i64::from(self.limit) => {
                self.depth = next;
                Ok(())
            }
            _ => Err(NestingError::TooDeep { limit: self.limit }),
        }
    }

    /// Records a closing delimiter. Going below zero is recorded as an
    /// underflow (see [`Self::balance`]) rather than failing immediately.
    ///
    /// # Errors
    ///
    /// [`NestingError::TooDeep`] if the unmatched closing delimiters would
    /// exceed the limit; the depth is left unchanged.
    #[inline]
    pub(crate) fn close(&mut self) -> Result<(), NestingError> {
        let floor = i64::from(self.limit).checked_neg();
        match (self.depth.checked_sub(1), floor) {
            (Some(next), Some(floor)) if next >= floor => {
                self.depth = next;
                if next < 0 {
                    self.underflowed = true;
                }
                Ok(())
            }
            _ => Err(NestingError::TooDeep { limit: self.limit }),
        }
    }

    /// Final balance check once the scan is complete.
    ///
    /// # Errors
    ///
    /// - [`NestingError::UnmatchedClose`] if the depth ever went below zero.
    /// - [`NestingError::Unclosed`] if a delimiter is still open.
    #[inline]
    pub(crate) fn balance(&self) -> Result<(), NestingError> {
        if self.underflowed {
            Err(NestingError::UnmatchedClose)
        } else if self.depth != 0 {
            Err(NestingError::Unclosed)
        } else {
            Ok(())
        }
    }
}

/// Returns the byte offset of the ASCII `close` that balances an `open`
/// assumed to sit immediately **before** `s` (so scanning starts at depth 1).
///
/// Nested `open` / `close` pairs inside `s` are skipped. Every byte of a
/// multibyte scalar is `>= 0x80`, so an ASCII comparison never matches inside
/// one and the returned offset is always a char boundary of `s`.
///
/// # Errors
///
/// - [`NestingError::Unclosed`] if `s` ends before the balancing `close`.
/// - [`NestingError::TooDeep`] if nesting (the assumed `open` included)
///   exceeds `limit`.
pub(crate) fn matching_close(
    s: &str,
    open: u8,
    close: u8,
    limit: u32,
) -> Result<usize, NestingError> {
    let mut depth = NestingDepth::new(limit);
    depth.open()?;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if b == open {
            depth.open()?;
        } else if b == close {
            depth.close()?;
            if depth.is_top_level() {
                return Ok(i);
            }
        }
    }
    Err(NestingError::Unclosed)
}

/// One segment produced by [`TopLevelSplit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment<'a> {
    /// The segment text (possibly empty), borrowed from the input.
    pub(crate) text: &'a str,
    /// `true` for the segment that runs to the end of the input.
    pub(crate) is_last: bool,
}

/// Splits a string at every ASCII `sep` found at nesting depth zero,
/// yielding borrowed slices — the text-parser equivalent of `str::split`
/// with bracket awareness.
///
/// Like `str::split`, an input with `n` top-level separators yields `n + 1`
/// segments (empty ones included); callers apply their own empty-segment
/// rules. Segmentation is exactly that of the pre-#174 signed-depth scans
/// (a separator after an unmatched closing delimiter is not top-level), so
/// every segment the old parsers handed to an element parser is still
/// produced, in order, and element errors keep their precedence.
///
/// Nesting is checked:
/// - exceeding the limit yields `Err(TooDeep)` at that point and ends the
///   iteration;
/// - after the last segment, an unbalanced input (an unmatched closing
///   delimiter anywhere, or a delimiter still open at the end) yields one
///   final `Err(UnmatchedClose | Unclosed)`.
#[derive(Debug, Clone)]
pub(crate) struct TopLevelSplit<'a> {
    rest: Option<&'a str>,
    balance_pending: bool,
    sep: u8,
    opens: &'static [u8],
    closes: &'static [u8],
    depth: NestingDepth,
}

impl<'a> TopLevelSplit<'a> {
    /// Splits `s` at top-level `sep`, nesting on any byte of `opens` /
    /// `closes`. All delimiter bytes must be ASCII.
    #[must_use]
    pub(crate) fn new(
        s: &'a str,
        sep: u8,
        opens: &'static [u8],
        closes: &'static [u8],
        limit: u32,
    ) -> Self {
        Self {
            rest: Some(s),
            balance_pending: true,
            sep,
            opens,
            closes,
            depth: NestingDepth::new(limit),
        }
    }

    fn fail(&mut self, error: NestingError) -> Option<Result<Segment<'a>, NestingError>> {
        self.rest = None;
        self.balance_pending = false;
        Some(Err(error))
    }
}

impl<'a> Iterator for TopLevelSplit<'a> {
    type Item = Result<Segment<'a>, NestingError>;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(rest) = self.rest else {
            if self.balance_pending {
                self.balance_pending = false;
                if let Err(e) = self.depth.balance() {
                    return Some(Err(e));
                }
            }
            return None;
        };
        for (i, &b) in rest.as_bytes().iter().enumerate() {
            if b == self.sep && self.depth.is_top_level() {
                // `i` is the position of an ASCII byte, hence a char
                // boundary, and `tail` starts with the one-byte separator.
                let Some((text, tail)) = rest.split_at_checked(i) else {
                    return self.fail(NestingError::Unclosed);
                };
                let Some(tail) = tail.get(1..) else {
                    return self.fail(NestingError::Unclosed);
                };
                self.rest = Some(tail);
                return Some(Ok(Segment {
                    text,
                    is_last: false,
                }));
            }
            let step = if self.opens.contains(&b) {
                self.depth.open()
            } else if self.closes.contains(&b) {
                self.depth.close()
            } else {
                Ok(())
            };
            if let Err(e) = step {
                return self.fail(e);
            }
        }
        self.rest = None;
        Some(Ok(Segment {
            text: rest,
            is_last: true,
        }))
    }
}

/// Appends `item` to `vec`, reserving through the fallible allocator API.
///
/// # Errors
///
/// [`PriceLevelError::CapacityExceeded`] (resource
/// [`CapacityResource::Text`]) if the vector cannot grow; `vec` is left
/// unchanged. The error is fixed-size, so reporting the failure does not
/// allocate (issue #164).
#[inline]
pub(crate) fn try_push<T>(vec: &mut Vec<T>, item: T) -> Result<(), PriceLevelError> {
    try_push_vec(vec, item, CapacityResource::Text)
}

/// Reserves room for `additional` more elements through `Vec::try_reserve`.
/// Test-only since #164: production parsers grow through [`try_push`].
///
/// # Errors
///
/// [`PriceLevelError::CapacityExceeded`] (resource
/// [`CapacityResource::Text`]) on capacity overflow or allocation failure;
/// `vec` is left unchanged.
#[cfg(test)]
#[inline]
pub(crate) fn try_reserve<T>(vec: &mut Vec<T>, additional: usize) -> Result<(), PriceLevelError> {
    try_reserve_vec(vec, additional, CapacityResource::Text)
}

/// Reserves room for `additional` more bytes in `s` through
/// `String::try_reserve_exact`.
///
/// # Errors
///
/// [`PriceLevelError::CapacityExceeded`] (resource
/// [`CapacityResource::Text`]) on capacity overflow or allocation failure;
/// `s` is left unchanged.
#[inline]
pub(crate) fn try_reserve_str(s: &mut String, additional: usize) -> Result<(), PriceLevelError> {
    try_reserve_string(s, additional, CapacityResource::Text)
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

    #[test]
    fn test_nesting_depth_limit_and_underflow_are_typed() {
        let mut d = NestingDepth::new(2);
        assert_eq!(d.balance(), Ok(()));
        assert_eq!(d.open(), Ok(()));
        assert_eq!(d.open(), Ok(()));
        assert_eq!(d.open(), Err(NestingError::TooDeep { limit: 2 }));
        assert_eq!(d.balance(), Err(NestingError::Unclosed));
        assert_eq!(d.close(), Ok(()));
        assert_eq!(d.close(), Ok(()));
        assert_eq!(d.balance(), Ok(()));
        assert_eq!(d.close(), Ok(()));
        assert!(!d.is_top_level());
        assert_eq!(d.close(), Ok(()));
        assert_eq!(d.close(), Err(NestingError::TooDeep { limit: 2 }));
        assert_eq!(d.open(), Ok(()));
        assert_eq!(d.open(), Ok(()));
        assert!(d.is_top_level());
        // Returning to zero does not hide the earlier underflow.
        assert_eq!(d.balance(), Err(NestingError::UnmatchedClose));

        let mut max = NestingDepth::new(u32::MAX);
        max.depth = i64::from(u32::MAX);
        assert_eq!(max.open(), Err(NestingError::TooDeep { limit: u32::MAX }));
    }

    #[test]
    fn test_matching_close_finds_balanced_bracket() {
        assert_eq!(matching_close("a[b]c]d", b'[', b']', 8), Ok(5));
        assert_eq!(matching_close("]", b'[', b']', 8), Ok(0));
        assert_eq!(matching_close("é]", b'[', b']', 8), Ok(2));
        assert_eq!(
            matching_close("[[", b'[', b']', 8),
            Err(NestingError::Unclosed)
        );
        assert_eq!(
            matching_close("[[", b'[', b']', 2),
            Err(NestingError::TooDeep { limit: 2 })
        );
    }

    fn collect(s: &str, limit: u32) -> Vec<Result<(&str, bool), NestingError>> {
        TopLevelSplit::new(s, b',', b"[(", b"])", limit)
            .map(|r| r.map(|seg| (seg.text, seg.is_last)))
            .collect()
    }

    #[test]
    fn test_top_level_split_matches_str_split_without_nesting() {
        for s in ["", ",", "a", "a,b", ",a,,b,", "é,日,\u{1F600}"] {
            let ours: Vec<&str> = collect(s, 8)
                .into_iter()
                .map(|r| r.expect("no nesting").0)
                .collect();
            let std: Vec<&str> = s.split(',').collect();
            assert_eq!(ours, std, "input {s:?}");
        }
    }

    #[test]
    fn test_top_level_split_skips_nested_separators() {
        assert_eq!(
            collect("a[1,2],b(3,[4,5]),c", 8),
            vec![
                Ok(("a[1,2]", false)),
                Ok(("b(3,[4,5])", false)),
                Ok(("c", true))
            ]
        );
    }

    #[test]
    fn test_top_level_split_reports_imbalance_after_all_segments() {
        // After an unmatched `]` the depth is negative, so the next `,` is not
        // top-level (pre-#174 segmentation); the imbalance is reported last.
        assert_eq!(
            collect("a,b],c", 8),
            vec![
                Ok(("a", false)),
                Ok(("b],c", true)),
                Err(NestingError::UnmatchedClose)
            ]
        );
        assert_eq!(
            collect("a,b)(,c", 8),
            vec![
                Ok(("a", false)),
                Ok(("b)(", false)),
                Ok(("c", true)),
                Err(NestingError::UnmatchedClose)
            ]
        );
        assert_eq!(
            collect("a,[b,c", 8),
            vec![
                Ok(("a", false)),
                Ok(("[b,c", true)),
                Err(NestingError::Unclosed)
            ]
        );
        assert_eq!(
            collect("a,[[[b", 2),
            vec![Ok(("a", false)), Err(NestingError::TooDeep { limit: 2 })]
        );
        assert_eq!(
            collect("a,]]]b", 2),
            vec![Ok(("a", false)), Err(NestingError::TooDeep { limit: 2 })]
        );
    }

    #[test]
    fn test_top_level_split_depth_limit_with_bounded_input() {
        let limit = MAX_TEXT_NESTING_DEPTH;
        let depth = usize::try_from(limit).expect("fits");
        let ok = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert_eq!(collect(&ok, limit), vec![Ok((ok.as_str(), true))]);
        let deep = format!("{}{}", "[".repeat(depth + 1), "]".repeat(depth + 1));
        assert_eq!(
            collect(&deep, limit),
            vec![Err(NestingError::TooDeep { limit })]
        );
        // A far larger (1 MiB) but bounded input still fails fast and typed,
        // in both directions.
        for c in ["[", "]"] {
            let huge = c.repeat(1 << 20);
            assert_eq!(
                collect(&huge, limit),
                vec![Err(NestingError::TooDeep { limit })]
            );
        }
    }

    #[test]
    fn test_try_reserve_reports_capacity_overflow_as_typed_error() {
        let mut v: Vec<u64> = vec![1];
        let err = try_reserve(&mut v, usize::MAX).expect_err("must overflow");
        assert!(matches!(
            err,
            PriceLevelError::CapacityExceeded {
                resource: CapacityResource::Text,
                additional: usize::MAX
            }
        ));
        assert_eq!(v, vec![1]);
        let mut s = String::from("x");
        let err = try_reserve_str(&mut s, usize::MAX).expect_err("must overflow");
        assert!(matches!(
            err,
            PriceLevelError::CapacityExceeded {
                resource: CapacityResource::Text,
                ..
            }
        ));
        assert_eq!(s, "x");
        try_push(&mut v, 2).expect("push");
        assert_eq!(v, vec![1, 2]);
    }
}
