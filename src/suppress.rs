//! Inline suppression: a `fc-allow: <reason>` marker **inside a comment** on the
//! offending line, or the line directly above it, silences one finding.
//!
//! The marker is recognised only inside a real comment — a string literal whose
//! contents spell `fc-allow:` does not suppress anything, and neither does a
//! `//` that lives inside a string (even a multi-line one), nor code that merely
//! spells the bytes (`fc-allow::Foo` is `fc - allow::Foo`).
//!
//! We do **not** hand-roll a lexer for this — that would be the exact NIH the
//! rest of the tool forbids. We already depend on `proc-macro2`, whose lexer is
//! correct about raw strings, byte strings, lifetimes and nested comments. The
//! trick: comments are not tokens (proc-macro2 discards them), so a `fc-allow`
//! occurrence is a real directive **iff its byte offset lies outside every token
//! span**. Strings/chars are `Literal` tokens; code is `Ident`/`Punct` tokens;
//! only a comment leaves a gap. Structure, never `str::contains` on a raw line.
//!
//! ```ignore
//! // fc-allow: bootstrap seed read once at composition; never in a hot path
//! let seed = std::env::var("SEED").ok();
//! ```

use std::collections::BTreeSet;
use std::ops::Range;

use proc_macro2::{TokenStream, TokenTree};

/// The marker that opts one line out of the effect gate.
pub const MARKER: &str = "fc-allow";

/// The set of 1-based source lines bearing a justified `fc-allow:` marker inside
/// a comment. Computed once per file; queried by [`is_suppressed`].
pub fn suppressed_lines(source: &str) -> BTreeSet<usize> {
    let Ok(tokens) = source.parse::<TokenStream>() else {
        // If it does not even lex we cannot tell a comment from a string, so we
        // suppress nothing — the safe direction: never hide a finding on a guess.
        return BTreeSet::new();
    };
    let mut spans: Vec<Range<usize>> = Vec::new();
    collect_token_spans(tokens, &mut spans);
    let line_starts: Vec<usize> = line_start_offsets(source);

    let mut out: BTreeSet<usize> = BTreeSet::new();
    for (offset, _) in source.match_indices(MARKER) {
        // Covered by a token (string, char, or code) → data, not a directive.
        if spans
            .iter()
            .any(|span: &Range<usize>| span.contains(&offset))
        {
            continue;
        }
        if is_justified(&source[offset + MARKER.len()..]) {
            out.insert(line_of(offset, &line_starts));
        }
    }
    out
}

/// Whether the finding on `line` (1-based) is suppressed by a justified marker
/// on that line or the one directly above it.
pub fn is_suppressed(suppressed: &BTreeSet<usize>, line: usize) -> bool {
    line != 0 && (suppressed.contains(&line) || suppressed.contains(&(line - 1)))
}

/// `: <reason>` with a non-empty reason on the same line as the marker. The
/// reason is bounded to the marker's own line so a bare `fc-allow:` at end of
/// line cannot borrow the next line's text as justification.
fn is_justified(after_marker: &str) -> bool {
    after_marker
        .strip_prefix(':')
        .and_then(|rest: &str| rest.lines().next())
        .is_some_and(|reason: &str| !reason.trim().is_empty())
}

/// Collect the byte range of every leaf token, recursing into groups so that a
/// comment *inside* `()`/`[]`/`{}` is still a gap (a group's own span would
/// otherwise swallow its contents). Whitespace and comments are not tokens, so
/// the gaps between these ranges are exactly the comments.
fn collect_token_spans(tokens: TokenStream, out: &mut Vec<Range<usize>>) {
    for tree in tokens {
        match tree {
            TokenTree::Group(group) => collect_token_spans(group.stream(), out),
            TokenTree::Ident(ident) => out.push(ident.span().byte_range()),
            TokenTree::Punct(punct) => out.push(punct.span().byte_range()),
            TokenTree::Literal(lit) => out.push(lit.span().byte_range()),
        }
    }
}

