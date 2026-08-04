//! Phase 2 — turn [`DefStmt`]s into entities and edges.
//!
//! Resolution happens here rather than during parsing so forward
//! references work: a body may name a child defined later in the file,
//! or in a sibling `.elv` the analyzer merges in afterwards.

use super::ast::{entity_id, leaf_segment, qualify_child, DefKind, DefStmt};
use super::super::language_parser::ParseResult;
use crate::models::{CodeEntity, Relationship, RelationshipKind, Span, Visibility};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Emit one entity per definition, then the edges every body declares.
pub(super) fn emit(path: &Path, source: &str, defs: &[DefStmt], result: &mut ParseResult) {
    let mut seen: HashMap<String, &DefStmt> = HashMap::new();
    for def in defs {
        let id = entity_id(def.kind, &def.qualname);
        if let Some(first) = seen.get(&id) {
            result.add_warning(format!(
                "elevator: `{} {}` is defined twice — first at {}, again at {}; the first definition wins",
                def.kind.keyword(),
                def.qualname,
                first.start.describe(),
                def.start.describe(),
            ));
            continue;
        }
        seen.insert(id.clone(), def);
        result.add_entity(build_entity(path, source, def, id));
    }

    // Edges come from every occurrence, including a duplicate whose
    // entity we dropped above — losing its children would turn an
    // authoring slip into a silently missing branch of the graph.
    let mut emitted: HashSet<(String, String, &'static str)> = HashSet::new();
    for def in defs {
        emit_edges(path, def, result, &mut emitted);
    }
}

/// Build the `CodeEntity` for one definition.
///
/// `parent_id` is deliberately left unset — the analyzer's
/// `derive_parent_from_contains` pass fills it from Contains edges
/// post-merge, which works uniformly for in-file and cross-file
/// children. Setting it here too would race the resolver's containment
/// pass and produce duplicate Contains edges.
fn build_entity(path: &Path, source: &str, def: &DefStmt, id: String) -> CodeEntity {
    let span = def.span();
    let mut entity = CodeEntity::new(
        leaf_segment(&def.qualname).to_string(),
        def.kind.entity_kind(),
        path,
        span,
    );
    entity.id = id;
    entity.qualified_name = def.qualname.clone();
    entity.visibility = Visibility::Public;
    entity.tags.insert("elevator".to_string());
    entity.documentation = def.description.clone();
    // The statement's own text, same as the tree-sitter parsers
    // capture. Without it `nao diff` / `assess_change` hash `None` for
    // every spec entity and can never report a reworded `d:` as a
    // change, and the MCP `context` tool has nothing to quote.
    entity.source_code = source.get(def.source_range()).map(str::to_string);
    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    // Code-refs land in `attributes` with a `cr:` (generic) or
    // `cr.<tag>:` (tagged) prefix so they ride the existing extension
    // point used by impex etc. — no schema change to CodeEntity, and
    // downstream consumers (renderers, UI) just filter by prefix.
    for (tag, cr_path) in &def.code_refs {
        let key = if tag.is_empty() {
            "cr".to_string()
        } else {
            format!("cr.{}", tag)
        };
        entity.attributes.push(format!("{}:{}", key, cr_path));
    }
    entity
}

fn emit_edges(
    path: &Path,
    def: &DefStmt,
    result: &mut ParseResult,
    emitted: &mut HashSet<(String, String, &'static str)>,
) {
    let parent_id = entity_id(def.kind, &def.qualname);

    for child in &def.children {
        let child_qualname = qualify_child(def, child.kind, &child.raw);
        let child_id = entity_id(child.kind, &child_qualname);
        if child.kind == DefKind::UiPage {
            // UI pages are conventionally labels — auto-stub one here
            // so a child UI ref shows up even without a top-level
            // `ui` definition.
            ensure_stub(path, child.kind, &child_qualname, &child_id, result);
        }
        add_edge(&parent_id, &child_id, "contains", result, emitted);
    }

    for target in &def.where_targets {
        let kind = target.resolve_kind(DefKind::UiPage);
        let target_id = entity_id(kind, &target.name);
        if kind == DefKind::UiPage {
            ensure_stub(path, kind, &target.name, &target_id, result);
        }
        add_edge(&parent_id, &target_id, "where", result, emitted);
    }
    for target in &def.references {
        let target_id = entity_id(target.resolve_kind(DefKind::Feature), &target.name);
        add_edge(&parent_id, &target_id, "references", result, emitted);
    }

    emit_used_by(def, &parent_id, result, emitted);
}

/// `used_by:` inverts: a Concept lists its consumers, and we emit one
/// edge from each consumer to the Concept.
fn emit_used_by(
    def: &DefStmt,
    parent_id: &str,
    result: &mut ParseResult,
    emitted: &mut HashSet<(String, String, &'static str)>,
) {
    if def.kind != DefKind::Concept {
        if let Some(first) = def.used_by.first() {
            result.add_warning(format!(
                "elevator: `used_by:` only valid inside `concept` (ignored on {} {}) at {}",
                def.kind.keyword(),
                def.qualname,
                first.at.describe(),
            ));
        }
        return;
    }
    for consumer in &def.used_by {
        let consumer_id = entity_id(consumer.resolve_kind(DefKind::Feature), &consumer.name);
        add_edge(&consumer_id, parent_id, "used_by", result, emitted);
    }
}

/// Add one edge, skipping an exact duplicate. Repeating a child in two
/// bodies of the same (duplicated) definition shouldn't inflate fan-in.
fn add_edge(
    source: &str,
    target: &str,
    link: &'static str,
    result: &mut ParseResult,
    emitted: &mut HashSet<(String, String, &'static str)>,
) {
    if !emitted.insert((source.to_string(), target.to_string(), link)) {
        return;
    }
    if link == "contains" {
        result.add_relationship(Relationship::new(
            source,
            target,
            RelationshipKind::Contains,
        ));
        return;
    }
    let mut rel = Relationship::new(source, target, RelationshipKind::References);
    rel.metadata.insert("link".to_string(), link.to_string());
    result.add_relationship(rel);
}

/// Auto-create a stub entity if the given id isn't already present.
/// Stubs are tagged so the renderer can flag them visually if needed.
fn ensure_stub(
    path: &Path,
    kind: DefKind,
    qualname: &str,
    id: &str,
    result: &mut ParseResult,
) {
    if result.entities.iter().any(|e| e.id == id) {
        return;
    }
    let leaf = leaf_segment(qualname).to_string();
    let mut entity = CodeEntity::new(leaf, kind.entity_kind(), path, Span::default());
    entity.id = id.to_string();
    entity.qualified_name = qualname.to_string();
    entity.visibility = Visibility::Public;
    entity.tags.insert("elevator".to_string());
    entity.tags.insert("auto_created".to_string());
    result.add_entity(entity);
}
