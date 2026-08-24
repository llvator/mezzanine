//! Type-usage edge extraction (KT-001, sibling of RS-001).
//!
//! Emits `UsesType` relationships from callables to the named types in
//! their signatures, from classes to their primary-constructor property
//! types, and from body properties to their declared types — so a type
//! used only in a signature or property gains real dependents in the
//! graph instead of relying on method-call approximations.
//!
//! Extraction rule (deliberately simple, per the RS-001 pattern): every
//! `Uppercase`-initial identifier token in the captured type *string* is
//! a candidate, then Kotlin stdlib names are dropped. Splitting on
//! non-identifier characters handles the Kotlin quirks for free:
//! nullability (`Order?` → `Order`), generics (`List<Order>` → `Order`),
//! and function types (`(Order) -> Receipt` → `Order`, `Receipt`).
//! No new tree-sitter queries — the pinned tree-sitter-kotlin grammar is
//! crusty, so we only reuse the type strings the parser already captures.

use crate::models::{EntityKind, Relationship, RelationshipKind};

use super::super::language_parser::ParseResult;

/// Kotlin stdlib types that carry no project-level dependency signal.
/// Kept small on purpose: an unknown project type slipping through
/// resolves to nothing and is dropped by the resolver, so false
/// positives are cheap.
const STD_TYPES: &[&str] = &[
    "String",
    "Char",
    "Int",
    "Long",
    "Short",
    "Byte",
    "Float",
    "Double",
    "Boolean",
    "Unit",
    "Any",
    "Nothing",
    "Number",
    "CharSequence",
    "List",
    "MutableList",
    "Map",
    "MutableMap",
    "Set",
    "MutableSet",
    "Collection",
    "MutableCollection",
    "Iterable",
    "Iterator",
    "Array",
    "ByteArray",
    "IntArray",
    "LongArray",
    "Sequence",
    "Pair",
    "Triple",
    "Result",
    "Lazy",
    "Regex",
    "Throwable",
    "Exception",
    "Flow",
    "Deferred",
    "Job",
    "CoroutineScope",
];

/// Uppercase-initial identifier tokens in a type string, minus stdlib names.
fn named_types(type_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in type_str.split(|c: char| !c.is_alphanumeric() && c != '_') {
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
///
/// Sources per entity kind:
/// - `Function` / `Method` — parameter types + return type.
/// - `Class` / `AbstractClass` — `fields`, which is where the parser
///   lands primary-constructor `val`/`var` properties (Kotlin's
///   idiomatic field declaration site; see `parse_class_parameters`).
/// - `Property` — body/top-level properties store their declared type
///   in `return_type`.
///
/// Enums are skipped deliberately: the Kotlin parser stores enum *entry
/// value arguments* (constructor expressions, not types) in `fields`.
pub(super) fn emit_uses_type_edges(result: &mut ParseResult) {
    let mut rels: Vec<Relationship> = Vec::new();
    for entity in &result.entities {
        let mut type_names: Vec<String> = Vec::new();
        match entity.kind {
            EntityKind::Function | EntityKind::Method => {
                for p in &entity.parameters {
                    if let Some(t) = &p.type_name {
                        type_names.extend(named_types(t));
                    }
                }
                if let Some(r) = &entity.return_type {
                    type_names.extend(named_types(r));
                }
            }
            EntityKind::Class | EntityKind::AbstractClass => {
                for f in &entity.fields {
                    if let Some(t) = &f.type_name {
                        type_names.extend(named_types(t));
                    }
                }
            }
            EntityKind::Property => {
                if let Some(t) = &entity.return_type {
                    type_names.extend(named_types(t));
                }
            }
            _ => continue,
        }
        type_names.sort();
        type_names.dedup();
        for t in type_names {
            // Recursive mentions of the owning type carry no signal.
            if t == entity.name {
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
    fn extracts_project_types_and_drops_stdlib_names() {
        assert_eq!(named_types("List<Order>"), vec!["Order"]);
        assert_eq!(named_types("Map<String, Invoice>"), vec!["Invoice"]);
        assert_eq!(named_types("Int"), Vec::<String>::new());
        assert_eq!(named_types("com.shop.model.Order"), vec!["Order"]);
    }

    #[test]
    fn nullability_marker_is_stripped() {
        assert_eq!(named_types("Order?"), vec!["Order"]);
        assert_eq!(named_types("List<Order?>?"), vec!["Order"]);
    }

    #[test]
    fn function_types_yield_both_sides() {
        assert_eq!(named_types("(Order) -> Receipt"), vec!["Order", "Receipt"]);
        assert_eq!(
            named_types("suspend (Order, Int) -> Flow<Receipt>"),
            vec!["Order", "Receipt"]
        );
    }

    #[test]
    fn dedups_within_one_type_string() {
        assert_eq!(named_types("Pair<Order, Order>"), vec!["Order"]);
    }
}
