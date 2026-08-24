//! Type-usage edge extraction (RS-001).
//!
//! Emits `UsesType` relationships from callables to the named types in
//! their signatures, and from structs/enums to their field types — so a
//! type used only as a parameter/return/field gains real dependents in
//! the graph instead of relying on method-call approximations.
//!
//! Extraction rule (deliberately simple, documented per RS-001): every
//! `Uppercase`-initial identifier token in the type string is a candidate
//! (`Option<Arc<DependencyGraph>>` → Option, Arc, DependencyGraph), then
//! std wrapper/container names are dropped. Primitives (`u32`, `bool`,
//! `str`) never match because Rust primitives are lowercase.

use crate::models::{EntityKind, Relationship, RelationshipKind};

use super::super::language_parser::ParseResult;

/// Std types that carry no project-level dependency signal. Kept small
/// on purpose: an unknown project type slipping through resolves to
/// nothing and is dropped by the resolver, so false positives are cheap.
const STD_TYPES: &[&str] = &[
    "Self",
    "String",
    "Str",
    "Option",
    "Result",
    "Vec",
    "VecDeque",
    "Box",
    "Rc",
    "Arc",
    "Cell",
    "RefCell",
    "RwLock",
    "Mutex",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "Cow",
    "Path",
    "PathBuf",
    "OsStr",
    "OsString",
    "Instant",
    "Duration",
    "SystemTime",
    "Ordering",
    "PhantomData",
    "Pin",
    "Future",
    "Iterator",
    "IntoIterator",
    "Default",
    "Clone",
    "Copy",
    "Debug",
    "Display",
    "Send",
    "Sync",
    "Sized",
    "Fn",
    "FnMut",
    "FnOnce",
    "AsRef",
    "AsMut",
    "From",
    "Into",
    "TryFrom",
    "TryInto",
    "ToString",
];

/// Uppercase-initial identifier tokens in a type string, minus std names.
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
            EntityKind::Struct | EntityKind::Enum => {
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
    fn extracts_project_types_and_drops_std_wrappers() {
        assert_eq!(
            named_types("Option<Arc<RwLock<DependencyGraph>>>"),
            vec!["DependencyGraph"]
        );
        assert_eq!(named_types("&mut Vec<Config>"), vec!["Config"]);
        assert_eq!(named_types("u32"), Vec::<String>::new());
        assert_eq!(named_types("crate::models::CodeEntity"), vec!["CodeEntity"]);
        // Lifetimes and references never look like types.
        assert_eq!(named_types("&'a str"), Vec::<String>::new());
    }
}
