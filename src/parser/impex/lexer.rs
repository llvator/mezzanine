//! Quote-aware splitter for Impex content.
//!
//! Two operations the rest of the parser builds on:
//!
//! 1. **Splitting a header / data line by `;`** while respecting double-
//!    quoted segments — `"foo;bar"` is one cell, not two.
//! 2. **Splitting a modifier expression by `,`** with the same quoting
//!    rules — `[default=foo,bar]` joined with quotes lives in one
//!    modifier value while `code,system` is two separate fields.
//!
//! Hybris's Impex docs are clear on this: double quotes wrap a value
//! that may contain the otherwise-special `;` and `,`; everything
//! between matched quotes is one literal token. We do not handle
//! escaped quotes (`""` to embed a literal quote) — they're rare in
//! practice and the SAP docs don't formalise them; if we ever see a
//! corpus that needs it, the splitter is the place to add it.

/// Split `line` on the unquoted occurrences of `sep`. Each returned
/// segment retains its surrounding quotes (callers strip them when
/// they want the literal content) so the split round-trips into a
/// recognisable representation.
pub(super) fn split_unquoted(line: &str, sep: char) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    for c in line.chars() {
        if c == '"' {
            in_quote = !in_quote;
            buf.push(c);
        } else if c == sep && !in_quote {
            out.push(std::mem::take(&mut buf));
        } else {
            buf.push(c);
        }
    }
    out.push(buf);
    out
}

/// Strip a single layer of surrounding double quotes if both ends
/// match. Trims whitespace first so `  "x"  ` round-trips to `x`.
pub(super) fn unquote(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_simple() {
        let parts = split_unquoted("a;b;c", ';');
        assert_eq!(parts, vec!["a", "b", "c"]);
    }

    #[test]
    fn split_respects_quotes() {
        let parts = split_unquoted("a;\"b;c\";d", ';');
        assert_eq!(parts, vec!["a", "\"b;c\"", "d"]);
    }

    #[test]
    fn split_modifier_commas_inside_brackets() {
        // The bracket structure is the parser's responsibility — this
        // helper just respects quoting. Nested-bracket handling lives
        // in `headers.rs` / `columns.rs`.
        let parts = split_unquoted("a,\"x,y\",b", ',');
        assert_eq!(parts, vec!["a", "\"x,y\"", "b"]);
    }

    #[test]
    fn unquote_trims_and_strips() {
        assert_eq!(unquote("  \"hello\"  "), "hello");
        assert_eq!(unquote("noquotes"), "noquotes");
        assert_eq!(unquote("\"only-left"), "\"only-left");
    }
}
