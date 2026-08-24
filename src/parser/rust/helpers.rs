//! What every declaration parser reads off a grammar node, whatever kind of
//! item it is building:
//! - visibility parsing
//! - base type-name extraction (strips generics / refs)
//! - generic parameter names

use super::super::language_parser::node_text;
use crate::models::Visibility;
use tree_sitter::Node;

pub(super) fn parse_visibility(node: &Node, source: &str) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let text = node_text(&child, source);
            return match text {
                "pub" => Visibility::Public,
                t if t.starts_with("pub(crate)") => Visibility::Crate,
                t if t.starts_with("pub(super)") => Visibility::Internal,
                t if t.starts_with("pub(in") => Visibility::Internal,
                _ => Visibility::Public,
            };
        }
    }
    Visibility::Private
}

/// Extract the base type name from a type node, stripping generic and
/// lifetime arguments. Handles `generic_type`, `scoped_type_identifier`,
/// and reference/pointer wrappers by descending into them.
///
/// The node-shaped counterpart to [`crate::parser::rust_type_names::base_type_name`], and
/// named apart from it deliberately: they take different input and promise
/// different things — this one always yields a name, because an `impl` head
/// always names a type, while the text one declines a primitive.
pub(super) fn parse_base_type_name(node: &Node, source: &str) -> String {
    match node.kind() {
        "generic_type" => {
            if let Some(inner) = node.child_by_field_name("type") {
                return parse_base_type_name(&inner, source);
            }
        }
        "reference_type" | "pointer_type" => {
            if let Some(inner) = node.child_by_field_name("type") {
                return parse_base_type_name(&inner, source);
            }
        }
        _ => {}
    }
    let text = node_text(node, source);
    text.split('<').next().unwrap_or(text).trim().to_string()
}

/// Collect generic parameter names from a `type_parameters` node.
pub(super) fn parse_generics(node: &Node, source: &str) -> Vec<String> {
    let mut generics = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "type_identifier" || child.kind() == "constrained_type_parameter" {
            generics.push(node_text(&child, source).to_string());
        }
    }

    generics
}
