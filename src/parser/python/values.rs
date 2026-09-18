//! `UsesValue` edges from imported names read as values (PY-030).
//!
//! `from .config import LIMIT` followed by `LIMIT` in a body is a dependency
//! on `config.py` — change the constant and the reader changes with it — but
//! no other pass records it. `Calls` needs a call, `UsesType` needs an
//! annotation, and a module of shared constants has neither, so it drew as a
//! stray nothing depends on. ADR 0027 has the reasoning; ADR 0021 has the
//! argument for why this is a dependency rather than a `References`.
//!
//! A post-pass over the tree rather than a branch inside the declaration
//! walk: the source of each edge is the entity whose span encloses the read,
//! which the finished entity list answers directly and `extract_calls` would
//! have to be threaded to answer.
//!
//! ### One kind, not two
//!
//! Rust splits this population in two: ADR 0021's `UsesFn` for a function
//! handed to a dispatcher, ADR 0027's `UsesValue` for a constant read. The
//! cut it makes is `is_fn_shaped` — `snake_case` names a function,
//! `SCREAMING_CASE` and `CamelCase` name a value or a type. Python spells a
//! function and a module-level variable the same way, so that cut does not
//! exist here and guessing it would be worse than not making it. Both kinds
//! answer `true` to [`RelationshipKind::is_dependency`], so fan-in, fan-out
//! and every coupling metric read the same either way; what a reader loses
//! is the label on the arrow, not the arrow.
//!
//! ### The filter
//!
//! ADR 0021 measured 626 spurious edges from admitting every path-qualified
//! Rust argument, and its filter is most of that ADR. An imported name is a
//! far narrower population — the `from … import …` is the file saying, in so
//! many words, that this name comes from elsewhere — but four cuts still
//! earn their place:
//!
//! - **Relative imports only.** `from .config import LIMIT` names a module
//!   inside the project. `from django.conf import settings` names a package
//!   this analysis never walked, and every name it binds would land on a
//!   ghost. It is also what makes the `<module>::<name>` target meaningful.
//!   This is the rule [`crate::analyzer::dependency_resolver`] already
//!   applies to import sites, for the same reason.
//! - **Nothing the interpreter erases.** An import under `if TYPE_CHECKING:`
//!   never runs (AN-022), and ADR 0026 keeps those arrows out of the shape
//!   scores; minting a value edge from one would put the erased dependency
//!   back through another door.
//! - **Nothing already recorded.** An imported name in callee position is a
//!   `Calls` or `Instantiates` edge, a decorator is a `Calls` edge (PY-017),
//!   and a name inside an annotation is a `UsesType` edge (PY-025); a second
//!   kind for the same site would double-count one dependency.
//! - **No shadowed name.** A parameter, a loop variable or an assignment
//!   carrying an import's name is what the body actually reads. Telling the
//!   two apart needs scope analysis; declining the name outright costs only
//!   the files that shadow.
//!
//! One shape is deliberately out of reach: `from .config import LIMIT as CAP`
//! binds `CAP`, but [`ImportInfo::items`] records the name the exporting
//! module used, so nothing in the body matches and no edge is emitted.
//!
//! [`ImportInfo::items`]: crate::parser::language_parser::ImportInfo::items
//! [`RelationshipKind::is_dependency`]: crate::models::RelationshipKind::is_dependency

use super::super::language_parser::{node_text, ParseResult};
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

/// Emit one `UsesValue` edge per (reader, imported name) pair.
///
/// Targets are `<module>::<name>`, where the module is the last segment of
/// the specifier the name came from. `DependencyGraph::from_analysis` keys
/// that spelling on the file stem (AN-006) and declines rather than guessing
/// when two files claim it — which is what a bare name could not do.
pub(super) fn emit_uses_value_edges(root: &Node, source: &str, result: &mut ParseResult) {
    let mut imported = imported_value_names(result);
    if imported.is_empty() {
        return;
    }
    let mut shadowed = HashSet::new();
    collect_shadows(root, source, &mut shadowed);
    imported.retain(|name, _| !shadowed.contains(name));
    if imported.is_empty() {
        return;
    }

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
        .filter(|i| i.is_relative && !i.is_type_only && !i.is_wildcard)
    {
        let module = module_segment(&import.path);
        if module.is_empty() {
            continue;
        }
        for item in &import.items {
            out.insert(item.clone(), format!("{module}::{item}"));
        }
    }
    out
}

