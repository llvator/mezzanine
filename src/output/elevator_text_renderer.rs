//! Compact textual rendering of an Elevator (`.elv`) spec.
//!
//! Designed as a *low-token consumable artifact* for LLM agents: one
//! line per entity, hierarchy via indentation, edges in bracketed
//! metadata. The whole spec usually fits in a few hundred tokens —
//! enough for an LLM session to onboard onto a project's high-level
//! shape without grepping through code.
//!
//! Honors `FilterConfig::root_entity`: when set, only the named
//! entity and its descendants are rendered, plus any Concepts that
//! cross-cut into the rendered subtree.

use super::{OutputFormat, Renderer};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

pub struct ElevatorTextRenderer;

impl Renderer for ElevatorTextRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::ElevatorText
    }

    fn render(&self, graph: &DependencyGraph, config: &Config) -> Result<String> {
        // `focus_entity` takes precedence: it's a richer artifact
        // (ancestors + siblings + descendants + relevant concepts)
        // than the plain subtree `root_entity` produces. Both flags
        // exist because the use cases differ — root for "show me this
        // branch", focus for "give me everything an LLM session
        // working *on* this node needs."
        let body = if let Some(focus) = config.filters.focus_entity.as_deref() {
            render_focus(graph, focus)
        } else {
            render_elevator_text(graph, config.filters.root_entity.as_deref())
        };
        if config.filters.suppress_legend {
            Ok(body)
        } else {
            Ok(format!("{}\n{}", LEGEND, body))
        }
    }
}

/// Compact format key prepended to every artifact so a fresh-context
/// LLM (or human) can interpret the markers and bracket syntax
/// without prior exposure to Elevator. Six lines, ~90 tokens —
/// cheap relative to the artifact and saves an entire round of
/// "what does `fu` mean?" guessing. The last line calibrates trust:
/// structure is checked by the analyzer, but descriptions and `cr:`
/// paths are authored — a consumer must not extend the "always derived
/// from code" trust of the code graph to the spec's semantic payload.
pub(crate) const LEGEND: &str = "# Elevator spec format
# - Indentation = containment (parent → child).
# - Kinds: e=Extension (optional top-level bundle containing Categories + Concepts; only used when the project has separable extensions/plugins), c=Category (grouping of Features), f=Feature (capability), fu=Functionality (qualified <feature>.<verb>; verb on a Feature), @=Concept (cross-cutting concern referenced by multiple Features), ui=UI Page.
# - Bracketed edges: [where: ui.X] = entity appears at UI page X; [refs: f.X] = entity cross-references Feature X; [cr: PATH] = code reference (file/folder implementing this entity); [cr.<tag>: PATH] = code reference partitioned by layer (common tags: fe=frontend, be=backend).
# - [UNRESOLVED] = the named entity has no definition in any parsed file; the reference is a broken link.
# - `← target` (focus mode only) marks the entity the artifact is centred on.
# - Trust: structure (hierarchy, references) is analyzer-checked, but descriptions and cr: paths are authored, not derived from code — they can lag recent changes. Treat them as strong leads, not ground truth; verify mechanism details against the cr: target before relying on them.";

const DESC_TRUNCATE: usize = 80;

