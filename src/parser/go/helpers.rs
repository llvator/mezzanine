//! Shared reading of a Go declaration's surface: visibility, parameters,
//! results, type parameters, and the receiver that binds a method to a type.

use super::super::language_parser::{node_text, node_to_span};
use crate::models::entity::Parameter;
use crate::models::Visibility;
use tree_sitter::Node;

/// Go's access rule, which is spelled in the identifier rather than in a
/// modifier: an uppercase initial exports the name from its package, and
/// anything else confines it to the package.
///
/// Unexported maps to [`Visibility::Internal`], not `Private`. There is no
/// narrower scope in the language — an unexported field is reachable from
/// every file of its own package — and `Internal` is the variant the other
/// parsers already use for exactly that (Java's package-private).
pub(super) fn visibility_of(name: &str) -> Visibility {
    match name.chars().next() {
        Some(c) if c.is_uppercase() => Visibility::Public,
        _ => Visibility::Internal,
    }
}

/// Parameters from a `parameter_list`.
///
/// Go lets one declaration carry several names against a single type
/// (`func Move(x, y int)`), and lets a parameter carry no name at all
/// (`func(io.Writer)` in an interface or a function type). Both shapes come
/// back as `parameter_declaration`; the first yields one `Parameter` per
/// name, the second one anonymous `Parameter` so the arity stays truthful.
pub(super) fn parse_parameters(node: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "parameter_declaration" => params.extend(plain_parameters(&child, source)),
            "variadic_parameter_declaration" => params.push(variadic_parameter(&child, source)),
            _ => {}
        }
    }

    params
}

/// `x, y int` — one type, one `Parameter` per name, or one anonymous
/// `Parameter` when the declaration names nothing.
fn plain_parameters(declaration: &Node, source: &str) -> Vec<Parameter> {
    let type_name = declaration
        .child_by_field_name("type")
        .map(|t| node_text(&t, source).to_string());
    let names = field_texts(declaration, "name", source);
    if names.is_empty() {
        return vec![parameter(String::new(), type_name)];
    }
    names
        .into_iter()
        .map(|name| parameter(name, type_name.clone()))
        .collect()
}

/// `rest ...string` — the trailing parameter, whose type is written with
/// the ellipsis so a reader of the signature sees the arity is open.
fn variadic_parameter(declaration: &Node, source: &str) -> Parameter {
    let type_name = declaration
        .child_by_field_name("type")
        .map(|t| format!("...{}", node_text(&t, source)));
    let name = declaration
        .child_by_field_name("name")
        .map(|n| node_text(&n, source).to_string())
        .unwrap_or_default();
    parameter(name, type_name)
}

/// A parameter carrying no default and no visibility of its own — which is
/// every Go parameter, since the language has neither.
fn parameter(name: String, type_name: Option<String>) -> Parameter {
    Parameter {
        name,
        type_name,
        default_value: None,
        visibility: None,
    }
}

/// Every child of `node` under the repeated field `name`. `child_by_field_name`
/// answers with the first only, which loses the `x, y int` shape.
pub(super) fn field_texts(node: &Node, field: &str, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    node.children_by_field_name(field, &mut cursor)
        .map(|n| node_text(&n, source).to_string())
        .collect()
}

/// The declared result of a function, method, or function type, verbatim.
///
/// Go's `result` field is either a single type (`error`) or a parameter
/// list (`(*User, error)`). Both are kept as written: the parenthesised
/// form is what the reader sees, and [`result_arity`] is where the count
/// is read off.
pub(super) fn result_text(node: &Node, source: &str) -> Option<String> {
    let result = node.child_by_field_name("result")?;
    Some(node_text(&result, source).to_string())
}

/// How many values a callable returns, when it returns more than one.
///
/// This is the metric `return_complexity` was defined for, and Go is the
/// language where it is a first-class fact rather than an inference about
/// tuples: `(*User, error)` returns two values, and the caller must name or
/// discard both. A single result and no result both score `None` — there is
/// nothing to weigh.
pub(super) fn result_arity(node: &Node) -> Option<u32> {
    let result = node.child_by_field_name("result")?;
    if result.kind() != "parameter_list" {
        return None;
    }
    let mut cursor = result.walk();
    let count: u32 = result
        .children(&mut cursor)
        .filter(|c| {
            matches!(
                c.kind(),
                "parameter_declaration" | "variadic_parameter_declaration"
            )
        })
        .count() as u32;
    (count > 1).then_some(count)
}

/// Type-parameter names from a `type_parameter_list` (Go 1.18 generics),
/// each written with its constraint as the source has it.
pub(super) fn parse_generics(node: &Node, source: &str) -> Vec<String> {
    let mut generics = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_parameter_declaration" {
            generics.push(node_text(&child, source).to_string());
        }
    }
    generics
}

