//! Post-merge resolution for the two markdown link forms a single file
//! cannot resolve on its own.
//!
//! Path-shaped links (`[t](../other.md)`) need nothing here — the parser
//! resolves them against the linking note's directory and emits the exact
//! ID the target file emits for itself. These two do need the whole corpus:
//!
//! 1. **Wikilinks.** `[[Other Note]]` names a note without saying where it
//!    lives, so it can only be resolved once every note is known. The
//!    parser emits a `md::wiki.<key>` placeholder target; this pass
//!    rewrites it to the real note ID.
//!
//! 2. **Code refs.** The parser records a link to a source file as an
//!    absolute `mdref:` attribute, because it has no idea what the repo
//!    root is. This pass rewrites those to the repo-relative `cr:` form
//!    Elevator uses, which is what the UI's pairing machinery reads
//!    (ADR 0005).
//!
//! A wikilink that matches no note is left pointing at its placeholder.
//! That is not a failure: `synthesise_unresolved_stubs` turns it into an
//! `unresolved` Note, which is Obsidian's ghost node, reached through the
//! path that already served Elevator's undefined references.

use crate::models::{CodeEntity, EntityKind, Relationship};
use crate::parser::markdown::wiki_key;
use std::collections::HashMap;
use std::path::Path;

/// Rewrite `md::wiki.<key>` targets to the notes they name.
///
/// Notes are indexed under both their title and their filename stem,
/// because authors write `[[Setup Guide]]` (the H1) and `[[setup-guide]]`
/// (the file) interchangeably. A key matching several notes resolves to the
/// lexicographically first and marks the edge `ambiguous`, so a vault with
/// two `index.md` files reports the collision rather than silently picking.
pub(super) fn resolve_wikilinks(
    entities: &HashMap<String, CodeEntity>,
    relationships: &mut [Relationship],
) -> Vec<String> {
    let index = build_index(entities);
    let mut unresolved: Vec<String> = Vec::new();

    for rel in relationships.iter_mut() {
        let Some(key) = rel.target_id.strip_prefix("md::wiki.") else {
            continue;
        };
        match index.get(key) {
            Some(candidates) => {
                if candidates.len() > 1 {
                    rel.metadata
                        .insert("ambiguous".to_string(), "true".to_string());
                }
                retarget(rel, candidates[0].clone());
            }
            None => unresolved.push(key.to_string()),
        }
    }

    unresolved.sort();
    unresolved.dedup();
    unresolved
}

/// Turn the parser's `mdref:` attributes into the repo-relative `cr:` code
/// refs the UI's pairing machinery reads.
///
/// The parser resolves a link against the linking note's directory, so what
/// arrives here is spelled however the analysis root was: absolute when the
/// user ran `mezz analyze /path/to/repo`, already relative when they ran it
/// from inside. Both have to end up relative, because `cr:` is compared
/// against the `file_path`s in the rendered JSON and the renderer strips the
/// root from those.
///
/// Refs outside the root are dropped. A doc linking above the repo names
/// code this graph does not contain, and a `cr:` that can never resolve
/// would be reported as drift forever.
pub(super) fn relativize_code_refs(entities: &mut HashMap<String, CodeEntity>, root: &Path) {
    for entity in entities.values_mut() {
        if entity.kind != EntityKind::Note {
            continue;
        }
        let mut rewritten = Vec::with_capacity(entity.attributes.len());
        for attr in entity.attributes.drain(..) {
            match attr.strip_prefix("mdref:") {
                Some(path) => {
                    if let Some(rel) = relative_to_root(Path::new(path), root) {
                        let ref_attr = format!("cr:{}", rel);
                        if !rewritten.contains(&ref_attr) {
                            rewritten.push(ref_attr);
                        }
                    }
                }
                None => rewritten.push(attr),
            }
        }
        entity.attributes = rewritten;
    }
}