/// Byte offset at which each line begins (`offsets[0] == 0`).
fn line_start_offsets(source: &str) -> Vec<usize> {
    let mut starts: Vec<usize> = vec![0];
    for (i, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// The 1-based line containing `offset`, given the line-start table.
fn line_of(offset: usize, line_starts: &[usize]) -> usize {
    match line_starts.binary_search(&offset) {
        Ok(index) => index + 1,
        Err(index) => index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suppressed_at(source: &str, line: usize) -> bool {
        is_suppressed(&suppressed_lines(source), line)
    }

    #[test]
    fn a_justified_comment_on_the_same_line_suppresses() {
        assert!(suppressed_at("let t = now(); // fc-allow: shell only", 1));
    }

    #[test]
    fn a_justified_comment_on_the_line_above_suppresses() {
        assert!(suppressed_at("// fc-allow: justified\nlet t = now();", 2));
    }

    #[test]
    fn no_marker_does_not_suppress() {
        assert!(!suppressed_at("let t = now();", 1));
    }

    #[test]
    fn a_marker_two_lines_up_does_not_reach() {
        assert!(!suppressed_at(
            "// fc-allow: x\nlet x = 1;\nlet t = now();",
            3
        ));
    }

    #[test]
    fn line_zero_is_never_suppressed() {
        assert!(!suppressed_at("// fc-allow: x", 0));
    }

    #[test]
    fn a_bare_marker_without_a_reason_does_not_suppress() {
        assert!(!suppressed_at("let t = now(); // fc-allow", 1));
    }

    #[test]
    fn a_marker_with_an_empty_reason_does_not_suppress() {
        assert!(!suppressed_at("let t = now(); // fc-allow:   ", 1));
    }

    // ─── The marker is comment-confined, not substring on the raw line ───

    #[test]
    fn a_marker_inside_a_string_literal_does_not_suppress() {
        // The exact false-negative the review proved against the binary.
        assert!(!suppressed_at(r#"let s = read("fc-allow: not real");"#, 1));
    }

    #[test]
    fn a_double_slash_inside_a_string_is_not_a_comment() {
        // The residual hole a cheap "first //" scan would still leak through.
        assert!(!suppressed_at(r#"let s = "// fc-allow: x";"#, 1));
    }

    #[test]
    fn a_double_slash_inside_a_multiline_string_is_not_a_comment() {
        // Line 2 begins inside the string opened on line 1, so its `//` is
        // string content — proc-macro2 spans the whole literal across lines.
        let src: &str = "let s = \"line one\n// fc-allow: still string\";\nlet t = now();";
        assert!(!suppressed_at(src, 2));
    }

    #[test]
    fn code_that_merely_spells_the_marker_does_not_suppress() {
        // `fc-allow::MAX` is `fc - allow::MAX`: real code whose bytes contain
        // "fc-allow:". The Ident tokens cover it, so it is not a directive.
        assert!(!suppressed_at("let n = fc-allow::MAX; let t = now();", 1));
    }

    #[test]
    fn a_real_comment_after_a_string_still_suppresses() {
        assert!(suppressed_at(r#"let s = "x"; // fc-allow: legit"#, 1));
    }

    #[test]
    fn a_block_comment_marker_suppresses() {
        assert!(suppressed_at("let t = now(); /* fc-allow: shell */", 1));
    }

    #[test]
    fn a_comment_marker_inside_a_group_suppresses() {
        // A comment nested in `( )` must still count — we recurse into groups
        // rather than trusting the group's outer span.
        assert!(suppressed_at(
            "let _ = f(/* fc-allow: justified */ now());",
            1
        ));
    }

    #[test]
    fn a_lifetime_does_not_derail_the_lexer() {
        assert!(suppressed_at(
            "fn f<'a>(x: &'a u8) {} // fc-allow: justified",
            1
        ));
    }
}
