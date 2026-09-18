//! Top-level function declarations (including generator functions).

use super::super::bodies::calls::extract_body_calls;
use super::super::bodies::complexity::populate_body_metrics;
use super::super::ctx::ExtractCtx;
use super::super::helpers::{extract_type_text, parse_generics, parse_parameters};
use super::super::tsdoc::extract_tsdoc;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn handle_function(
    node: &Node,
    parent_id: Option<&str>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    if let Some(entity) = parse_function(node, ctx.source, ctx.path, parent_id) {
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

fn parse_function(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let kind = if parent_id.is_some() {
        EntityKind::Method
    } else {
        EntityKind::Function
    };

    let mut entity = CodeEntity::new(&name, kind, path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "async" {
            entity.tags.insert("async".to_string());
            entity.attributes.push("async".to_string());
            break;
        }
    }

    if node.kind() == "generator_function_declaration" {
        entity.tags.insert("generator".to_string());
        entity.attributes.push("generator".to_string());
    }

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    if let Some(ret) = node.child_by_field_name("return_type") {
        entity.return_type = Some(extract_type_text(&ret, source));
    }

    populate_body_metrics(node.child_by_field_name("body"), source, &mut entity);

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}
