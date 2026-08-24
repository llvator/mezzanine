//! Container parsers: classes (regular/data/sealed/annotation/abstract),
//! interfaces, enums, objects, and companion objects.
//!
//! Class declarations are dispatched here through `handle_class_declaration`
//! because the same `class_declaration` node may represent any of class /
//! interface / enum / data / sealed / annotation depending on its modifiers.

use super::super::ctx::{Descent, ExtractCtx};
use super::super::helpers::{
    extract_default_value, extract_parameter_type, extract_type_identifier, extract_type_name,
    parse_generics,
};
use super::super::kdoc::extract_kdoc;
use super::super::modifiers::{parse_modifier_attributes, parse_visibility};
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{find_child_by_kind, node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

/// Internal classification of Kotlin class declarations.
pub(super) enum DetectedClassKind {
    Class,
    Interface,
    Enum,
    Data,
    Sealed,
    Annotation,
}

pub(super) fn handle_class_declaration<'t>(
    node: &Node<'t>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let class_kind = detect_class_kind(node);

    // An enum keeps its methods in an `enum_class_body`; everything else in a
    // plain `class_body`.
    let (parsed, body_kind) = match class_kind {
        DetectedClassKind::Interface => (
            parse_interface(node, ctx.source, ctx.path, parent_id, ctx.package),
            "class_body",
        ),
        DetectedClassKind::Enum => (
            parse_enum(node, ctx.source, ctx.path, parent_id, ctx.package),
            "enum_class_body",
        ),
        // Regular class, data class, sealed class, annotation class
        _ => (
            parse_class(
                node,
                ctx.source,
                ctx.path,
                parent_id,
                ctx.package,
                &class_kind,
            ),
            "class_body",
        ),
    };

    register_then_descend(node, body_kind, parsed, ctx)
}

pub(super) fn handle_object_declaration<'t>(
    node: &Node<'t>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let parsed = parse_object(node, ctx.source, ctx.path, parent_id, ctx.package);
    register_then_descend(node, "class_body", parsed, ctx)
}

pub(super) fn handle_companion_object<'t>(
    node: &Node<'t>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let owner = match parse_companion_object(node, ctx.source, ctx.path, parent_id, ctx.package) {
        Some(entity) => {
            let entity_id = entity.id.clone();
            ctx.result.add_entity(entity);
            Some(entity_id)
        }
        // Even if we couldn't create an entity for the companion, its body is
        // still walked with the parent_id so methods/properties attach to the
        // enclosing class.
        None => parent_id.map(String::from),
    };

    body_of(node, "class_body", owner)
}

/// Register a parsed container and hand its body back to the dispatcher, which
/// owns the descent. A container that didn't parse leaves nothing to walk.
fn register_then_descend<'t>(
    node: &Node<'t>,
    body_kind: &str,
    parsed: Option<CodeEntity>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let Some(entity) = parsed else {
        return Descent::Stop;
    };
    let entity_id = entity.id.clone();
    ctx.result.add_entity(entity);
    body_of(node, body_kind, Some(entity_id))
}

fn body_of<'t>(node: &Node<'t>, body_kind: &str, owner: Option<String>) -> Descent<'t> {
    match find_child_by_kind(node, body_kind) {
        Some(body) => Descent::Into { owner, body },
        None => Descent::Stop,
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

    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }

    // Add tags for Kotlin-specific class modifiers
    match class_kind {
        DetectedClassKind::Data => {
            entity.tags.insert("data".to_string());
        }
        DetectedClassKind::Sealed => {
            entity.tags.insert("sealed".to_string());
        }
        DetectedClassKind::Annotation => {
            entity.tags.insert("annotation".to_string());
        }
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
    let name = extract_type_identifier(node, source).unwrap_or_else(|| "Companion".to_string());
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

/// Parse `class_parameter` nodes from a `primary_constructor`.
/// These are val/var parameters that become class fields.
fn parse_class_parameters(ctor_node: &Node, source: &str) -> Vec<Parameter> {
    let mut fields = Vec::new();
    let mut cursor = ctor_node.walk();

    for child in ctor_node.children(&mut cursor) {
        if child.kind() == "class_parameter" {
            let name = find_child_by_kind(&child, "simple_identifier")
                .map(|n| node_text(&n, source).to_string())
                .unwrap_or_default();

            let type_name = extract_parameter_type(&child, source);
            let default_value = extract_default_value(&child, source);

            // Determine visibility from modifiers on the class_parameter
            let visibility = if find_child_by_kind(&child, "modifiers").is_some() {
                Some(parse_visibility(&child))
            } else {
                None
            };

            fields.push(Parameter {
                name,
                type_name,
                default_value,
                visibility,
            });
        }
    }

    fields
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
