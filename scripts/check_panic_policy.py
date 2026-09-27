#!/usr/bin/env python3
"""Production Panic Policy syntax gate (issue #173).

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
`test_seam::check_add_trade`), NOT a test, and stays fully in scope.

Comments and string/char literal contents are masked out before scanning
(replaced with spaces, same length, same line numbers) so a doc comment or
string that merely mentions "unwrap" or "panic!" is never flagged.

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

# (pattern, human-readable label). Order does not matter: every pattern
# names a distinct macro/method, none is a substring of another's match.
FORBIDDEN_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"\bassert_eq!\s*\("), "assert_eq!(...)"),
    (re.compile(r"\bassert_ne!\s*\("), "assert_ne!(...)"),
    (re.compile(r"\bassert!\s*\("), "assert!(...)"),
    (re.compile(r"\bdebug_assert_eq!\s*\("), "debug_assert_eq!(...)"),
    (re.compile(r"\bdebug_assert_ne!\s*\("), "debug_assert_ne!(...)"),
    (re.compile(r"\bdebug_assert!\s*\("), "debug_assert!(...)"),
    (re.compile(r"\bpanic!\s*\("), "panic!(...)"),
    (re.compile(r"\btodo!\s*\("), "todo!(...)"),
    (re.compile(r"\bunimplemented!\s*\("), "unimplemented!(...)"),
    (re.compile(r"\bunreachable!\s*\("), "unreachable!(...)"),
    (re.compile(r"\bpanic_any\s*\("), "panic_any(...)"),
    (re.compile(r"\bresume_unwind\s*\("), "resume_unwind(...)"),
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

# A test-module name the co-located test convention uses (`mod tests`, `mod
# tests_eq`, `mod transaction_serialization_tests`, ...). Deliberately does
# NOT match singular `test_...` names (`test_seam`, `test_order_type_display`
# would only reach this check if outside a `tests/` directory, which does
# not happen in this crate) — see the module docstring.
_TEST_MODULE_NAME = re.compile(r"^(tests|tests_.*|.*_tests)$")

_ATTR_LINE = re.compile(r"[ \t]*#\[[^\]]*\][ \t]*(//[^\n]*)?\n")
_BLANK_LINE = re.compile(r"[ \t]*(//[^\n]*)?\n")
_MOD_HEAD = re.compile(r"[ \t]*mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{")
_FN_HEAD = re.compile(
    r"[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[^{;]*\{"
)


def mask_comments_and_strings(text: str) -> str:
    """Blanks comment and string/char literal contents, same length/lines.

    Handles line comments, nested block comments, escaped string literals
    and raw strings/byte strings (`r"..."`, `r#"..."#`, `br##"..."##`, ...).
    Char literals and lifetimes are intentionally left untouched: the
    longest forbidden pattern is longer than any char literal can be, so
    leaving them unmasked cannot produce a false positive.
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


def find_test_skip_spans(masked: str) -> list[Skip]:
    """Finds every exempt test-module / `#[test]`-fn span (see module doc)."""
    spans: list[Skip] = []
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
            spans.append(Skip(attr_match.start(), end + 1))
            continue
        # `#[cfg(test)]`: a qualifying `mod` name, or a `test_`-prefixed
        # standalone fn (this crate's "test-invoked-only" convention — see
        # the module docstring). Anything else (a bare `fn` that is not
        # `test_`-prefixed, `impl`, `struct`, `type`, `thread_local!`, or a
        # `mod` with a non-qualifying name) is a production-adjacent test
        # seam and is deliberately NOT added to `spans`.
        mod_head = _MOD_HEAD.match(masked, pos)
        if mod_head and _TEST_MODULE_NAME.match(mod_head.group(1)):
            brace_index = mod_head.end() - 1
            end = find_matching_brace(masked, brace_index)
            spans.append(Skip(attr_match.start(), end + 1))
            continue
        fn_head = _FN_HEAD.match(masked, pos)
        if fn_head and fn_head.group(1).startswith("test_"):
            brace_index = fn_head.end() - 1
            end = find_matching_brace(masked, brace_index)
            spans.append(Skip(attr_match.start(), end + 1))
            continue
    return spans


def in_any_span(offset: int, spans: list[Skip]) -> bool:
    return any(span.start <= offset < span.end for span in spans)


@dataclass
class Finding:
    path: Path
    line: int
    label: str
    snippet: str


def scan_text(path: Path, text: str, *, allowed: list[Finding] | None = None) -> list[Finding]:
    """Scans `text` and returns the un-allowed findings.

    Findings from `SATURATING_PATTERNS` whose line carries the
    `panic-policy-allow-saturating` marker are appended to `allowed` (if
    given) instead of the returned list, so a caller can print them as
    reviewed, narrow exceptions rather than silently dropping them.
    """
    masked = mask_comments_and_strings(text)
    skip_spans = find_test_skip_spans(masked)
    findings: list[Finding] = []
    for pattern, label in [*FORBIDDEN_PATTERNS, *SATURATING_PATTERNS]:
        is_saturating = (pattern, label) in SATURATING_PATTERNS
        for match in pattern.finditer(masked):
            if in_any_span(match.start(), skip_spans):
                continue
            line_no = text.count("\n", 0, match.start()) + 1
            line_start = text.rfind("\n", 0, match.start()) + 1
            line_end = text.find("\n", match.start())
            if line_end == -1:
                line_end = len(text)
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
