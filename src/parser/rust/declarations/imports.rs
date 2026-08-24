//! Rust `use` declaration parsing.

use crate::parser::language_parser::{node_text, node_to_span, ImportInfo};
use tree_sitter::Node;

pub(super) fn parse_use(node: &Node, source: &str) -> Option<ImportInfo> {
    let text = node_text(node, source);
    let span = node_to_span(node);

    // Simple parsing - extract path after "use"
    let path = text
        .trim_start_matches("use")
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string();

    Some(ImportInfo::new(path, span))
}
