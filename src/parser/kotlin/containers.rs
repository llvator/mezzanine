//! Container parsers: classes (regular/data/sealed/annotation/abstract),
//! interfaces, enums, objects, and companion objects.
//!
//! Class declarations are dispatched here through `handle_class_declaration`
//! because the same `class_declaration` node may represent any of class /
//! interface / enum / data / sealed / annotation depending on its modifiers.

use super::super::language_parser::{find_child_by_kind, node_text, node_to_span};
use super::helpers::{
    extract_type_identifier, extract_type_name, parse_class_parameters, parse_generics,
};
use super::kdoc::extract_kdoc;
use super::modifiers::{parse_modifier_attributes, parse_visibility};
use super::ExtractCtx;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use std::path::Path;
use tree_sitter::Node;

/// Recursion target supplied by the dispatcher in `mod.rs`.
pub(super) type ExtractInto = fn(Node, Option<&str>, &mut ExtractCtx<'_>);

/// Internal classification of Kotlin class declarations.
pub(super) enum DetectedClassKind {
    Class,
    Interface,
    Enum,
    Data,
    Sealed,
    Annotation,
}

pub(super) fn handle_class_declaration(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    extract_entities: ExtractInto,
) {
    let class_kind = detect_class_kind(node);

    match class_kind {
        DetectedClassKind::Interface => {
            if let Some(entity) = parse_interface(node, ctx.source, ctx.path, parent_id, ctx.package) {
                let entity_id = entity.id.clone();
                ctx.result.add_entity(entity);
                if let Some(body) = find_child_by_kind(node, "class_body") {
                    extract_entities(body, Some(&entity_id), ctx);
                }
            }
        }
        DetectedClassKind::Enum => {
            if let Some(entity) = parse_enum(node, ctx.source, ctx.path, parent_id, ctx.package) {
                let entity_id = entity.id.clone();
                ctx.result.add_entity(entity);
                // Enum body may contain methods
                if let Some(body) = find_child_by_kind(node, "enum_class_body") {
                    extract_entities(body, Some(&entity_id), ctx);
                }
            }
        }
        _ => {
            // Regular class, data class, sealed class, annotation class
            if let Some(entity) = parse_class(node, ctx.source, ctx.path, parent_id, ctx.package, &class_kind) {
                let entity_id = entity.id.clone();
                ctx.result.add_entity(entity);
                if let Some(body) = find_child_by_kind(node, "class_body") {
                    extract_entities(body, Some(&entity_id), ctx);
                }
            }
        }
    }
}

pub(super) fn handle_object_declaration(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    extract_entities: ExtractInto,
) {
    if let Some(entity) = parse_object(node, ctx.source, ctx.path, parent_id, ctx.package) {
        let entity_id = entity.id.clone();
        ctx.result.add_entity(entity);
        if let Some(body) = find_child_by_kind(node, "class_body") {
            extract_entities(body, Some(&entity_id), ctx);
        }
    }
}

pub(super) fn handle_companion_object(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    extract_entities: ExtractInto,
) {
    if let Some(entity) = parse_companion_object(node, ctx.source, ctx.path, parent_id, ctx.package) {
        let entity_id = entity.id.clone();
        ctx.result.add_entity(entity);
        if let Some(body) = find_child_by_kind(node, "class_body") {
            extract_entities(body, Some(&entity_id), ctx);
        }
    } else {
        // Even if we couldn't create an entity for the companion, recurse
        // into its body with the parent_id so methods/properties attach
        // to the enclosing class.
        if let Some(body) = find_child_by_kind(node, "class_body") {
            extract_entities(body, parent_id, ctx);
        }
    }
}

fn parse_class(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
    class_kind: &DetectedClassKind,
) -> Option<CodeEntity> {
    let name = extract_type_identifier(node, source)?;
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

    // Add tags for Kotlin-specific class modifiers
    match class_kind {
        DetectedClassKind::Data => { entity.tags.insert("data".to_string()); }
        DetectedClassKind::Sealed => { entity.tags.insert("sealed".to_string()); }
        DetectedClassKind::Annotation => { entity.tags.insert("annotation".to_string()); }
        _ => {}
    }

    // Type parameters
    if let Some(tp) = find_child_by_kind(node, "type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    // Primary constructor parameters as fields (val/var class_parameter)
    if let Some(ctor) = find_child_by_kind(node, "primary_constructor") {
        entity.fields = parse_class_parameters(&ctor, source);
    }

    // Delegation specifiers for extends/implements
    extract_supertypes(node, source, &mut entity);

    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn parse_interface(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name = extract_type_identifier(node, source)?;
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Interface, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    if let Some(tp) = find_child_by_kind(node, "type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    // Interfaces can extend other interfaces via delegation_specifier
    extract_supertypes(node, source, &mut entity);

    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn parse_enum(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name = extract_type_identifier(node, source)?;
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Enum, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    // Extract enum entries as fields
    if let Some(body) = find_child_by_kind(node, "enum_class_body") {
        entity.fields = extract_enum_entries(&body, source);
    }

    extract_supertypes(node, source, &mut entity);

    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn parse_object(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    let name = extract_type_identifier(node, source)?;
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Class, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.tags.insert("object".to_string());

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    extract_supertypes(node, source, &mut entity);

    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn parse_companion_object(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    package: &str,
) -> Option<CodeEntity> {
    // Companion objects may have an explicit name via type_identifier,
    // otherwise they are anonymous "Companion".
    let name = extract_type_identifier(node, source)
        .unwrap_or_else(|| "Companion".to_string());
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Class, path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);
    entity.tags.insert("companion".to_string());
    entity.tags.insert("object".to_string());

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

/// Extract extends/implements from `delegation_specifier` children.
/// In Kotlin, all supertypes appear as `delegation_specifier` nodes after `:`.
/// A `constructor_invocation` indicates a class (extends), while a plain
/// `user_type` indicates an interface (implements).
fn extract_supertypes(node: &Node, source: &str, entity: &mut CodeEntity) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "delegation_specifier" {
            let mut inner = child.walk();
            for ds_child in child.children(&mut inner) {
                match ds_child.kind() {
                    "constructor_invocation" => {
                        // This is a class being extended (parent constructor call)
                        if let Some(ut) = find_child_by_kind(&ds_child, "user_type") {
                            let type_name = extract_type_name(&ut, source);
                            entity.extends.push(type_name);
                        }
                    }
                    "user_type" => {
                        // This is an interface being implemented
                        let type_name = extract_type_name(&ds_child, source);
                        entity.implements.push(type_name);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn extract_enum_entries(body: &Node, source: &str) -> Vec<Parameter> {
    let mut entries = Vec::new();
    let mut cursor = body.walk();

    for child in body.children(&mut cursor) {
        if child.kind() == "enum_entry" {
            let name = find_child_by_kind(&child, "simple_identifier")
                .map(|n| node_text(&n, source).to_string())
                .unwrap_or_default();
            let type_name = find_child_by_kind(&child, "value_arguments")
                .map(|a| node_text(&a, source).to_string());

            entries.push(Parameter {
                name,
                type_name,
                default_value: None,
                visibility: None,
            });
        }
    }

    entries
}

/// Detect what kind of class declaration this is by checking for specific
/// keyword children (`interface`, `enum`, `class`) and modifiers.
fn detect_class_kind(node: &Node) -> DetectedClassKind {
    let mut has_interface = false;
    let mut has_enum = false;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "interface" => has_interface = true,
            "enum" => has_enum = true,
            "modifiers" => {
                let mut mc = child.walk();
                for mod_child in child.children(&mut mc) {
                    if mod_child.kind() == "class_modifier" {
                        let mut inner = mod_child.walk();
                        for leaf in mod_child.children(&mut inner) {
                            match leaf.kind() {
                                "data" => return DetectedClassKind::Data,
                                "sealed" => return DetectedClassKind::Sealed,
                                "annotation" => return DetectedClassKind::Annotation,
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if has_interface {
        DetectedClassKind::Interface
    } else if has_enum {
        DetectedClassKind::Enum
    } else {
        DetectedClassKind::Class
    }
}
