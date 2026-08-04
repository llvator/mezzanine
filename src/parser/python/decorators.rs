//! Decorator collection for Python functions and classes: the text kept on
//! the entity, and the dependency edges the decorators imply.

use super::super::language_parser::{node_text, ParseResult};
use crate::models::{Relationship, RelationshipKind};
use tree_sitter::Node;

/// Decorators that are language protocol, not dependencies. Each is already
/// captured as a tag on the entity (`static`, `classmethod`, `property`,
/// `abstract`, `overload_stub`), so an edge would add noise to every method
/// in a codebase without adding information.
const BUILTIN_DECORATORS: &[&str] = &[
    "property",
    "staticmethod",
    "classmethod",
    "abstractmethod",
    "abstractproperty",
    "override",
    "overload",
    "final",
    "cached_property",
];

/// Extract decorator attributes from the `decorated_definition` parent.
pub(super) fn extract_decorators(node: &Node, source: &str, attrs: &mut Vec<String>) {
    for decorator in decorators_of(node) {
        attrs.push(node_text(&decorator, source).to_string());
    }
}

/// Emit one `Calls` edge per decorator, from the decorated entity to the
/// decorator that wraps it (PY-017).
///
/// A decorator runs at definition time and changes what the decorated name
/// means, which makes it a real dependency — but it was only ever captured as
/// text on `attributes`, so a function wrapped in a project's own `@retry` or
/// `@router.get` looked unconnected to it.
///
/// The target is the decorator's *name*, with any call stripped:
/// `@lru_cache(maxsize=8)` depends on `lru_cache`, not on `lru_cache(...)`.
/// Arguments are not walked — a decorator factory's arguments are
/// configuration, and treating them as call sites of the decorated function
/// would attribute them to the wrong scope.
///
/// The edge carries `decorator: true` so a UI can render it apart from an
/// ordinary call. It carries no `order`: decorators apply bottom-up at
/// definition time, which is not the call ordering that key means.
pub(super) fn emit_decorator_edges(
    node: &Node,
    source: &str,
    entity_id: &str,
    result: &mut ParseResult,
) {
    for decorator in decorators_of(node) {
        let Some(name) = decorator_target(&decorator, source) else {
            continue;
        };
        if BUILTIN_DECORATORS.contains(&name.rsplit('.').next().unwrap_or(name.as_str())) {
            continue;
        }
        let mut rel = Relationship::new(entity_id.to_string(), name, RelationshipKind::Calls);
        rel.metadata
            .insert("decorator".to_string(), "true".to_string());
        result.add_relationship(rel);
    }
}

/// The name a decorator refers to: `@foo` → `foo`, `@foo.bar` → `foo.bar`,
/// `@foo(args)` → `foo`.
///
/// Dotted names are kept whole rather than reduced to a last segment:
/// `@router.get` and `@app.get` are different dependencies, and the receiver
/// is the part that says which.
fn decorator_target(decorator: &Node, source: &str) -> Option<String> {
    let inner = decorator.named_child(0)?;
    let named = match inner.kind() {
        "call" => inner.child_by_field_name("function")?,
        _ => inner,
    };
    match named.kind() {
        "identifier" | "attribute" | "dotted_name" => Some(node_text(&named, source).to_string()),
        _ => None,
    }
}

/// The `decorator` children of a definition's `decorated_definition` parent,
/// or nothing when the definition is undecorated.
fn decorators_of<'a>(node: &Node<'a>) -> Vec<Node<'a>> {
    let Some(parent) = node.parent() else {
        return Vec::new();
    };
    if parent.kind() != "decorated_definition" {
        return Vec::new();
    }
    let mut cursor = parent.walk();
    let found: Vec<Node<'a>> = parent
        .children(&mut cursor)
        .filter(|c| c.kind() == "decorator")
        .collect();
    found
}
