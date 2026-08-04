//! Java `import` declaration parsing.

use super::super::language_parser::{node_text, node_to_span, ImportInfo};
use tree_sitter::Node;

pub(super) fn parse_import(node: &Node, source: &str) -> Option<ImportInfo> {
    let text = node_text(node, source);
    let span = node_to_span(node);

    let path = text
        .trim_start_matches("import")
        .trim()
        .trim_start_matches("static")
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string();

    Some(ImportInfo::new(path, span))
}
