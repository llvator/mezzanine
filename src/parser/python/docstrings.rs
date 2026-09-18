//! Python docstring extraction and cleaning.

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// Extract a Python docstring from the body of a function or class.
///
/// Docstrings are string literals appearing as the first expression-statement
/// inside the body block. We skip leading comments and extract the string
/// content, stripping quotes and dedenting multiline text.
pub(super) fn extract_docstring(body_owner: &Node, source: &str) -> Option<String> {
    leading_string(&body_owner.child_by_field_name("body")?, source)
}

/// The module's own docstring — the string literal that opens the file
/// (PY-029).
///
/// It documents no entity, which is why it is read here rather than by the
/// declaration walk: a `.py` file *is* a module, but the module entity the
/// import graph resolves against is minted by the analyzer from the file, not
/// by this parse. It travels as `ParseResult::file_documentation` and is
/// lifted onto `FileInfo`, exactly as Rust's `//!` header and Go's package
/// comment are.
///
/// The module root is the block, rather than owning one under a `body` field
/// the way a `def` or a `class` does, so it reaches the same scan directly.
pub(super) fn module_docstring(root: &Node, source: &str) -> Option<String> {
    leading_string(root, source)
}

/// The string literal a block opens with, if it opens with one.
///
/// Leading comments are skipped — a licence header above the docstring is
/// still a file with a docstring — but the first statement that is not a
/// string ends the search: a string in the middle of a body is an expression,
/// not documentation.
fn leading_string(block: &Node, source: &str) -> Option<String> {
    let mut cursor = block.walk();
    for child in block.children(&mut cursor) {
        if child.kind() == "comment" {
            continue;
        }
        if child.kind() != "expression_statement" {
            return None;
        }
        let mut inner = child.walk();
        let literal = child
            .children(&mut inner)
            .find(|e| matches!(e.kind(), "string" | "concatenated_string"))?;
        return clean_docstring(node_text(&literal, source));
    }
    None
}

/// Strip triple/single quotes, dedent, and trim a raw docstring.
fn clean_docstring(raw: &str) -> Option<String> {
    let inner = strip_quotes(raw)?;
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    let dedented = dedent(trimmed);
    if dedented.is_empty() {
        None
    } else {
        Some(dedented)
    }
}

/// Peel the quote delimiters off a string literal's source text.
///
/// Only the plain forms are handled: a prefixed literal (`r"""…"""`,
/// `f"…"`) is not documentation Python would put in `__doc__`, so declining
/// it is the correct answer rather than a gap.
fn strip_quotes(raw: &str) -> Option<&str> {
    for delimiter in ["\"\"\"", "'''", "\"", "'"] {
        if raw.starts_with(delimiter) {
            return raw.get(delimiter.len()..raw.len().saturating_sub(delimiter.len()));
        }
    }
    None
}

/// Remove the common indentation the source layout added to a multi-line
/// docstring, leaving the author's own relative indentation intact.
///
/// The first line is left alone: it sits directly after the opening quotes,
/// so it never carries the block's indentation in the first place.
fn dedent(trimmed: &str) -> String {
    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() <= 1 {
        return trimmed.to_string();
    }
    // Minimum indentation of non-empty continuation lines.
    let min_indent = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    let dedented: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| match i {
            0 => line.to_string(),
            _ if line.len() > min_indent => line[min_indent..].to_string(),
            _ => line.trim().to_string(),
        })
        .collect();
    dedented.join("\n").trim().to_string()
}
