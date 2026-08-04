//! Java field-declaration parsing.
//!
//! A single `field_declaration` can contain multiple variable declarators
//! (`int a, b, c;`), so this returns a Vec rather than an Option.

use super::super::language_parser::{node_text, node_to_span};
use super::helpers::{parse_modifier_attributes, parse_visibility};
use super::javadoc::extract_javadoc;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_field(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Vec<CodeEntity> {
    let mut entities = Vec::new();
    let visibility = parse_visibility(node);
    let attributes = parse_modifier_attributes(node, source);
    let type_name = node.child_by_field_name("type")
        .map(|t| node_text(&t, source).to_string());

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source).to_string();
                let span = node_to_span(&child);

                let mut entity = CodeEntity::new(&name, EntityKind::Property, path, span);
                entity.visibility = visibility;
                entity.attributes = attributes.clone();
                entity.parent_id = parent_id.map(String::from);
                entity.return_type = type_name.clone();
                entity.documentation = extract_javadoc(node, source);
                entity.source_code = Some(node_text(node, source).to_string());

                entities.push(entity);
            }
        }
    }

    entities
}
