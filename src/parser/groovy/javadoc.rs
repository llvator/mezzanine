//! Groovydoc / Javadoc (`/** ... */`) extraction for Groovy.
//!
//! Groovy uses the same Javadoc-style block comment as Java, so the
//! extraction logic is the same — duplicated rather than re-exported
//! from `parser/java` to keep each language's helper surface
//! self-contained per the area-map convention.

use super::super::language_parser::node_text;
use tree_sitter::Node;

pub(super) fn extract_javadoc(node: &Node, source: &str) -> Option<String> {
    let mut current = node.prev_sibling();
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

    let inner = text
        .trim_start_matches("/**")
        .trim_end_matches("*/")
        .trim();
    let lines: Vec<String> = inner
        .lines()
        .map(|l| {
            let trimmed = l.trim();
            match trimmed.strip_prefix('*') {
                Some(rest) => rest.trim_start().to_string(),
                None => trimmed.to_string(),
            }
        })
        .collect();

    if lines.is_empty() || lines.iter().all(|l| l.is_empty()) {
        return None;
    }

    Some(lines.join("\n").trim().to_string())
}
