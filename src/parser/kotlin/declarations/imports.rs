//! Kotlin import-header parsing.

use crate::parser::language_parser::{find_child_by_kind, node_text, node_to_span, ImportInfo};
use tree_sitter::Node;

pub(super) fn parse_import(node: &Node, source: &str) -> Option<ImportInfo> {
    // import_header contains an `identifier` node with the full dotted path
    let id_node = find_child_by_kind(node, "identifier")?;
    let path = node_text(&id_node, source).to_string();
    let span = node_to_span(node);

    Some(ImportInfo::new(path, span))
}
