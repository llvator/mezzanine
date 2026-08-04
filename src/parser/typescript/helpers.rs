//! Shared helpers: visibility, parameters, generics, and type-text trimming.

use super::super::language_parser::node_text;
use crate::models::entity::Parameter;
use crate::models::Visibility;
use tree_sitter::Node;

pub(super) fn parse_accessibility(node: &Node, source: &str) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "accessibility_modifier" {
            let text = node_text(&child, source);
            return match text {
                "public" => Visibility::Public,
                "private" => Visibility::Private,
                "protected" => Visibility::Protected,
                _ => Visibility::Public,
            };
        }
    }
    // TypeScript default is public
    Visibility::Public
}

pub(super) fn parse_parameters(node: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "required_parameter" | "optional_parameter" => {
                let name = child
                    .child_by_field_name("pattern")
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                let type_name = child
                    .child_by_field_name("type")
                    .map(|t| extract_type_text(&t, source));
                let default_value = child
                    .child_by_field_name("value")
                    .map(|v| node_text(&v, source).to_string());

                params.push(Parameter {
                    name,
                    type_name,
                    default_value,
                    visibility: None,
                });
            }
            "rest_parameter" => {
                let name = child
                    .child_by_field_name("pattern")
                    .map(|n| format!("...{}", node_text(&n, source)))
                    .unwrap_or_default();
                let type_name = child
                    .child_by_field_name("type")
                    .map(|t| extract_type_text(&t, source));

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

/// Extract just the type portion from a `type_annotation` node (`: Type` → `Type`).
pub(super) fn extract_type_text(type_ann: &Node, source: &str) -> String {
    node_text(type_ann, source)
        .trim_start_matches(':')
        .trim()
        .to_string()
}
