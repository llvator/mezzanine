//! Small shared helpers: node lookup, declared types, parameters, generics.
//!
//! The grammar names no fields, so every lookup here is by node kind. It
//! does wrap every declared type in a `type` node, which is what keeps this
//! module small — a type is read, not reconstructed, and `Future<Order?>`
//! arrives spelled the way the author wrote it.

use super::super::language_parser::{node_text, ParseResult};
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, Position, Span, Visibility};
use tree_sitter::Node;

/// Dart visibility is a naming convention, not a modifier: a leading
/// underscore makes a declaration private to its library, and everything
/// else is public. Same rule the Python parser applies.
pub(super) fn visibility_of(name: &str) -> Visibility {
    if name.starts_with('_') {
        Visibility::Private
    } else {
        Visibility::Public
    }
}

/// First direct child of `node` with the given kind.
pub(super) fn child_of_kind<'t>(node: &Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|c| c.kind() == kind);
    found
}

/// First direct child of `node` whose kind is any of `kinds`, in tree order.
pub(super) fn child_of_any<'t>(node: &Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|c| kinds.contains(&c.kind()));
    found
}

/// Every direct child of `node` with the given kind.
pub(super) fn children_of_kind<'t>(node: &Node<'t>, kind: &str) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| c.kind() == kind)
        .collect()
}

/// The declared type written on `node`, as the author spelled it.
///
/// A `type` node covers the whole spelling — nullability, generics, library
/// prefix — so there is nothing to reassemble. Only the *first* one counts:
/// a `formal_parameter` has exactly one, and on a signature the first is the
/// return type, every later one belonging to a parameter.
pub(super) fn declared_type(node: &Node, source: &str) -> Option<String> {
    let type_node = child_of_kind(node, "type")?;
    let text = node_text(&type_node, source).trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A span running from the start of `first` to the end of `last`.
pub(super) fn span_over(first: &Node, last: &Node) -> Span {
    Span::new(
        Position::new(
            first.start_position().row,
            first.start_position().column,
            first.start_byte(),
        ),
        Position::new(
            last.end_position().row,
            last.end_position().column,
            last.end_byte(),
        ),
    )
}

/// Parse a `formal_parameter_list` into parameters.
///
/// Dart's optional groups — `{named, params}` and `[positional, ones]` —
/// arrive as an `optional_formal_parameters` child holding its own
/// `formal_parameter`s, so the two levels are flattened here: a caller
/// asking for a signature's parameters wants all of them.
pub(super) fn parse_parameters(list: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = list.walk();
    for child in list.children(&mut cursor) {
        match child.kind() {
            "formal_parameter" => params.push(parse_parameter(&child, source)),
            "optional_formal_parameters" => parse_optional_parameters(&child, source, &mut params),
            _ => {}
        }
    }
    params
}

/// Parse one optional group. A default value follows its parameter as a
/// sibling expression rather than a child of it, so each is attached to the
/// parameter most recently pushed.
fn parse_optional_parameters(group: &Node, source: &str, params: &mut Vec<Parameter>) {
    let mut cursor = group.walk();
    for child in group.children(&mut cursor) {
        if child.kind() == "formal_parameter" {
            params.push(parse_parameter(&child, source));
        } else if child.is_named() {
            if let Some(param) = params.last_mut() {
                param.default_value = Some(node_text(&child, source).to_string());
            }
        }
    }
}

/// Parse one `formal_parameter`.
///
/// A field-initialising parameter (`this.client`) arrives as a
/// `constructor_param` child and carries no type of its own — the type lives
/// on the field it initialises, which this parser does record, so leaving it
/// `None` here loses nothing and inventing one would lie.
fn parse_parameter(node: &Node, source: &str) -> Parameter {
    let (name_holder, type_name) = match child_of_kind(node, "constructor_param") {
        Some(field_param) => (field_param, None),
        None => (*node, declared_type(node, source)),
    };
    let name = child_of_kind(&name_holder, "identifier")
        .map(|n| node_text(&n, source).to_string())
        .unwrap_or_else(|| node_text(node, source).to_string());

    Parameter {
        name,
        type_name,
        default_value: None,
        visibility: None,
    }
}

/// Parse a `type_parameters` node into generic-parameter spellings.
pub(super) fn parse_generics(node: &Node, source: &str) -> Vec<String> {
    children_of_kind(node, "type_parameter")
        .iter()
        .map(|n| node_text(n, source).to_string())
        .collect()
}

/// Register a parsed entity when the extractor produced one.
pub(super) fn add_parsed(parsed: Option<CodeEntity>, result: &mut ParseResult) {
    if let Some(entity) = parsed {
        result.add_entity(entity);
    }
}
