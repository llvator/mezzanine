//! Type-usage edge extraction for Groovy (sibling of JV-001 / RS-001).
//!
//! Emits `UsesType` relationships from methods and constructors to the
//! named types in their signatures, and from field entities to their
//! declared types — so a type used only as a parameter, a return, or a
//! field gains real dependents in the graph instead of relying on the
//! "used via members" approximation.
//!
//! Extraction rule, identical to the Java pass: every `Uppercase`-initial
//! identifier token in the type string is a candidate (`List<Order>` →
//! List, Order; `Order[]` → Order; `com.acme.Order` → Order), then common
//! JDK and GDK names are dropped via a small std table. Unknown names
//! resolve to nothing and are dropped by the resolver, so false positives
//! are cheap and the table stays small on purpose.
//!
//! Groovy adds two wrinkles to the Java shape:
//!
//! * **`def` and `var` never arrive here.** They are the absence of a
//!   type, and [`super::helpers::declared_type`] drops them at the parse
//!   step (GR-015). The uppercase-initial rule would exclude them anyway;
//!   the two guards are independent on purpose.
//! * **Script-scope `@Field` state is included.** Those are `Variable`
//!   entities rather than `Property` ones, but a declared
//!   `HybrisJdbcTemplate template` at script scope is the same dependency
//!   signal as a class field, so both kinds are read the same way.
//!
//! Annotations are attributes, not types: the parser reads types from the
//! grammar's `type` field, which excludes leading annotations, but
//! type-use annotations inside generics (`List<@NonNull Order>`) survive
//! in the raw text — `@Name` sequences are stripped before tokenizing.

use std::collections::HashMap;

use crate::models::{EntityKind, Relationship, RelationshipKind};

use super::super::language_parser::ParseResult;

/// Common JDK and GDK names that carry no project-level dependency
/// signal. The JDK half matches the Java parser's table so the two
/// languages drop the same names; the tail is Groovy's own vocabulary.
const STD_TYPES: &[&str] = &[
    "String",
    "Integer",
    "Long",
    "Double",
    "Float",
    "Boolean",
    "Byte",
    "Short",
    "Character",
    "Object",
    "Void",
    "Number",
    "CharSequence",
    "StringBuilder",
    "Comparable",
    "Runnable",
    "List",
    "ArrayList",
    "LinkedList",
    "Map",
    "HashMap",
    "LinkedHashMap",
    "TreeMap",
    "Set",
    "HashSet",
    "LinkedHashSet",
    "TreeSet",
    "Queue",
    "Deque",
    "ArrayDeque",
    "Collection",
    "Iterable",
    "Iterator",
    "Optional",
    "Stream",
    "Exception",
    "RuntimeException",
    "Error",
    "Throwable",
    "IllegalArgumentException",
    "IllegalStateException",
    "IOException",
    "CompletableFuture",
    "Future",
    "LocalDate",
    "LocalDateTime",
    "LocalTime",
    "Instant",
    "Duration",
    "BigDecimal",
    "BigInteger",
    "UUID",
    // Groovy's own always-imported vocabulary.
    "GString",
    "Closure",
    "Binding",
    "Script",
    "Range",
    "Tuple",
    "GroovyObject",
    "GroovyRuntimeException",
    "MetaClass",
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

/// Collect the declared type names an entity depends on, or `None` when
/// the entity kind carries no type information at all.
fn declared_types(entity: &crate::models::CodeEntity) -> Option<Vec<String>> {
    match entity.kind {
        EntityKind::Function | EntityKind::Method => {
            let mut names: Vec<String> = entity
                .parameters
                .iter()
                .filter_map(|p| p.type_name.as_deref())
                .flat_map(named_types)
                .collect();
            if let Some(r) = &entity.return_type {
                names.extend(named_types(r));
            }
            Some(names)
        }
        // Class fields (`Property`) and script-scope `@Field` state
        // (`Variable`) both keep the declared type in `return_type`.
        // Untyped locals leave it `None` and contribute nothing.
        EntityKind::Property | EntityKind::Variable => Some(
            entity
                .return_type
                .as_deref()
                .map(named_types)
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

/// Post-pass over a file's entities: push one `UsesType` relationship per
/// distinct named type per entity. Targets are raw type names — the
/// analyzer's resolver maps them to entity ids exactly as it does for
/// `Calls` targets, and drops the unresolvable ones.
pub(super) fn emit_uses_type_edges(result: &mut ParseResult) {
    // Fields live under their container; resolve the container name so a
    // self-referential field (`Node next` inside `Node`) is skipped.
    let names_by_id: HashMap<&str, &str> = result
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e.name.as_str()))
        .collect();

    let mut rels: Vec<Relationship> = Vec::new();
    for entity in &result.entities {
        let Some(mut type_names) = declared_types(entity) else {
            continue;
        };
        let owner_name = entity
            .parent_id
            .as_deref()
            .and_then(|pid| names_by_id.get(pid).copied());
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
        assert_eq!(named_types("Map<String, List<LineItem>>"), vec!["LineItem"]);
        assert_eq!(named_types("int"), Vec::<String>::new());
        assert_eq!(named_types("Optional<String>"), Vec::<String>::new());
    }

    #[test]
    fn drops_groovy_dynamic_keywords_and_gdk_names() {
        assert_eq!(named_types("def"), Vec::<String>::new());
        assert_eq!(named_types("var"), Vec::<String>::new());
        assert_eq!(named_types("Closure<Order>"), vec!["Order"]);
        assert_eq!(named_types("GString"), Vec::<String>::new());
    }

    #[test]
    fn handles_arrays_varargs_and_fully_qualified_names() {
        assert_eq!(named_types("Order[]"), vec!["Order"]);
        assert_eq!(named_types("Order..."), vec!["Order"]);
        assert_eq!(named_types("com.acme.Order"), vec!["Order"]);
    }

    #[test]
    fn strips_type_use_annotations() {
        assert_eq!(named_types("@NonNull Order"), vec!["Order"]);
        assert_eq!(named_types("List<@NonNull Order>"), vec!["Order"]);
    }
}
