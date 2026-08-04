//! Container-entity parsers: classes, interfaces, enums, annotation types,
//! and records.

use super::super::language_parser::{find_child_by_kind, node_text, node_to_span};
use super::helpers::{
    extract_first_type, extract_type_list, parse_generics, parse_modifier_attributes,
    parse_parameters, parse_visibility,
};
use super::javadoc::extract_javadoc;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_class(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let attributes = parse_modifier_attributes(node, source);
    let is_abstract = attributes.contains(&"abstract".to_string());
    let kind = if is_abstract { EntityKind::AbstractClass } else { EntityKind::Class };
    let mut entity = CodeEntity::new(&name, kind, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = attributes;
    entity.parent_id = parent_id.map(String::from);

    if is_abstract {
        entity.tags.insert("abstract".to_string());
    }

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    if let Some(sc) = find_child_by_kind(node, "superclass") {
        if let Some(base) = extract_first_type(&sc, source) {
            entity.extends.push(base);
        }
    }

    if let Some(si) = find_child_by_kind(node, "super_interfaces") {
        entity.implements = extract_type_list(&si, source);
    }

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_interface(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Interface, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    // Interfaces extend other interfaces — stored in extends (Java allows multiple)
    if let Some(ext) = find_child_by_kind(node, "extends_interfaces") {
        entity.extends = extract_type_list(&ext, source);
    }

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_enum(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Enum, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    if let Some(si) = find_child_by_kind(node, "super_interfaces") {
        entity.implements = extract_type_list(&si, source);
    }

    // Extract enum constants as fields
    if let Some(body) = node.child_by_field_name("body") {
        entity.fields = extract_enum_constants(&body, source);
    }

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_annotation_type(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Interface, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.tags.insert("annotation".to_string());

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_record(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Class, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.tags.insert("record".to_string());

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    // Record components from formal_parameters
    if let Some(params) = node.child_by_field_name("parameters") {
        entity.fields = parse_parameters(&params, source);
    }

    if let Some(si) = find_child_by_kind(node, "super_interfaces") {
        entity.implements = extract_type_list(&si, source);
    }

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn extract_enum_constants(body: &Node, source: &str) -> Vec<Parameter> {
    let mut constants = Vec::new();
    let mut cursor = body.walk();

    for child in body.children(&mut cursor) {
        if child.kind() == "enum_constant" {
            let name = child.child_by_field_name("name")
                .map(|n| node_text(&n, source).to_string())
                .unwrap_or_default();
            let type_name = child.child_by_field_name("arguments")
                .map(|a| node_text(&a, source).to_string());

            constants.push(Parameter {
                name,
                type_name,
                default_value: None,
                visibility: None,
            });
        }
    }

    constants
}
