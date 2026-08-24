//! `UsesValue` edges from `use`d names read as values (AN-028).
//!
//! `use crate::vocab::LIMIT;` followed by `LIMIT` in a body is a dependency
//! on `vocab.rs` — change the constant and the reader changes with it — but
//! no other pass records it. `Calls` needs a call, `UsesType` needs a type
//! position, and a module of shared constants has neither, so it drew as a
//! stray nothing depends on. ADR 0027 has the reasoning; ADR 0021 has the
//! argument for why this is a dependency rather than a `References`.
//!
//! A post-pass over the tree rather than a branch inside call extraction:
//! the source of each edge is the entity whose span encloses the read, which
//! the finished entity list answers directly, and `extract_calls` — already
//! over the complexity ceiling — would have to be threaded to answer.
//!
//! ### The filter
//!
//! ADR 0021 measured 626 spurious edges from admitting every path-qualified
//! argument, and its filter is most of that ADR. A `use`d name is a far
//! narrower population — the declaration is the file saying that this name
//! comes from elsewhere — but the same care applies. Measured over this
//! repo, the cuts below take 195 candidate edges down to 7:
//!
//! - **In-crate paths only.** `crate::`, `self::` and `super::` name a
//!   module this analysis walked. `use std::collections::HashMap` and
//!   `use serde::Serialize` name something outside it, and every name they
//!   bind would land on a ghost.
//! - **Nothing inside a path, unless the path names a variant.** `CodeEntity`
//!   in `CodeEntity::new()` is an `identifier` to the grammar, exactly like a
//!   bare variable read, and admitting those would bind one edge per
//!   associated-function call on every imported type — the shape ADR 0021
//!   rejected, and redundant besides, because the call is a `Calls` edge.
//!   `Kind` in `match Kind::A` is not: no call happens and no signature
//!   mentions the type, so the file drew as depending on nothing (AN-029,
//!   ADR 0028). Both segments CamelCase is the cut — see [`owns_a_variant`].
//! - **Nothing already recorded.** A `use`d name in callee position is a
//!   `Calls` edge; a second kind for the same site would double-count one
//!   dependency.
//! - **Nothing inside a macro.** A `token_tree` is unparsed tokens: the
//!   `EntityKind` of `matches!(e.kind, EntityKind::File)` and the
//!   `named_types` of `assert_eq!(named_types(t), …)` are bare identifiers
//!   there, indistinguishable from a value read, and the two cuts above
//!   cannot see them. They were 174 of the 195. A constant read only inside
//!   a `format!` loses its edge, which is the price of not re-importing the
//!   population ADR 0021 filtered out.
//! - **No shadowed name.** `fn sanitize_git_stderr(clone_dir: &Path)` in a
//!   file that also `use`s `repo::clone_dir` reads the parameter, not the
//!   import. Telling the two apart needs scope analysis; declining the name
//!   outright costs only the files that shadow, and it is the same answer
//!   for a file whose `mod tests` `use`s a helper the file itself declares.

use super::super::language_parser::{node_text, ParseResult};
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

/// The path prefixes that name a module inside the crate being analysed.
const IN_CRATE: [&str; 3] = ["crate::", "self::", "super::"];

/// Emit one `UsesValue` edge per (reader, `use`d name) pair.
///
/// Targets are the names as the declaring module spells them, so an `as`
/// alias resolves to what it renamed rather than to a ghost. The analyzer's
/// graph builder maps them to entity ids exactly as it does `Calls` targets,
/// ranking same-name candidates by locality.
pub(super) fn emit_uses_value_edges(root: &Node, source: &str, result: &mut ParseResult) {
    let mut imported = HashMap::new();
    collect_bindings(root, source, &mut imported);
    let mut shadowed = HashSet::new();
    collect_shadows(root, source, &mut shadowed);
    imported.retain(|local, _| !shadowed.contains(local));
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

/// Every name an in-crate `use` declaration binds, mapped to the target the
/// analyzer should resolve it against.
///
/// Read from the tree rather than from the `use` text `ImportInfo` carries,
/// because a nested group — `use crate::{a::B, c::{D, E}}` — is a shape
/// string-splitting gets wrong and the grammar already models.
fn collect_bindings(node: &Node, source: &str, out: &mut HashMap<String, String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "use_declaration" {
            collect_bindings(&child, source, out);
            continue;
        }
        if let Some(arg) = child.child_by_field_name("argument") {
            if IN_CRATE
                .iter()
                .any(|p| node_text(&arg, source).starts_with(p))
            {
                bind(&arg, source, "", out);
            }
        }
    }
}

