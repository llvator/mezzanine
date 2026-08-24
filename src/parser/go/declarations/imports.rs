//! Go `import` declarations.
//!
//! Two consumers, one pass. The [`ImportInfo`] list is what every language
//! contributes to the dependency graph; the [`Imports`] table is Go's own
//! need, because a call written `store.Load()` cannot be read without
//! knowing that this file bound `store` to an import path.

use super::super::packages::Imports;
use crate::parser::language_parser::{node_text, node_to_span, ImportInfo, ParseResult};
use tree_sitter::Node;

/// Read every import in a file, recording each as an `ImportInfo` and
/// building the local-name table call extraction reads.
pub(super) fn collect(root: Node, source: &str, result: &mut ParseResult) -> Imports {
    let mut imports = Imports::default();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        for spec in specs(&child) {
            record(&spec, source, &mut imports, result);
        }
    }
    imports
}

/// The specs of one `import` — the single-line form declares one directly,
/// the parenthesised form wraps them in a list.
fn specs<'a>(declaration: &Node<'a>) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    let mut cursor = declaration.walk();
    for child in declaration.children(&mut cursor) {
        match child.kind() {
            "import_spec" => out.push(child),
            "import_spec_list" => {
                let mut inner = child.walk();
                out.extend(
                    child
                        .children(&mut inner)
                        .filter(|c| c.kind() == "import_spec"),
                );
            }
            _ => {}
        }
    }
    out
}

fn record(spec: &Node, source: &str, imports: &mut Imports, result: &mut ParseResult) {
    let Some(path_node) = spec.child_by_field_name("path") else {
        return;
    };
    let path = unquote(node_text(&path_node, source));
    let name = spec
        .child_by_field_name("name")
        .map(|n| node_text(&n, source));

    imports.insert(path, name);

    let mut info = ImportInfo::new(path, node_to_span(spec));
    match name {
        // `import . "x"` pulls every exported name of the package into
        // this file's scope — the same claim as a wildcard import.
        Some(".") => info = info.wildcard(),
        // `import _ "x"` imports for the side effects of its `init`.
        // There is no alias to record: nothing in the file can name it.
        Some("_") => {}
        Some(alias) => info = info.with_alias(alias),
        None => {}
    }
    result.add_import(info);
}

/// Go import paths are string literals, interpreted or raw.
fn unquote(text: &str) -> &str {
    text.trim_matches(|c| c == '"' || c == '`')
}
