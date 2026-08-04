//! TSDoc (`/** ... */`) extraction. Skips decorator nodes between the
//! comment and the entity.

use super::super::language_parser::node_text;
use tree_sitter::Node;

pub(super) fn extract_tsdoc(node: &Node, source: &str) -> Option<String> {
    let mut current = node.prev_sibling();
    while let Some(sib) = current {
        if sib.kind() == "decorator" {
            current = sib.prev_sibling();
        } else {
            break;
        }
    }
    let comment_node = current?;
    if comment_node.kind() != "comment" {
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
            if t.starts_with('*') { t[1..].trim_start().to_string() } else { t.to_string() }
        })
        .collect();
    if lines.is_empty() || lines.iter().all(|l| l.is_empty()) {
        return None;
    }
    Some(lines.join("\n").trim().to_string())
}