/// The names one `use` argument introduces, under the module path collected
/// so far: the last segment of a path, the alias of an `as` clause, or every
/// entry of a group, recursively.
///
/// `use crate::vocab::*` binds an unlisted set and `use crate::vocab::{self}`
/// binds the module rather than a name in it; neither answers "which name
/// came from where", so both fall through to no binding.
fn bind(node: &Node, source: &str, prefix: &str, out: &mut HashMap<String, String>) {
    match node.kind() {
        "use_as_clause" => bind_alias(node, source, prefix, out),
        "scoped_use_list" => {
            let (Some(path), Some(list)) = (
                node.child_by_field_name("path"),
                node.child_by_field_name("list"),
            ) else {
                return;
            };
            bind(
                &list,
                source,
                &joined(prefix, node_text(&path, source)),
                out,
            );
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                bind(&child, source, prefix, out);
            }
        }
        _ => {
            if let Some((local, target)) = bound_name(node, source, prefix) {
                out.insert(local, target);
            }
        }
    }
}

/// `use crate::a::base_type_name as rust_base_type_name` — read under the
/// local name, resolved under the declared one. Recording only the alias put
/// the one such edge in this repo onto a ghost.
fn bind_alias(node: &Node, source: &str, prefix: &str, out: &mut HashMap<String, String>) {
    let (Some(alias), Some(path)) = (
        node.child_by_field_name("alias"),
        node.child_by_field_name("path"),
    ) else {
        return;
    };
    if let Some((_, target)) = bound_name(&path, source, prefix) {
        out.insert(node_text(&alias, source).to_string(), target);
    }
}

/// One leaf of a `use` tree as `(the name written here, the name to resolve)`.
///
/// The resolved form is `<module>::<name>`, which is the key AN-006 built for
/// Rust free functions and the only spelling that tells three same-named
/// `base_type_name`s apart: a bare name reaches `pick_nearest`, which answers
/// even from an equidistant caller, and the wrong answer redrew a folder.
/// Under the crate root there is no module segment to qualify with — nothing
/// is referenced as `lib::foo` — so the bare name stands.
fn bound_name(node: &Node, source: &str, prefix: &str) -> Option<(String, String)> {
    let (module, name) = match node.kind() {
        "scoped_identifier" => {
            let path = node.child_by_field_name("path")?;
            let name = node.child_by_field_name("name")?;
            (
                joined(prefix, node_text(&path, source)),
                node_text(&name, source),
            )
        }
        "identifier" | "type_identifier" => (prefix.to_string(), node_text(node, source)),
        _ => return None,
    };
    let module = module.rsplit("::").next().unwrap_or("");
    let target = match module {
        "" | "crate" | "self" | "super" => name.to_string(),
        module => format!("{module}::{name}"),
    };
    Some((name.to_string(), target))
}

/// Two halves of a module path, either of which may be empty.
fn joined(prefix: &str, path: &str) -> String {
    match prefix.is_empty() {
        true => path.to_string(),
        false => format!("{prefix}::{path}"),
    }
}

