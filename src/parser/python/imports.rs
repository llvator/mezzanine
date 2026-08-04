//! Python import statement parsing.

use super::super::language_parser::{node_text, node_to_span, ImportInfo};
use tree_sitter::Node;

/// Dispatch on the two import statement kinds. The bodies live in their own
/// functions: they share nothing but the span, and `import x, y as z` has
/// almost no structure in common with `from m import a, b`.
pub(super) fn parse_import(node: &Node, source: &str) -> Vec<ImportInfo> {
    match node.kind() {
        "import_statement" => parse_plain_import(node, source),
        "import_from_statement" => parse_from_import(node, source),
        _ => Vec::new(),
    }
}

/// `import os`, `import os.path`, `import numpy as np` — one `ImportInfo`
/// per name, since a single statement can import several modules.
fn parse_plain_import(node: &Node, source: &str) -> Vec<ImportInfo> {
    let span = node_to_span(node);
    let mut imports = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                imports.push(ImportInfo::new(node_text(&child, source), span));
            }
            "aliased_import" => {
                if let Some(import) = parse_aliased_import(&child, source, span) {
                    imports.push(import);
                }
            }
            _ => {}
        }
    }
    imports
}

/// `import numpy as np` — the `name` is what's imported, the `alias` is what
/// the file calls it.
fn parse_aliased_import(
    node: &Node,
    source: &str,
    span: crate::models::Span,
) -> Option<ImportInfo> {
    let name = node.child_by_field_name("name")?;
    let mut import = ImportInfo::new(node_text(&name, source), span);
    if let Some(alias) = node.child_by_field_name("alias") {
        import = import.with_alias(node_text(&alias, source));
    }
    Some(import)
}

/// `from m import a, b as c` / `from . import x` / `from m import *` — one
/// `ImportInfo` for the module, carrying the imported names as items.
fn parse_from_import(node: &Node, source: &str) -> Vec<ImportInfo> {
    let span = node_to_span(node);
    let module = node
        .child_by_field_name("module_name")
        .map(|n| node_text(&n, source).to_string())
        .unwrap_or_default();

    let (items, is_wildcard) = collect_from_items(node, source, &module);

    let mut import = ImportInfo::new(module, span);
    if !items.is_empty() {
        import = import.with_items(items);
    }
    if is_wildcard {
        import = import.wildcard();
    }
    let text = node_text(node, source);
    if text.contains("from .") || text.contains("from ..") {
        import = import.relative();
    }
    vec![import]
}

/// The names a `from … import …` brings in, and whether it was a star.
///
/// PY-018: the `wildcard_import` token used to be dropped, which left an
/// empty `items` meaning either "the parser captured nothing" or "the user
/// asked for the module's whole surface" — two very different claims.
fn collect_from_items(node: &Node, source: &str, module: &str) -> (Vec<String>, bool) {
    let mut items = Vec::new();
    let mut is_wildcard = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "wildcard_import" => is_wildcard = true,
            "dotted_name" | "identifier" => {
                // Skip the module name itself.
                let text = node_text(&child, source);
                if text != module {
                    items.push(text.to_string());
                }
            }
            "aliased_import" => {
                if let Some(name) = child.child_by_field_name("name") {
                    items.push(node_text(&name, source).to_string());
                }
            }
            _ => {}
        }
    }
    (items, is_wildcard)
}