/// The module a relative specifier names: its last dotted segment.
///
/// `.config` and `..pkg.config` both name `config`. A bare `.` or `..` — the
/// `from . import helpers` shape — names the package directory, which this
/// spelling cannot address, so it answers empty and the import is skipped.
fn module_segment(specifier: &str) -> &str {
    specifier.rsplit('.').find(|s| !s.is_empty()).unwrap_or("")
}

/// Every name this file binds itself: a `def`, a `class`, a parameter, an
/// assignment target, a loop variable, an `as` alias, or a walrus.
fn collect_shadows(node: &Node, source: &str, out: &mut HashSet<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_import_statement(child.kind()) {
            continue;
        }
        if child.kind() == "identifier" && is_binding(&child) {
            out.insert(node_text(&child, source).to_string());
        }
        collect_shadows(&child, source, out);
    }
}

/// The two statement kinds that bind the names this pass is about. Skipped
/// wholesale by both walks: the statement that brought a name in is neither
/// a shadow of it nor a use of it.
fn is_import_statement(kind: &str) -> bool {
    matches!(kind, "import_statement" | "import_from_statement")
}

/// Whether this identifier introduces a name rather than reading one.
///
/// The parent kinds are named rather than trusting a `name` field on its
/// own: `keyword_argument` has one too, so `render(limit=LIMIT)` would
/// otherwise read as a declaration of `limit`, and `attribute` has one, so
/// `cfg.LIMIT` would shadow the import it has nothing to do with.
fn is_binding(ident: &Node) -> bool {
    let Some(parent) = ident.parent() else {
        return false;
    };
    let kind = parent.kind();
    let field_is = |field| parent.child_by_field_name(field).as_ref() == Some(ident);
    // `def f(a)`, `def f(a: T)`, `def f(a=1)`, `lambda a: …`, `*args`,
    // `**kwargs`, and the `a, b` of `for a, b in …` / `a, b = …`.
    if kind.ends_with("_parameter") || kind.ends_with("_pattern") || kind == "parameters" {
        return true;
    }
    // `def f`, `class C`, `(x := …)`, `type X = …`.
    if matches!(
        kind,
        "function_definition" | "class_definition" | "named_expression" | "type_alias_statement"
    ) {
        return field_is("name");
    }
    // `x = …`, `for x in …`, `with … as x`, `global x`, `nonlocal x`.
    match kind {
        "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => {
            field_is("left")
        }
        "as_pattern" => !field_is("value"),
        "global_statement" | "nonlocal_statement" => true,
        _ => false,
    }
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
        // Three subtrees are somebody else's edge already: the statement
        // that bound the name (no edge at all), a decorator (a `Calls` edge,
        // PY-017), and an annotation (a `UsesType` edge, PY-025). Emitting a
        // value edge from any of them would double-count one dependency.
        if is_import_statement(child.kind()) || matches!(child.kind(), "decorator" | "type") {
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
/// name: a binding, a callee already recorded as a `Calls` or `Instantiates`
/// edge, a keyword-argument label, or an attribute after the dot.
fn is_already_recorded(ident: &Node) -> bool {
    if is_binding(ident) {
        return true;
    }
    let Some(parent) = ident.parent() else {
        return false;
    };
    let field_is = |field| parent.child_by_field_name(field).as_ref() == Some(ident);
    match parent.kind() {
        "call" => field_is("function"),
        "keyword_argument" => field_is("name"),
        "attribute" => field_is("attribute"),
        _ => false,
    }
}

/// The id of the innermost entity whose span holds `offset`.
///
/// Two kinds of node are deliberately not candidates, though their spans
/// enclose the read:
///
/// - **Synthetic control-flow nodes.** A call inside an `if` arm is still
///   sourced from the enclosing callable and re-attached to the arm later,
///   from the `branch` key call extraction writes. Sourcing a value read
///   from the arm directly would put the two passes on different footings.
/// - **Locals.** `label = DEFAULT_LABEL` mints a `local_var` node spanning
///   the whole assignment, so it is the innermost thing holding the read —
///   but it is the *result* of the read, not the reader. The callable that
///   binds it is. A module-level constant is left in: `TOTAL = MAX_ROWS * 2`
///   at file scope really is one name depending on another.
fn enclosing_entity(entities: &[CodeEntity], offset: usize) -> Option<&str> {
    entities
        .iter()
        .filter(|e| !matches!(e.kind, EntityKind::Branch | EntityKind::Loop))
        .filter(|e| !e.tags.contains("local_var"))
        .filter(|e| e.span.start.offset <= offset && offset < e.span.end.offset)
        .min_by_key(|e| e.span.end.offset - e.span.start.offset)
        .map(|e| e.id.as_str())
}