/// A method's receiver: the type it hangs off, and whether it takes that
/// type by pointer.
///
/// The receiver *variable* is deliberately not here. It matters just as
/// much — inside the method every `s.other()` is a call to `Server.other` —
/// but the code that needs it,
/// [`super::bodies::inference::Locals::for_body`], reads the whole receiver
/// list along with the parameter list, because a receiver is a parameter
/// with a keyword in front of it. Copying the name into a second place
/// would give the two readings a chance to disagree.
///
/// Pointer-ness is kept because it is a distinction a reader of the graph
/// cares about: a value receiver copies, so it cannot mutate what it was
/// called on, and a type whose methods are split between the two forms is
/// usually a bug waiting.
pub(super) struct Receiver {
    pub type_name: String,
    pub is_pointer: bool,
}

/// Read the receiver of a `method_declaration`, or `None` for anything
/// that has none.
pub(super) fn receiver(node: &Node, source: &str) -> Option<Receiver> {
    let list = node.child_by_field_name("receiver")?;
    let mut cursor = list.walk();
    let declaration = list
        .children(&mut cursor)
        .find(|c| c.kind() == "parameter_declaration")?;
    let written = node_text(&declaration.child_by_field_name("type")?, source);
    Some(Receiver {
        type_name: base_type_name(written),
        is_pointer: written.trim_start().starts_with('*'),
    })
}

/// A written-out type with its decoration removed but its package kept:
/// `*Server` → `Server`, `[]*store.Row` → `store.Row`,
/// `Cache[string]` → `Cache`.
///
/// Pointers, slices, and type arguments are all decoration around one name.
/// The package is *not* decoration — `sync.WaitGroup` and a project type
/// called `WaitGroup` are different types, and telling them apart is what
/// keeps `wg.Add(1)` from drawing an edge to a standard-library method.
pub(super) fn strip_decoration(text: &str) -> &str {
    let head = text
        .trim()
        .trim_start_matches(['*', '[', ']', '&', '~', '<', '-'])
        .trim();
    head.split(['[', '(', ' ']).next().unwrap_or(head)
}

/// The bare name at the heart of a written-out type, package and all
/// dropped: `[]*store.Row` → `Row`.
///
/// Right where the package cannot matter — a method receiver is always a
/// type of the declaring package, and an embedded field is named for its
/// own last segment.
pub(super) fn base_type_name(text: &str) -> String {
    let head = strip_decoration(text);
    head.rsplit('.').next().unwrap_or(head).to_string()
}

/// Go's predeclared type names — the ones every file can use without
/// importing anything.
///
/// They are dropped from `UsesType` edges and from call qualifiers alike.
/// `err.Error()` names the built-in `error`, and an edge to `error.Error`
/// is noise in every graph it turns up in; `count int` names a type no
/// project owns.
pub(super) fn is_predeclared_type(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "bool"
            | "byte"
            | "comparable"
            | "complex64"
            | "complex128"
            | "error"
            | "float32"
            | "float64"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "rune"
            | "string"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
    )
}

/// Per-callable metrics that do not depend on the body: span, arity, and
/// the result count. Complexity is added by the caller, which is the only
/// place that knows whether there is a body to walk.
pub(super) fn populate_signature_metrics(node: &Node, entity: &mut crate::models::CodeEntity) {
    let span = node_to_span(node);
    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    entity.metrics.param_count = Some(entity.parameters.len() as u32);
    entity.metrics.return_complexity = result_arity(node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uppercase_initial_is_what_exports_a_name() {
        assert_eq!(visibility_of("Handle"), Visibility::Public);
        assert_eq!(visibility_of("handle"), Visibility::Internal);
        assert_eq!(visibility_of("_private"), Visibility::Internal);
        assert_eq!(visibility_of(""), Visibility::Internal);
    }

    #[test]
    fn decoration_around_a_type_name_is_not_part_of_it() {
        assert_eq!(base_type_name("*Server"), "Server");
        assert_eq!(base_type_name("[]*store.Row"), "Row");
        assert_eq!(base_type_name("Cache[string]"), "Cache");
        assert_eq!(base_type_name("  Order  "), "Order");
    }

    /// The package survives, because `sync.WaitGroup` and a project type
    /// called `WaitGroup` are different types.
    #[test]
    fn a_package_qualifier_is_not_decoration() {
        assert_eq!(strip_decoration("*store.Row"), "store.Row");
        assert_eq!(strip_decoration("[]sync.Mutex"), "sync.Mutex");
        assert_eq!(strip_decoration("<-chan"), "chan");
        assert_eq!(strip_decoration("Pool[T]"), "Pool");
    }
}
