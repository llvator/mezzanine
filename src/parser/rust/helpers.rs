//! Small shared helpers used by multiple submodules:
//! - visibility parsing
//! - base type-name extraction (strips generics / refs)
//! - generic-argument stripping for path strings

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
pub(super) fn base_type_name(node: &Node, source: &str) -> String {
    match node.kind() {
        "generic_type" => {
            if let Some(inner) = node.child_by_field_name("type") {
                return base_type_name(&inner, source);
            }
        }
        "reference_type" | "pointer_type" => {
            if let Some(inner) = node.child_by_field_name("type") {
                return base_type_name(&inner, source);
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

/// Strip angle-bracketed generic arguments from a path string while preserving
/// `::` separators. `Foo<T>::bar<U>` → `Foo::bar`, `Self::new` → `Self::new`.
pub(super) fn strip_generics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth: i32 = 0;
    for ch in s.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}
