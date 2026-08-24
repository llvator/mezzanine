//! Type-usage edge extraction (sibling of the Java and Kotlin passes).
//!
//! Emits `UsesType` relationships from callables to the named types in
//! their signatures, from `Property` entities to their declared types, and
//! from a typedef to what it aliases — so a type used only as a parameter,
//! a return or a field gains real dependents in the graph instead of being
//! reachable only through call approximations.
//!
//! Extraction rule, deliberately simple and shared with the other parsers:
//! every `Uppercase`-initial identifier token in the captured type *string*
//! is a candidate, then Dart core-library names are dropped. Splitting on
//! non-identifier characters handles Dart's spellings for free —
//! nullability (`Order?` → `Order`), generics (`Future<List<Order>>` →
//! `Order`), function types (`Receipt Function(Order)` → both), and library
//! prefixes (`http.Client` → `Client`).
//!
//! The core-name table is kept small on purpose: an unknown project type
//! slipping through resolves to nothing and is dropped by the resolver, so
//! a false positive is cheap, while filtering a name a project defines
//! deletes a real edge.
//!
//! Enum constants are excluded. They live in `fields` with no type of their
//! own, so there is nothing here to read.

use std::collections::HashMap;

use crate::models::{EntityKind, Relationship, RelationshipKind};

use super::super::language_parser::ParseResult;

/// `dart:core` and the handful of `dart:async` names every Dart file uses,
/// which carry no project-level dependency signal.
const CORE_TYPES: &[&str] = &[
    "Object",
    "String",
    "int",
    "double",
    "num",
    "bool",
    "Null",
    "Never",
    "dynamic",
    "void",
    "Function",
    "Type",
    "Symbol",
    "Comparable",
    "Iterable",
    "Iterator",
    "List",
    "Map",
    "MapEntry",
    "Set",
    "Runes",
    "StringBuffer",
    "RegExp",
    "Match",
    "Pattern",
    "DateTime",
    "Duration",
    "Uri",
    "Exception",
    "Error",
    "StateError",
    "ArgumentError",
    "RangeError",
    "UnimplementedError",
    "UnsupportedError",
    "FormatException",
    "Future",
    "FutureOr",
    "Stream",
    "StreamSubscription",
    "StreamController",
    "Completer",
    "Timer",
    "Zone",
];

/// Uppercase-initial identifier tokens in a type string, minus core names.
fn named_types(type_str: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in type_str.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$') {
        if token.chars().next().is_some_and(char::is_uppercase)
            && !CORE_TYPES.contains(&token)
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
    // A field lives as its own `Property` under its container, so the
    // container's name has to be looked up to skip a self-referential
    // field (`Node? next` inside `Node`).
    let names_by_id: HashMap<&str, &str> = result
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e.name.as_str()))
        .collect();

    let mut rels: Vec<Relationship> = Vec::new();
    for entity in &result.entities {
        let Some(type_names) = types_used_by(entity) else {
            continue;
        };
        let owner = entity
            .parent_id
            .as_deref()
            .and_then(|pid| names_by_id.get(pid).copied());

        for name in type_names {
            // A recursive mention of the owning type carries no signal.
            if name == entity.name || Some(name.as_str()) == owner {
                continue;
            }
            rels.push(Relationship::new(
                entity.id.clone(),
                name,
                RelationshipKind::UsesType,
            ));
        }
    }
    for rel in rels {
        result.add_relationship(rel);
    }
}

/// The distinct project types one entity's declaration names, or `None`
/// for a kind that declares no types.
fn types_used_by(entity: &crate::models::CodeEntity) -> Option<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    match entity.kind {
        EntityKind::Function | EntityKind::Method => {
            for parameter in &entity.parameters {
                if let Some(declared) = &parameter.type_name {
                    names.extend(named_types(declared));
                }
            }
            if let Some(declared) = &entity.return_type {
                names.extend(named_types(declared));
            }
        }
        // Fields and top-level variables keep their declared type in
        // `return_type`; so does a typedef's aliased type.
        EntityKind::Property | EntityKind::TypeAlias => {
            if let Some(declared) = &entity.return_type {
                names.extend(named_types(declared));
            }
        }
        _ => return None,
    }
    names.sort();
    names.dedup();
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::named_types;

    #[test]
    fn extracts_project_types_and_drops_core_names() {
        assert_eq!(named_types("List<Order>"), vec!["Order"]);
        assert_eq!(named_types("Map<String, Invoice>"), vec!["Invoice"]);
        assert_eq!(named_types("int"), Vec::<String>::new());
        assert_eq!(named_types("Future<void>"), Vec::<String>::new());
    }

    #[test]
    fn nullability_and_nesting_resolve_to_bare_names() {
        assert_eq!(named_types("Order?"), vec!["Order"]);
        assert_eq!(named_types("Future<List<Order?>>?"), vec!["Order"]);
    }

    #[test]
    fn a_library_prefix_is_not_a_type() {
        assert_eq!(named_types("http.Client"), vec!["Client"]);
    }

    #[test]
    fn function_types_yield_both_sides() {
        assert_eq!(
            named_types("Receipt Function(Order)"),
            vec!["Receipt", "Order"]
        );
    }

    #[test]
    fn dedups_within_one_type_string() {
        assert_eq!(named_types("Map<Order, Order>"), vec!["Order"]);
    }
}
