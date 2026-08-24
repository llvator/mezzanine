//! Small shared helpers: parameter / generics parsing and the various
//! `extract_*` text-fetching utilities.
//!
//! Nothing here reads modifiers. A declaration's visibility belongs to the
//! declaration, so the one parser that needed it — a class's primary
//! constructor — sits in [`super::containers`] with the class it describes,
//! and this module stays under every caller that does read modifiers.

use super::super::language_parser::{find_child_by_kind, node_text};
use crate::models::entity::Parameter;
use tree_sitter::Node;

pub(super) fn parse_parameters(node: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "parameter" {
            let name = find_child_by_kind(&child, "simple_identifier")
                .map(|n| node_text(&n, source).to_string())
                .unwrap_or_default();

            // Type is the user_type / nullable_type after `:`
            let type_name = extract_parameter_type(&child, source);

            // Default value — check for `=` followed by an expression
            let default_value = extract_default_value(&child, source);

            params.push(Parameter {
                name,
                type_name,
                default_value,
                visibility: None,
            });
        }
    }

    params
}

pub(super) fn parse_generics(node: &Node, source: &str) -> Vec<String> {
    let mut generics = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "type_parameter" {
            generics.push(node_text(&child, source).to_string());
        }
    }

    generics
}

pub(super) fn has_child_kind(node: &Node, kind: &str) -> bool {
    find_child_by_kind(node, kind).is_some()
}

/// Extract the `type_identifier` text from a node.
pub(super) fn extract_type_identifier(node: &Node, source: &str) -> Option<String> {
    find_child_by_kind(node, "type_identifier").map(|n| node_text(&n, source).to_string())
}

/// Extract the `simple_identifier` text from a node.
pub(super) fn extract_simple_identifier(node: &Node, source: &str) -> Option<String> {
    find_child_by_kind(node, "simple_identifier").map(|n| node_text(&n, source).to_string())
}

/// Extract the full type name from a `user_type` node, including generics.
pub(super) fn extract_type_name(user_type: &Node, source: &str) -> String {
    node_text(user_type, source).to_string()
}

/// Extract the type from a `parameter` or `class_parameter` node (after `:`).
pub(super) fn extract_parameter_type(param: &Node, source: &str) -> Option<String> {
    let mut cursor = param.walk();
    let mut found_colon = false;

    for child in param.children(&mut cursor) {
        if child.kind() == ":" {
            found_colon = true;
        } else if found_colon {
            match child.kind() {
                "user_type" | "nullable_type" | "function_type" => {
                    return Some(node_text(&child, source).to_string());
                }
                _ => {}
            }
        }
    }

    None
}

/// Extract the default value from a parameter (the expression after `=`).
pub(super) fn extract_default_value(param: &Node, source: &str) -> Option<String> {
    let mut cursor = param.walk();
    let mut found_eq = false;

    for child in param.children(&mut cursor) {
        if child.kind() == "=" {
            found_eq = true;
        } else if found_eq {
            // The next non-punctuation child is the default value expression
            let text = node_text(&child, source).to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    None
}
