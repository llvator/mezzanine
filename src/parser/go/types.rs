//! `UsesType` edges from the types a declaration names — the Go half of
//! what RS-001 did for Rust and JV-001 for Java.
//!
//! The difference is where the names come from. Those parsers read the
//! type back out of the string they stored on the entity and pick out the
//! uppercase-initial tokens, because in their languages an uppercase
//! initial is what a type looks like. In Go it is what an *exported* name
//! looks like — `type parser struct` is a perfectly ordinary type — so
//! that rule would silently drop every unexported type in a package, which
//! in most Go code is most of them.
//!
//! So this reads the grammar instead of the text. Tree-sitter-go
//! distinguishes `type_identifier` from `identifier`, which means the
//! parameter names inside `func(count int) error` are never mistaken for
//! types — the failure a text scan makes first and hides longest.
//!
//! Two names are dropped: the predeclared types, which carry no dependency
//! anywhere, and anything qualified by a standard-library import, for the
//! same reason `fmt.Println` is not drawn as a call.

use super::helpers::is_predeclared_type;
use super::packages::Imports;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use tree_sitter::Node;

/// Everything one emission needs to know beyond the nodes to read.
pub(super) struct TypeUse<'a> {
    /// The entity the edges start from.
    pub entity_id: &'a str,
    /// Its own name, so a recursive mention (`next *Node` inside `Node`)
    /// does not become an edge to itself.
    pub owner_name: &'a str,
    pub source: &'a str,
    pub imports: &'a Imports,
}

/// Emit one `UsesType` edge per distinct type named in `roots`.
///
/// `skip` names node kinds whose subtrees belong to some *other* entity —
/// an interface's `method_elem`s, whose signature types are the method's
/// dependency and not the interface's.
pub(super) fn emit(
    roots: &[Node],
    skip: &[&str],
    use_site: &TypeUse<'_>,
    result: &mut ParseResult,
) {
    let mut named: Vec<String> = Vec::new();
    for root in roots {
        collect(root, skip, use_site, &mut named);
    }
    for target in named {
        result.add_relationship(Relationship::new(
            use_site.entity_id.to_string(),
            target,
            RelationshipKind::UsesType,
        ));
    }
}

/// Walk a subtree gathering the types it names, in source order and
/// without repeats.
fn collect(node: &Node, skip: &[&str], use_site: &TypeUse<'_>, out: &mut Vec<String>) {
    let mut stack = vec![*node];
    // Reversed pushes would be needed for strict source order; the order
    // only has to be stable, and a depth-first walk of a fixed tree is.
    while let Some(current) = stack.pop() {
        if skip.contains(&current.kind()) {
            continue;
        }
        // A qualified type is one name. Descending into it would yield its
        // own `type_identifier` a second time, unqualified.
        if current.kind() == "qualified_type" {
            if let Some(name) = qualified_name(&current, use_site) {
                push_unique(out, name, use_site.owner_name);
            }
            continue;
        }
        if current.kind() == "type_identifier" {
            let name = node_text(&current, use_site.source);
            if !is_predeclared_type(name) {
                push_unique(out, name.to_string(), use_site.owner_name);
            }
            continue;
        }
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// `store.Row` — kept whole, under the package's real name rather than
/// this file's alias for it, so it lines up with the `qualified_name`
/// the `store` package's own entities carry.
fn qualified_name(node: &Node, use_site: &TypeUse<'_>) -> Option<String> {
    let local = node_text(&node.child_by_field_name("package")?, use_site.source);
    if use_site.imports.is_stdlib_qualifier(local) {
        return None;
    }
    let name = node_text(&node.child_by_field_name("name")?, use_site.source);
    let package = use_site.imports.package_for(local).unwrap_or(local);
    Some(format!("{}.{}", package, name))
}

fn push_unique(out: &mut Vec<String>, name: String, owner_name: &str) {
    if name == owner_name || out.contains(&name) {
        return;
    }
    out.push(name);
}
