#!/usr/bin/env python3
r"""Production Panic Policy syntax gate (issue #173).

`cargo clippy` with the restriction lints in `[lints.clippy]` (Cargo.toml)
catches `.unwrap()` / `.expect()` / `panic!` / `unreachable!` / `todo!` /
`unimplemented!` / indexing / string-slicing / narrowing casts / raw
arithmetic in production code. It has NO lint at all for the
`assert!`/`assert_eq!`/`assert_ne!`/`debug_assert!`/`debug_assert_eq!`/
`debug_assert_ne!` macro family (`rules/global_rules.md`'s Production Panic
Policy forbids all of these in production, "including checks intended only
for development builds").

Clippy's own `#[cfg(test)]` / `#[test]` detection (used by `clippy.toml`'s
`allow-*-in-tests` keys) is also coarser than this crate's policy: it treats
ANY item carrying `#[cfg(test)]`, anywhere, as test code — including a
standalone `#[cfg(test)] fn` production helper that is not inside a `mod
tests { ... }` block and IS reachable from production (see
`src/execution/match_result.rs`'s `test_seam`, called from `add_trade`).
`rules/global_rules.md`'s Testing section is explicit that this permission
"does not extend to production functions, including their `cfg(test)`
branches, or to helpers shared with production." This script re-checks EVERY
forbidden form (not just the assert family) but only inside exactly that
gap: `#[cfg(test)]` items that are not one of the co-located test module
shapes this crate uses everywhere else.

What counts as "test code" here, matching the project's actual layout
(`CLAUDE.md`, `rules/global_rules.md` Testing section):

1. Any file under a `tests/` directory component (the co-located
   `src/<module>/tests/*.rs` convention) — skipped entirely.
2. A `#[cfg(test)] mod <name> { ... }` block whose name is exactly `tests`,
   starts with `tests_`, or ends with `_tests` — the shape every co-located
   test module in this crate uses (`mod tests`, `mod tests_eq`, `mod
   transaction_serialization_tests`, ...). Skipped for that block only.
3. A `#[test] fn ... { ... }` — skipped for that function only, whatever its
   name.
4. A standalone `#[cfg(test)] fn test_<name>(...) { ... }` — a `test_`-
   prefixed function name is this crate's convention for a helper called
   ONLY from test code, never from production (`test_poison_guard`,
   `test_release_after_removal`, `test_shard_runs`, `test_rest_unadmitted`,
   `test_take_front_scan_visits`; verified by grep against every call site
   under `src/` at the time this script was written — re-verify if the
   convention ever changes). Skipped for that function only.

Anything else under `#[cfg(test)]` — a bare `fn` whose name does NOT start
with `test_`, an `impl`, `struct`, `type`, `thread_local!`, or a `mod` with
any other name (e.g. `mod test_seam`, `mod snapshot_hook`) — is a
production-adjacent test seam (a hook a production code path calls under
`cfg(test)`, e.g. `fire_post_only_decision_hook`, `apply_update_decision_hook`,
`test_seam::check_add_trade`), NOT a test, and stays fully in scope. Its
extent (to the matching `}` or `;`) is also where the indexing check below
runs.

Comments and string/char/byte-char literal contents are masked out before
scanning (replaced with spaces, same length, same line numbers) so a doc
comment or string that merely mentions "unwrap" or "panic!" is never
flagged. Char/byte-char literals (`'x'`, `'\''`, `'\\'`, `'\u{2764}'`,
`b'"'`, ...) are lexed and masked separately from lifetimes/labels (`'a`,
`'static`, `'outer:`), which are left untouched — a char literal containing
a quote (`'"'`) must not be mistaken for the start of a string, or
everything up to the next unrelated `"` gets wrongly masked out (issue
#173 review).

Every macro pattern below (the `assert!`/`panic!`/... family, not the
`.unwrap()`-style method calls, which cannot use macro delimiters) matches
all three Rust macro delimiters — `(...)`, `{...}`, `[...]` — and permits
whitespace or comments before the `!` and before the delimiter: Rust does
not require `assert!(x)` to be written with no space, and `assert! { x }` /
`assert![x]` are equally valid macro invocations that must not slip past
as an unmatched delimiter shape (issue #173 review).

A syntax-aware indexing/slicing check (`INDEXING_PATTERN`) additionally runs
— ONLY inside the production-adjacent `#[cfg(test)]` scope above, never over
ordinary production code, where clippy's own AST-accurate `indexing_slicing`
lint already applies — because `clippy.toml`'s `allow-indexing-slicing-in-
tests` exempts that scope too and has no way to distinguish it from a real
test. It is a heuristic (see `INDEXING_PATTERN`'s comment for exactly what it
matches and does not).

This is a lightweight, non-exhaustive lexer over Rust syntax, not a real
parser — see the module docstring for the exact shapes it recognizes. It is
one layer of the Production Panic Policy gate, not a proof that the crate
never panics: `doc/panic-boundaries.md` and the manual review checklist in
`rules/global_rules.md` cover what neither this script nor clippy can see
(callback obligations, dependency preconditions, allocator OOM).

Usage:
    scripts/check_panic_policy.py [--path PATH ...]
    scripts/check_panic_policy.py --self-test

Exit status is non-zero if any forbidden production form is found (normal
mode) or if any fixture does not match its expected outcome (`--self-test`).
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

def _macro_pattern(name: str) -> re.Pattern[str]:
    """Matches `name` invoked as a macro with ANY of Rust's three delimiter
    pairs (`(...)`, `{...}`, `[...]`), with optional whitespace (comments
    are already masked to whitespace by the time this runs) both before the
    `!` and before the opening delimiter. Rust does not require `assert!(x)`
    to be written with no space: `assert ! (x)`, `assert!{x}` and
    `assert![x]` are equally valid macro invocations (issue #173 review).
    """
    escaped = re.escape(name)
    return re.compile(rf"\b{escaped}\s*!\s*[(\[{{]")


# (pattern, human-readable label). Order does not matter: every pattern
# names a distinct macro/method, none is a substring of another's match.
FORBIDDEN_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (_macro_pattern("assert_eq"), "assert_eq!(...)"),
    (_macro_pattern("assert_ne"), "assert_ne!(...)"),
    (_macro_pattern("assert"), "assert!(...)"),
    (_macro_pattern("debug_assert_eq"), "debug_assert_eq!(...)"),
    (_macro_pattern("debug_assert_ne"), "debug_assert_ne!(...)"),
    (_macro_pattern("debug_assert"), "debug_assert!(...)"),
    (_macro_pattern("panic"), "panic!(...)"),
    (_macro_pattern("todo"), "todo!(...)"),
    (_macro_pattern("unimplemented"), "unimplemented!(...)"),
    (_macro_pattern("unreachable"), "unreachable!(...)"),
    # `panic_any` / `resume_unwind` are plain functions (`std::panic::`), not
    # macros: only the `(...)` call form is valid Rust for them.
    (re.compile(r"\bpanic_any\s*\("), "panic_any(...)"),
    (re.compile(r"\bresume_unwind\s*\("), "resume_unwind(...)"),
    # Method calls: `(...)` is the only valid form, no macro delimiters.
    (re.compile(r"\.unwrap\s*\("), ".unwrap()"),
    (re.compile(r"\.unwrap_err\s*\("), ".unwrap_err()"),
    (re.compile(r"\.expect\s*\("), ".expect()"),
    (re.compile(r"\.expect_err\s*\("), ".expect_err()"),
    (re.compile(r"\.get_unwrap\s*\("), ".get_unwrap()"),
    (re.compile(r"\bstd::process::exit\s*\("), "std::process::exit(...)"),
    (re.compile(r"(?<![:\w])process::exit\s*\("), "process::exit(...)"),
]

# `saturating_*` / `wrapping_*` are an Arithmetic-rules violation, not a
# Production Panic Policy one (`rules/global_rules.md`: "Never `saturating_*`
# or `wrapping_*` on quantity / value / counter state"), but the same
# clippy-blind-spot problem applies (no clippy restriction lint bans them),
# so the same script closes the gap. Unlike `FORBIDDEN_PATTERNS`, a finding
# here MAY be allowed with an inline `panic-policy-allow-saturating` marker
# comment on the same line — narrowly, for a documented, reviewed,
# compile-time-only case (see `src/utils/uuid.rs`'s `DECIMAL_RADIX`) — the
# same "justified narrow allow, on the exact expression, with a comment"
# discipline `clippy::allow` uses elsewhere in this crate.
SATURATING_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (
        re.compile(r"\.(?:saturating|wrapping)_(?:add|sub|mul|div|rem|neg|shl|shr)\s*\("),
        "saturating_*/wrapping_* arithmetic",
    ),
]

ALLOW_SATURATING_MARKER = "panic-policy-allow-saturating"

# Syntax-aware indexing/slicing check (issue #173 review): `clippy.toml`'s
# `allow-indexing-slicing-in-tests` exempts `indexing_slicing` for ANY
# `#[cfg(test)]` item, including the production-adjacent test-seam shape
# `SATURATING_PATTERNS` / `FORBIDDEN_PATTERNS` already re-check for other
# forms. Dropping that clippy.toml key instead, and adding an explicit
# `#[allow(clippy::indexing_slicing, clippy::string_slice)]` to every
# existing co-located `mod tests { ... }` block that indexes or slices,
# would touch most of the ~30 test files in this crate for no behavioural
# change (churn evaluated and rejected); this check only runs where
# `find_test_skip_spans` finds a `cfg_test_scope` span, never over ordinary
# production code (where clippy's own AST-accurate `indexing_slicing` lint
# already applies without this heuristic's limitations).
#
# What it matches: an identifier, or a closing `)` / `]` (so a chained
# `get_vec()[0]` / `matrix[0][1]` still counts), immediately (only
# whitespace/masked-comments between) followed by `[`, with that `[` not
# immediately followed by `]` (excludes the invalid, so irrelevant, empty
# `v[]`). A handful of keywords that can precede an array-literal /
# range-in-a-`for` rather than an indexing expression (`return [...]`,
# `break [...]`, `in [...]`, ...) are excluded by name below.
#
# What it deliberately does NOT try to distinguish, as a documented
# limitation (see `doc/panic-boundaries.md`): a field access or a more
# complex expression right before `[` (`self.buf[i]`, `(a + b)[i]`) is not
# matched (a false negative, not a false positive), and an unusual macro or
# path shape immediately before `[` could in principle still slip through
# either direction. It is one heuristic layer, not a parser.
INDEXING_PATTERN = re.compile(
    r"(?<![A-Za-z0-9_])(?P<target>[A-Za-z_][A-Za-z0-9_]*|[)\]])\s*\[(?!\s*\])"
)
_INDEXING_EXCLUDED_KEYWORDS = frozenset(
    {"return", "yield", "break", "in", "else", "move", "let", "const", "static"}
)

# A test-module name the co-located test convention uses (`mod tests`, `mod
# tests_eq`, `mod transaction_serialization_tests`, ...). Deliberately does
# NOT match singular `test_...` names (`test_seam`, `test_order_type_display`
# would only reach this check if outside a `tests/` directory, which does
# not happen in this crate) — see the module docstring.
_TEST_MODULE_NAME = re.compile(r"^(tests|tests_.*|.*_tests)$")

_ATTR_LINE = re.compile(r"[ \t]*#\[[^\]]*\][ \t]*(//[^\n]*)?\n")
_BLANK_LINE = re.compile(r"[ \t]*(//[^\n]*)?\n")
_MOD_HEAD = re.compile(
    r"[ \t]*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{"
)
_FN_HEAD = re.compile(
    r"[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[^{;]*\{"
)


# A char/byte-char literal's body: an escape sequence, or exactly one
# non-`'`/non-`\` scalar value (Python's `str` indexes by Unicode scalar
# value / code point, same granularity as a Rust `char`).
_CHAR_ESCAPE = re.compile(r"\\(?:['\"nrt0\\]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\})")


def match_char_literal(text: str, i: int) -> int | None:
    r"""If `text[i] == "'"` starts a char or byte-char literal (`'x'`,
    `'\''`, `'\u{2764}'`, `b'"'`, ...), returns the index just past its
    closing quote. Returns `None` for an empty `''` or anything that is not
    terminated by a second `'` right after one escape / one scalar value —
    in particular a lifetime or loop label (`'a`, `'static`, `'outer:`),
    which have no closing quote and must be left untouched, not masked.

    Distinguishing these matters (issue #173 review): a char literal
    containing a quote character, like `'"'`, must not be mistaken by the
    string-literal branch below for the START of a string — that would mask
    everything up to the next unrelated `"` as string content, hiding real
    code (and any violation in it) in between.
    """
    n = len(text)
    if i >= n or text[i] != "'":
        return None
    j = i + 1
    if j >= n or text[j] == "'":
        return None
    if text[j] == "\\":
        m = _CHAR_ESCAPE.match(text, j)
        if not m:
            return None
        j = m.end()
    else:
        j += 1
    if j < n and text[j] == "'":
        return j + 1
    return None


def mask_comments_and_strings(text: str) -> str:
    """Blanks comment and string/char/byte-char literal contents, same
    length/lines.

    Handles line comments, nested block comments, escaped string literals,
    raw strings/byte strings (`r"..."`, `r#"..."#`, `br##"..."##`, ...), and
    char/byte-char literals via `match_char_literal`. Lifetimes and loop
    labels (`'a`, `'static`, `'outer:`) are intentionally left untouched —
    unlike a char literal, they have no closing quote to blank up to, and
    none of the forbidden patterns can appear inside a bare identifier.
    """
    out = list(text)
    n = len(text)
    i = 0
    while i < n:
        c = text[i]
        # Line comment.
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = i
            while j < n and text[j] != "\n":
                out[j] = " "
                j += 1
            i = j
            continue
        # Nested block comment.
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 1
            out[i] = out[i + 1] = " "
            j = i + 2
            while j < n and depth > 0:
                if text[j] == "/" and j + 1 < n and text[j + 1] == "*":
                    depth += 1
                    if text[j] != "\n":
                        out[j] = " "
                    if text[j + 1] != "\n":
                        out[j + 1] = " "
                    j += 2
                    continue
                if text[j] == "*" and j + 1 < n and text[j + 1] == "/":
                    depth -= 1
                    if text[j] != "\n":
                        out[j] = " "
                    if text[j + 1] != "\n":
                        out[j + 1] = " "
                    j += 2
                    continue
                if text[j] != "\n":
                    out[j] = " "
                j += 1
            i = j
            continue
        # Char / byte-char literal: must run BEFORE the string branch below,
        # since a char literal containing `"` (`'"'`) would otherwise be
        # mistaken for the start of a string (see `match_char_literal`).
        if c == "'":
            end = match_char_literal(text, i)
            if end is not None:
                for k in range(i, end):
                    if text[k] != "\n":
                        out[k] = " "
                i = end
                continue
            # Not a char/byte-char literal: a lifetime or label, or a bare
            # `'`. Left untouched.
            i += 1
            continue
        # Raw string / raw byte string: (b)r#*"..."#* (matching hash count).
        m = re.match(r'(?:b)?r(#*)"', text[i:i + 64])
        if m:
            hashes = m.group(1)
            start = i + m.end()
            closer = '"' + hashes
            end = text.find(closer, start)
            if end == -1:
                end = n
            else:
                end += len(closer)
            for k in range(i, min(end, n)):
                if text[k] != "\n":
                    out[k] = " "
            i = end
            continue
        # Regular string literal (handles backslash escapes).
        if c == '"':
            out[i] = " "
            j = i + 1
            while j < n:
                if text[j] == "\\" and j + 1 < n:
                    if text[j] != "\n":
                        out[j] = " "
                    if text[j + 1] != "\n":
                        out[j + 1] = " "
                    j += 2
                    continue
                if text[j] == '"':
                    out[j] = " "
                    j += 1
                    break
                if text[j] != "\n":
                    out[j] = " "
                j += 1
            i = j
            continue
        i += 1
    return "".join(out)


def find_matching_brace(masked: str, open_brace_index: int) -> int:
    """Returns the index of the `}` matching the `{` at `open_brace_index`.

    Operates on the masked text, so braces that are only comment/string
    *content* are already blanked out and cannot be miscounted as real
    syntax.
    """
    depth = 0
    n = len(masked)
    i = open_brace_index
    while i < n:
        if masked[i] == "{":
            depth += 1
        elif masked[i] == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return n - 1


@dataclass
class Skip:
    start: int
    end: int


@dataclass
class SpanSets:
    """`skip`: exempt test-module / `#[test]`-fn spans (see module doc).
    `cfg_test_scope`: the complementary production-adjacent `#[cfg(test)]`
    spans (test seams) — where the indexing check below additionally runs.
    The two sets are disjoint by construction: each `#[cfg(test)]` /
    `#[test]` attribute contributes to at most one of them.
    """

    skip: list[Skip]
    cfg_test_scope: list[Skip]


def find_test_skip_spans(masked: str) -> SpanSets:
    """Finds every exempt test-module / `#[test]`-fn span, and every
    production-adjacent `#[cfg(test)]` test-seam span (see module doc)."""
    skip: list[Skip] = []
    cfg_test_scope: list[Skip] = []
    for attr_match in re.finditer(r"#\[cfg\(test\)\]|#\[test\]", masked):
        is_test_attr = attr_match.group(0) == "#[test]"
        pos = attr_match.end()
        # Skip the newline right after the attribute, then any further
        # attribute lines and blank/comment-only lines, before the item.
        if pos < len(masked) and masked[pos] == "\n":
            pos += 1
        while True:
            attr_line = _ATTR_LINE.match(masked, pos)
            if attr_line:
                pos = attr_line.end()
                continue
            blank_line = _BLANK_LINE.match(masked, pos)
            if blank_line and blank_line.group(0).strip("\n \t") == "":
                pos = blank_line.end()
                continue
            break
        if is_test_attr:
            fn_head = _FN_HEAD.match(masked, pos)
            if not fn_head:
                continue
            brace_index = fn_head.end() - 1
            end = find_matching_brace(masked, brace_index)
            skip.append(Skip(attr_match.start(), end + 1))
            continue
        # `#[cfg(test)]`: a qualifying `mod` name, or a `test_`-prefixed
        # standalone fn (this crate's "test-invoked-only" convention — see
        # the module docstring) is exempt. Anything else (a bare `fn` that
        # is not `test_`-prefixed, `impl`, `struct`, `type`,
        # `thread_local!`, or a `mod` with a non-qualifying name) is a
        # production-adjacent test seam: NOT added to `skip`, but its
        # extent (found the same way, or — for shapes with no `fn`/`mod`
        # head, such as `impl` / `struct` / `type` / `thread_local!` — up
        # to its next top-level `{...}` or `;`) is recorded in
        # `cfg_test_scope`.
        mod_head = _MOD_HEAD.match(masked, pos)
        if mod_head and _TEST_MODULE_NAME.match(mod_head.group(1)):
            brace_index = mod_head.end() - 1
            end = find_matching_brace(masked, brace_index)
            skip.append(Skip(attr_match.start(), end + 1))
            continue
        fn_head = _FN_HEAD.match(masked, pos)
        if fn_head and fn_head.group(1).startswith("test_"):
            brace_index = fn_head.end() - 1
            end = find_matching_brace(masked, brace_index)
            skip.append(Skip(attr_match.start(), end + 1))
            continue
        if mod_head:
            brace_index = mod_head.end() - 1
            end = find_matching_brace(masked, brace_index)
            cfg_test_scope.append(Skip(attr_match.start(), end + 1))
            continue
        if fn_head:
            brace_index = fn_head.end() - 1
            end = find_matching_brace(masked, brace_index)
            cfg_test_scope.append(Skip(attr_match.start(), end + 1))
            continue
        # `impl` / `struct` / `type` / `thread_local!` / anything else with
        # no `fn`/`mod` head: bounded by its next top-level `{` (matched)
        # or `;`, whichever comes first.
        brace_pos = masked.find("{", pos)
        semi_pos = masked.find(";", pos)
        if brace_pos == -1 and semi_pos == -1:
            continue
        if brace_pos != -1 and (semi_pos == -1 or brace_pos < semi_pos):
            end = find_matching_brace(masked, brace_pos)
            cfg_test_scope.append(Skip(attr_match.start(), end + 1))
        else:
            cfg_test_scope.append(Skip(attr_match.start(), semi_pos + 1))
    return SpanSets(skip=skip, cfg_test_scope=cfg_test_scope)


def in_any_span(offset: int, spans: list[Skip]) -> bool:
    return any(span.start <= offset < span.end for span in spans)


@dataclass
class Finding:
    path: Path
    line: int
    label: str
    snippet: str


def _line_bounds(text: str, offset: int) -> tuple[int, int, int]:
    """Returns `(line_no, line_start, line_end)` for the line containing
    `offset` (1-indexed line number; `line_end` excludes the newline)."""
    line_no = text.count("\n", 0, offset) + 1
    line_start = text.rfind("\n", 0, offset) + 1
    line_end = text.find("\n", offset)
    if line_end == -1:
        line_end = len(text)
    return line_no, line_start, line_end


def scan_text(path: Path, text: str, *, allowed: list[Finding] | None = None) -> list[Finding]:
    """Scans `text` and returns the un-allowed findings.

    Findings from `SATURATING_PATTERNS` whose line carries the
    `panic-policy-allow-saturating` marker are appended to `allowed` (if
    given) instead of the returned list, so a caller can print them as
    reviewed, narrow exceptions rather than silently dropping them.
    """
    masked = mask_comments_and_strings(text)
    span_sets = find_test_skip_spans(masked)
    findings: list[Finding] = []
    for pattern, label in [*FORBIDDEN_PATTERNS, *SATURATING_PATTERNS]:
        is_saturating = (pattern, label) in SATURATING_PATTERNS
        for match in pattern.finditer(masked):
            if in_any_span(match.start(), span_sets.skip):
                continue
            line_no, line_start, line_end = _line_bounds(text, match.start())
            snippet = text[line_start:line_end].strip()
            finding = Finding(path, line_no, label, snippet)
            # The marker may sit on the flagged line itself, or on one of the
            # (at most 5) comment lines directly above it — matching how
            # `#[allow(clippy::...)]` plus an explanatory comment is placed
            # immediately above the guarded expression elsewhere in this
            # crate (e.g. `src/utils/value.rs`'s `from_f64`).
            context_start = line_start
            for _ in range(5):
                if context_start == 0:
                    break
                previous_newline = text.rfind("\n", 0, context_start - 1)
                context_start = previous_newline + 1 if previous_newline != -1 else 0
            context = text[context_start:line_end]
            if is_saturating and ALLOW_SATURATING_MARKER in context:
                if allowed is not None:
                    allowed.append(finding)
                continue
            findings.append(finding)
    # Indexing/slicing: only inside a production-adjacent `#[cfg(test)]`
    # test-seam span (see `INDEXING_PATTERN`'s comment) — never over
    # ordinary production code, where clippy's own lint already applies.
    for match in INDEXING_PATTERN.finditer(masked):
        if not in_any_span(match.start(), span_sets.cfg_test_scope):
            continue
        if in_any_span(match.start(), span_sets.skip):
            continue
        target = match.group("target")
        if target in _INDEXING_EXCLUDED_KEYWORDS:
            continue
        line_no, line_start, line_end = _line_bounds(text, match.start())
        snippet = text[line_start:line_end].strip()
        findings.append(Finding(path, line_no, "indexing/slicing (v[...])", snippet))
    findings.sort(key=lambda f: f.line)
    return findings


def is_under_tests_dir(path: Path) -> bool:
    return "tests" in path.parts[:-1]


def iter_rust_files(paths: list[Path]) -> list[Path]:
    files: list[Path] = []
    for root in paths:
        if root.is_file():
            files.append(root)
            continue
        files.extend(sorted(root.rglob("*.rs")))
    return files


def scan_paths(paths: list[Path], *, allowed: list[Finding]) -> list[Finding]:
    findings: list[Finding] = []
    for file_path in iter_rust_files(paths):
        rel = file_path.relative_to(REPO_ROOT) if file_path.is_absolute() else file_path
        if is_under_tests_dir(rel):
            continue
        text = file_path.read_text(encoding="utf-8")
        findings.extend(scan_text(file_path, text, allowed=allowed))
    return findings


def run_gate(paths: list[str]) -> int:
    resolved = [(REPO_ROOT / p) for p in paths]
    allowed: list[Finding] = []
    findings = scan_paths(resolved, allowed=allowed)
    for finding in allowed:
        rel = finding.path.relative_to(REPO_ROOT) if finding.path.is_absolute() else finding.path
        print(f"{rel}:{finding.line}: allowed ({ALLOW_SATURATING_MARKER}) {finding.label}: {finding.snippet}")
    if not findings:
        print("check_panic_policy: no forbidden production forms found.")
        return 0
    for finding in findings:
        rel = finding.path.relative_to(REPO_ROOT) if finding.path.is_absolute() else finding.path
        print(f"{rel}:{finding.line}: forbidden production form {finding.label}: {finding.snippet}")
    print(
        f"check_panic_policy: {len(findings)} forbidden production form(s) found. "
        "See doc/panic-boundaries.md and rules/global_rules.md's Production Panic Policy."
    )
    return 1


def run_self_test(fixtures_dir: Path) -> int:
    failures: list[str] = []
    fixture_files = sorted(fixtures_dir.glob("*.rs"))
    if not fixture_files:
        print(f"check_panic_policy --self-test: no fixtures found under {fixtures_dir}")
        return 1
    for fixture in fixture_files:
        text = fixture.read_text(encoding="utf-8")
        findings = scan_text(fixture, text)
        if fixture.name.startswith("bad_"):
            if not findings:
                failures.append(f"{fixture.name}: expected a violation, found none")
        elif fixture.name.startswith("good_"):
            if findings:
                lines = ", ".join(f"{f.line}:{f.label}" for f in findings)
                failures.append(f"{fixture.name}: expected no violation, found {lines}")
        else:
            failures.append(f"{fixture.name}: fixture name must start with 'bad_' or 'good_'")
    if failures:
        for failure in failures:
            print(f"check_panic_policy --self-test: FAIL {failure}")
        return 1
    print(f"check_panic_policy --self-test: {len(fixture_files)} fixture(s) OK")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--path",
        dest="paths",
        action="append",
        default=None,
        help="Root file or directory to scan, relative to the repo root (default: src). May repeat.",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Validate the scanner itself against scripts/panic_policy_fixtures/ instead of scanning the crate.",
    )
    args = parser.parse_args()
    if args.self_test:
        return run_self_test(REPO_ROOT / "scripts" / "panic_policy_fixtures")
    return run_gate(args.paths or ["src"])


if __name__ == "__main__":
    sys.exit(main())
