//! `UsesValue` edges from imported names read as values (AN-028).
//!
//! `import { LIMIT } from './settings'` followed by `LIMIT` in a body is a
//! dependency on `settings.ts` — change the constant and the reader changes
//! with it — but no other pass records it. `Calls` needs a call, `UsesType`
//! needs a type annotation, and a module of shared constants has neither, so
//! it drew as a stray nothing depends on. ADR 0027 has the reasoning; ADR
//! 0021 has the argument for why this is a dependency rather than a
//! `References`.
//!
//! A post-pass over the tree rather than a branch inside the declaration
//! walk: the source of each edge is the entity whose span encloses the read,
//! which is a question the finished entity list answers directly and
//! `extract_entities` — already over the complexity ceiling — would have to
//! be threaded to answer.
//!
//! ### The filter
//!
//! ADR 0021 measured 626 spurious edges from admitting every path-qualified
//! Rust argument, and its filter is most of that ADR. An imported name is a
//! far narrower population — the import statement is the file saying, in so
//! many words, that this name comes from elsewhere — but four cuts still
//! earn their place:
//!
//! - **Relative specifiers only.** A bare `react` names a package, not an
//!   analysed file, and every name it binds would land on a ghost. It is
//!   also what makes the `<module>::<name>` target below meaningful: the
//!   specifier names a file this analysis walked. This is the rule
//!   [`crate::analyzer::dependency_resolver`] already applies to import
//!   sites, for the same reason.
//! - **Nothing the build erases.** An `import type` specifier resolves to
//!   nothing at runtime (AN-022), and ADR 0026 keeps those arrows out of the
//!   shape scores; minting a value edge from one would put the erased
//!   dependency back through another door. A re-export names a symbol to
//!   hand it on rather than to use it, and is dropped for the same reason.
//! - **Nothing already recorded.** An imported name in callee or constructor
//!   position is a `Calls` or an `Instantiates` edge, and emitting a second
//!   kind for the same site would double-count one dependency.
//! - **No shadowed name.** A parameter or a `const` carrying the same name
//!   as an import is what the body actually reads. Telling the two apart
//!   needs scope analysis; declining the name outright costs only the files
//!   that shadow.
//!
//! One shape is deliberately out of reach: `import { X as Y }` binds `Y`,
//! but [`ImportInfo::items`] records the name the exporting module used, so
//! nothing in the body matches and no edge is emitted. Rust reads its
//! aliases off the tree because its own import parsing is there already;
//! doing the same here would mean re-deriving the specifier gate above from
//! the tree, for a spelling TypeScript rarely uses.
//!
//! [`ImportInfo::items`]: crate::parser::language_parser::ImportInfo::items

use super::super::language_parser::{node_text, ParseResult};
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

/// Emit one `UsesValue` edge per (reader, imported name) pair.
///
/// Targets are `<module>::<name>`, where the module is the last segment of
/// the specifier the name came from. `DependencyGraph::from_analysis` keys
/// that spelling on the file stem (AN-006) and declines rather than guessing
/// when two files claim it — which is what a bare name could not do: over
/// `ui/`, 57 of 449 bare reads bound to a same-named local in an unrelated
/// file, and two folders changed verdict on the strength of it.
pub(super) fn emit_uses_value_edges(root: &Node, source: &str, result: &mut ParseResult) {
    let mut imported = imported_value_names(result);
    if imported.is_empty() {
        return;
    }
    let mut shadowed = HashSet::new();
    collect_shadows(root, source, &mut shadowed);
    imported.retain(|name, _| !shadowed.contains(name));

    let mut reads = Vec::new();
    collect_reads(root, source, &imported, &mut reads);

    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut rels = Vec::new();
    for (offset, target) in reads {
        let Some(reader) = enclosing_entity(&result.entities, offset) else {
            continue;
        };
        if !seen.insert((reader.to_string(), target.clone())) {
            continue;
        }
        rels.push(Relationship::new(
            reader.to_string(),
            target,
            RelationshipKind::UsesValue,
        ));
    }
    for rel in rels {
        result.add_relationship(rel);
    }
}

/// Every name bound by an import this file can be said to depend on, mapped
/// to the `<module>::<name>` the analyzer resolves it by.
fn imported_value_names(result: &ParseResult) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for import in result
        .imports
        .iter()
        .filter(|i| i.is_relative && !i.is_type_only && !i.is_reexport)
    {
        let module = module_segment(&import.path);
        for item in &import.items {
            out.insert(item.clone(), format!("{module}::{item}"));
        }
    }
    out
}

