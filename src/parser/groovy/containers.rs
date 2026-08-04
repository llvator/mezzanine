//! Container-entity parsers: classes, interfaces, enums.

use super::super::language_parser::{find_child_by_kind, node_text, node_to_span};
use super::helpers::{
    extract_first_type, extract_type_list, parse_generics, parse_modifier_attributes,
    parse_visibility,
};
use super::javadoc::extract_javadoc;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

/// Qualify a container with its file's `package` declaration, matching
/// the Java parser so a Groovy class and a Java class in the same package
/// carry the same qualified name and resolve to each other.
fn qualify(entity: &mut CodeEntity, package: &str, name: &str) {
    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }
}

/// Record `extends` / `implements` so the resolver can build the
/// inheritance edges. Groovy's grammar exposes both under the same node
/// names as Java's (`superclass`, `super_interfaces`), so the extraction
/// is the same; `interfaces` is the field name Groovy uses for the
/// wrapper, but the node kind is what we search on.
fn record_supertypes(entity: &mut CodeEntity, node: &Node, source: &str) {
    if let Some(sc) = find_child_by_kind(node, "superclass") {
        if let Some(base) = extract_first_type(&sc, source) {
            entity.extends.push(base);
        }
    }
    if let Some(si) = find_child_by_kind(node, "super_interfaces") {
        entity.implements = extract_type_list(&si, source);
    }
}

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
    let kind = if is_abstract {
        EntityKind::AbstractClass
    } else {
        EntityKind::Class
    };
    let mut entity = CodeEntity::new(&name, kind, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = attributes;
    entity.parent_id = parent_id.map(String::from);

    if is_abstract {
        entity.tags.insert("abstract".to_string());
    }

    qualify(&mut entity, package, &name);
    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }
    record_supertypes(&mut entity, node, source);

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

    qualify(&mut entity, package, &name);
    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }
    // An interface extends other interfaces — Groovy, like Java, allows
    // more than one, so the whole list lands in `extends`.
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

    qualify(&mut entity, package, &name);
    if let Some(si) = find_child_by_kind(node, "super_interfaces") {
        entity.implements = extract_type_list(&si, source);
    }

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}
