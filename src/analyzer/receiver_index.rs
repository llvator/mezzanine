//! Cross-file field types for receiver resolution (AN-012).
//!
//! The Rust parser types a dotted receiver as far as one file's declarations
//! reach, then stops: `entity.kind.is_callable()` types `entity` as
//! `CodeEntity` from its parameter and stalls on `.kind`, because
//! `CodeEntity`'s fields are declared in `src/models/entity.rs`. The parser
//! cannot see that file — it parses one file at a time, and the parse store
//! caches the result per file, so a parse whose output depended on a second
//! file would be cached wrong.
//!
//! By merge time every struct in the tree is an entity carrying its fields,
//! so the walk can be finished here. The parser hands over where it stalled
//! (`recv_path` / `recv_member` on the edge); this module resolves the rest
//! against every struct it saw, or leaves the edge exactly as the parser left
//! it — on a ghost, visibly unresolved.

use crate::models::{CodeEntity, EntityKind, Relationship};
use crate::parser::rust_base_type_name;
use std::collections::HashMap;

/// `struct name → (field name → declared type)` over every Rust struct in the
/// tree.
///
/// A field maps to `None` when two same-named structs declare it with
/// different types: ambiguity is declined, never broken by a tie-break, so
/// this index can only ever add exact matches. That makes it independent of
/// the order structs are fed in, which is what keeps identical trees
/// producing identical graphs even though entities arrive from a hash map.
pub(super) struct FieldIndex {
    by_struct: HashMap<String, HashMap<String, Option<String>>>,
}

impl FieldIndex {
    /// Build the index from the analyzer's entity set.
    ///
    /// Rust structs only: the deferred hints come from the Rust parser, and a
    /// Java class sharing a struct's name would otherwise type its fields.
    pub(super) fn build<'a>(entities: impl Iterator<Item = &'a CodeEntity>) -> Self {
        let mut structs: Vec<&CodeEntity> = entities
            .filter(|e| e.kind == EntityKind::Struct && !e.fields.is_empty())
            .filter(|e| e.file_path.extension().and_then(|x| x.to_str()) == Some("rs"))
            .collect();
        // Sorted so the pass is a function of the tree rather than of hash
        // iteration order. The conflict rule below already makes the outcome
        // order-independent; this makes it inspectable too.
        structs.sort_by(|a, b| a.id.cmp(&b.id));

        let mut by_struct: HashMap<String, HashMap<String, Option<String>>> = HashMap::new();
        for entity in structs {
            let fields = by_struct.entry(entity.name.clone()).or_default();
            for field in &entity.fields {
                let Some(declared) = field
                    .type_name
                    .as_deref()
                    .and_then(rust_base_type_name)
                else {
                    continue;
                };
                fields
                    .entry(field.name.clone())
                    .and_modify(|known| {
                        if known.as_deref() != Some(declared.as_str()) {
                            *known = None;
                        }
                    })
                    .or_insert(Some(declared));
            }
        }
        Self { by_struct }
    }

    /// Walk `Type.field[.field…]` to the type of its last segment.
    ///
    /// `None` for a type this tree never declared, a field that type doesn't
    /// have, or a field two same-named structs disagree on — every step reads
    /// a declared type or gives up.
    fn walk(&self, path: &str) -> Option<String> {
        let mut segments = path.split('.');
        let mut current = segments.next()?.to_string();
        for field in segments {
            match self.by_struct.get(&current).and_then(|f| f.get(field)) {
                Some(Some(declared)) => current = declared.clone(),
                _ => return None,
            }
        }
        Some(current)
    }
}

