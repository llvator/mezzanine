//! What a body's receivers are known to be (PY-031).
//!
//! `self.store.save(order)` is a call on whatever `store` holds. Without
//! knowing that, the target can only be spelled from the receiver's text —
//! `store.save` — which resolves to nothing and mints a ghost, so a class
//! that reaches its collaborator through a field draws as depending on none
//! of them. The same is true of `repo.find(id)` on an annotated parameter.
//!
//! This is the Python side of the pass Rust does with `struct_fields`
//! (AN-010) and Go does with its `TypeIndex`: resolve the receiver through a
//! *declared* type, and where no declaration says what a name holds, leave
//! the receiver text alone rather than guess. Python annotations are
//! optional, so this fills in where they are present and changes nothing
//! where they are not.
//!
//! Both tables are read off entities the walk has already built — the
//! enclosing class's `fields` and the callable's own `parameters` — so
//! nothing here re-walks the tree.

use super::super::super::language_parser::node_text;
use crate::models::entity::Parameter;
use std::collections::HashMap;
use tree_sitter::Node;

/// What a call to each name declared in this file evaluates to (PY-032).
///
/// One table per file, built before the walk: a builder's `set_size` may be
/// declared below the `build` that chains off it, so the answer cannot
/// depend on how far the walk has got.
///
/// Keyed by the bare method name, because that is all a chain gives us —
/// `builder.set_size(...).set_dough()` reduces to a receiver named
/// `set_size`, with no way back to the class it was called on. Where two
/// classes in one file declare the same method name with different return
/// types the name is dropped rather than resolved to one of them: a
/// confidently wrong receiver is worse than a receiver-shaped ghost.
#[derive(Default)]
pub(in crate::parser::python) struct Returns(HashMap<String, Option<String>>);

impl Returns {
    /// The type a call to `name` evaluates to, if this file says so
    /// unambiguously.
    pub(in crate::parser::python) fn of(&self, name: &str) -> Option<&str> {
        self.0.get(name)?.as_deref()
    }

    pub(in crate::parser::python) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Record what one `def` returns, dropping the name if a previous `def`
    /// in this file claimed it for a different type.
    fn record(&mut self, name: String, returns: Option<String>) {
        match self.0.get(&name) {
            Some(existing) if *existing == returns => {}
            Some(_) => {
                self.0.insert(name, None);
            }
            None => {
                self.0.insert(name, returns);
            }
        }
    }
}

/// Read every `def` in a file and record what calling it yields.
///
/// Two things say what a method returns, and Python's builders use both:
/// the return annotation (`def set_size(...) -> "PizzaBuilder"`), and the
/// fluent `return self`, which yields the enclosing class whether or not
/// anyone annotated it. The second is why this walks the tree rather than
/// reading the finished entity list — the parser's own fluent-self
/// detection runs per declaration, and the chain that needs the answer may
/// be several declarations above.
pub(in crate::parser::python) fn return_types(root: &Node, source: &str) -> Returns {
    let mut out = Returns::default();
    collect_returns(root, source, None, &mut out);
    out
}

fn collect_returns(node: &Node, source: &str, class: Option<&str>, out: &mut Returns) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_definition" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, source).to_string());
                collect_returns(&child, source, name.as_deref(), out);
            }
            "function_definition" => {
                if let Some(name) = child.child_by_field_name("name") {
                    let returns = declared_return(&child, source, class);
                    out.record(node_text(&name, source).to_string(), returns);
                }
                // A nested `def` is its own scope but still a name a chain
                // can land on, and it is not a method of `class`.
                collect_returns(&child, source, None, out);
            }
            _ => collect_returns(&child, source, class, out),
        }
    }
}

/// What one `def` returns: its annotation, or the enclosing class when the
/// body hands back `self`.
fn declared_return(function: &Node, source: &str, class: Option<&str>) -> Option<String> {
    if let Some(annotation) = function.child_by_field_name("return_type") {
        return receiver_type(node_text(&annotation, source));
    }
    let class = class?;
    let body = function.child_by_field_name("body")?;
    returns_self(&body, source).then(|| class.to_string())
}

/// Does this body hand back `self`? Stops at nested scopes — an inner
/// function returning `self` is returning the outer method's receiver, not
/// declaring itself fluent.
fn returns_self(node: &Node, source: &str) -> bool {
    if matches!(node.kind(), "function_definition" | "class_definition") {
        return false;
    }
    if node.kind() == "return_statement" {
        let mut inner = node.walk();
        let mut returned = node.named_children(&mut inner);
        if returned.any(|rc| rc.kind() == "identifier" && node_text(&rc, source) == "self") {
            return true;
        }
    }
    let mut cursor = node.walk();
    let mut children = node.children(&mut cursor);
    children.any(|child| returns_self(&child, source))
}

/// The types a body's receivers are declared to have.
pub(in crate::parser::python) struct Locals {
    /// Field name → type, for the class this callable hangs off. Reached as
    /// `self.<name>`.
    fields: HashMap<String, String>,
    /// Parameter name → type, from this callable's own signature.
    params: HashMap<String, String>,
}

impl Locals {
    /// Build the two tables from the class's fields and the callable's own
    /// parameters. Unannotated entries are dropped: a parameter whose type
    /// nobody wrote down tells the walk nothing.
    pub(in crate::parser::python) fn new(fields: &[Parameter], params: &[Parameter]) -> Self {
        Self {
            fields: declared_types(fields),
            params: declared_types(params),
        }
    }

