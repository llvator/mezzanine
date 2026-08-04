//! Rust doc-comment extraction (`///`, `//!`, `/** */`, `/*! */`).

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// Extract documentation comments (`///` and `//!`) from nodes immediately
/// preceding the given node. Walks backward through `prev_sibling` until a
/// non-comment/non-whitespace node is found; collects and joins all
/// contiguous doc comments into a single string with the prefix stripped.
pub(super) fn extract_doc_comment(node: &Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sib) = current {
        let kind = sib.kind();
        match kind {
            // Rust doc comments: `///` outer and `//!` inner.
            "line_comment" => {
                let text = node_text(&sib, source);
                if text.starts_with("///") || text.starts_with("//!") {
                    let stripped = text
                        .trim_start_matches("///")
                        .trim_start_matches("//!")
                        .strip_prefix(' ')
                        .unwrap_or(text.trim_start_matches("///").trim_start_matches("//!"));
                    lines.push(stripped.to_string());
                } else {
                    break; // non-doc comment ends the run
                }
            }
            // Block doc comments: `/** ... */`
            "block_comment" => {
                let text = node_text(&sib, source);
                if text.starts_with("/**") || text.starts_with("/*!") {
                    let inner = text
                        .trim_start_matches("/**")
                        .trim_start_matches("/*!")
                        .trim_end_matches("*/")
                        .trim();
                    // Strip leading `* ` on each line (Javadoc-style block comments).
                    let cleaned: Vec<&str> = inner
                        .lines()
                        .map(|l| l.trim().trim_start_matches('*').trim_start_matches(' '))
                        .collect();
                    lines.push(cleaned.join("\n"));
                } else {
                    break;
                }
            }
            // Skip attribute macros (#[...]) between doc comments and the item.
            "attribute_item" | "inner_attribute_item" => {}
            _ => break,
        }
        current = sib.prev_sibling();
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse(); // comments were collected bottom-up
    Some(lines.join("\n"))
}
