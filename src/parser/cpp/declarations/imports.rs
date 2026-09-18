//! What a C++ file pulls in: `#include`, `using`, and namespace aliases.
//!
//! C++ has no import statement, and the three constructs below are not
//! interchangeable — so each fills the [`ImportInfo`] fields that are true
//! of it and leaves the rest alone (§A2):
//!
//! * `#include "store/repository.h"` is a *file* dependency and a relative
//!   one; `#include <vector>` names a search path and is not.
//! * `using namespace std;` opens a whole namespace — the `is_wildcard`
//!   case, spelled differently from Python's `import *` and meaning the
//!   same thing.
//! * `using app::Order;` binds one name, which travels in `items`.
//! * `namespace fs = std::filesystem;` is an alias and nothing else.
//!
//! An include inside `#ifdef` is recorded as [`ImportCondition::Guarded`],
//! the same fact PY-024 records for a Python import inside an `if`: the
//! dependency is real when the gate opens, and a build that never defines
//! the macro never has it.
//!
//! Two `ImportInfo` fields stay unset on purpose. `is_reexport` has no
//! spelling in C++ — a header that includes another and re-exposes its
//! names does so by having included it, with no syntax that says whether
//! that was the intent. `is_type_only` has none either: every C++ include
//! is compiled, and nothing in the language erases one from the build.

use crate::parser::language_parser::{node_text, node_to_span, ImportCondition, ImportInfo, ParseResult};
use tree_sitter::Node;

/// Record every include, using-declaration and namespace alias in a file.
///
/// A pre-pass over the whole tree rather than an arm of the declaration
/// walk, because includes hide inside `#ifdef` blocks and header guards
/// where the walk has no reason to look for them.
pub(super) fn collect(root: Node, source: &str, result: &mut ParseResult) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let import = match node.kind() {
            "preproc_include" => include(&node, source),
            "using_declaration" => using(&node, source),
            "namespace_alias_definition" => alias(&node, source),
            _ => None,
        };
        if let Some(import) = import {
            result.add_import(guard(import, &node));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// `#include "x.h"` — relative, a file this project owns — or
/// `#include <x>`, which names a search path and not a file.
fn include(node: &Node, source: &str) -> Option<ImportInfo> {
    let path_node = node.child_by_field_name("path")?;
    let text = node_text(&path_node, source);
    let trimmed = text.trim_matches(['"', '<', '>']).to_string();
    let import = ImportInfo::new(trimmed, node_to_span(node));
    Some(if path_node.kind() == "string_literal" {
        import.relative()
    } else {
        import
    })
}

/// `using namespace std;` or `using app::Order;`.
fn using(node: &Node, source: &str) -> Option<ImportInfo> {
    let mut cursor = node.walk();
    let named = node.children(&mut cursor).find(|c| c.is_named())?;
    let span = node_to_span(node);
    let is_directive = node_text(node, source).trim_start().starts_with("using namespace");
    if is_directive {
        return Some(ImportInfo::new(node_text(&named, source), span).wildcard());
    }
    let (scope, name) = super::super::helpers::qualified_parts(&named, source);
    let import = ImportInfo::new(node_text(&named, source), span);
    Some(match scope {
        Some(_) => import.with_items(vec![name]),
        None => import,
    })
}

/// `namespace fs = std::filesystem;`
fn alias(node: &Node, source: &str) -> Option<ImportInfo> {
    let name = node.child_by_field_name("name")?;
    let mut cursor = node.walk();
    let target = node
        .children(&mut cursor)
        .find(|c| c.is_named() && c.id() != name.id())?;
    Some(
        ImportInfo::new(node_text(&target, source), node_to_span(node))
            .with_alias(node_text(&name, source)),
    )
}

/// Mark an import that sits inside a preprocessor conditional.
///
/// A header guard is a conditional too, and every declaration in a guarded
/// header sits inside one — so the outermost `#ifndef` is not what this
/// looks for. It looks for an `#ifdef` / `#if` that the import is *inside*
/// and that does not wrap the whole file, which is what a feature gate
/// looks like.
fn guard(import: ImportInfo, node: &Node) -> ImportInfo {
    let mut current = node.parent();
    while let Some(parent) = current {
        let conditional = matches!(parent.kind(), "preproc_ifdef" | "preproc_if" | "preproc_elif");
        if conditional && parent.parent().is_some_and(|g| g.kind() != "translation_unit") {
            return import.conditional(ImportCondition::Guarded);
        }
        current = parent.parent();
    }
    import
}