    /// A body with no annotations anywhere, which is the common case in
    /// untyped Python — checked so call resolution can skip the lookup
    /// entirely rather than pay for two misses per receiver.
    pub(in crate::parser::python) fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.params.is_empty()
    }

    /// The type `self.<name>` holds, if the class declared one.
    pub(in crate::parser::python) fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// The type the bare name `<name>` holds, if the signature declared one.
    pub(in crate::parser::python) fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }
}

/// Name → receiver type, over the entries that carry a usable annotation.
fn declared_types(params: &[Parameter]) -> HashMap<String, String> {
    params
        .iter()
        .filter_map(|p| {
            let annotation = p.type_name.as_deref()?;
            Some((p.name.clone(), receiver_type(annotation)?))
        })
        .collect()
}

/// The single name a call target can hang off, for an annotation.
///
/// This is the *head* of the annotation, which is the opposite of what
/// [`super::super::types`] wants: a `UsesType` edge from `list[Row]` is
/// about `Row`, but a call on something annotated `list[Row]` is a call on
/// `list`. The rules, in the order they apply:
///
/// - `Store` → `Store`, and `models.Store` → `Store`: the last dotted
///   segment is the name the resolver keys on, exactly as `receiver_name`
///   reduces `self.repo` to `repo`.
/// - `Store | None` → `Store`: a PEP 604 union of a type with `None` is how
///   Python spells "optional", and the call is on the type.
/// - `Optional[Store]` / `Union[Store, None]` → `Store`: the same fact in
///   the older spelling, so it reduces the same way.
/// - `list[Row]` → `list`: the receiver is the container. It resolves to a
///   stdlib ghost, which is the same honest answer `[1, 2].append(x)`
///   already gets from `literal_type_name`.
/// - `"Store"` → `Store`: a string annotation is a forward reference, not a
///   different type.
///
/// Anything that is not a bare identifier after all that — a `Callable[…]`
/// arrow, a literal, an expression — answers `None`, and the caller keeps
/// the receiver text it already had.
pub(in crate::parser::python) fn receiver_type(annotation: &str) -> Option<String> {
    let head = unwrap_optional(unquote(annotation.trim()))?;
    let name = head.rsplit('.').next()?.trim();
    let is_identifier = !name.is_empty()
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
        && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    is_identifier.then(|| name.to_string())
}

/// Strip the quotes off a forward-reference annotation.
fn unquote(annotation: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = annotation
            .strip_prefix(quote)
            .and_then(|s| s.strip_suffix(quote))
        {
            return inner.trim();
        }
    }
    annotation
}

/// Peel one layer of "optional" off an annotation and return its head.
///
/// `Optional[Store]` and `Union[Store, None]` unwrap to their first argument;
/// `Store | None` takes its first arm. Any other subscript keeps its head, so
/// `list[Row]` stays `list`. Recursion is one level deep per call and
/// terminates because each step strictly shortens the text it hands on.
fn unwrap_optional(annotation: &str) -> Option<&str> {
    let annotation = annotation.trim();
    if let Some((first, _)) = annotation.split_once('|') {
        return unwrap_optional(first);
    }
    let Some((head, rest)) = annotation.split_once('[') else {
        return (!annotation.is_empty()).then_some(annotation);
    };
    let head = head.trim();
    if !matches!(head.rsplit('.').next(), Some("Optional") | Some("Union")) {
        return (!head.is_empty()).then_some(head);
    }
    let inner = rest.strip_suffix(']').unwrap_or(rest);
    let first = inner.split(',').next()?;
    unwrap_optional(first)
}

#[cfg(test)]
mod tests {
    use super::receiver_type;

    fn head(annotation: &str) -> Option<String> {
        receiver_type(annotation)
    }

    #[test]
    fn plain_and_dotted_names_reduce_to_the_last_segment() {
        assert_eq!(head("Store").as_deref(), Some("Store"));
        assert_eq!(head("models.Store").as_deref(), Some("Store"));
        assert_eq!(head("  Store  ").as_deref(), Some("Store"));
    }

    #[test]
    fn optional_spellings_all_reduce_to_the_type() {
        assert_eq!(head("Store | None").as_deref(), Some("Store"));
        assert_eq!(head("Optional[Store]").as_deref(), Some("Store"));
        assert_eq!(head("Union[Store, None]").as_deref(), Some("Store"));
        assert_eq!(
            head("typing.Optional[models.Store]").as_deref(),
            Some("Store")
        );
    }

    #[test]
    fn a_container_is_the_receiver_not_its_element() {
        assert_eq!(head("list[Row]").as_deref(), Some("list"));
        assert_eq!(head("dict[str, Row]").as_deref(), Some("dict"));
    }

    #[test]
    fn a_forward_reference_reads_like_the_name_it_quotes() {
        assert_eq!(head("\"Store\"").as_deref(), Some("Store"));
        assert_eq!(head("'Store'").as_deref(), Some("Store"));
    }

    #[test]
    fn anything_that_is_not_a_name_declines() {
        assert_eq!(head(""), None);
        assert_eq!(head("   "), None);
        assert_eq!(head("Callable[[int], str]").as_deref(), Some("Callable"));
        assert_eq!(head("3"), None);
    }
}
