//! Small leaf-entity parsers: modules, constants, type aliases, macros.
//!
//! Modules are technically containers (the dispatcher recurses into their
//! body), but the entity itself is built the same way as the others.

use super::super::language_parser::{node_text, node_to_span};
use super::doc_comments::{extract_doc_comment, extract_inner_doc};
use super::helpers::parse_visibility;
use crate::models::{CodeEntity, EntityKind};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn parse_module(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Module, path, span);
    entity.visibility = parse_visibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.source_code = Some(node_text(node, source).to_string());
    // Both spellings document a module: `/// …` above the declaration, and
    // `//!` at the top of its body. A `mod foo;` can only have the first,
    // and the file it names carries its own header — see
    // `RustParser::parse`, which lifts that onto the file.
    entity.documentation = extract_doc_comment(node, source).or_else(|| {
        node.child_by_field_name("body")
            .and_then(|body| extract_inner_doc(&body, source))
    });

    Some(entity)
}

pub(super) fn parse_constant(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Constant, path, span);
    entity.visibility = parse_visibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.source_code = Some(node_text(node, source).to_string());
    entity.documentation = extract_doc_comment(node, source);

    Some(entity)
}

pub(super) fn parse_type_alias(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::TypeAlias, path, span);
    entity.visibility = parse_visibility(node, source);
    entity.parent_id = parent_id.map(String::from);
    entity.source_code = Some(node_text(node, source).to_string());
    entity.documentation = extract_doc_comment(node, source);

    Some(entity)
}

pub(super) fn parse_macro(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Macro, path, span);
    entity.parent_id = parent_id.map(String::from);
    entity.source_code = Some(node_text(node, source).to_string());
    entity.documentation = extract_doc_comment(node, source);

    Some(entity)
}
