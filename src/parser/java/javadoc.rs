//! Javadoc (`/** ... */`) extraction.

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// Extract Javadoc (`/** ... */`) comments from nodes immediately preceding
/// the given entity node. Strips the `/** */` delimiters and the leading
/// `*` on each line.
pub(super) fn extract_javadoc(node: &Node, source: &str) -> Option<String> {
    let mut current = node.prev_sibling();
    // Skip annotation nodes (#[...] in Rust terms — Java uses @Override etc.)
    while let Some(sib) = current {
        match sib.kind() {
            "marker_annotation" | "annotation" | "modifiers" => {
                current = sib.prev_sibling();
            }
            _ => break,
        }
    }
    let comment_node = current?;
    if comment_node.kind() != "block_comment" && comment_node.kind() != "comment" {
        return None;
    }
    let text = node_text(&comment_node, source);
    if !text.starts_with("/**") {
        return None;
    }

    // Strip delimiters and leading `* ` on each line.
    let inner = text.trim_start_matches("/**").trim_end_matches("*/").trim();
    let lines: Vec<String> = inner
        .lines()
        .map(|l| {
            let trimmed = l.trim();
            if trimmed.starts_with('*') {
                trimmed[1..].trim_start().to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect();

    if lines.is_empty() || lines.iter().all(|l| l.is_empty()) {
        return None;
    }

    Some(lines.join("\n").trim().to_string())
}