/// A ref's path relative to the analysis root, or `None` when it falls
/// outside. A path that is already relative is inside by construction — the
/// parser rejects anything that climbs above its own root.
fn relative_to_root(path: &Path, root: &Path) -> Option<String> {
    if path.is_relative() {
        return Some(path.to_string_lossy().to_string());
    }
    path.strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

/// Wikilink key → note IDs, sorted so the winner is stable across runs
/// (AN-002).
fn build_index(entities: &HashMap<String, CodeEntity>) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    for entity in entities.values() {
        if entity.kind != EntityKind::Note {
            continue;
        }
        let mut keys = vec![wiki_key(&entity.name)];
        if let Some(stem) = entity
            .attributes
            .iter()
            .find_map(|a| a.strip_prefix("stem:"))
        {
            keys.push(wiki_key(stem));
        }
        for key in keys {
            if key.is_empty() {
                continue;
            }
            let ids = index.entry(key).or_default();
            if !ids.contains(&entity.id) {
                ids.push(entity.id.clone());
            }
        }
    }
    for ids in index.values_mut() {
        ids.sort();
    }
    index
}

/// Point a relationship at a new target. The ID is derived from both
/// endpoints, so it has to be rebuilt or two rewritten edges collide.
fn retarget(rel: &mut Relationship, target_id: String) {
    rel.id = format!("{}->{}:{:?}", rel.source_id, target_id, rel.kind);
    rel.target_id = target_id;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RelationshipKind, Span};
    use std::path::PathBuf;

    /// A note as the parser would have emitted it.
    fn note(path: &str, title: &str, stem: &str) -> CodeEntity {
        let mut e = CodeEntity::new(
            title,
            EntityKind::Note,
            PathBuf::from(path),
            Span::default(),
        );
        e.id = format!("md::note.{}", path);
        e.attributes.push(format!("stem:{}", stem));
        e
    }

    fn corpus(notes: Vec<CodeEntity>) -> HashMap<String, CodeEntity> {
        notes.into_iter().map(|e| (e.id.clone(), e)).collect()
    }

    fn wiki_edge(from: &str, key: &str) -> Relationship {
        Relationship::new(
            from,
            format!("md::wiki.{}", key),
            RelationshipKind::References,
        )
    }

    #[test]
    fn a_wikilink_naming_a_title_finds_the_note() {
        let entities = corpus(vec![note("docs/setup.md", "Getting Started", "setup")]);
        let mut rels = vec![wiki_edge("md::note.a.md", "getting started")];
        assert!(resolve_wikilinks(&entities, &mut rels).is_empty());
        assert_eq!(rels[0].target_id, "md::note.docs/setup.md");
    }

    #[test]
    fn a_wikilink_naming_the_file_finds_the_same_note() {
        let entities = corpus(vec![note("docs/setup.md", "Getting Started", "setup")]);
        let mut rels = vec![wiki_edge("md::note.a.md", "setup")];
        resolve_wikilinks(&entities, &mut rels);
        assert_eq!(rels[0].target_id, "md::note.docs/setup.md");
    }

    #[test]
    fn resolving_rebuilds_the_edge_id_so_two_edges_cannot_collide() {
        let entities = corpus(vec![note("a.md", "A", "a"), note("b.md", "B", "b")]);
        let mut rels = vec![
            wiki_edge("md::note.x.md", "a"),
            wiki_edge("md::note.x.md", "b"),
        ];
        resolve_wikilinks(&entities, &mut rels);
        assert_ne!(rels[0].id, rels[1].id);
    }

    #[test]
    fn a_wikilink_matching_nothing_is_left_for_the_ghost_pass_and_reported() {
        let entities = corpus(vec![note("a.md", "A", "a")]);
        let mut rels = vec![wiki_edge("md::note.a.md", "nowhere")];
        assert_eq!(resolve_wikilinks(&entities, &mut rels), vec!["nowhere"]);
        assert_eq!(rels[0].target_id, "md::wiki.nowhere");
    }

    #[test]
    fn a_name_two_notes_share_is_marked_rather_than_silently_picked() {
        let entities = corpus(vec![
            note("one/index.md", "Index", "index"),
            note("two/index.md", "Index", "index"),
        ]);
        let mut rels = vec![wiki_edge("md::note.a.md", "index")];
        resolve_wikilinks(&entities, &mut rels);
        assert_eq!(
            rels[0].metadata.get("ambiguous").map(String::as_str),
            Some("true")
        );
        // Lexicographically first, so the pick is stable across runs.
        assert_eq!(rels[0].target_id, "md::note.one/index.md");
    }

    #[test]
    fn an_unambiguous_match_is_not_marked() {
        let entities = corpus(vec![note("a.md", "A", "a")]);
        let mut rels = vec![wiki_edge("md::note.x.md", "a")];
        resolve_wikilinks(&entities, &mut rels);
        assert!(!rels[0].metadata.contains_key("ambiguous"));
    }

    #[test]
    fn an_edge_that_is_not_a_wikilink_is_left_alone() {
        let entities = corpus(vec![note("a.md", "A", "a")]);
        let mut rels = vec![Relationship::new(
            "md::note.x.md",
            "md::note.a.md",
            RelationshipKind::References,
        )];
        let before = rels[0].clone();
        resolve_wikilinks(&entities, &mut rels);
        assert_eq!(rels[0], before);
    }

    #[test]
    fn an_absolute_ref_is_made_relative_to_the_root() {
        let mut entities = corpus(vec![note("/repo/docs/a.md", "A", "a")]);
        entities
            .get_mut("md::note./repo/docs/a.md")
            .unwrap()
            .attributes
            .push("mdref:/repo/src/parser/mod.rs".to_string());

        relativize_code_refs(&mut entities, Path::new("/repo"));

        let refs = &entities["md::note./repo/docs/a.md"].attributes;
        assert!(refs.contains(&"cr:src/parser/mod.rs".to_string()));
        assert!(!refs.iter().any(|a| a.starts_with("mdref:")));
    }

    #[test]
    fn an_already_relative_ref_is_kept_as_written() {
        let mut entities = corpus(vec![note("docs/a.md", "A", "a")]);
        entities
            .get_mut("md::note.docs/a.md")
            .unwrap()
            .attributes
            .push("mdref:src/parser/mod.rs".to_string());

        relativize_code_refs(&mut entities, Path::new("."));

        assert!(entities["md::note.docs/a.md"]
            .attributes
            .contains(&"cr:src/parser/mod.rs".to_string()));
    }

    #[test]
    fn a_ref_outside_the_root_is_dropped_rather_than_left_to_read_as_drift() {
        let mut entities = corpus(vec![note("/repo/docs/a.md", "A", "a")]);
        entities
            .get_mut("md::note./repo/docs/a.md")
            .unwrap()
            .attributes
            .push("mdref:/elsewhere/x.rs".to_string());

        relativize_code_refs(&mut entities, Path::new("/repo"));

        assert!(!entities["md::note./repo/docs/a.md"]
            .attributes
            .iter()
            .any(|a| a.starts_with("cr:")));
    }

    #[test]
    fn the_same_file_referenced_twice_yields_one_ref() {
        let mut entities = corpus(vec![note("/repo/a.md", "A", "a")]);
        let attrs = &mut entities.get_mut("md::note./repo/a.md").unwrap().attributes;
        attrs.push("mdref:/repo/src/x.rs".to_string());
        attrs.push("mdref:/repo/src/x.rs".to_string());

        relativize_code_refs(&mut entities, Path::new("/repo"));

        let count = entities["md::note./repo/a.md"]
            .attributes
            .iter()
            .filter(|a| a.as_str() == "cr:src/x.rs")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn the_stem_attribute_survives_relativization() {
        let mut entities = corpus(vec![note("/repo/a.md", "A", "a")]);
        relativize_code_refs(&mut entities, Path::new("/repo"));
        assert!(entities["md::note./repo/a.md"]
            .attributes
            .contains(&"stem:a".to_string()));
    }

    #[test]
    fn a_non_note_entity_is_not_touched() {
        let mut code = CodeEntity::new(
            "f",
            EntityKind::Function,
            PathBuf::from("/repo/a.rs"),
            Span::default(),
        );
        code.attributes.push("mdref:/repo/src/x.rs".to_string());
        let mut entities = HashMap::from([(code.id.clone(), code.clone())]);

        relativize_code_refs(&mut entities, Path::new("/repo"));

        assert_eq!(entities[&code.id].attributes, vec!["mdref:/repo/src/x.rs"]);
    }
}