/// Finish every deferred receiver the Rust parser handed over, and strip the
/// transient metadata from all of them.
///
/// An edge whose path resolves is retargeted at `Type::member`, which the
/// graph resolver looks up like any other qualified callee. One that doesn't
/// keeps the parser's receiver-text target and lands on a ghost — the same
/// place it landed before this pass existed. Either way both keys are gone by
/// the time the graph is built, so they never reach a renderer.
pub(super) fn resolve_deferred(relationships: &mut [Relationship], index: &FieldIndex) {
    for rel in relationships.iter_mut() {
        let hint = match (rel.metadata.get("recv_path"), rel.metadata.get("recv_member")) {
            (Some(path), Some(member)) => Some((path.clone(), member.clone())),
            _ => None,
        };
        rel.metadata.remove("recv_path");
        rel.metadata.remove("recv_member");
        let Some((path, member)) = hint else {
            continue;
        };
        if let Some(receiver_type) = index.walk(&path) {
            rel.target_id = format!("{}::{}", receiver_type, member);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::entity::Parameter;
    use crate::models::{RelationshipKind, Span};
    use std::path::Path;

    fn rust_struct(name: &str, file: &str, fields: &[(&str, &str)]) -> CodeEntity {
        let mut entity = CodeEntity::new(name, EntityKind::Struct, Path::new(file), Span::default());
        entity.fields = fields
            .iter()
            .map(|(fname, ftype)| Parameter {
                name: fname.to_string(),
                type_name: Some(ftype.to_string()),
                default_value: None,
                visibility: None,
            })
            .collect();
        entity
    }

    fn deferred_call(path: &str, member: &str) -> Relationship {
        let mut rel = Relationship::new(
            "caller",
            format!("recv.{}::{}", path, member),
            RelationshipKind::Calls,
        );
        rel.metadata.insert("recv_path".to_string(), path.to_string());
        rel.metadata.insert("recv_member".to_string(), member.to_string());
        rel
    }

    fn resolve_one(entities: &[CodeEntity], path: &str, member: &str) -> Relationship {
        let index = FieldIndex::build(entities.iter());
        let mut rels = vec![deferred_call(path, member)];
        resolve_deferred(&mut rels, &index);
        rels.pop().unwrap()
    }

    #[test]
    fn a_field_declared_in_another_file_types_the_receiver() {
        // The reported bug: `entity.kind.is_callable()` in a parser file, with
        // `CodeEntity`'s fields declared in `src/models/entity.rs`.
        let entities = vec![rust_struct(
            "CodeEntity",
            "src/models/entity.rs",
            &[("kind", "EntityKind")],
        )];
        let rel = resolve_one(&entities, "CodeEntity.kind", "is_callable");
        assert_eq!(rel.target_id, "EntityKind::is_callable");
    }

    #[test]
    fn declared_types_are_normalised_like_the_parser_normalises_them() {
        // `ctx.result.add_entity()` — the field is `&'a mut ParseResult`.
        let entities = vec![rust_struct(
            "ExtractCtx",
            "src/parser/rust/mod.rs",
            &[("result", "&'a mut ParseResult")],
        )];
        let rel = resolve_one(&entities, "ExtractCtx.result", "add_entity");
        assert_eq!(rel.target_id, "ParseResult::add_entity");
    }

    #[test]
    fn a_multi_hop_path_walks_field_by_field() {
        let entities = vec![
            rust_struct("Outer", "src/a.rs", &[("mid", "Middle")]),
            rust_struct("Middle", "src/b.rs", &[("inner", "Inner")]),
        ];
        let rel = resolve_one(&entities, "Outer.mid.inner", "act");
        assert_eq!(rel.target_id, "Inner::act");
    }

    #[test]
    fn an_unknown_struct_leaves_the_edge_where_the_parser_left_it() {
        let entities = vec![rust_struct("Known", "src/a.rs", &[("kind", "EntityKind")])];
        let rel = resolve_one(&entities, "Mystery.kind", "is_callable");
        assert_eq!(rel.target_id, "recv.Mystery.kind::is_callable");
    }

    #[test]
    fn an_unknown_field_leaves_the_edge_where_the_parser_left_it() {
        let entities = vec![rust_struct("CodeEntity", "src/a.rs", &[("kind", "EntityKind")])];
        let rel = resolve_one(&entities, "CodeEntity.nonesuch", "act");
        assert_eq!(rel.target_id, "recv.CodeEntity.nonesuch::act");
    }

    #[test]
    fn a_primitive_field_types_nothing() {
        // `base_type_name` declines lowercase types, so `self.count.to_string()`
        // must not become `usize::to_string`.
        let entities = vec![rust_struct("Counter", "src/a.rs", &[("count", "usize")])];
        let rel = resolve_one(&entities, "Counter.count", "to_string");
        assert_eq!(rel.target_id, "recv.Counter.count::to_string");
    }

    #[test]
    fn same_named_structs_disagreeing_on_a_field_resolve_to_neither() {
        // Two `Config`s, two `store` types. Picking either would trade a
        // recall bug for a precision bug, so the field is declined.
        let entities = vec![
            rust_struct("Config", "src/a.rs", &[("store", "StoreA")]),
            rust_struct("Config", "src/b.rs", &[("store", "StoreB")]),
        ];
        let rel = resolve_one(&entities, "Config.store", "get");
        assert_eq!(rel.target_id, "recv.Config.store::get");
    }

    #[test]
    fn same_named_structs_agreeing_on_a_field_still_resolve() {
        let entities = vec![
            rust_struct("Config", "src/a.rs", &[("store", "ParseStore")]),
            rust_struct("Config", "src/b.rs", &[("store", "ParseStore")]),
        ];
        let rel = resolve_one(&entities, "Config.store", "get");
        assert_eq!(rel.target_id, "ParseStore::get");
    }

    #[test]
    fn struct_order_does_not_change_the_outcome() {
        let forwards = vec![
            rust_struct("Config", "src/a.rs", &[("store", "StoreA")]),
            rust_struct("Config", "src/b.rs", &[("store", "StoreB")]),
            rust_struct("Other", "src/c.rs", &[("store", "StoreC")]),
        ];
        let backwards: Vec<CodeEntity> = forwards.iter().rev().cloned().collect();
        for path in ["Config.store", "Other.store"] {
            assert_eq!(
                resolve_one(&forwards, path, "get").target_id,
                resolve_one(&backwards, path, "get").target_id,
            );
        }
    }

    #[test]
    fn a_same_named_class_in_another_language_types_nothing() {
        let mut java = rust_struct("Config", "src/Config.java", &[("store", "StoreJ")]);
        java.kind = EntityKind::Class;
        let rel = resolve_one(&[java], "Config.store", "get");
        assert_eq!(rel.target_id, "recv.Config.store::get");
    }

    #[test]
    fn the_transient_hint_never_survives_the_pass() {
        // Resolved or not, `recv_path` must not reach the graph — renderers
        // show relationship metadata.
        let entities = [rust_struct("Known", "src/a.rs", &[("kind", "EntityKind")])];
        let index = FieldIndex::build(entities.iter());
        let mut rels = vec![
            deferred_call("Known.kind", "is_callable"),
            deferred_call("Mystery.kind", "is_callable"),
        ];
        resolve_deferred(&mut rels, &index);
        for rel in &rels {
            assert!(!rel.metadata.contains_key("recv_path"), "{:?}", rel.metadata);
            assert!(!rel.metadata.contains_key("recv_member"), "{:?}", rel.metadata);
        }
    }

    #[test]
    fn edges_without_a_hint_are_untouched() {
        let index = FieldIndex::build(std::iter::empty());
        let mut rels = vec![Relationship::new("caller", "Foo::bar", RelationshipKind::Calls)];
        resolve_deferred(&mut rels, &index);
        assert_eq!(rels[0].target_id, "Foo::bar");
    }
}
