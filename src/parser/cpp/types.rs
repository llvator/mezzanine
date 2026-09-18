//! `UsesType` edges from the types a declaration names (§C3) — the C++
//! half of what RS-001 did for Rust and JV-001 for Java.
//!
//! It reads the grammar rather than the type's text. Tree-sitter-cpp
//! distinguishes `type_identifier` from `identifier` and `primitive_type`,
//! which means the parameter names inside `run(int count)` are never
//! mistaken for types — the failure a text scan makes first and hides
//! longest — and `int`, `double` and `void` never reach the graph at all.
//!
//! Targets are the bare type name, not the qualified one. `app::Order` in
//! a signature emits `Order`, and the resolver ranks the candidates by
//! locality. The alternative — emitting `app::Order` — reads better and
//! resolves worse: C++ writes a type by whichever prefix is in scope at
//! the point of use (`Order`, `core::Order`, `app::core::Order` are the
//! same type on three lines of the same file), so a qualified target
//! matches the entity only when the author happened to spell it in full.
//!
//! One filter, on the standard library's own names. `std::vector<Item>`
//! names two types and only one of them is a dependency worth drawing;
//! the table below is what tells them apart, and it is deliberately small
//! — an unknown name resolves to nothing and is dropped by the resolver,
//! so a false positive costs a ghost and never a wrong edge.

use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use tree_sitter::Node;

/// Everything one emission needs beyond the nodes to read.
pub(super) struct TypeUse<'a> {
    /// The entity the edges start from.
    pub entity_id: &'a str,
    /// Its own name, so a recursive mention — `Node* next` inside `Node` —
    /// does not become an edge to itself.
    pub owner_name: &'a str,
    pub source: &'a str,
}

/// Standard-library type names that carry no project-level dependency.
const STD_TYPES: &[&str] = &[
    "string", "wstring", "string_view", "vector", "array", "deque", "list", "forward_list", "map",
    "unordered_map", "multimap", "set", "unordered_set", "multiset", "pair", "tuple", "optional",
    "variant", "any", "function", "unique_ptr", "shared_ptr", "weak_ptr", "atomic", "mutex",
    "lock_guard", "unique_lock", "thread", "future", "promise", "exception", "runtime_error",
    "logic_error", "invalid_argument", "out_of_range", "initializer_list", "size_t", "ptrdiff_t",
    "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t",
    "ostream", "istream", "ostringstream", "istringstream", "stringstream", "filesystem", "path",
    "chrono", "duration", "time_point", "byte", "nullptr_t",
];

/// Emit one `UsesType` edge per distinct type named in `roots`.
///
/// `skip` names node kinds whose subtrees belong to some other entity — a
/// class body, whose members each emit their own.
pub(super) fn emit(roots: &[Node], skip: &[&str], use_site: &TypeUse<'_>, result: &mut ParseResult) {
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

/// Walk a subtree gathering the types it names, without repeats.
fn collect(node: &Node, skip: &[&str], use_site: &TypeUse<'_>, out: &mut Vec<String>) {
    let mut stack = vec![*node];
    while let Some(current) = stack.pop() {
        if skip.contains(&current.kind()) {
            continue;
        }
        if current.kind() == "type_identifier" {
            let name = node_text(&current, use_site.source);
            if !STD_TYPES.contains(&name) && name != use_site.owner_name && !out.iter().any(|t| t == name)
            {
                out.push(name.to_string());
            }
            continue;
        }
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
    }
}
