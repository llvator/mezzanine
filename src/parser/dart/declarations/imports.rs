//! Dart directives: `import`, `export`, `part` and `part of`.
//!
//! All four name another library by URI, so all four become an
//! [`ImportInfo`]. What differs is what the URI *means*, and that is
//! recorded in `items` as a marker the resolver can read:
//!
//! * `import 'a.dart' show A, B;` — the shown names, which is exactly what
//!   `items` is for.
//! * `import 'a.dart' hide C;` — recorded as `hide:C`, because dropping it
//!   would make a hiding import look like a wildcard one.
//! * `export`, `part` and `part of` — a single marker item naming the
//!   directive, since none of them import a symbol list.
//!
//! A `package:` or `dart:` URI is absolute; anything else is a path
//! relative to the importing file, which is what `is_relative` records.

use crate::parser::language_parser::{node_text, node_to_span, ImportInfo};
use tree_sitter::Node;

use super::super::helpers::{child_of_kind, children_of_kind};

/// Parse an `import_or_export` node — the wrapper the grammar puts around
/// both directives.
pub(super) fn parse_import_or_export(node: &Node, source: &str) -> Option<ImportInfo> {
    if let Some(import) = child_of_kind(node, "library_import") {
        return parse_import(&import, source);
    }
    let export = child_of_kind(node, "library_export")?;
    let uri = uri_text(&export, source)?;
    Some(marker(uri, node, "export"))
}

/// `import 'package:http/http.dart' as http show Client;`
fn parse_import(node: &Node, source: &str) -> Option<ImportInfo> {
    let spec = child_of_kind(node, "import_specification").unwrap_or(*node);
    let uri = uri_text(&spec, source)?;
    let relative = is_relative(&uri);

    let mut info = ImportInfo::new(uri, node_to_span(node));
    if relative {
        info = info.relative();
    }
    if let Some(alias) = child_of_kind(&spec, "identifier") {
        info = info.with_alias(node_text(&alias, source));
    }

    let items = combinator_items(&spec, source);
    Some(if items.is_empty() {
        // No `show` list: the import claims the library's whole surface,
        // which is the same statement `from x import *` makes.
        info.wildcard()
    } else {
        info.with_items(items)
    })
}

/// `part 'cart.g.dart';` and `part of 'cart.dart';` — a file-splitting
/// directive rather than a dependency on another library, but still a
/// pointer at another file, so it travels the same way.
pub(super) fn parse_part(node: &Node, source: &str) -> Option<ImportInfo> {
    let uri = uri_text(node, source)?;
    let kind = if node.kind() == "part_of_directive" {
        "part-of"
    } else {
        "part"
    };
    Some(marker(uri, node, kind))
}

/// An import-shaped record whose `items` say what the directive was.
fn marker(uri: String, node: &Node, kind: &str) -> ImportInfo {
    let relative = is_relative(&uri);
    let info = ImportInfo::new(uri, node_to_span(node)).with_items(vec![kind.to_string()]);
    if relative {
        info.relative()
    } else {
        info
    }
}

/// The quoted URI a directive points at, with its quotes removed.
fn uri_text(node: &Node, source: &str) -> Option<String> {
    let uri = child_of_kind(node, "uri").or_else(|| {
        child_of_kind(node, "configurable_uri").and_then(|c| child_of_kind(&c, "uri"))
    })?;
    let text = node_text(&uri, source).trim();
    Some(
        text.trim_start_matches(['r', 'R'])
            .trim_matches(['\'', '"'])
            .to_string(),
    )
}

/// The names a `show` / `hide` combinator lists. A hidden name is prefixed
/// so the two lists stay distinguishable downstream.
fn combinator_items(spec: &Node, source: &str) -> Vec<String> {
    let mut items = Vec::new();
    for combinator in children_of_kind(spec, "combinator") {
        let hiding = child_of_kind(&combinator, "hide").is_some();
        for name in children_of_kind(&combinator, "identifier") {
            let name = node_text(&name, source);
            items.push(if hiding {
                format!("hide:{}", name)
            } else {
                name.to_string()
            });
        }
    }
    items
}

/// `dart:` and `package:` URIs are absolute; everything else is a path
/// resolved against the importing file.
fn is_relative(uri: &str) -> bool {
    !uri.starts_with("dart:") && !uri.starts_with("package:")
}
