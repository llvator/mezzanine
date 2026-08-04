//! Type-usage edge extraction (TS-001, sibling of the Rust RS-001 pass).
//!
//! Emits `UsesType` relationships from callables to the named types in
//! their signatures, and from class/interface members to their declared
//! types — so a TS interface used in twenty components gains real
//! dependents in the graph instead of relying on the "Used via members"
//! approximation.
//!
//! Extraction rule (deliberately simple, same as RS-001): every
//! `Uppercase`-initial identifier token in the type string is a candidate
//! (`Promise<Widget[]>` → Promise, Widget; `Foo | Bar` → Foo, Bar), then
//! JS/TS builtin and utility-type names are dropped. Primitives (`string`,
//! `number`, `boolean`) never match because TS primitives are lowercase.
//! DOM names like `HTMLElement` are kept on purpose — they resolve to
//! nothing and are dropped by the analyzer's resolver.
//!
//! Parser quirk this pass accounts for: the TS parser does not populate
//! `entity.fields` on classes/interfaces. Class fields and interface
//! property signatures are separate `Property` entities whose declared
//! type lands in `entity.return_type` (see `members.rs`). Only enums
//! populate `fields`, and their members carry no `type_name` — the
//! container-field branch below exists for parity/future-proofing.

use std::collections::HashMap;

use crate::models::{EntityKind, Relationship, RelationshipKind};

use super::super::language_parser::ParseResult;

/// Builtin/global and utility-type names that carry no project-level
/// dependency signal. Kept small on purpose: an unknown project type
/// slipping through resolves to nothing and is dropped by the resolver,
/// so false positives are cheap.
const BUILTIN_TYPES: &[&str] = &[
    // TS utility types (their arguments are the real signal).
    "Partial", "Pick", "Omit", "Record", "Readonly", "Required",
    "ReturnType", "Awaited", "NonNullable", "Exclude", "Extract",
    "Parameters", "InstanceType",
    // JS builtins / globals.
    "Promise", "Array", "ReadonlyArray", "Map", "Set", "WeakMap",
    "WeakSet", "Date", "Error", "RegExp", "String", "Number", "Boolean",
    "Object", "Symbol", "BigInt", "Function", "Iterable", "Iterator",
    "AsyncIterable", "AsyncIterator",
];

/// Uppercase-initial identifier tokens in a type string, minus builtins.
fn named_types(type_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in type_str.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$') {
        if token.chars().next().is_some_and(|c| c.is_uppercase())
            && !BUILTIN_TYPES.contains(&token)
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
    // id → name, to skip a Property whose type is its own container
    // (the analogue of Rust's struct-field self-mention skip).
    let parent_names: HashMap<String, String> = result
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.name.clone()))
        .collect();

    let mut rels: Vec<Relationship> = Vec::new();
    for entity in &result.entities {
        let mut type_names: Vec<String> = Vec::new();
        let mut owner_name: Option<&str> = None;
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
            // Class fields and interface property signatures: the
            // declared type is captured in `return_type` (members.rs).
            EntityKind::Property => {
                if let Some(r) = &entity.return_type {
                    type_names.extend(named_types(r));
                }
                owner_name = entity
                    .parent_id
                    .as_ref()
                    .and_then(|pid| parent_names.get(pid))
                    .map(String::as_str);
            }
            // Containers that carry `fields` directly (today: enums,
            // whose members have no type annotation — kept for parity).
            EntityKind::Class
            | EntityKind::AbstractClass
            | EntityKind::Interface
            | EntityKind::Enum => {
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
            // Recursive mentions of the entity itself — or, for a
            // property, of its owning container — carry no signal.
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
    fn extracts_project_types_and_drops_builtins() {
        assert_eq!(named_types("Promise<Widget[]>"), vec!["Widget"]);
        assert_eq!(named_types("Foo | Bar"), vec!["Foo", "Bar"]);
        assert_eq!(named_types("Partial<Config>"), vec!["Config"]);
        assert_eq!(named_types("Map<string, Entry>"), vec!["Entry"]);
        assert_eq!(named_types("string"), Vec::<String>::new());
        assert_eq!(named_types("number | null"), Vec::<String>::new());
        // Intersections split like unions; namespace paths keep segments.
        assert_eq!(named_types("Base & Mixin"), vec!["Base", "Mixin"]);
        // DOM names stay — the resolver drops what it can't resolve.
        assert_eq!(named_types("HTMLElement"), vec!["HTMLElement"]);
    }

    #[test]
    fn dedups_within_one_type_string() {
        assert_eq!(named_types("Record<string, Foo | Foo>"), vec!["Foo"]);
    }
}
