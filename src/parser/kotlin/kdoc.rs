//! KDoc (`/** ... */`) extraction. Same syntax as Javadoc — strips delimiters
//! and leading `*` per line, skipping annotations between the comment and the
//! entity.

use super::super::language_parser::node_text;
use tree_sitter::Node;

pub(super) fn extract_kdoc(node: &Node, source: &str) -> Option<String> {
    let mut current = node.prev_sibling();
    while let Some(sib) = current {
        match sib.kind() {
            "annotation" | "modifiers" => {
                current = sib.prev_sibling();
            }
            _ => break,
        }
    }
    let comment_node = current?;
    if comment_node.kind() != "multiline_comment" && comment_node.kind() != "comment" {
        return None;
    }
    let text = node_text(&comment_node, source);
    if !text.starts_with("/**") {
        return None;
    }
    let inner = text.trim_start_matches("/**").trim_end_matches("*/").trim();
    let lines: Vec<String> = inner
        .lines()
        .map(|l| {
            let t = l.trim();
            if t.starts_with('*') {
                t[1..].trim_start().to_string()
            } else {
                t.to_string()
            }
        })
        .collect();
    if lines.is_empty() || lines.iter().all(|l| l.is_empty()) {
        return None;
    }
    Some(lines.join("\n").trim().to_string())
}
