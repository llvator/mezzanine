//! Rust function and trait-method parsing — including parameters, generics,
//! body metrics, and call extraction wiring.

use super::super::language_parser::{node_text, node_to_span};
use super::calls::extract_calls;
use super::complexity::{compute_complexity, count_return_tuple_elements};
use super::doc_comments::extract_doc_comment;
use super::helpers::{parse_generics, parse_visibility};
use super::inference::{infer_local_types, infer_param_types, TypeEnv};
use super::ExtractCtx;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

/// Parse a function, add it, and extract its call relationships.
/// `self_type` qualifies `Self::foo`/`self.foo()` targets inside impl blocks.
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
            // Parameters first, then locals — a `let` rebinding shadows the
            // parameter of the same name, so locals win.
            let mut locals = infer_param_types(node, ctx.source);
            locals.extend(infer_local_types(&body, ctx.source));
            let env = TypeEnv { locals, fields: ctx.struct_fields, self_type };
            let mut call_order = 0u32;
            extract_calls(
                &body,
                ctx.source,
                &caller_id,
                &caller_name,
                &env,
                &mut call_order,
                ctx.result,
            );
        }
    }
}

pub(super) fn parse_function(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Function, path, span);
    entity.visibility = parse_visibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.documentation = extract_doc_comment(node, source);

    // Extract parameters
    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    // Extract return type + return complexity (tuple element count).
    if let Some(ret) = node.child_by_field_name("return_type") {
        entity.return_type = Some(node_text(&ret, source).trim_start_matches("->").trim().to_string());
        entity.metrics.return_complexity = count_return_tuple_elements(&ret);
    }

    // Extract generics
    if let Some(generics) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&generics, source);
    }

    // Metrics: LOC, parameter count, cyclomatic, nesting depth.
    // Computed once here so renderers never have to recompute.
    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    entity.metrics.param_count = Some(
        entity
            .parameters
            .iter()
            .filter(|p| p.name != "self" && p.name != "&self" && p.name != "&mut self")
            .count() as u32,
    );
    if let Some(body) = node.child_by_field_name("body") {
        let (cc, nesting, cog) = compute_complexity(&body);
        entity.metrics.cyclomatic = Some(cc);
        entity.metrics.max_nesting = Some(nesting);
        entity.metrics.cognitive_complexity = Some(cog);
    } else {
        entity.metrics.cyclomatic = Some(1);
        entity.metrics.max_nesting = Some(0);
        entity.metrics.cognitive_complexity = Some(0);
    }

    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_trait_method(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Method, path, span);
    entity.parent_id = parent_id.map(String::from);
    entity.documentation = extract_doc_comment(node, source);

    // Extract parameters
    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    // Extract return type
    if let Some(ret) = node.child_by_field_name("return_type") {
        entity.return_type = Some(node_text(&ret, source).trim_start_matches("->").trim().to_string());
    }

    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    entity.metrics.param_count = Some(
        entity
            .parameters
            .iter()
            .filter(|p| p.name != "self" && p.name != "&self" && p.name != "&mut self")
            .count() as u32,
    );
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_parameters(node: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "parameter" | "self_parameter" => {
                let text = node_text(&child, source);
                let parts: Vec<&str> = text.splitn(2, ':').collect();
                let name = parts[0].trim().to_string();
                let type_name = parts.get(1).map(|t| t.trim().to_string());

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
