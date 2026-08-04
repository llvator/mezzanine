//! Rust trait declaration parsing.

use super::super::language_parser::{node_text, node_to_span};
use super::doc_comments::extract_doc_comment;
use super::helpers::parse_visibility;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_trait(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Trait, path, span);
    entity.visibility = parse_visibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.documentation = extract_doc_comment(node, source);
    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    entity.source_code = Some(node_text(node, source).to_string());

    Some(entity)
}
