//! Type-alias declarations.

use super::super::helpers::parse_generics;
use super::super::tsdoc::extract_tsdoc;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

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
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    if let Some(tp) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, source);
    }

    entity.documentation = extract_tsdoc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}
