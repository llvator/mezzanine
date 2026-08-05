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
                        .map(|b| summarize_variant_body(&b, source));
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

/// Render a variant's associated data as a one-line signature.
///
/// The body's raw source is not a signature: doc comments and attributes
/// ride along with it, so a clap subcommand variant came out as a
/// paragraph of source and became a node label that long in the graph.
/// Keep the field names and types, drop everything else.
fn summarize_variant_body(body: &Node, source: &str) -> String {
    match body.kind() {
        // `Variant { path: PathBuf, format: OutputFormatArg }`
        "field_declaration_list" => named_fields(body, source),
        // `Variant(String, u32)`
        "ordered_field_declaration_list" => tuple_fields(body, source),
        _ => one_line(node_text(body, source)),
    }
}

/// `{ name: Type, name: Type }` from a struct-shaped variant body.
fn named_fields(body: &Node, source: &str) -> String {
    let mut cursor = body.walk();
    let fields: Vec<String> = body
        .children(&mut cursor)
        .filter(|c| c.kind() == "field_declaration")
        .map(|f| {
            let name = field_text(&f, "name", source);
            format!("{}: {}", name, field_text(&f, "type", source))
        })
        .collect();
    if fields.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", fields.join(", "))
    }
}

/// `(Type, Type)` from a tuple-shaped variant body. Attributes, comments
/// and per-field visibility sit alongside the types here, so filter to
/// what a signature would show.
fn tuple_fields(body: &Node, source: &str) -> String {
    let mut cursor = body.walk();
    let types: Vec<String> = body
        .children(&mut cursor)
        .filter(|c| c.is_named() && !is_decoration(c.kind()))
        .map(|t| one_line(node_text(&t, source)))
        .collect();
    format!("({})", types.join(", "))
}

fn is_decoration(kind: &str) -> bool {
    matches!(
        kind,
        "visibility_modifier" | "attribute_item" | "line_comment" | "block_comment"
    )
}

/// One named child of a field declaration, flattened to a single line.
fn field_text(field: &Node, field_name: &str, source: &str) -> String {
    field
        .child_by_field_name(field_name)
        .map(|n| one_line(node_text(&n, source)))
        .unwrap_or_default()
}

/// Collapse whitespace runs so a type that wraps in the source still
/// reads as one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