/// Every name this file binds itself: a `let`, a parameter, a closure
/// argument, a match pattern, or a declaration of its own.
fn collect_shadows(node: &Node, source: &str, out: &mut HashSet<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "use_declaration" {
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
/// own: `scoped_identifier` has one too, so a written-out `crate::vocab::LIMIT`
/// would otherwise read as a declaration of `LIMIT` and shadow the import it
/// is a use of.
fn is_binding(ident: &Node) -> bool {
    let Some(parent) = ident.parent() else {
        return false;
    };
    let kind = parent.kind();
    let field_is = |field| parent.child_by_field_name(field).as_ref() == Some(ident);
    // `|x|`, `Some(x)`, `let x`, `for x in`, `fn f(x: T)`.
    if kind == "closure_parameters" || kind.ends_with("_pattern") || field_is("pattern") {
        return true;
    }
    // `fn f`, `const C`, `mod m`, `struct S`, … and the `as` rename.
    (kind.ends_with("_item") || kind == "enum_variant") && field_is("name")
        || kind == "use_as_clause" && field_is("alias")
}

/// Walk the tree collecting `(byte offset, target name)` for every read of a
/// bound name, in source order.
fn collect_reads(
    node: &Node,
    source: &str,
    imported: &HashMap<String, String>,
    reads: &mut Vec<(usize, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // The declaration that bound the name is not a use of it, and a
        // macro's token tree carries no structure to read one from.
        if matches!(child.kind(), "use_declaration" | "token_tree") {
            continue;
        }
        if child.kind() == "identifier" && !is_already_recorded(&child, source) {
            if let Some(target) = imported.get(node_text(&child, source)) {
                reads.push((child.start_byte(), target.clone()));
            }
        }
        collect_reads(&child, source, imported, reads);
    }
}

/// Whether this identifier is something other than a read of the bound name:
/// a binding, a position no value can be read from, or a callee already
/// recorded as a `Calls` edge.
fn is_already_recorded(ident: &Node, source: &str) -> bool {
    if is_binding(ident) {
        return true;
    }
    let Some(parent) = ident.parent() else {
        return false;
    };
    if reads_no_value(ident, &parent, source) {
        return true;
    }
    parent.kind() == "call_expression"
        && parent.child_by_field_name("function").as_ref() == Some(ident)
}

/// Whether the enclosing node makes this identifier something other than a
/// value read: a macro's unparsed arguments, or a path segment.
///
/// A path segment is cut *except* where the path names a variant or an
/// associated constant — see [`owns_a_variant`].
fn reads_no_value(ident: &Node, parent: &Node, source: &str) -> bool {
    if parent.kind() == "macro_invocation" {
        return true;
    }
    parent.kind().starts_with("scoped_") && !owns_a_variant(ident, parent, source)
}

/// Whether this path segment owns a variant or an associated constant —
/// `Kind::A`, `Config::DEFAULT` — rather than a function (AN-029).
///
/// ADR 0021 dropped `Type::name` paths on the stated ground that "the
/// dependency on the type they belong to is what matters and `UsesType`
/// already carries it". That holds wherever the final segment is a function:
/// the call is a `Calls` edge and the receiver reaches a type position
/// somewhere. It fails for a variant, which appears in no signature and no
/// call, so a file whose only reach into a vocabulary module was
/// `match Kind::A` drew as depending on nothing at all.
///
/// Told apart by the convention ADR 0021's own filter runs on: a type and a
/// variant are CamelCase, a module and a free function are snake_case.
/// Requiring *both* segments to be CamelCase is that filter inverted. It
/// keeps `Kind::A`, and drops `CodeEntity::new` — whose call is recorded
/// already — and `vocab::LIMIT` alike, the latter because `vocab` is a
/// module, whose only entity is the `mod` line in the file declaring it, so
/// the edge would land on the wrong file.
fn owns_a_variant(ident: &Node, parent: &Node, source: &str) -> bool {
    if parent.kind() != "scoped_identifier"
        || parent.child_by_field_name("path").as_ref() != Some(ident)
    {
        return false;
    }
    let Some(name) = parent.child_by_field_name("name") else {
        return false;
    };
    let camel = |n: &Node| node_text(n, source).starts_with(char::is_uppercase);
    camel(ident) && camel(&name)
}

/// The id of the innermost entity whose span holds `offset`.
///
/// Synthetic control-flow nodes are not candidates, for the reason call
/// extraction sources its edges from the enclosing callable: a read inside an
/// arm belongs to the function that wrote the arm.
fn enclosing_entity(entities: &[CodeEntity], offset: usize) -> Option<&str> {
    entities
        .iter()
        .filter(|e| !matches!(e.kind, EntityKind::Branch | EntityKind::Loop))
        .filter(|e| e.span.start.offset <= offset && offset < e.span.end.offset)
        .min_by_key(|e| e.span.end.offset - e.span.start.offset)
        .map(|e| e.id.as_str())
}
