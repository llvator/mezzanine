//! Container parsers: classes (regular and abstract), interfaces, and enums.
//!
//! Container `parse_*` functions live here; the dispatcher in `mod.rs`
//! handles recursion into class/interface bodies after adding the entity.

use super::super::language_parser::{find_child_by_kind, node_text, node_to_span};
use super::decorators::extract_decorators;
use super::helpers::parse_generics;
use super::tsdoc::extract_tsdoc;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_class(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let is_abstract = node.kind() == "abstract_class_declaration";
    let kind = if is_abstract { EntityKind::AbstractClass } else { EntityKind::Class };
    let mut entity = CodeEntity::new(&name, kind, path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    if is_abstract {
        entity.tags.insert("abstract".to_string());
        entity.attributes.push("abstract".to_string());
    }

    // Decorators (child nodes on class declarations)
    extract_decorators(node, source, &mut entity.attributes);

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    extract_class_heritage(node, source, &mut entity);

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_interface(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Interface, path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    // Interfaces extend via extends_type_clause (sibling, not field)
    extract_interface_heritage(node, source, &mut entity);

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_enum(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Enum, path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    // Check for `const enum`
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "const" {
            entity.tags.insert("const".to_string());
            entity.attributes.push("const".to_string());
            break;
        }
    }

    if let Some(body) = node.child_by_field_name("body") {
        entity.fields = extract_enum_members(&body, source);
    }

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn extract_enum_members(body: &Node, source: &str) -> Vec<Parameter> {
    let mut members = Vec::new();
    let mut cursor = body.walk();

    for child in body.children(&mut cursor) {
        match child.kind() {
            "enum_assignment" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                let value = child
                    .child_by_field_name("value")
                    .map(|v| node_text(&v, source).to_string());
                members.push(Parameter {
                    name,
                    type_name: None,
                    default_value: value,
                    visibility: None,
                });
            }
            // Enum member without initializer (aliased from property_identifier)
            "property_identifier" | "enum_member" => {
                let name = node_text(&child, source).to_string();
                members.push(Parameter {
                    name,
                    type_name: None,
                    default_value: None,
                    visibility: None,
                });
            }
            _ => {}
        }
    }

    members
}

fn extract_class_heritage(node: &Node, source: &str, entity: &mut CodeEntity) {
    let heritage = match find_child_by_kind(node, "class_heritage") {
        Some(h) => h,
        None => return,
    };
    let mut cursor = heritage.walk();
    for child in heritage.children(&mut cursor) {
        match child.kind() {
            "extends_clause" => {
                let mut ec = child.walk();
                for ec_child in child.children(&mut ec) {
                    if ec_child.is_named() && ec_child.kind() != "type_arguments" {
                        entity.extends.push(node_text(&ec_child, source).to_string());
                    }
                }
            }
            "implements_clause" => {
                let mut ic = child.walk();
                for ic_child in child.children(&mut ic) {
                    if ic_child.is_named() {
                        entity.implements.push(node_text(&ic_child, source).to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

fn extract_interface_heritage(node: &Node, source: &str, entity: &mut CodeEntity) {
    let extends = match find_child_by_kind(node, "extends_type_clause") {
        Some(e) => e,
        None => return,
    };
    let mut cursor = extends.walk();
    for child in extends.children(&mut cursor) {
        if child.is_named() {
            entity.extends.push(node_text(&child, source).to_string());
        }
    }
}
