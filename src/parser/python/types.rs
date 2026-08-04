//! Type-usage edge extraction (PY-025, sibling of RS-001).
//!
//! Emits `UsesType` relationships from callables to the named types in
//! their parameter/return annotations, and from classes (incl. dataclass /
//! abstract / enum promotions) to their annotated attribute types — so a
//! type used only in annotations gains real dependents in the graph.
//!
//! Strictly annotation-based: Python annotations are optional, and where
//! they are absent this pass emits nothing. No inference — the value comes
//! from what the code already says.
//!
//! Extraction rule (deliberately simple, mirroring RS-001): strip any
//! surrounding quotes from the annotation (string annotations / `from
//! __future__ import annotations` — `"Order"` behaves like `Order`), then
//! every `Uppercase`-initial identifier token is a candidate, minus a small
//! std/typing table. The token split handles PEP 604 unions
//! (`Order | None` → Order) and subscripted generics (`dict[str, Order]` →
//! Order) for free; lowercase builtins (`list`, `dict`, `str`) never match
//! the uppercase rule, which is correct. The table is kept small on
//! purpose: an unknown name slipping through resolves to nothing and is
//! dropped by the resolver, so false positives are cheap.

use std::collections::HashMap;

use crate::models::{EntityKind, Relationship, RelationshipKind};

use super::super::language_parser::ParseResult;

/// Std/typing names that carry no project-level dependency signal.
const STD_TYPES: &[&str] = &[
    // typing / collections.abc
    "List", "Dict", "Set", "FrozenSet", "Tuple", "Optional", "Union", "Any",
    "Callable", "Iterator", "Iterable", "Sequence", "Mapping",
    "MutableMapping", "Type", "Self", "Literal", "ClassVar", "Final",
    "Annotated", "TypeVar", "Generic", "Protocol", "Awaitable", "Coroutine",
    "Generator", "AsyncIterator", "AsyncIterable", "AsyncGenerator",
    "NoReturn", "Never", "IO", "TextIO", "BinaryIO", "NamedTuple",
    "TypedDict",
    // builtins / constants that can appear inside annotations
    "None", "True", "False",
    // common stdlib classes
    "Path", "Exception", "ValueError", "TypeError", "KeyError",
    "RuntimeError",
];

/// Uppercase-initial identifier tokens in an annotation string, minus std
/// names. Surrounding quotes (single or double) are stripped first so
/// string annotations tokenize like plain ones; embedded quotes inside
/// subscripts (`list["Order"]`) fall out of the split anyway since quotes
/// are not identifier characters.
fn named_types(annotation: &str) -> Vec<String> {
    let trimmed = annotation.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed);

    let mut out = Vec::new();
    for token in unquoted.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if token.chars().next().is_some_and(|c| c.is_uppercase())
            && !STD_TYPES.contains(&token)
            && !out.iter().any(|t| t == token)
        {
            out.push(token.to_string());
        }
    }
    out
}

/// Post-pass over a file's entities: push one `UsesType` relationship per
/// distinct named type per entity. Targets are raw type names — the
/// analyzer's resolver maps them to entity ids exactly as it does for
/// `Calls` targets, and drops the unresolvable ones.
pub(super) fn emit_uses_type_edges(result: &mut ParseResult) {
    // id → name lookup so methods can skip mentions of their owning class
    // (the recursive-self rule): `-> "Order"` inside `class Order` carries
    // no signal, and the fluent `return self` promotion writes the class
    // name into `return_type` without any annotation being present.
    let names_by_id: HashMap<String, String> = result
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.name.clone()))
        .collect();

    let mut rels: Vec<Relationship> = Vec::new();
    for entity in &result.entities {
        let mut type_names: Vec<String> = Vec::new();
        match entity.kind {
            EntityKind::Function | EntityKind::Method | EntityKind::Property => {
                for p in &entity.parameters {
                    if let Some(t) = &p.type_name {
                        type_names.extend(named_types(t));
                    }
                }
                if let Some(r) = &entity.return_type {
                    type_names.extend(named_types(r));
                }
            }
            EntityKind::Class
            | EntityKind::Dataclass
            | EntityKind::AbstractClass
            | EntityKind::Enum => {
                for f in &entity.fields {
                    if let Some(t) = &f.type_name {
                        type_names.extend(named_types(t));
                    }
                }
            }
            _ => continue,
        }
        let owner_name = entity
            .parent_id
            .as_ref()
            .and_then(|pid| names_by_id.get(pid));
        type_names.sort();
        type_names.dedup();
        for t in type_names {
            // Recursive mentions of the owning type carry no signal —
            // neither the entity's own name (a class field annotated with
            // the class itself) nor, for methods, the enclosing class.
            if t == entity.name || owner_name.is_some_and(|n| *n == t) {
                continue;
            }
            rels.push(Relationship::new(
                entity.id.clone(),
                t,
                RelationshipKind::UsesType,
            ));
        }
    }
    for rel in rels {
        result.add_relationship(rel);
    }
}

#[cfg(test)]
mod tests {
    use super::named_types;

    #[test]
    fn extracts_project_types_and_drops_typing_names() {
        assert_eq!(named_types("Order"), vec!["Order"]);
        assert_eq!(named_types("Optional[Order]"), vec!["Order"]);
        assert_eq!(
            named_types("Dict[str, LineItem]"),
            vec!["LineItem"]
        );
        assert_eq!(named_types("int"), Vec::<String>::new());
        assert_eq!(named_types("List[int]"), Vec::<String>::new());
    }

    #[test]
    fn strips_quoted_string_annotations() {
        assert_eq!(named_types("\"Order\""), vec!["Order"]);
        assert_eq!(named_types("'Order'"), vec!["Order"]);
        assert_eq!(named_types("\"Optional[Order]\""), vec!["Order"]);
        // Quoted forward ref nested inside a subscript.
        assert_eq!(named_types("list[\"Order\"]"), vec!["Order"]);
    }

    #[test]
    fn handles_pep604_unions() {
        assert_eq!(named_types("Order | None"), vec!["Order"]);
        assert_eq!(
            named_types("Order | LineItem | None"),
            vec!["Order", "LineItem"]
        );
    }

    #[test]
    fn handles_subscripted_lowercase_generics() {
        assert_eq!(named_types("list[Order]"), vec!["Order"]);
        assert_eq!(named_types("dict[str, Order]"), vec!["Order"]);
        // Lowercase builtins never match the uppercase rule.
        assert_eq!(named_types("dict[str, int]"), Vec::<String>::new());
    }

    #[test]
    fn dedups_and_keeps_dotted_tails() {
        assert_eq!(
            named_types("tuple[Order, Order]"),
            vec!["Order"]
        );
        // Dotted annotations split on the dot; both segments are
        // candidates, lowercase module prefixes drop out.
        assert_eq!(named_types("models.Order"), vec!["Order"]);
    }
}
