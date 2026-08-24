//! Shared parsing helpers: visibility, modifier attributes, parameters,
//! generics, and annotation-name extraction.

use super::super::language_parser::{find_child_by_kind, node_text};
use crate::models::entity::Parameter;
use crate::models::Visibility;
use tree_sitter::Node;

pub(super) fn parse_visibility(node: &Node) -> Visibility {
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            match child.kind() {
                "public" => return Visibility::Public,
                "private" => return Visibility::Private,
                "protected" => return Visibility::Protected,
                _ => {}
            }
        }
    }
    // Groovy defaults to `public` for top-level definitions; `private`
    // is rare. Match Java's convention and surface the absence as
    // package-private (`Internal`) so a missing modifier is visible
    // rather than masquerading as `public`.
    Visibility::Internal
}

pub(super) fn parse_modifier_attributes(node: &Node, source: &str) -> Vec<String> {
    let mut attrs = Vec::new();
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            match child.kind() {
                "public" | "private" | "protected" => {}
                "static" | "final" | "abstract" | "synchronized" | "native" | "transient"
                | "volatile" | "default" | "strictfp" => {
                    attrs.push(child.kind().to_string());
                }
                "marker_annotation" | "annotation" => {
                    attrs.push(node_text(&child, source).to_string());
                }
                _ => {}
            }
        }
    }
    attrs
}

pub(super) fn parse_parameters(node: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
            let name = child
                .child_by_field_name("name")
                .map(|n| node_text(&n, source).to_string())
                .unwrap_or_default();
            let type_name = child
                .child_by_field_name("type")
                .and_then(|t| declared_type(&t, source));
            params.push(Parameter {
                name,
                type_name,
                default_value: None,
                visibility: None,
            });
        }
    }
    params
}

/// Control-flow keywords `tree-sitter-groovy` degrades into a
/// `method_invocation` when the statement sits directly in the body of a
/// parameterized closure — `list.each { it -> if (ok(it)) { use(it) } }`
/// parses the `if` as an invocation named `if`, with the condition as its
/// `arguments` and the guarded block as its `body`.
///
/// Left unrecognised, that shape does two kinds of damage: the keyword
/// becomes a ghost entity accumulating `Calls` edges from across the
/// codebase (the same defect `def` had, GR-015), and the branch it guards
/// disappears from the graph. Since `{ it -> … }` is *the* Groovy idiom,
/// the shape is common rather than exotic.
///
/// Returns the keyword when `node` is one of these degraded statements.
/// Receiver-qualified calls are excluded — `foo.if(…)` would be a real
/// method named `if`, not a statement.
pub(super) fn flow_keyword_invocation(node: &Node, source: &str) -> Option<&'static str> {
    if node.kind() != "method_invocation" || node.child_by_field_name("object").is_some() {
        return None;
    }
    let name = node.child_by_field_name("name")?;
    match node_text(&name, source) {
        "if" => Some("if"),
        "while" => Some("while"),
        "for" => Some("for"),
        "switch" => Some("switch"),
        _ => None,
    }
}

/// Groovy's dynamic-typing keywords. The grammar has no separate node for
/// them — `def x` and `String x` both come through as a `type` field
/// holding a `type_identifier` — so they arrive looking exactly like a
/// declared type. They are the *absence* of one (GR-015).
const DYNAMIC_TYPE_KEYWORDS: &[&str] = &["def", "var"];

/// Read a declared type off a grammar `type` node, returning `None` when
/// the "type" is really `def` or `var`. Emitting those verbatim made
/// every dynamically-typed declaration in a Groovy codebase point at one
/// shared ghost entity named `def` — a high-degree node meaning nothing,
/// which distorted every centrality and fan-in reading of the graph.
pub(super) fn declared_type(node: &Node, source: &str) -> Option<String> {
    let text = node_text(node, source).trim();
    if DYNAMIC_TYPE_KEYWORDS.contains(&text) {
        return None;
    }
    Some(text.to_string())
}

pub(super) fn parse_generics(node: &Node, source: &str) -> Vec<String> {
    let mut generics = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_parameter" {
            generics.push(node_text(&child, source).to_string());
        }
    }
    generics
}

/// Type-node kinds a `superclass` / `super_interfaces` wrapper can hold.
const TYPE_KINDS: &[&str] = &["type_identifier", "scoped_type_identifier", "generic_type"];

/// Extract the first type named under a wrapper node (`superclass`).
pub(super) fn extract_first_type(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if TYPE_KINDS.contains(&child.kind()) {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

/// Extract every type named under a wrapper node (`super_interfaces`),
/// descending one level into the `type_list` the grammar nests them in.
pub(super) fn extract_type_list(node: &Node, source: &str) -> Vec<String> {
    let mut types = Vec::new();
    push_direct_types(node, source, &mut types);
    if let Some(list) = find_child_by_kind(node, "type_list") {
        push_direct_types(&list, source, &mut types);
    }
    types
}

/// Append the type-node children of `node` in source order.
fn push_direct_types(node: &Node, source: &str, out: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if TYPE_KINDS.contains(&child.kind()) {
            out.push(node_text(&child, source).to_string());
        }
    }
}

/// Return the name text of an annotation node (`marker_annotation` or
/// `annotation`). Used to spot `@Field` / `@groovy.transform.Field`.
pub(super) fn annotation_name(node: &Node, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    Some(node_text(&name_node, source).to_string())
}

/// True if the annotation is a Groovy `@Field` declaration — accepts
/// the short form (`@Field`) and the fully qualified form
/// (`@groovy.transform.Field`).
pub(super) fn is_field_annotation(name: &str) -> bool {
    name == "Field" || name == "groovy.transform.Field"
}
