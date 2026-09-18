//! The comment block sitting directly above a declaration, and the one at
//! the top of the file.
//!
//! C++ has three documentation spellings in wide use — Doxygen's `/** */`
//! and `///`, and the plain `//` block a great deal of real code uses
//! instead — and no compiler-blessed one. All three are accepted: a run of
//! `//`-comments immediately above a declaration is its documentation, the
//! same rule the Go parser applies to a package comment.
//!
//! The file header (§A3) is the comment run that opens the file. It is
//! only the file's own documentation when a blank line separates it from
//! whatever comes next — otherwise it is the first declaration's doc
//! comment, and claiming it here would put one comment in two places.

use crate::parser::language_parser::node_text;
use tree_sitter::Node;

/// The documentation attached to a declaration: the comment run directly
/// above it, with no blank line in between.
pub(super) fn extract_doc(node: &Node, source: &str) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = node.prev_sibling();
    let mut next_start = node.start_position().row;
    while let Some(sibling) = current {
        if sibling.kind() != "comment" {
            break;
        }
        // A blank line between two comments ends the run: the earlier
        // block documents something else, or nothing.
        if next_start > sibling.end_position().row + 1 {
            break;
        }
        lines.push(node_text(&sibling, source).to_string());
        next_start = sibling.start_position().row;
        current = sibling.prev_sibling();
    }
    lines.reverse();
    clean(&lines)
}

/// The file's own header comment (§A3), or `None` when the opening
/// comment belongs to the first declaration instead.
pub(super) fn file_doc(root: Node, source: &str) -> Option<String> {
    let mut cursor = root.walk();
    let mut lines: Vec<String> = Vec::new();
    let mut last_row: Option<usize> = None;
    for child in root.children(&mut cursor) {
        if child.kind() != "comment" {
            // The run touches the first declaration, so it documents that
            // declaration and not the file.
            let touching = last_row.is_some_and(|row| child.start_position().row <= row + 1);
            return if touching { None } else { clean(&lines) };
        }
        if last_row.is_some_and(|row| child.start_position().row > row + 1) {
            break;
        }
        last_row = Some(child.end_position().row);
        lines.push(node_text(&child, source).to_string());
    }
    clean(&lines)
}

/// Strip the comment markers a run uses and join what is left. Returns
/// `None` when nothing but markers was there.
fn clean(lines: &[String]) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in lines {
        for line in strip_block(raw).lines() {
            let trimmed = line.trim();
            let trimmed = trimmed
                .strip_prefix("///")
                .or_else(|| trimmed.strip_prefix("//!"))
                .or_else(|| trimmed.strip_prefix("//"))
                .or_else(|| trimmed.strip_prefix('*'))
                .unwrap_or(trimmed);
            out.push(trimmed.trim().to_string());
        }
    }
    let joined = out.join("\n");
    let joined = joined.trim();
    if joined.is_empty() {
        return None;
    }
    Some(joined.to_string())
}

/// Remove the delimiters of a `/* … */` block, leaving the inner lines for
/// per-line cleaning.
fn strip_block(text: &str) -> &str {
    let text = text.trim();
    match text.strip_prefix("/*") {
        Some(rest) => rest.trim_end_matches("*/"),
        None => text,
    }
}
