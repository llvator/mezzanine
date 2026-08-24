//! Declaration modifiers and annotations, read off the tree.
//!
//! Kept apart from `helpers` because these two lists are the parser's
//! opinion about Dart rather than a way of reading nodes: what counts as a
//! modifier worth recording, and where an annotation is written relative to
//! the thing it annotates.

use crate::parser::language_parser::node_text;
use tree_sitter::Node;

/// Modifier tokens worth recording. The grammar leaves them as bare
/// keywords, so the token's own kind is the word.
///
/// The Dart 3 class modifiers are in here with the rest: the vendored
/// grammar reads them natively, so `sealed class Shape` needs no special
/// handling anywhere in this parser.
const MODIFIERS: &[&str] = &[
    "abstract",
    "base",
    "const",
    "covariant",
    "external",
    "factory",
    "final",
    "interface",
    "late",
    "mixin",
    "sealed",
    "static",
];

/// True when `word` is a modifier this parser records.
pub(super) fn is_modifier(word: &str) -> bool {
    MODIFIERS.contains(&word)
}

/// Modifiers written on `node`, and any annotation attached to it.
pub(super) fn declared_attributes(node: &Node, source: &str) -> Vec<String> {
    let mut attributes = modifiers_on(node);
    attributes.extend(annotations_on(node, source));
    attributes
}

/// Modifiers written directly on a node — not on its children, since a
/// member's `static` and its signature's `factory` are separate nodes and
/// callers read both deliberately.
pub(super) fn modifiers_on(node: &Node) -> Vec<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| is_modifier(child.kind()))
        .map(|child| child.kind().to_string())
        .collect()
}

/// The `@override` / `@Deprecated('…')` attached to a declaration.
///
/// The grammar nests an annotation *inside* the declaration it annotates, so
/// this is an ordinary child scan rather than the backwards sibling walk the
/// Java and Kotlin extractors need.
fn annotations_on(node: &Node, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| matches!(child.kind(), "annotation" | "marker_annotation"))
        .map(|child| node_text(&child, source).to_string())
        .collect()
}
