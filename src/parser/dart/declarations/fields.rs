//! Fields and top-level variables.
//!
//! One declaration can name several variables (`int a, b;`), so both
//! extractors return a `Vec`. They differ only in the node the grammar wraps
//! the names in — and not much, since a top-level variable and a class field
//! use the same two list shapes: `initialized_identifier_list` for a plain
//! one, `static_final_declaration_list` for a `static`, `final` or `const`.
//!
//! Every one of them becomes a `Property`, with its declared type in
//! `return_type` — the convention the Java and Kotlin parsers set, and what
//! [`super::super::types`] reads to emit `UsesType` edges.

use super::super::doc_comments;
use super::super::helpers::{child_of_kind, children_of_kind, declared_type, visibility_of};
use super::callables::{annotations, spine_modifiers};
use crate::models::{CodeEntity, EntityKind};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

/// The list nodes a declaration hangs its names off.
const NAME_LISTS: &[&str] = &[
    "initialized_identifier_list",
    "static_final_declaration_list",
];

/// The nodes inside those lists that carry one name each.
const NAME_HOLDERS: &[&str] = &["initialized_identifier", "static_final_declaration"];

/// Parse a class member that turned out to be a field declaration.
///
/// `member` is the `class_member` — it carries the documentation;
/// `declaration` is the node inside it holding the modifiers, the type and
/// the names.
pub(super) fn parse_field(
    member: &Node,
    declaration: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Vec<CodeEntity> {
    parse_variables(
        declaration,
        source,
        path,
        parent_id,
        "",
        doc_comments::extract(member, source),
    )
}

/// Parse a `top_level_variable_declaration` — a file-scope variable or
/// constant.
pub(super) fn parse_top_level_variable(
    node: &Node,
    source: &str,
    path: &Path,
    library: &str,
) -> Vec<CodeEntity> {
    let documentation = doc_comments::extract(node, source);
    parse_variables(node, source, path, None, library, documentation)
}

/// The shared body of both: one entity per name the declaration lists.
fn parse_variables(
    declaration: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    library: &str,
    documentation: Option<String>,
) -> Vec<CodeEntity> {
    let Some(list) = NAME_LISTS
        .iter()
        .find_map(|kind| child_of_kind(declaration, kind))
    else {
        return Vec::new();
    };
    let type_name = declared_type(declaration, source);
    let mut attributes = spine_modifiers(declaration);
    attributes.extend(annotations(declaration, source));

    let mut entities = Vec::new();
    for holder_kind in NAME_HOLDERS {
        for holder in children_of_kind(&list, holder_kind) {
            let Some(name_node) = child_of_kind(&holder, "identifier") else {
                continue;
            };
            let name = node_text(&name_node, source).to_string();
            let mut entity =
                CodeEntity::new(&name, EntityKind::Property, path, node_to_span(&holder));
            entity.visibility = visibility_of(&name);
            entity.attributes = attributes.clone();
            entity.parent_id = parent_id.map(String::from);
            entity.return_type = type_name.clone();
            entity.documentation = documentation.clone();
            entity.source_code = Some(node_text(declaration, source).to_string());
            if parent_id.is_none() && !library.is_empty() {
                entity.qualified_name = format!("{}.{}", library, name);
            }
            entities.push(entity);
        }
    }
    entities
}
