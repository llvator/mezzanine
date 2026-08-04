//! Shared parsing helpers: visibility, modifier attributes, parameters,
//! generics, and type-list extraction.

use super::super::language_parser::{find_child_by_kind, node_text};
use crate::models::entity::Parameter;
use crate::models::Visibility;
use tree_sitter::Node;

pub(super) fn parse_visibility(node: &Node) -> Visibility {
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            match child.kind() {
                "public" => return Visibility::Public,
                "private" => return Visibility::Private,
                "protected" => return Visibility::Protected,
                _ => {}
            }
        }
    }
    // No access modifier in Java means package-private
    Visibility::Internal
}

pub(super) fn parse_modifier_attributes(node: &Node, source: &str) -> Vec<String> {
    let mut attrs = Vec::new();
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            match child.kind() {
                // Skip access modifiers (handled by parse_visibility)
                "public" | "private" | "protected" => {}
                "static" | "final" | "abstract" | "synchronized"
                | "native" | "transient" | "volatile" | "default"
                | "strictfp" | "sealed" | "non-sealed" => {
                    attrs.push(child.kind().to_string());
                }
                "marker_annotation" | "annotation" => {
                    attrs.push(node_text(&child, source).to_string());
                }
                _ => {}
            }
        }
    }
    attrs
}

pub(super) fn parse_parameters(node: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "formal_parameter" => {
                let name = child.child_by_field_name("name")
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                let type_name = child.child_by_field_name("type")
                    .map(|t| node_text(&t, source).to_string());

                params.push(Parameter {
                    name,
                    type_name,
                    default_value: None,
                    visibility: None,
                });
            }
            "spread_parameter" => {
                // The grammar gives spread_parameter no named fields:
                // seq(modifiers?, _unannotated_type, "...", variable_declarator).
                let mut inner = child.walk();
                let name = child
                    .children(&mut inner)
                    .find(|c| c.kind() == "variable_declarator")
                    .and_then(|d| d.child_by_field_name("name"))
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                let mut inner = child.walk();
                let type_name = child
                    .children(&mut inner)
                    .find(|c| {
                        c.is_named()
                            && c.kind() != "modifiers"
                            && c.kind() != "variable_declarator"
                    })
                    .map(|t| format!("{}...", node_text(&t, source)));

                params.push(Parameter {
                    name,
                    type_name,
                    default_value: None,
                    visibility: None,
                });
            }
            _ => {}
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

/// Extract the first type from a wrapper node (e.g., `superclass`).
pub(super) fn extract_first_type(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" | "scoped_type_identifier" | "generic_type" => {
                return Some(node_text(&child, source).to_string());
            }
            _ => {}
        }
    }
    None
}

/// Extract a list of types from a wrapper node (e.g., `super_interfaces`).
pub(super) fn extract_type_list(node: &Node, source: &str) -> Vec<String> {
    let mut types = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" | "scoped_type_identifier" | "generic_type" => {
                types.push(node_text(&child, source).to_string());
            }
            "type_list" => {
                let mut inner = child.walk();
                for inner_child in child.children(&mut inner) {
                    match inner_child.kind() {
                        "type_identifier" | "scoped_type_identifier" | "generic_type" => {
                            types.push(node_text(&inner_child, source).to_string());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    types
}
