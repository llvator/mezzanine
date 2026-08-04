//! Properties and type aliases — non-callable leaf entities.

use super::super::language_parser::{find_child_by_kind, node_text, node_to_span};
use super::helpers::{extract_type_identifier, has_child_kind};
use super::kdoc::extract_kdoc;
use super::modifiers::{parse_modifier_attributes, parse_visibility};
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_property(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    // Property has a `variable_declaration` child with simple_identifier and optional type
    let var_decl = find_child_by_kind(node, "variable_declaration")?;
    let name_node = find_child_by_kind(&var_decl, "simple_identifier")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Property, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);

    // Type annotation on the variable_declaration
    entity.return_type = extract_variable_type(&var_decl, source);

    // Check if it's val or var
    if has_child_kind(node, "binding_pattern_kind") {
        let bpk = find_child_by_kind(node, "binding_pattern_kind").unwrap();
        if has_child_kind(&bpk, "var") {
            entity.tags.insert("mutable".to_string());
        }
    }

    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_type_alias(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name = extract_type_identifier(node, source)?;
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::TypeAlias, path, span);
    entity.visibility = parse_visibility(node);
    entity.parent_id = parent_id.map(String::from);
    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

/// Extract the type from a `variable_declaration` node (after `:`).
fn extract_variable_type(var_decl: &Node, source: &str) -> Option<String> {
    let mut cursor = var_decl.walk();
    let mut found_colon = false;

    for child in var_decl.children(&mut cursor) {
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