pub(crate) fn render_elevator_text(graph: &DependencyGraph, root: Option<&str>) -> String {
    let entities: Vec<&CodeEntity> = graph
        .entities()
        .filter(|e| e.tags.contains("elevator"))
        .collect();

    if entities.is_empty() {
        return "(no Elevator entities in this project)\n".to_string();
    }

    let by_id: HashMap<String, &CodeEntity> = entities
        .iter()
        .map(|e| (e.id.clone(), *e))
        .collect();

    // Walk relationships once into per-source maps. Edges are
    // de-duplicated by (source, target) pair: when the analysis spans
    // several `.elv` files that each declare the same containment
    // (e.g. multiple fixtures asserting `c library { f protocol }`),
    // every file contributes its own Contains edge — visually
    // pointless to render the same child twice.
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut children_seen: HashSet<(String, String)> = HashSet::new();
    let mut where_targets: HashMap<String, Vec<String>> = HashMap::new();
    let mut where_seen: HashSet<(String, String)> = HashSet::new();
    let mut references: HashMap<String, Vec<String>> = HashMap::new();
    let mut references_seen: HashSet<(String, String)> = HashSet::new();
    let mut concept_users: HashMap<String, Vec<String>> = HashMap::new();
    let mut concept_users_seen: HashSet<(String, String)> = HashSet::new();
    let mut unresolved_contains: HashSet<(String, String)> = HashSet::new();

    for rel in graph.relationships() {
        let unresolved = rel
            .metadata
            .get("unresolved")
            .map(|v| v.as_str() == "true")
            .unwrap_or(false);
        let pair = (rel.source_id.clone(), rel.target_id.clone());
        match rel.kind {
            RelationshipKind::Contains => {
                if unresolved {
                    unresolved_contains.insert(pair.clone());
                }
                if children_seen.insert(pair.clone()) {
                    children
                        .entry(rel.source_id.clone())
                        .or_default()
                        .push(rel.target_id.clone());
                }
            }
            RelationshipKind::References => {
                let link = rel
                    .metadata
                    .get("link")
                    .map(String::as_str)
                    .unwrap_or("");
                match link {
                    "where" => {
                        if where_seen.insert(pair.clone()) {
                            where_targets
                                .entry(rel.source_id.clone())
                                .or_default()
                                .push(rel.target_id.clone());
                        }
                    }
                    "references" => {
                        if references_seen.insert(pair.clone()) {
                            references
                                .entry(rel.source_id.clone())
                                .or_default()
                                .push(rel.target_id.clone());
                        }
                    }
                    "used_by" => {
                        // Concept consumers — reverse direction
                        // (target = concept, source = consumer).
                        let concept_pair = (rel.target_id.clone(), rel.source_id.clone());
                        if concept_users_seen.insert(concept_pair) {
                            concept_users
                                .entry(rel.target_id.clone())
                                .or_default()
                                .push(rel.source_id.clone());
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Determine roots. With a `--root` argument we render exactly one
    // subtree; without, we render every Category and any orphan
    // top-level Feature.
    let root_ids: Vec<String> = match root {
        Some(arg) => match resolve_root(arg, &entities, &by_id) {
            Some(id) => vec![id],
            None => return format!("(no Elevator entity matches `{}`)\n", arg),
        },
        None => collect_default_roots(&entities),
    };

    // Compute the set of entity IDs reachable from the roots — used to
    // scope which Concepts are relevant when `--root` is set.
    let scoped: HashSet<String> = root_ids
        .iter()
        .flat_map(|r| collect_descendants(r, &children, &by_id))
        .collect();

    // Concepts: include all when unscoped, only the cross-cuts that
    // reach into the scoped subtree when `--root` is set.
    let concepts: Vec<&CodeEntity> = {
        let mut cs: Vec<&CodeEntity> = entities
            .iter()
            .copied()
            .filter(|e| e.kind == EntityKind::Concept)
            .filter(|c| {
                root.is_none()
                    || concept_users
                        .get(&c.id)
                        .map(|users| users.iter().any(|u| scoped.contains(u)))
                        .unwrap_or(false)
            })
            .collect();
        cs.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        cs
    };

    let mut out = String::new();

    if let Some(arg) = root {
        let _ = writeln!(out, "# Elevator subtree: {}", arg);
    } else {
        let _ = writeln!(out, "# Project map (Elevator)");
    }
    // Extension count is only mentioned when at least one exists;
    // most specs don't use the optional top layer and the omission
    // keeps the header line tight for them.
    let ext_count = count_kind(&entities, EntityKind::Extension);
    let ext_prefix = if ext_count > 0 {
        format!("{} extensions, ", ext_count)
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "> {}{} categories, {} features, {} functionalities, {} concepts, {} ui pages",
        ext_prefix,
        count_kind(&entities, EntityKind::Category),
        count_kind(&entities, EntityKind::Feature),
        count_kind(&entities, EntityKind::Functionality),
        count_kind(&entities, EntityKind::Concept),
        count_kind(&entities, EntityKind::UiPage),
    );
    let _ = writeln!(out);

    for root_id in &root_ids {
        render_subtree(
            root_id,
            0,
            &children,
            &by_id,
            &where_targets,
            &references,
            &unresolved_contains,
            &mut out,
        );
    }

    if !concepts.is_empty() {
        let _ = writeln!(out, "\n# Cross-cutting");
        for c in &concepts {
            let desc = description_suffix(c, Some(DESC_TRUNCATE));
            let unresolved = if c.tags.contains("unresolved") {
                " [UNRESOLVED]"
            } else {
                ""
            };
            let _ = writeln!(out, "@ {}{}{}", c.qualified_name, desc, unresolved);
            if let Some(users) = concept_users.get(&c.id) {
                let names: Vec<String> = users
                    .iter()
                    .filter_map(|u| by_id.get(u).map(|e| short_ref(e)))
                    .collect();
                if !names.is_empty() {
                    let _ = writeln!(out, "  used by: {}", names.join(", "));
                }
            }
        }
    }

    out
}

/// Render the *context* of a single entity for an LLM consumer:
/// ancestor chain (root Category → … → target's parent), siblings at
/// the target's level, the target's full subtree, and any Concepts
/// whose `used_by:` reaches into the path.
///
/// The artifact answers "what does an LLM session working *on* this
/// node need to know?" — what the target sits inside, what else is at
/// its level (so the LLM doesn't duplicate sibling work), and which
/// cross-cutting concerns apply.
pub(crate) fn render_focus(graph: &DependencyGraph, target_arg: &str) -> String {
    let entities: Vec<&CodeEntity> = graph
        .entities()
        .filter(|e| e.tags.contains("elevator"))
        .collect();
    if entities.is_empty() {
        return "(no Elevator entities in this project)\n".to_string();
    }
    let by_id: HashMap<String, &CodeEntity> = entities
        .iter()
        .map(|e| (e.id.clone(), *e))
        .collect();

    let target_id = match resolve_root(target_arg, &entities, &by_id) {
        Some(id) => id,
        None => {
            let suggestions = suggest_close_matches(target_arg, &entities);
            return format!(
                "(no Elevator entity matches `{}`){}",
                target_arg,
                if suggestions.is_empty() {
                    "\n\nRun `elevator . --list` to see every defined entity.\n".to_string()
                } else {
                    suggestions
                }
            );
        }
    };

    // Build the relationship maps the same way `render_elevator_text`
    // does. Duplicated rather than shared because the two renderers
    // need slightly different shapes (focus uses parent_id chains
    // directly; subtree walks Contains edges).
    let (children, where_targets, references, concept_users) = build_edge_maps(graph);

    // 1. Ancestor chain: walk parent_id up to a top-level node.
    //    `path[0]` is the root-most ancestor; the last element is the
    //    target itself.
    let mut path: Vec<String> = Vec::new();
    let mut cursor = Some(target_id.clone());
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(id) = cursor {
        if !seen.insert(id.clone()) {
            break; // cycle guard
        }
        path.push(id.clone());
        cursor = by_id
            .get(&id)
            .and_then(|e| e.parent_id.clone())
            .filter(|p| by_id.contains_key(p));
    }
    path.reverse();

    // 2. Siblings of the target: other children of target's parent.
    let parent_id = by_id.get(&target_id).and_then(|e| e.parent_id.clone());
    let mut siblings: Vec<String> = parent_id
        .as_ref()
        .and_then(|p| children.get(p))
        .cloned()
        .unwrap_or_default();
    siblings.retain(|id| id != &target_id);
    siblings.sort();

    // 3. Target's own subtree (full descendants).
    let descendants: HashSet<String> = collect_descendants(&target_id, &children, &by_id)
        .into_iter()
        .collect();

    // 4. Concepts whose `used_by:` touches anything in the path or
    //    the target's subtree. These are the cross-cutting concerns
    //    the LLM needs to know apply here.
    let touched: HashSet<String> = path.iter().cloned().chain(descendants.iter().cloned()).collect();
    let mut relevant_concepts: Vec<&CodeEntity> = entities
        .iter()
        .copied()
        .filter(|e| e.kind == EntityKind::Concept)
        .filter(|c| {
            concept_users
                .get(&c.id)
                .map(|users| users.iter().any(|u| touched.contains(u)))
                .unwrap_or(false)
        })
        .collect();
    relevant_concepts.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));

    // 5. Render.
    let mut out = String::new();
    let target = by_id.get(&target_id).copied().unwrap();
    let _ = writeln!(out, "# Focus: {} {}", kind_marker(target.kind), target.qualified_name);
    let _ = writeln!(
        out,
        "> ancestors, siblings, target subtree, and cross-cutting concepts"
    );
    let _ = writeln!(out);

    // Ancestor chain rendered as a tree where each ancestor's body
    // contains only the next ancestor in the path — until we hit the
    // target, where we render siblings + target (marked) + target's
    // subtree.
    for (i, id) in path.iter().enumerate() {
        let depth = i;
        let is_target = id == &target_id;
        let entity = match by_id.get(id) {
            Some(e) => *e,
            None => continue,
        };
        render_focus_line(
            entity,
            depth,
            is_target,
            &where_targets,
            &references,
            &by_id,
            &mut out,
        );
        if is_target {
            // Render target's children in full.
            let mut sorted_kids: Vec<String> = children.get(id).cloned().unwrap_or_default();
            sorted_kids.sort();
            sorted_kids.dedup();
            for child_id in sorted_kids {
                render_focus_subtree(
                    &child_id,
                    depth + 1,
                    &children,
                    &by_id,
                    &where_targets,
                    &references,
                    &mut out,
                );
            }
        }
    }

    // Siblings — rendered AFTER the target's own block so the target
    // and its descendants stay visually grouped, with siblings as a
    // peer list below.
    if !siblings.is_empty() {
        let parent_name = parent_id
            .as_ref()
            .and_then(|p| by_id.get(p))
            .map(|e| short_ref(e))
            .unwrap_or_else(|| "(top-level)".to_string());
        let _ = writeln!(out, "\n# Siblings (other children of {})", parent_name);
        let parent_depth = path.len().saturating_sub(2);
        for sib_id in &siblings {
            if let Some(sib) = by_id.get(sib_id).copied() {
                render_focus_line(
                    sib,
                    parent_depth + 1,
                    false,
                    &where_targets,
                    &references,
                    &by_id,
                    &mut out,
                );
            }
        }
    }

    if !relevant_concepts.is_empty() {
        let _ = writeln!(out, "\n# Cross-cutting concepts touching this path");
        for c in relevant_concepts {
            let desc = description_suffix(c, None);
            let _ = writeln!(out, "@ {}{}", c.qualified_name, desc);
            if let Some(users) = concept_users.get(&c.id) {
                let mut names: Vec<String> = users
                    .iter()
                    .filter_map(|u| by_id.get(u).map(|e| short_ref(e)))
                    .collect();
                names.sort();
                names.dedup();
                if !names.is_empty() {
                    let _ = writeln!(out, "  used by: {}", names.join(", "));
                }
            }
        }
    }

    out
}

/// Single line for a node in the focus view. Marks the target with a
/// trailing `   ← target` so an LLM (or human) can locate it instantly.
fn render_focus_line(
    entity: &CodeEntity,
    depth: usize,
    is_target: bool,
    where_targets: &HashMap<String, Vec<String>>,
    references: &HashMap<String, Vec<String>>,
    by_id: &HashMap<String, &CodeEntity>,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    let kind = kind_marker(entity.kind);
    let desc = description_suffix(entity, None);
    let brackets = bracket_metadata(&entity.id, entity, where_targets, references, by_id);
    let unresolved = if entity.tags.contains("unresolved") {
        " [UNRESOLVED]"
    } else {
        ""
    };
    let target_marker = if is_target { "   ← target" } else { "" };
    let _ = writeln!(
        out,
        "{}{} {}{}{}{}{}",
        indent, kind, entity.name, desc, brackets, unresolved, target_marker
    );
}

/// Recursively render the target's subtree under the focus view.
fn render_focus_subtree(
    id: &str,
    depth: usize,
    children: &HashMap<String, Vec<String>>,
    by_id: &HashMap<String, &CodeEntity>,
    where_targets: &HashMap<String, Vec<String>>,
    references: &HashMap<String, Vec<String>>,
    out: &mut String,
) {
    let Some(entity) = by_id.get(id).copied() else {
        return;
    };
    render_focus_line(entity, depth, false, where_targets, references, by_id, out);
    let mut child_ids: Vec<String> = children.get(id).cloned().unwrap_or_default();
    child_ids.sort();
    child_ids.dedup();
    for child_id in child_ids {
        render_focus_subtree(&child_id, depth + 1, children, by_id, where_targets, references, out);
    }
}

/// Walk the graph's relationships once into the per-source maps both
/// renderers consume. Returns `(children, where_targets, references,
/// concept_users)`. De-duplicates by (source, target) pair so multiple
/// `.elv` files declaring the same containment don't produce visual
/// duplicates.
fn build_edge_maps(
    graph: &DependencyGraph,
) -> (
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
) {
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut where_targets: HashMap<String, Vec<String>> = HashMap::new();
    let mut references: HashMap<String, Vec<String>> = HashMap::new();
    let mut concept_users: HashMap<String, Vec<String>> = HashMap::new();
    let mut seen: HashSet<(String, String, &'static str)> = HashSet::new();

    for rel in graph.relationships() {
        let pair = (rel.source_id.clone(), rel.target_id.clone());
        match rel.kind {
            RelationshipKind::Contains => {
                if seen.insert((pair.0.clone(), pair.1.clone(), "contains")) {
                    children
                        .entry(rel.source_id.clone())
                        .or_default()
                        .push(rel.target_id.clone());
                }
            }
            RelationshipKind::References => {
                let link = rel.metadata.get("link").map(String::as_str).unwrap_or("");
                match link {
                    "where" => {
                        if seen.insert((pair.0.clone(), pair.1.clone(), "where")) {
                            where_targets
                                .entry(rel.source_id.clone())
                                .or_default()
                                .push(rel.target_id.clone());
                        }
                    }
                    "references" => {
                        if seen.insert((pair.0.clone(), pair.1.clone(), "references")) {
                            references
                                .entry(rel.source_id.clone())
                                .or_default()
                                .push(rel.target_id.clone());
                        }
                    }
                    "used_by" => {
                        // Concept consumers — reverse direction.
                        if seen.insert((rel.target_id.clone(), rel.source_id.clone(), "used_by")) {
                            concept_users
                                .entry(rel.target_id.clone())
                                .or_default()
                                .push(rel.source_id.clone());
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (children, where_targets, references, concept_users)
}

fn collect_default_roots(entities: &[&CodeEntity]) -> Vec<String> {
    // Render order, root-first: Extensions (optional top layer),
    // then Categories not contained by any Extension, then orphan
    // Features. Concepts and UI Pages render in their own section;
    // Functionalities never appear top-level.
    let mut extensions: Vec<&&CodeEntity> = entities
        .iter()
        .filter(|e| e.kind == EntityKind::Extension)
        .collect();
    extensions.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));

    // Categories with no parent (i.e. not under any Extension)
    // remain top-level so legacy single-Extension/no-Extension specs
    // keep their original output shape.
    let mut categories: Vec<&&CodeEntity> = entities
        .iter()
        .filter(|e| e.kind == EntityKind::Category && e.parent_id.is_none())
        .collect();
    categories.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));

    let mut orphan_features: Vec<&&CodeEntity> = entities
        .iter()
        .filter(|e| e.kind == EntityKind::Feature && e.parent_id.is_none())
        .collect();
    orphan_features.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));

    extensions
        .into_iter()
        .chain(categories.into_iter())
        .chain(orphan_features.into_iter())
        .map(|e| e.id.clone())
        .collect()
}

fn render_subtree(
    id: &str,
    depth: usize,
    children: &HashMap<String, Vec<String>>,
    by_id: &HashMap<String, &CodeEntity>,
    where_targets: &HashMap<String, Vec<String>>,
    references: &HashMap<String, Vec<String>>,
    unresolved_contains: &HashSet<(String, String)>,
    out: &mut String,
) {
    let Some(entity) = by_id.get(id).copied() else {
        return;
    };
    let indent = "  ".repeat(depth);
    let kind = kind_marker(entity.kind);
    let desc = description_suffix(entity, Some(DESC_TRUNCATE));
    let brackets = bracket_metadata(id, entity, where_targets, references, by_id);
    let unresolved = if entity.tags.contains("unresolved") {
        " [UNRESOLVED]"
    } else {
        ""
    };
    let _ = writeln!(
        out,
        "{}{} {}{}{}{}",
        indent, kind, entity.name, desc, brackets, unresolved
    );

    let mut child_ids: Vec<String> = children.get(id).cloned().unwrap_or_default();
    child_ids.sort();
    for child_id in child_ids {
        // Render an out-of-scope Contains edge inline with a marker so
        // the reader can see the reference even when the target's file
        // isn't imported. We don't recurse into it — the disconnect is
        // the point.
        let edge = (id.to_string(), child_id.clone());
        if unresolved_contains.contains(&edge) {
            if let Some(child) = by_id.get(&child_id).copied() {
                let kind = kind_marker(child.kind);
                let _ = writeln!(
                    out,
                    "{}  {} {} [UNRESOLVED — out-of-scope: missing import]",
                    indent, kind, child.name
                );
            }
            continue;
        }
        render_subtree(
            &child_id,
            depth + 1,
            children,
            by_id,
            where_targets,
            references,
            unresolved_contains,
            out,
        );
    }
}

fn bracket_metadata(
    id: &str,
    entity: &CodeEntity,
    where_targets: &HashMap<String, Vec<String>>,
    references: &HashMap<String, Vec<String>>,
    by_id: &HashMap<String, &CodeEntity>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(targets) = where_targets.get(id) {
        let names: Vec<String> = targets
            .iter()
            .filter_map(|t| by_id.get(t).map(|e| format!("ui.{}", e.qualified_name)))
            .collect();
        if !names.is_empty() {
            parts.push(format!("where: {}", names.join(", ")));
        }
    }
    if let Some(targets) = references.get(id) {
        let names: Vec<String> = targets
            .iter()
            .filter_map(|t| by_id.get(t).map(|e| short_ref(e)))
            .collect();
        if !names.is_empty() {
            parts.push(format!("refs: {}", names.join(", ")));
        }
    }
    // Code refs from `attributes`. Two prefix shapes:
    //   - `cr:<path>`         — generic / unscoped
    //   - `cr.<tag>:<path>`   — partitioned by layer (e.g. `cr.fe`,
    //                           `cr.be`)
    // We group paths by tag while preserving first-seen tag order so
    // an LLM consumer can pick out a single layer with a textual
    // grep ("`cr.be:`") instead of inferring from path conventions.
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for attr in &entity.attributes {
        let (tag, path) = if let Some(rest) = attr.strip_prefix("cr.") {
            match rest.split_once(':') {
                Some((tag, path)) => (tag.to_string(), path.to_string()),
                None => continue,
            }
        } else if let Some(path) = attr.strip_prefix("cr:") {
            (String::new(), path.to_string())
        } else {
            continue;
        };
        if let Some((_, paths)) = grouped.iter_mut().find(|(t, _)| t == &tag) {
            paths.push(path);
        } else {
            grouped.push((tag, vec![path]));
        }
    }
    for (tag, paths) in grouped {
        let label = if tag.is_empty() {
            "cr".to_string()
        } else {
            format!("cr.{}", tag)
        };
        parts.push(format!("{}: {}", label, paths.join(", ")));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join("; "))
    }
}

fn collect_descendants(
    root: &str,
    children: &HashMap<String, Vec<String>>,
    by_id: &HashMap<String, &CodeEntity>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if !by_id.contains_key(&id) {
            continue;
        }
        out.push(id.clone());
        if let Some(c) = children.get(&id) {
            stack.extend(c.iter().cloned());
        }
    }
    out
}

/// Resolve a user-supplied root argument to an entity ID. Accepts:
///   - a full ID (`elevator::c.library`)
///   - a kind-prefixed qualname (`c.library`, `f.protocol`,
///     `fu.protocol.creation`)
///   - a bare name — searched against `qualified_name`/`name`, with
///     Category/Feature/Concept/Functionality/UiPage priority order
///     for ambiguity resolution.
///
/// For kind-prefixed input (e.g. `fu.protocol.creation`), the
/// resolver tries the deterministic ID lookup first, then falls back
/// to matching the stripped form against `qualified_name` or `name`
/// of the corresponding kind. This makes typos like
/// `fu.protocol_creation` (underscore) discoverable when the real
/// entity is `fu.protocol.creation` (dot) — we won't accept the
/// typo, but the suggestion path below will.
pub(crate) fn resolve_root(
    input: &str,
    entities: &[&CodeEntity],
    by_id: &HashMap<String, &CodeEntity>,
) -> Option<String> {
    if by_id.contains_key(input) {
        return Some(input.to_string());
    }
    if input.starts_with("elevator::") && by_id.contains_key(input) {
        return Some(input.to_string());
    }
    for prefix in ["e.", "c.", "fu.", "concept.", "ui.", "f."] {
        if let Some(rest) = input.strip_prefix(prefix) {
            // Try deterministic ID lookup first (covers the common
            // case where the input is already a valid qualified name).
            let id = format!("elevator::{}{}", prefix, rest);
            if by_id.contains_key(&id) {
                return Some(id);
            }
            // Fall back to matching the stripped form against
            // qualified_name / name of the appropriate kind. Lets the
            // user write `fu.creation` and find the unique
            // Functionality named `<feature>.creation`.
            let target_kind = kind_from_prefix(prefix);
            if let Some(e) = entities.iter().find(|e| {
                e.kind == target_kind && (e.qualified_name == rest || e.name == rest)
            }) {
                return Some(e.id.clone());
            }
        }
    }
    let priority = |k: EntityKind| match k {
        EntityKind::Extension => 0,
        EntityKind::Category => 1,
        EntityKind::Feature => 2,
        EntityKind::Concept => 3,
        EntityKind::Functionality => 4,
        EntityKind::UiPage => 5,
        _ => 99,
    };
    let mut candidates: Vec<&&CodeEntity> = entities
        .iter()
        .filter(|e| e.qualified_name == input || e.name == input)
        .collect();
    candidates.sort_by_key(|e| priority(e.kind));
    candidates.first().map(|e| e.id.clone())
}

/// Strip a leading kind prefix (`c.`, `f.`, `fu.`, `concept.`,
/// `ui.`) from the input. Mirrors the parser's private helper of
/// the same name — duplicated rather than re-exported to keep the
/// renderer independent of parser internals.
fn strip_kind_prefix(input: &str) -> &str {
    for prefix in ["concept.", "fu.", "ui.", "e.", "c.", "f."] {
        if let Some(rest) = input.strip_prefix(prefix) {
            return rest;
        }
    }
    input
}

/// Map a kind prefix string (`e.`, `c.`, `f.`, `fu.`, `concept.`,
/// `ui.`) back to its `EntityKind`. Used by the resolver and the
/// did-you-mean suggester below.
fn kind_from_prefix(prefix: &str) -> EntityKind {
    match prefix {
        "e." => EntityKind::Extension,
        "c." => EntityKind::Category,
        "f." => EntityKind::Feature,
        "fu." => EntityKind::Functionality,
        "concept." => EntityKind::Concept,
        "ui." => EntityKind::UiPage,
        _ => EntityKind::Unknown,
    }
}

/// Build the suggestion block printed when a `--focus` / `--root`
/// argument doesn't match any entity. Filters candidates to the kind
/// implied by the input's prefix (if any) and ranks by overlap with
/// the user's input. Returns an empty string when no useful hint
/// can be produced.
pub(crate) fn suggest_close_matches(input: &str, entities: &[&CodeEntity]) -> String {
    // If the user prefixed a kind, restrict suggestions to it —
    // makes the list focused and short.
    let kind_filter: Option<EntityKind> = ["e.", "c.", "fu.", "concept.", "ui.", "f."]
        .iter()
        .find(|p| input.starts_with(*p))
        .map(|p| kind_from_prefix(p));
    let stripped = strip_kind_prefix(input).to_lowercase();
    if stripped.is_empty() {
        return String::new();
    }

    // Substring match in either direction so `fu.creation` finds
    // `fu protocol.creation`, and `fu.entity_type_creation` finds
    // `fu entity_type.creation` (the underscore→dot typo case the
    // bug report surfaced).
    let mut hits: Vec<&CodeEntity> = entities
        .iter()
        .filter(|e| kind_filter.map_or(true, |k| e.kind == k))
        .filter(|e| {
            let q = e.qualified_name.to_lowercase();
            let n = e.name.to_lowercase();
            q.contains(&stripped)
                || n.contains(&stripped)
                || stripped.contains(&q)
                || stripped.contains(&n)
        })
        .copied()
        .collect();
    hits.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    hits.dedup_by(|a, b| a.id == b.id);
    hits.truncate(8);

    if hits.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nDid you mean one of these?\n");
    for e in hits {
        let _ = writeln!(out, "  {} {}", kind_marker(e.kind), e.qualified_name);
    }
    out
}

fn count_kind(entities: &[&CodeEntity], k: EntityKind) -> usize {
    entities.iter().filter(|e| e.kind == k).count()
}

/// `max = None` renders the description in full — used by focus mode,
/// whose artifact exists to feed an LLM: clipping `d:` there discards
/// the back half of the sentence, which is usually where the mechanism
/// lives. The tree views keep the cap for scanability.
fn description_suffix(entity: &CodeEntity, max: Option<usize>) -> String {
    entity
        .documentation
        .as_deref()
        .map(|d| match max {
            Some(m) => format!(" — {}", truncate(d, m)),
            None => format!(" — {}", d),
        })
        .unwrap_or_default()
}

fn kind_marker(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Extension => "e",
        EntityKind::Category => "c",
        EntityKind::Feature => "f",
        EntityKind::Functionality => "fu",
        EntityKind::UiPage => "ui",
        EntityKind::Concept => "@",
        _ => "?",
    }
}

fn short_ref(e: &CodeEntity) -> String {
    let prefix = match e.kind {
        EntityKind::Feature => "f",
        EntityKind::Functionality => "fu",
        EntityKind::Extension => "e",
        EntityKind::Category => "c",
        EntityKind::Concept => "concept",
        EntityKind::UiPage => "ui",
        _ => "?",
    };
    format!("{}.{}", prefix, e.qualified_name)
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Relationship, Span};

    const LONG_DESC: &str = "REST-pull inbound feed retrieving image CDN URLs so downstream exports can prefer the pristine variant over the original when both exist.";

    fn elevator_entity(id: &str, name: &str, kind: EntityKind) -> CodeEntity {
        let mut e = CodeEntity::new(name, kind, "spec.elv", Span::default());
        e.id = id.to_string();
        e.qualified_name = name.to_string();
        e.tags.insert("elevator".to_string());
        e
    }

    fn graph_with_long_description() -> DependencyGraph {
        let mut g = DependencyGraph::new();
        let cat = elevator_entity("elevator::c.library", "library", EntityKind::Category);
        let mut feat = elevator_entity("elevator::f.protocol", "protocol", EntityKind::Feature);
        feat.documentation = Some(LONG_DESC.to_string());
        feat.parent_id = Some("elevator::c.library".to_string());
        g.add_entity(cat);
        g.add_entity(feat);
        g.add_relationship(Relationship::new(
            "elevator::c.library",
            "elevator::f.protocol",
            RelationshipKind::Contains,
        ));
        g
    }

    #[test]
    fn tree_view_truncates_long_descriptions() {
        assert!(LONG_DESC.chars().count() > DESC_TRUNCATE);
        let out = render_elevator_text(&graph_with_long_description(), None);
        assert!(out.contains('…'), "tree view should clip at DESC_TRUNCATE:\n{out}");
        assert!(!out.contains(LONG_DESC));
    }

    #[test]
    fn focus_mode_renders_descriptions_in_full() {
        let out = render_focus(&graph_with_long_description(), "f.protocol");
        assert!(
            out.contains(LONG_DESC),
            "focus mode is the LLM bundle — `d:` must not be clipped:\n{out}"
        );
    }
}
