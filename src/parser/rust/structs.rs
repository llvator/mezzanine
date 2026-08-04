//! Rust struct parsing — fields, generics, public-field ratio.

use super::super::language_parser::{node_text, node_to_span};
use super::doc_comments::extract_doc_comment;
use super::helpers::{parse_generics, parse_visibility};
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_struct(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Struct, path, span);
    entity.visibility = parse_visibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.documentation = extract_doc_comment(node, source);

    if let Some(generics) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&generics, source);
    }

    // Extract struct fields
    entity.fields = parse_struct_fields(node, source);

    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    entity.metrics.field_count = Some(entity.fields.len() as u32);
    // Public-field ratio is only meaningful when we captured per-field
    // visibility, which we do for structs (not enum variants). A unit
    // struct (no fields) has no ratio — keep it None to avoid NaN.
    if !entity.fields.is_empty() {
        let pub_count = entity
            .fields
            .iter()
            .filter(|f| matches!(f.visibility, Some(Visibility::Public)))
            .count();
        entity.metrics.public_field_ratio =
            Some(pub_count as f32 / entity.fields.len() as f32);
    }
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn parse_struct_fields(node: &Node, source: &str) -> Vec<Parameter> {
    let mut fields = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "field_declaration_list" {
            let mut inner_cursor = child.walk();
            for field in child.children(&mut inner_cursor) {
                if field.kind() == "field_declaration" {
                    let name = field
                        .child_by_field_name("name")
                        .map(|n| node_text(&n, source).to_string())
                        .unwrap_or_default();
                    let type_name = field
                        .child_by_field_name("type")
                        .map(|t| node_text(&t, source).to_string());
                    // Per-field visibility is needed for the
                    // public_field_ratio metric. Tree-sitter gives us
                    // the `visibility_modifier` as a child of the
                    // field_declaration when present; absence means
                    // private (Rust's default).
                    let visibility = Some(parse_visibility(&field, source));
                    fields.push(Parameter {
                        name,
                        type_name,
                        default_value: None,
                        visibility,
                    });
                }
            }
        }
    }

    fields
}