/// The module a specifier names: its last segment, without an extension,
/// stepping past an explicit `index` to the directory that holds it.
///
/// `'./settings'` and `'./settings/index.ts'` are the same module, and
/// `DependencyGraph::from_analysis` keys both under `settings` — the same
/// equivalence [`crate::analyzer::dependency_resolver`] applies when it
/// resolves the specifier to a file.
fn module_segment(specifier: &str) -> &str {
    let mut segments = specifier
        .rsplit('/')
        .map(|s| s.split('.').next().unwrap_or(s))
        .filter(|s| !s.is_empty());
    match segments.next() {
        Some("index") => segments.next().unwrap_or("index"),
        Some(other) => other,
        None => "",
    }
}

/// Every name this file binds itself: a declaration, a `const`, a parameter,
/// or a destructuring pattern.
fn collect_shadows(node: &Node, source: &str, out: &mut HashSet<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_statement" {
            continue;
        }
        if child.kind() == "identifier" && is_binding(&child) {
            out.insert(node_text(&child, source).to_string());
        }
        collect_shadows(&child, source, out);
    }
}

/// Whether this identifier introduces a name rather than reading one.
///
/// The declaring kinds are named rather than trusting a `name` field on its
/// own: `jsx_opening_element` has one too, so `<Sidebar/>` would otherwise
/// read as a declaration of the component it renders and shadow the import
/// it is the whole use of.
fn is_binding(ident: &Node) -> bool {
    let Some(parent) = ident.parent() else {
        return false;
    };
    let kind = parent.kind();
    let field_is = |field| parent.child_by_field_name(field).as_ref() == Some(ident);
    // `[a, b] = …`, `{ a } = …`, `function f(a: T)`, `a => …`, `catch (e)`.
    if kind.ends_with("_pattern") || kind.ends_with("_parameter") {
        return true;
    }
    if kind == "arrow_function" || kind == "catch_clause" {
        return field_is("parameter");
    }
    // `const x = …`, `function f`, `class C`, `interface I`, `enum E`,
    // `type T = …`, and a class member's own name.
    (kind.ends_with("_declaration") || kind == "variable_declarator" || kind == "method_definition")
        && field_is("name")
}

/// Walk the tree collecting `(byte offset, target name)` for every read of
/// an imported name, in source order.
fn collect_reads(
    node: &Node,
    source: &str,
    imported: &HashMap<String, String>,
    reads: &mut Vec<(usize, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // The statement that bound the name is not a use of it, and neither
        // is the clause of an `export … from` re-export.
        if matches!(child.kind(), "import_statement" | "export_clause") {
            continue;
        }
        if child.kind() == "identifier" && !is_already_recorded(&child) {
            if let Some(target) = imported.get(node_text(&child, source)) {
                reads.push((child.start_byte(), target.clone()));
            }
        }
        collect_reads(&child, source, imported, reads);
    }
}

/// Whether this identifier is something other than a read of the imported
/// name: a binding, a callee already recorded as a `Calls` edge, or a
/// constructor already recorded as an `Instantiates` edge.
fn is_already_recorded(ident: &Node) -> bool {
    if is_binding(ident) {
        return true;
    }
    let Some(parent) = ident.parent() else {
        return false;
    };
    let field_is = |field| parent.child_by_field_name(field).as_ref() == Some(ident);
    (parent.kind() == "call_expression" && field_is("function"))
        || (parent.kind() == "new_expression" && field_is("constructor"))
}

/// The id of the innermost entity whose span holds `offset`.
///
/// Synthetic control-flow nodes are not candidates: a call inside an `if`
/// arm is still sourced from the enclosing callable and re-attached to the
/// arm later, from the `branch` key call extraction writes (TS-002).
/// Sourcing a value read from the arm directly would put the two passes on
/// different footings.
fn enclosing_entity(entities: &[CodeEntity], offset: usize) -> Option<&str> {
    entities
        .iter()
        .filter(|e| !matches!(e.kind, EntityKind::Branch | EntityKind::Loop))
        .filter(|e| e.span.start.offset <= offset && offset < e.span.end.offset)
        .min_by_key(|e| e.span.end.offset - e.span.start.offset)
        .map(|e| e.id.as_str())
}
