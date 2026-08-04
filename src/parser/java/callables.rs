//! Java method and constructor parsing, plus the dispatcher hook that
//! extracts call-site relationships from their bodies.

use super::super::language_parser::{node_text, node_to_span};
use super::calls::{extract_calls, CallCtx};
use super::complexity::compute_complexity;
use super::helpers::{
    parse_generics, parse_modifier_attributes, parse_parameters, parse_visibility,
};
use super::javadoc::extract_javadoc;
use super::ExtractCtx;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

/// Parse a method or constructor, add it, and extract call relationships
/// from its body.
pub(super) fn handle_callable(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse: fn(&Node, &str, &Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse(node, ctx.source, ctx.path, parent_id) {
        // Use the full entity ID as the relationship source so methods
        // with the same name in different classes/files don't collapse
        // onto a single node. The bare `entity_name` is kept only for
        // the self-recursion guard.
        let caller_id = entity.id.clone();
        let caller_name = entity.name.clone();
        // Resolve the parent class name for qualifying `this.method()`
        // and receiver-less calls to siblings within the same class.
        let parent_class_name = parent_id.and_then(|pid| {
            ctx.result.entities.iter().find(|e| e.id == pid).map(|e| e.name.clone())
        });
        ctx.result.add_entity(entity);
        if let Some(body) = node.child_by_field_name("body") {
            let mut call_order = 0u32;
            let mut arm_counter = 0u32;
            let mut loop_counter = 0u32;
            let mut call_ctx = CallCtx {
                source: ctx.source,
                path: ctx.path,
                caller_id: &caller_id,
                caller_name: &caller_name,
                parent_class: parent_class_name.as_deref(),
                call_order: &mut call_order,
                arm_counter: &mut arm_counter,
                loop_counter: &mut loop_counter,
                result: &mut *ctx.result,
            };
            extract_calls(&body, &mut call_ctx, None);
        }
    }
}

pub(super) fn parse_method(
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
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    if let Some(ret) = node.child_by_field_name("type") {
        entity.return_type = Some(node_text(&ret, source).to_string());
    }

    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    populate_body_metrics(node, &mut entity);

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

pub(super) fn parse_constructor(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Method, path, span);
    entity.visibility = parse_visibility(node);
    entity.attributes = parse_modifier_attributes(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.tags.insert("constructor".to_string());

    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    populate_body_metrics(node, &mut entity);

    entity.documentation = extract_javadoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

/// Populate per-callable metrics: LOC, parameter count, and the three
/// body-complexity numbers. Bodyless methods (abstract / interface)
/// default to `cyclomatic = 1` and zero nesting/cognitive — they have one
/// straight-through path by virtue of existing as a signature.
fn populate_body_metrics(node: &Node, entity: &mut CodeEntity) {
    entity.metrics.loc = (entity.span.end.line - entity.span.start.line + 1) as u32;
    entity.metrics.param_count = Some(entity.parameters.len() as u32);
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
}
