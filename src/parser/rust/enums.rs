//! Rust enum parsing — variant collection and entity assembly.

use super::super::language_parser::{node_text, node_to_span};
use super::doc_comments::extract_doc_comment;
use super::helpers::parse_visibility;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_enum(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Enum, path, span);
    entity.visibility = parse_visibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.documentation = extract_doc_comment(node, source);

    // Extract enum variants
    entity.fields = parse_enum_variants(node, source);

    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    entity.metrics.field_count = Some(entity.fields.len() as u32);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn parse_enum_variants(node: &Node, source: &str) -> Vec<Parameter> {
    let mut variants = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "enum_variant_list" {
            let mut inner_cursor = child.walk();
            for variant in child.children(&mut inner_cursor) {
                if variant.kind() == "enum_variant" {
                    let name = variant
                        .child_by_field_name("name")
                        .map(|n| node_text(&n, source).to_string())
                        .unwrap_or_default();
                    // Capture associated data as type_name (e.g. "(String, u32)" or "{ x: i32 }")
                    let type_name = variant
                        .child_by_field_name("body")
                        .map(|b| node_text(&b, source).to_string());
                    variants.push(Parameter {
                        name,
                        type_name,
                        default_value: None,
                        visibility: None,
                    });
                }
            }
        }
    }

    variants
}
