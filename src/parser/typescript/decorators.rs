//! Decorator collection — both child-style (on class declarations) and
//! sibling-style (preceding methods/fields in a class body).

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// Collect decorator child nodes (for class declarations where decorators
/// are fields of the node itself).
pub(super) fn extract_decorators(node: &Node, source: &str, attrs: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "decorator" {
            attrs.push(node_text(&child, source).to_string());
        }
    }
}

/// Collect decorator nodes that precede the current node as siblings
/// (for methods/fields in class body).
pub(super) fn extract_sibling_decorators(node: &Node, source: &str, attrs: &mut Vec<String>) {
    let mut current = node.prev_sibling();
    while let Some(sib) = current {
        if sib.kind() == "decorator" {
            attrs.push(node_text(&sib, source).to_string());
            current = sib.prev_sibling();
        } else {
            break;
        }
    }
}
