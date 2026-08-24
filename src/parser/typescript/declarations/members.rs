//! Class and interface body members:
//! - methods (`method_definition`) and abstract method signatures
//! - fields (`public_field_definition`)
//! - interface property + method signatures

use super::super::bodies::calls::extract_body_calls;
use super::super::bodies::complexity::populate_body_metrics;
use super::super::ctx::ExtractCtx;
use super::super::decorators::extract_sibling_decorators;
use super::super::helpers::{
    extract_type_text, parse_accessibility, parse_generics, parse_parameters,
};
use super::super::tsdoc::extract_tsdoc;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn handle_method(
    node: &Node,
    parent_id: Option<&str>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    if let Some(entity) = parse_method(node, ctx.source, ctx.path, parent_id) {
        let caller_id = entity.id.clone();
        let caller_name = entity.name.clone();
        ctx.result.add_entity(entity);
        if let Some(body) = node.child_by_field_name("body") {
            extract_body_calls(
                &body,
                &caller_id,
                &caller_name,
                node.child_by_field_name("parameters"),
                self_type,
                ctx,
            );
        }
    }
}

fn parse_method(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Method, path, span);
    entity.visibility = parse_accessibility(node, source);
    entity.parent_id = parent_id.map(String::from);

    if name == "constructor" {
        entity.tags.insert("constructor".to_string());
    }

    // Modifiers: static, async, override, readonly, get, set
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "static" => {
                entity.attributes.push("static".to_string());
                entity.tags.insert("static".to_string());
            }
            "async" => {
                entity.attributes.push("async".to_string());
                entity.tags.insert("async".to_string());
            }
            "override" => {
                entity.attributes.push("override".to_string());
            }
            "readonly" => {
                entity.attributes.push("readonly".to_string());
            }
            "get" => {
                entity.attributes.push("get".to_string());
                entity.tags.insert("getter".to_string());
            }
            "set" => {
                entity.attributes.push("set".to_string());
                entity.tags.insert("setter".to_string());
            }
            _ => {}
        }
    }

    // Decorators from preceding siblings
    extract_sibling_decorators(node, source, &mut entity.attributes);

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    if let Some(ret) = node.child_by_field_name("return_type") {
        entity.return_type = Some(extract_type_text(&ret, source));
    }

    populate_body_metrics(node.child_by_field_name("body"), &mut entity);

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_abstract_method(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Method, path, span);
    entity.visibility = parse_accessibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.tags.insert("abstract".to_string());
    entity.attributes.push("abstract".to_string());
    // Bodyless by definition — metrics are filled in below, after the
    // parameter list is read, so `param_count` is right.

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    if let Some(ret) = node.child_by_field_name("return_type") {
        entity.return_type = Some(extract_type_text(&ret, source));
    }

    populate_body_metrics(None, &mut entity);

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_field(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Property, path, span);
    entity.visibility = parse_accessibility(node, source);
    entity.parent_id = parent_id.map(String::from);

    if let Some(type_ann) = node.child_by_field_name("type") {
        entity.return_type = Some(extract_type_text(&type_ann, source));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "static" => {
                entity.attributes.push("static".to_string());
                entity.tags.insert("static".to_string());
            }
            "readonly" => {
                entity.attributes.push("readonly".to_string());
            }
            "override" => {
                entity.attributes.push("override".to_string());
            }
            "abstract" => {
                entity.attributes.push("abstract".to_string());
            }
            "?" => {
                entity.tags.insert("optional".to_string());
            }
            _ => {}
        }
    }

    extract_sibling_decorators(node, source, &mut entity.attributes);

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_property_signature(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Property, path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    if let Some(type_ann) = node.child_by_field_name("type") {
        entity.return_type = Some(extract_type_text(&type_ann, source));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "?" => {
                entity.tags.insert("optional".to_string());
            }
            "readonly" => {
                entity.attributes.push("readonly".to_string());
            }
            _ => {}
        }
    }

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_method_signature(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Method, path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    if let Some(ret) = node.child_by_field_name("return_type") {
        entity.return_type = Some(extract_type_text(&ret, source));
    }

    // An interface method signature has no body — one straight-through path.
    populate_body_metrics(None, &mut entity);

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}
