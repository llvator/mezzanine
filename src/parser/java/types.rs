//! Type-usage edge extraction (JV-001, sibling of RS-001).
//!
//! Emits `UsesType` relationships from methods/constructors to the named
//! types in their signatures, from field (`Property`) entities to their
//! declared types, and from records to their component types — so a type
//! used only as a parameter/return/field gains real dependents in the
//! graph instead of relying on method-call approximations.
//!
//! Extraction rule (deliberately simple, per RS-001): every
//! `Uppercase`-initial identifier token in the type string is a candidate
//! (`List<Order>` → List, Order; `Order[]` → Order; `com.acme.Order` →
//! Order; `List<? extends Order>` → List, Order), then common
//! java.lang/java.util/java.time names are dropped via a small std table.
//! Unknown names resolve to nothing and are dropped by the resolver, so
//! false positives are cheap — the table is kept small on purpose.
//!
//! Annotations are attributes, not types: the parser stores parameter and
//! field types from the grammar's `type` field, which excludes leading
//! annotations (`@Autowired Order order` captures only `Order`), but
//! type-use annotations inside generics (`List<@NonNull Order>`) survive
//! in the raw text — `@Name` sequences are stripped before tokenizing.
//!
//! Deliberately excluded: enum constants. The Java parser stores them in
//! `fields` with their constructor *arguments* as `type_name`
//! (`USD("US Dollar", 2)`), which are expressions, not types — extracting
//! tokens from them would emit edges to string-literal words.

use std::collections::HashMap;

use crate::models::{EntityKind, Relationship, RelationshipKind};

use super::super::language_parser::ParseResult;

/// Common java.lang / java.util / java.time / java.math names that carry
/// no project-level dependency signal.
const STD_TYPES: &[&str] = &[
    "String", "Integer", "Long", "Double", "Float", "Boolean", "Byte",
    "Short", "Character", "Object", "Void", "Number", "CharSequence",
    "StringBuilder", "Comparable", "Runnable", "List", "ArrayList",
    "LinkedList", "Map", "HashMap", "LinkedHashMap", "TreeMap", "Set",
    "HashSet", "LinkedHashSet", "TreeSet", "Queue", "Deque", "ArrayDeque",
    "Collection", "Iterable", "Iterator", "Optional", "Stream",
    "Exception", "RuntimeException", "Error", "Throwable",
    "IllegalArgumentException", "IllegalStateException", "IOException",
    "CompletableFuture", "Future", "LocalDate", "LocalDateTime",
    "LocalTime", "Instant", "Duration", "BigDecimal", "BigInteger", "UUID",
];

/// Drop `@Annotation` names (including dotted `@com.acme.Anno`) so
/// type-use annotations inside generics never register as types.
fn strip_annotations(type_str: &str) -> String {
    let mut out = String::with_capacity(type_str.len());
    let mut chars = type_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '@' {
            while chars
                .next_if(|&n| n.is_alphanumeric() || n == '_' || n == '$' || n == '.')
                .is_some()
            {}
        } else {
            out.push(c);
        }
    }
    out
}

/// Uppercase-initial identifier tokens in a type string, minus std names.
fn named_types(type_str: &str) -> Vec<String> {
    let cleaned = strip_annotations(type_str);
    let mut out = Vec::new();
    for token in cleaned.split(|c: char| !c.is_alphanumeric() && c != '_') {
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
    // Fields live as `Property` entities under their container; resolve
    // the container name so a recursive field (`Node next` in `Node`)
    // is skipped like a recursive struct field is in Rust.
    let names_by_id: HashMap<&str, &str> = result
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e.name.as_str()))
        .collect();

    let mut rels: Vec<Relationship> = Vec::new();
    for entity in &result.entities {
        let mut owner_name: Option<&str> = None;
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
            // Class/interface fields; the declared type is stored in
            // `return_type` by `fields::parse_field`.
            EntityKind::Property => {
                if let Some(t) = &entity.return_type {
                    type_names.extend(named_types(t));
                }
                owner_name = entity
                    .parent_id
                    .as_deref()
                    .and_then(|pid| names_by_id.get(pid).copied());
            }
            // Records land here (Class tagged `record`) with their
            // components in `fields`. Enums are excluded on purpose —
            // see the module header.
            EntityKind::Class | EntityKind::AbstractClass | EntityKind::Interface => {
                for f in &entity.fields {
                    if let Some(t) = &f.type_name {
                        type_names.extend(named_types(t));
                    }
                }
            }
            _ => continue,
        }
        type_names.sort();
        type_names.dedup();
        for t in type_names {
            // Recursive mentions of the owning type carry no signal.
            if t == entity.name || Some(t.as_str()) == owner_name {
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
    fn extracts_project_types_and_drops_std_names() {
        assert_eq!(named_types("List<Order>"), vec!["Order"]);
        assert_eq!(
            named_types("Map<String, List<LineItem>>"),
            vec!["LineItem"]
        );
        assert_eq!(named_types("int"), Vec::<String>::new());
        assert_eq!(named_types("Optional<String>"), Vec::<String>::new());
    }

    #[test]
    fn handles_arrays_varargs_and_fully_qualified_names() {
        assert_eq!(named_types("Order[]"), vec!["Order"]);
        assert_eq!(named_types("Order..."), vec!["Order"]);
        assert_eq!(named_types("com.acme.Order"), vec!["Order"]);
    }

    #[test]
    fn handles_wildcard_bounds() {
        assert_eq!(named_types("List<? extends Order>"), vec!["Order"]);
        assert_eq!(named_types("Map<?, ? super Receipt>"), vec!["Receipt"]);
    }

    #[test]
    fn strips_type_use_annotations() {
        assert_eq!(named_types("@NonNull Order"), vec!["Order"]);
        assert_eq!(named_types("List<@NonNull Order>"), vec!["Order"]);
        assert_eq!(
            named_types("@org.checkerframework.checker.nullness.qual.NonNull Order"),
            vec!["Order"]
        );
    }

    #[test]
    fn dedups_within_a_single_type_string() {
        assert_eq!(named_types("Map<Order, Order>"), vec!["Order"]);
    }
}
