//! TypeScript `import` statement parsing — handles default imports,
//! named imports, and namespace (star) imports, plus the `export … from`
//! re-export, which is an import wearing the other keyword.

use crate::parser::language_parser::{node_text, node_to_span, ImportInfo};
use tree_sitter::Node;

pub(super) fn parse_import(node: &Node, source: &str) -> Option<ImportInfo> {
    let import = specifier(node, source)?;

    // `import './polyfill'` binds nothing — the one import whose entire
    // point is the side effect it has at runtime.
    let Some(clause) = child_of_kind(node, "import_clause") else {
        return Some(import);
    };

    Some(bound_onto(import, node, bindings(&clause, source)))
}

/// `export { X } from './y'` and `export * from './y'` — the same
/// dependency an `import` would create, recorded as one.
///
/// Returns `None` for every other `export_statement`: `export const x = 1`
/// and `export class C {}` declare rather than depend, and they are the
/// reason this is a separate entry point instead of a branch inside
/// [`parse_import`] — the dispatcher hands both shapes to the same arm.
///
/// The `reexport` flag is the load-bearing part. Without it this edge is
/// indistinguishable from a direct import, and a reader chasing a
/// dependency on a file nobody in their call path named has to open every
/// intermediate module to find the shim (AN-024).
pub(super) fn parse_reexport(node: &Node, source: &str) -> Option<ImportInfo> {
    let import = specifier(node, source)?.reexport();
    Some(bound_onto(import, node, reexported_names(node, source)))
}

/// Everything a clause tells the statement, written onto it.
///
/// Shared by both entry points because an `import` and an `export … from`
/// answer these questions identically — the names bound, the module
/// alias, and whether the build keeps any of it. Writing it once is what
/// keeps the two from drifting the way they would if each grew its own
/// copy per ticket.
fn bound_onto(mut import: ImportInfo, node: &Node, bound: Bound) -> ImportInfo {
    if let Some(alias) = bound.namespace {
        import = import.with_alias(alias);
    }
    if !bound.items.is_empty() {
        import = import.with_items(bound.items);
    }
    if erases_everything(node, bound.kept, bound.erased) {
        import = import.type_only();
    }
    import
}

/// The half every `… from '<path>'` statement shares: the specifier, its
/// relativity, and the span of the whole statement.
///
/// `None` when the node has no `source` — an `import` always does, an
/// `export` only when it re-exports.
fn specifier(node: &Node, source: &str) -> Option<ImportInfo> {
    let source_node = node.child_by_field_name("source")?;
    let raw_path = node_text(&source_node, source)
        .trim_matches('\'')
        .trim_matches('"')
        .to_string();
    let is_relative = raw_path.starts_with('.') || raw_path.starts_with('/');
    let import = ImportInfo::new(&raw_path, node_to_span(node));
    Some(if is_relative {
        import.relative()
    } else {
        import
    })
}

/// What one `import_clause` or `export_clause` binds, split by whether the
/// build keeps the binding.
#[derive(Default)]
struct Bound {
    /// Every name introduced, erased or not — what the resolver matches on,
    /// and unaffected by this ticket.
    items: Vec<String>,
    /// `import * as Foo` renames the whole module rather than naming items.
    namespace: Option<String>,
    /// Bindings that survive to the emitted code: a default import, a
    /// namespace import, a specifier written without `type`.
    kept: usize,
    /// Specifiers written `type X`, which the compiler drops.
    erased: usize,
}

/// Whether the statement leaves nothing behind for the bundler to resolve
/// (AN-022).
///
/// Two spellings reach the same answer. `import type { X } from './y'` puts
/// the keyword on the statement, erasing every name at once. `import
/// { type X } from './y'` puts it on each name, and the statement is erased
/// only when it has no other name to keep the specifier alive — which is
/// why `import { type X, y }` is deliberately *not* type-only: `y` is still
/// resolved, and calling that edge erased would be the same lie in the
/// other direction.
fn erases_everything(node: &Node, kept: usize, erased: usize) -> bool {
    has_type_keyword(node) || (kept == 0 && erased > 0)
}

/// The `type` keyword written directly on a statement or on one specifier.
///
/// tree-sitter gives it as an anonymous child in both places, so the same
/// walk answers `import type { X } from …` and `import { type X, y } from …`.
fn has_type_keyword(node: &Node) -> bool {
    children(*node).any(|c| c.kind() == "type")
}

/// The first child of `kind`, or `None`.
fn child_of_kind<'a>(node: &Node<'a>, kind: &str) -> Option<Node<'a>> {
    children(*node).find(|c| c.kind() == kind)
}

/// Every child, named and anonymous, without a cursor to keep alive.
///
/// `Node::children` borrows a `TreeCursor` for as long as the iterator
/// lives, which a helper cannot return past. Indexing does the same walk
/// and owns nothing — and the `type` keyword this module reads is an
/// anonymous child, so `named_children` would not see it.
fn children<'a>(node: Node<'a>) -> impl Iterator<Item = Node<'a>> {
    (0..node.child_count()).filter_map(move |i| node.child(i))
}

/// The names an `import_clause` introduces, and whether each survives.
fn bindings(clause: &Node, source: &str) -> Bound {
    let mut bound = Bound::default();
    for child in children(*clause) {
        match child.kind() {
            // Default import — `import D from './x'`.
            "identifier" => {
                bound.items.push(node_text(&child, source).to_string());
                bound.kept += 1;
            }
            "named_imports" => named_imports(&child, source, &mut bound),
            "namespace_import" => {
                if let Some(name) = child_of_kind(&child, "identifier") {
                    bound.namespace = Some(node_text(&name, source).to_string());
                    bound.kept += 1;
                }
            }
            _ => {}
        }
    }
    bound
}

/// The `{ a, type B }` half of a clause.
fn named_imports(node: &Node, source: &str, bound: &mut Bound) {
    for spec in children(*node) {
        if spec.kind() != "import_specifier" {
            continue;
        }
        if let Some(name) = spec.child_by_field_name("name") {
            bound.items.push(node_text(&name, source).to_string());
        }
        if has_type_keyword(&spec) {
            bound.erased += 1;
        } else {
            bound.kept += 1;
        }
    }
}

/// The names an `export { a, b as c } from './y'` passes on, and whether
/// each survives. Both counts are zero for `export * from './y'`, which
/// names none — and a star re-export forwards values, so it is not erased.
fn reexported_names(node: &Node, source: &str) -> Bound {
    let mut bound = Bound::default();
    let Some(clause) = child_of_kind(node, "export_clause") else {
        return bound;
    };
    for spec in children(clause) {
        if spec.kind() != "export_specifier" {
            continue;
        }
        if let Some(name) = spec.child_by_field_name("name") {
            bound.items.push(node_text(&name, source).to_string());
        }
        if has_type_keyword(&spec) {
            bound.erased += 1;
        } else {
            bound.kept += 1;
        }
    }
    bound
}
