//! Kotlin function parsing — including extension-function detection, the
//! call-extraction wiring, and the body-metrics wiring (KT-002).

use super::super::bodies::calls::{extract_calls, CallCtx, Caller};
use super::super::bodies::complexity::populate_body_metrics;
use super::super::helpers::{extract_simple_identifier, parse_generics, parse_parameters};
use super::super::kdoc::extract_kdoc;
use super::super::modifiers::{parse_modifier_attributes, parse_visibility};
use crate::models::{CodeEntity, EntityKind};
use crate::parser::language_parser::{find_child_by_kind, node_text, node_to_span, ParseResult};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn handle_function(
    node: &Node,
    parent_id: Option<&str>,
    source: &str,
    path: &Path,
    result: &mut ParseResult,
) {
    if let Some(entity) = parse_function(node, source, path, parent_id) {
        let caller_id = entity.id.clone();
        let caller_name = entity.name.clone();
        let parent_class_name = parent_id.and_then(|pid| {
            result
                .entities
                .iter()
                .find(|e| e.id == pid)
                .map(|e| e.name.clone())
        });
        result.add_entity(entity);
        if let Some(body) = find_child_by_kind(node, "function_body") {
            let caller = Caller {
                source,
                path,
                id: &caller_id,
                name: &caller_name,
                parent_class: parent_class_name.as_deref(),
            };
            let mut ctx = CallCtx::new(caller, result);
            extract_calls(&body, &mut ctx, None);
        }
    }
}

fn parse_function(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name = extract_simple_identifier(node, source)?;
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

    // Check if this is an extension function (has a receiver type before the name)
    if is_extension_function(node) {
        entity.tags.insert("extension".to_string());
    }

    // Type parameters
    if let Some(tp) = find_child_by_kind(node, "type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    // Return type (after `:` following parameter list)
    entity.return_type = extract_function_return_type(node, source);

    // Parameters
    if let Some(params) = find_child_by_kind(node, "function_value_parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    populate_body_metrics(find_child_by_kind(node, "function_body"), source, &mut entity);

    entity.documentation = extract_kdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

/// Check whether a function_declaration is an extension function.
/// Extension functions have a `user_type` child followed by `.` before the name.
fn is_extension_function(node: &Node) -> bool {
    let mut cursor = node.walk();
    let mut saw_user_type = false;

    for child in node.children(&mut cursor) {
        match child.kind() {
            "user_type" if !saw_user_type => saw_user_type = true,
            "." if saw_user_type => return true,
            "simple_identifier" => return false,
            "fun" => {
                saw_user_type = false;
            }
            _ => {}
        }
    }

    false
}

/// Extract the return type of a function.
/// The return type is a `user_type` or `nullable_type` node that appears
/// after the `:` following `function_value_parameters`.
fn extract_function_return_type(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let mut found_colon_after_params = false;

    for child in node.children(&mut cursor) {
        if child.kind() == "function_value_parameters" {
            found_colon_after_params = false;
        } else if found_colon_after_params {
            match child.kind() {
                "user_type" | "nullable_type" | "function_type" => {
                    return Some(node_text(&child, source).to_string());
                }
                _ => {}
            }
        }
        if child.kind() == ":" {
            found_colon_after_params = true;
        }
    }

    None
}
