//! Structural diff engine: compares two analysis results at the entity level.
//!
//! Matches entities by `(name, kind, file_path_relative, parent, arity)` so
//! that line-number shifts from unrelated edits don't create false "removed +
//! added" pairs, and so that overloads of one name stay distinct.
//! For each matched entity, compares source code hash and metrics to classify
//! it as unchanged or modified. Unmatched entities are added or removed.

use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind, EntityMetrics, RelationshipKind};
use crate::output::{self, JsonRenderer, OutputFormat};

/// Stable key for matching entities across commits. Uses name + kind +
/// relative file path + arity (not the raw entity ID, which includes line
/// numbers that shift on every edit).
///
/// Arity is what keeps overloads apart. Without it, `foo(a)` and `foo(a, b)`
/// in one file are the same key, and since the base side is indexed into a map
/// only one of them survives — so both head overloads match against whichever
/// came last, and the parameter list they don't share reads as an edit. On a
/// Java repo where overloading is the norm that turns a 7-file change into an
/// 80-file one, in files `git diff` reports nothing for (UI-087).
///
/// Arity is not a full discriminator — `foo(String)` and `foo(int)` still
/// collide — so it does not stand alone. [`match_group`] resolves the
/// remainder by parameter types, and the key deliberately stops short of
/// carrying those: a type whose spelling resolves differently on the two sides
/// would split one method into an addition plus a removal, which is a worse
/// lie than the one being fixed.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct EntityKey {
    name: String,
    kind: EntityKind,
    file_path: String,
    /// Parent name (for methods inside a class/struct — disambiguates
    /// `Foo.bar` from `Baz.bar` in the same file).
    parent_name: Option<String>,
    /// Parameter count for callables, `None` for everything else — so
    /// non-callables all share one value and are unaffected by this field.
    arity: Option<u32>,
}

fn entity_key(e: &CodeEntity, root: &std::path::Path) -> EntityKey {
    let file_path = e
        .file_path
        .strip_prefix(root)
        .unwrap_or(&e.file_path)
        .display()
        .to_string();
    EntityKey {
        name: e.name.clone(),
        kind: e.kind,
        file_path,
        parent_name: e.parent_id.as_ref().map(|pid| {
            // parent_id might be a full entity ID or a bare name.
            // Extract just the name part for stable matching.
            pid.rsplit(':').next().unwrap_or(pid).to_string()
        }),
        arity: e.metrics.param_count,
    }
}

/// An entity's declared parameter types, for telling same-arity overloads
/// apart. Declared spelling, not a resolved type: both sides are parsed by the
/// same parser from the same text, so the spelling matches whenever the
/// signature genuinely did.
fn signature(e: &CodeEntity) -> Vec<&str> {
    e.parameters
        .iter()
        .map(|p| p.type_name.as_deref().unwrap_or(""))
        .collect()
}

/// Claim `head`'s counterpart out of the base entities sharing its key,
/// removing it so no two head entities can match the same one.
///
/// Signature first, so reordering overloads doesn't report both as modified;
/// then the first one left, which is right whenever the group is a single
/// entity — the overwhelming case — and is at worst a stable pairing when a
/// group of same-arity overloads has genuinely churned.
fn match_group<'a>(group: &mut Vec<&'a CodeEntity>, head: &CodeEntity) -> Option<&'a CodeEntity> {
    if group.is_empty() {
        return None;
    }
    let want = signature(head);
    let i = group.iter().position(|b| signature(b) == want).unwrap_or(0);
    Some(group.remove(i))
}

/// The same key with arity dropped — what the second matching pass runs on.
fn loose_key(key: &EntityKey) -> EntityKey {
    EntityKey {
        arity: None,
        ..key.clone()
    }
}

/// Pair head entities with their base counterparts.
///
/// Returns the rows for everything on the head side, where each survivor
/// landed (the relationship pass hangs its deltas off those indices), and the
/// base entities nothing claimed — the removals.
///
/// Two passes, because arity has to be in the key and cannot be the last word:
///
/// 1. **Exact key, arity included.** This is what keeps overloads apart, and
///    on an unedited file it consumes the whole group — which is why the
///    second pass cannot resurrect the phantom edits (UI-087).
/// 2. **Key minus arity, over what pass 1 left.** A method that gained a
///    parameter has no exact counterpart, and reporting it as an addition
///    plus an unrelated removal loses the before/after the reader came for.
///    Only entities that failed pass 1 on *both* sides are in play here, so
///    an overload group that merely exists cannot reach it.
fn match_entities<'a>(
    base_graph: &'a DependencyGraph,
    head_graph: &'a DependencyGraph,
    base_root: &std::path::Path,
    head_root: &std::path::Path,
) -> (
    Vec<EntityDiff>,
    HashMap<EntityKey, usize>,
    Vec<&'a CodeEntity>,
) {
    // A key holds a *list*, not a single entity: overloads that survive the
    // arity split (same name, same arity, different types) still share one,
    // and a map that keeps only the last of them is what made unedited files
    // report edits.
    let mut exact: HashMap<EntityKey, Vec<&CodeEntity>> = HashMap::new();
    for e in base_graph.entities() {
        if e.kind == EntityKind::Parameter {
            continue;
        }
        exact.entry(entity_key(e, base_root)).or_default().push(e);
    }

    let mut diffs = Vec::new();
    let mut survivors: HashMap<EntityKey, usize> = HashMap::new();
    let mut pending = Vec::new();

    for e in head_graph.entities() {
        if e.kind == EntityKind::Parameter {
            continue;
        }
        let key = entity_key(e, head_root);
        let file_path = rel_path(e, head_root);
        match exact.get_mut(&key).and_then(|group| match_group(group, e)) {
            Some(base_e) => {
                survivors.insert(key, diffs.len());
                diffs.push(diff_matched(e, base_e, file_path));
            }
            None => pending.push((e, key, file_path)),
        }
    }

    let mut loose: HashMap<EntityKey, Vec<&CodeEntity>> = HashMap::new();
    for (key, group) in exact {
        loose.entry(loose_key(&key)).or_default().extend(group);
    }
    for (e, key, file_path) in pending {
        match loose
            .get_mut(&loose_key(&key))
            .and_then(|group| match_group(group, e))
        {
            Some(base_e) => {
                survivors.insert(key, diffs.len());
                diffs.push(diff_matched(e, base_e, file_path));
            }
            None => diffs.push(diff_unmatched(e, file_path, ChangeStatus::Added)),
        }
    }

    let unclaimed = loose.into_values().flatten().collect();
    (diffs, survivors, unclaimed)
}

/// Change status for a single entity.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Added,
    Removed,
    Modified,
    Unchanged,
}

/// Per-metric delta: old value, new value, numeric change.
#[derive(Debug, Clone, Serialize)]
pub struct MetricDelta {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<f64>,
    /// Positive = increased, negative = decreased, 0 = unchanged.
    pub delta: f64,
}

/// Which end of a changed edge the entity carrying the delta sits on.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelDirection {
    /// The entity is the source: it now calls / imports / contains something
    /// it didn't, or has stopped.
    Outgoing,
    /// The entity is the target: something now calls it, or no longer does.
    Incoming,
}

/// One relationship that appeared or disappeared, seen from one of its ends.
///
/// Metric deltas answer *how much* the shape moved (`fan_out` went 9 → 10);
/// this answers *what* moved, which is the question a reader of a diff
/// actually has. It also catches the swap a count cannot: dropping one call
/// and adding another leaves `fan_out` untouched.
#[derive(Debug, Clone, Serialize)]
pub struct RelationshipDelta {
    /// `Added` or `Removed` — never `Modified`: an edge either exists or not.
    pub status: ChangeStatus,
    pub direction: RelDirection,
    /// Snake-case relationship kind (`calls`, `imports`), matching the kind
    /// strings on the graph JSON's links.
    pub kind: String,
    /// Human label already inflected for the direction — "calls" outgoing,
    /// "called by" incoming.
    pub label: String,
    /// The entity at the other end.
    pub other_name: String,
    pub other_kind: String,
    pub other_file: String,
    /// The other end's ID in the head graph, when it still exists there — so
    /// the UI can navigate to it. Absent when the edge died with its target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_entity_id: Option<String>,
}

/// Diff result for a single entity.
#[derive(Debug, Clone, Serialize)]
pub struct EntityDiff {
    /// The entity ID in the HEAD (to) graph. For removed entities, this
    /// is the base (from) ID.
    pub entity_id: String,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub status: ChangeStatus,
    /// True if the entity's source code changed (core change).
    /// False means only relational metrics (fan_in/fan_out) changed (impact).
    #[serde(default)]
    pub source_changed: bool,
    /// Non-empty only for Modified entities.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metric_deltas: Vec<MetricDelta>,
    /// Relationships this entity gained or lost. Only carried by entities
    /// that exist on both sides — see [`attach_rel_deltas`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rel_deltas: Vec<RelationshipDelta>,
    /// Entity ID in the base (from) graph, if it existed there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_entity_id: Option<String>,
}

/// Full diff output: the head graph + per-entity change annotations.
#[derive(Debug, Clone, Serialize)]
pub struct DiffResult {
    pub from_ref: String,
    pub to_ref: String,
    pub summary: DiffSummary,
    pub entities: Vec<EntityDiff>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffSummary {
    pub total_base: usize,
    pub total_head: usize,
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
    /// Subset of modified: entities whose source code changed (core changes).
    pub modified_source: usize,
    /// Subset of modified: entities where only relational metrics changed (impacts).
    pub modified_impact: usize,
    pub unchanged: usize,
    /// Edges that appeared, counted once each (not once per endpoint), over
    /// the edges [`attach_rel_deltas`] reports — those with at least one end
    /// that survived the diff. An added entity's own edges are implied by the
    /// entity and are left out of both this count and the per-entity lists.
    #[serde(default)]
    pub relationships_added: usize,
    /// Edges that disappeared, on the same terms.
    #[serde(default)]
    pub relationships_removed: usize,
}

/// Compare metrics between two entities and return deltas for any that changed.
/// Also returns whether any "intrinsic" metrics changed (vs just relational
/// metrics like fan_in/fan_out which change due to other entities).
fn compare_metrics(base: &EntityMetrics, head: &EntityMetrics) -> (Vec<MetricDelta>, bool) {
    let mut deltas = Vec::new();
    let mut intrinsic_changed = false;

    let mut check = |name: &str, old: Option<f64>, new: Option<f64>, is_intrinsic: bool| {
        let o = old.unwrap_or(0.0);
        let n = new.unwrap_or(0.0);
        let d = n - o;
        if d.abs() > 0.001 {
            if is_intrinsic {
                intrinsic_changed = true;
            }
            deltas.push(MetricDelta {
                name: name.to_string(),
                old,
                new,
                delta: d,
            });
        }
    };

    // Intrinsic metrics: change when the entity's own code changes
    check(
        "cyclomatic",
        base.cyclomatic.map(|v| v as f64),
        head.cyclomatic.map(|v| v as f64),
        true,
    );
    check(
        "max_nesting",
        base.max_nesting.map(|v| v as f64),
        head.max_nesting.map(|v| v as f64),
        true,
    );
    check("loc", Some(base.loc as f64), Some(head.loc as f64), true);
    check(
        "param_count",
        base.param_count.map(|v| v as f64),
        head.param_count.map(|v| v as f64),
        true,
    );
    check(
        "field_count",
        base.field_count.map(|v| v as f64),
        head.field_count.map(|v| v as f64),
        true,
    );
    check(
        "method_count",
        Some(base.method_count as f64),
        Some(head.method_count as f64),
        true,
    );
    check(
        "public_field_ratio",
        base.public_field_ratio.map(|v| v as f64),
        head.public_field_ratio.map(|v| v as f64),
        true,
    );

    // Relational metrics: change when OTHER entities change their relationships
    check(
        "fan_in",
        Some(base.fan_in as f64),
        Some(head.fan_in as f64),
        false,
    );
    check(
        "fan_out",
        Some(base.fan_out as f64),
        Some(head.fan_out as f64),
        false,
    );

    (deltas, intrinsic_changed)
}

// ------------------------------------------------------------------
//  Relationship diff
// ------------------------------------------------------------------

/// An edge identified by its endpoints' stable keys rather than their entity
/// IDs, for the same reason [`EntityKey`] exists: an edge whose target moved
/// down a few lines is the same edge, not a removal plus an addition.
type EdgeKey = (EntityKey, EntityKey, RelationshipKind);

/// What a reader needs to be told about the entity at the far end of a
/// changed edge.
struct Endpoint {
    name: String,
    kind: String,
    file_path: String,
    /// The head-graph ID, when this entity is still in the head graph. `None`
    /// for an endpoint that only exists in the base — a removed entity.
    head_id: Option<String>,
    language: Language,
}

/// Index one graph for edge diffing: entity ID → stable key, and stable key →
/// endpoint description. `in_head` decides whether the descriptions carry a
/// navigable head ID.
fn index_endpoints(
    graph: &DependencyGraph,
    root: &std::path::Path,
    in_head: bool,
) -> (HashMap<String, EntityKey>, HashMap<EntityKey, Endpoint>) {
    let mut key_of = HashMap::new();
    let mut endpoints = HashMap::new();
    for e in graph.entities() {
        if e.kind == EntityKind::Parameter {
            continue;
        }
        let key = entity_key(e, root);
        endpoints.insert(
            key.clone(),
            Endpoint {
                name: e.name.clone(),
                kind: e.kind.display_name().to_string(),
                file_path: key.file_path.clone(),
                head_id: in_head.then(|| e.id.clone()),
                language: Language::from_path(&e.file_path),
            },
        );
        key_of.insert(e.id.clone(), key);
    }
    (key_of, endpoints)
}

/// The graph's edges as stable keys. Edges touching an entity that isn't in
/// the key index (parameters) are dropped, and parallel edges of the same kind
/// collapse — this set answers "is there a `calls` edge here", which is the
/// question the panel asks.
fn edge_set(graph: &DependencyGraph, key_of: &HashMap<String, EntityKey>) -> HashSet<EdgeKey> {
    graph
        .relationships()
        .filter_map(|r| {
            let s = key_of.get(&r.source_id)?;
            let t = key_of.get(&r.target_id)?;
            Some((s.clone(), t.clone(), r.kind))
        })
        .collect()
}

/// One end's view of a changed edge. `lang` is the *source* entity's language
/// in both directions, matching how the graph JSON labels its links.
fn rel_delta(
    status: ChangeStatus,
    direction: RelDirection,
    kind: RelationshipKind,
    other: &Endpoint,
    lang: Language,
) -> RelationshipDelta {
    let kind_str = serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{:?}", kind).to_lowercase());
    let label = match direction {
        RelDirection::Outgoing => kind.display_label_for(lang),
        RelDirection::Incoming => kind.incoming_label_for(lang),
    };
    RelationshipDelta {
        status,
        direction,
        kind: kind_str,
        label: label.to_string(),
        other_name: other.name.clone(),
        other_kind: other.kind.clone(),
        other_file: other.file_path.clone(),
        other_entity_id: other.head_id.clone(),
    }
}

/// Record one changed edge on whichever of its ends survived the diff.
/// Returns whether it landed anywhere.
fn attach_edge(
    diffs: &mut [EntityDiff],
    survivors: &HashMap<EntityKey, usize>,
    endpoints: &HashMap<EntityKey, Endpoint>,
    edge: &EdgeKey,
    status: ChangeStatus,
) -> bool {
    let (src, tgt, kind) = edge;
    let (Some(src_end), Some(tgt_end)) = (endpoints.get(src), endpoints.get(tgt)) else {
        return false;
    };
    let lang = src_end.language;
    let mut landed = false;
    if let Some(&i) = survivors.get(src) {
        diffs[i].rel_deltas.push(rel_delta(
            status,
            RelDirection::Outgoing,
            *kind,
            tgt_end,
            lang,
        ));
        landed = true;
    }
    if let Some(&i) = survivors.get(tgt) {
        diffs[i].rel_deltas.push(rel_delta(
            status,
            RelDirection::Incoming,
            *kind,
            src_end,
            lang,
        ));
        landed = true;
    }
    landed
}

/// Hang every appeared/disappeared edge off the entities at its ends, and
/// return `(added, removed)` counts of the edges that found a home.
///
/// Only entities present on *both* sides carry deltas. Every edge of a
/// brand-new function is new by construction, and listing them under it would
/// bury the deltas that carry information — the ones on an entity that stayed,
/// and now points somewhere else.
fn attach_rel_deltas(
    diffs: &mut [EntityDiff],
    survivors: &HashMap<EntityKey, usize>,
    base_graph: &DependencyGraph,
    head_graph: &DependencyGraph,
    base_root: &std::path::Path,
    head_root: &std::path::Path,
) -> (usize, usize) {
    let (base_keys, base_ends) = index_endpoints(base_graph, base_root, false);
    let (head_keys, mut endpoints) = index_endpoints(head_graph, head_root, true);
    // Head descriptions win; the base fills in the entities that are gone.
    for (key, end) in base_ends {
        endpoints.entry(key).or_insert(end);
    }

    let base_edges = edge_set(base_graph, &base_keys);
    let head_edges = edge_set(head_graph, &head_keys);

    let mut added = 0;
    let mut removed = 0;
    for edge in head_edges.difference(&base_edges) {
        if attach_edge(diffs, survivors, &endpoints, edge, ChangeStatus::Added) {
            added += 1;
        }
    }
    for edge in base_edges.difference(&head_edges) {
        if attach_edge(diffs, survivors, &endpoints, edge, ChangeStatus::Removed) {
            removed += 1;
        }
    }

    // Set iteration order is not stable, and a list that reshuffles on every
    // recompute reads as churn the diff didn't find. Gained before lost,
    // outgoing before incoming, then by kind and by the far end's name.
    let order = |d: &RelationshipDelta| {
        (
            d.status != ChangeStatus::Added,
            d.direction != RelDirection::Outgoing,
            d.kind.clone(),
            d.other_name.clone(),
        )
    };
    for d in diffs.iter_mut() {
        d.rel_deltas.sort_by_key(&order);
    }
    (added, removed)
}

/// Simple hash of source code for quick equality check.
fn source_hash(e: &CodeEntity) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    e.source_code.hash(&mut hasher);
    hasher.finish()
}

/// The repo-relative spelling of an entity's file, for the diff row.
fn rel_path(e: &CodeEntity, root: &std::path::Path) -> String {
    e.file_path
        .strip_prefix(root)
        .unwrap_or(&e.file_path)
        .display()
        .to_string()
}

/// Compare one entity against its counterpart in the base graph.
///
/// Core change = the source moved, or an intrinsic metric did. Impact-only =
/// nothing but the relational metrics (`fan_in`/`fan_out`) moved, which
/// happens when *other* entities changed around this one.
fn diff_matched(head: &CodeEntity, base: &CodeEntity, file_path: String) -> EntityDiff {
    let source_code_changed = source_hash(head) != source_hash(base);
    let (metric_deltas, intrinsic_metrics_changed) = compare_metrics(&base.metrics, &head.metrics);
    let is_core_change = source_code_changed || intrinsic_metrics_changed;
    let status = if is_core_change || !metric_deltas.is_empty() {
        ChangeStatus::Modified
    } else {
        ChangeStatus::Unchanged
    };
    EntityDiff {
        entity_id: head.id.clone(),
        name: head.name.clone(),
        kind: head.kind.display_name().to_string(),
        file_path,
        status,
        source_changed: is_core_change,
        metric_deltas,
        rel_deltas: Vec::new(),
        base_entity_id: Some(base.id.clone()),
    }
}

/// A row for an entity that exists on one side only. Both are core changes:
/// whole entities arriving and leaving is what the diff is most sure of.
fn diff_unmatched(e: &CodeEntity, file_path: String, status: ChangeStatus) -> EntityDiff {
    EntityDiff {
        entity_id: e.id.clone(),
        name: e.name.clone(),
        kind: e.kind.display_name().to_string(),
        file_path,
        status,
        source_changed: true,
        metric_deltas: Vec::new(),
        rel_deltas: Vec::new(),
        base_entity_id: (status == ChangeStatus::Removed).then(|| e.id.clone()),
    }
}

/// Roll the finished rows up into the headline counts.
fn summarize(
    diffs: &[EntityDiff],
    total_base: usize,
    total_head: usize,
    relationships_added: usize,
    relationships_removed: usize,
) -> DiffSummary {
    let count = |s: ChangeStatus| diffs.iter().filter(|d| d.status == s).count();
    let modified = count(ChangeStatus::Modified);
    let modified_source = diffs
        .iter()
        .filter(|d| d.status == ChangeStatus::Modified && d.source_changed)
        .count();
    DiffSummary {
        total_base,
        total_head,
        added: count(ChangeStatus::Added),
        removed: count(ChangeStatus::Removed),
        modified,
        modified_source,
        modified_impact: modified - modified_source,
        unchanged: count(ChangeStatus::Unchanged),
        relationships_added,
        relationships_removed,
    }
}

/// Compute the structural diff between two graphs.
///
/// `base` is the "from" state, `head` is the "to" state. The returned
/// annotations reference entity IDs from the HEAD graph (for added +
/// modified + unchanged) or the BASE graph (for removed).
pub fn compute_diff(
    base_graph: &DependencyGraph,
    head_graph: &DependencyGraph,
    base_root: &std::path::Path,
    head_root: &std::path::Path,
    from_ref: &str,
    to_ref: &str,
) -> DiffResult {
    // `survivors` remembers where each entity that exists on both sides
    // landed, so the relationship pass can hang its deltas off the right rows.
    let (mut diffs, survivors, unclaimed) =
        match_entities(base_graph, head_graph, base_root, head_root);
    let total_base = base_graph
        .entities()
        .filter(|e| e.kind != EntityKind::Parameter)
        .count();

    // Removed entities: whatever neither matching pass claimed.
    for base_e in unclaimed {
        let file_path = rel_path(base_e, base_root);
        diffs.push(diff_unmatched(base_e, file_path, ChangeStatus::Removed));
    }

    let (rel_added, rel_removed) = attach_rel_deltas(
        &mut diffs, &survivors, base_graph, head_graph, base_root, head_root,
    );
    // An entity that swapped one call for another has identical metrics on
    // both sides — including `fan_out`. Rewiring is a change; say so, as an
    // impact rather than a core change, since its own source did not move.
    for d in diffs.iter_mut() {
        if d.status == ChangeStatus::Unchanged && !d.rel_deltas.is_empty() {
            d.status = ChangeStatus::Modified;
        }
    }

    let total_head = head_graph
        .entities()
        .filter(|e| e.kind != EntityKind::Parameter)
        .count();
    let summary = summarize(&diffs, total_base, total_head, rel_added, rel_removed);

    DiffResult {
        from_ref: from_ref.to_string(),
        to_ref: to_ref.to_string(),
        summary,
        entities: diffs,
    }
}

// ------------------------------------------------------------------
//  Shared git/analysis helpers for CLI and server diff workflows
// ------------------------------------------------------------------

use anyhow::Result as AnyhowResult;
use std::path::Path;
use std::process::Command;

/// Verify the given path is inside a git repository.
pub fn verify_git_repo(repo_root: &Path) -> AnyhowResult<()> {
    let status = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(repo_root)
        .output()?;
    if !status.status.success() {
        anyhow::bail!("{} is not inside a git repository", repo_root.display());
    }
    Ok(())
}

/// Resolve a git ref to its short SHA.
pub fn resolve_git_ref(repo_root: &Path, git_ref: &str) -> AnyhowResult<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", git_ref])
        .current_dir(repo_root)
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The subject on the commit that names the index, so anything that comes
/// across one loose in the object database can tell what wrote it and why.
const STAGED_COMMIT_SUBJECT: &str = "nao: the staged tree";

/// Name the git index as a commit, so the staged tree can be checked out like
/// any other ref. `None` when nothing is staged.
///
/// The index is the one tree in a repository that no ref resolves to, and that
/// is the whole of why this function exists: with it, a staged comparison is
/// two refs and [`compute_diff`] needs no change at all — the same property
/// that made a stash comparable for free.
///
/// Nothing is copied and nothing is hashed. Everything staged is *already* in
/// the object database, because `git add` writes the blob at the moment it is
/// run; this only writes the tree and commit objects that point at them.
///
/// Two decisions are load-bearing:
///
/// The index is copied and `GIT_INDEX_FILE` aimed at the copy rather than
/// running `write-tree` in place. `git write-tree` updates the cache-tree
/// extension of whichever index it reads, so in place it would write to the
/// user's `.git/index`. The tree that comes out is identical either way — nao
/// only ever reads the repository it watches, and racing a concurrent
/// `git add` for the microsecond is not a trade worth making.
///
/// The commit is parented on `HEAD`, which is what makes `HEAD → this` the
/// staged change and nothing besides. It is also *unreferenced*: nothing points
/// at it, so `git worktree add --detach` resolves it but it is garbage the
/// moment the index moves. It must therefore be produced and consumed by one
/// call, never handed to a client to hold — the mirror of the `stash@{N}`
/// problem, where the label outlives the thing but stops meaning it.
pub fn staged_commit(repo_root: &Path) -> AnyhowResult<Option<String>> {
    let Some(head_tree) = git_lines(repo_root, &["rev-parse", "HEAD^{tree}"]).pop() else {
        anyhow::bail!("no HEAD to compare the index against — this repository has no commits");
    };

    // Unique per call, not per process. Two servers watching the same
    // repository must not share a scratch index — and neither must two calls
    // in one process, which is not hypothetical: a copy taken from another
    // repository lands as `invalid object` from `write-tree`, since the blobs
    // it names are in a different object database.
    static SCRATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "nao-staged-index-{}-{}",
        std::process::id(),
        SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let index = index_path(repo_root);
    std::fs::copy(&index, &scratch)
        .map_err(|e| anyhow::anyhow!("could not read the git index at {:?}: {}", index, e))?;
    let tree = Command::new("git")
        .args(["write-tree"])
        .env("GIT_INDEX_FILE", &scratch)
        .current_dir(repo_root)
        .output()?;
    let _ = std::fs::remove_file(&scratch);
    if !tree.status.success() {
        anyhow::bail!(
            "could not read the staged tree: {}",
            String::from_utf8_lossy(&tree.stderr).trim()
        );
    }
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();

    // Nothing staged is the ordinary state of a repository, and the index tree
    // being HEAD's tree is exactly what that looks like. Answered here rather
    // than left to the caller to derive: this is the only place that holds both
    // trees, and an empty diff read as a comparison says "nothing changed"
    // about the wrong thing.
    if tree == head_tree {
        return Ok(None);
    }

    let commit = Command::new("git")
        .args([
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            STAGED_COMMIT_SUBJECT,
        ])
        .current_dir(repo_root)
        .output()?;
    if !commit.status.success() {
        anyhow::bail!(
            "could not name the staged tree: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        );
    }
    Ok(Some(
        String::from_utf8_lossy(&commit.stdout).trim().to_string(),
    ))
}

/// Where this repository keeps its index.
///
/// Asked of git rather than assumed to be `.git/index`: in a linked worktree
/// `.git` is a *file* and the index lives under the common directory's
/// `worktrees/<name>/`, so the guess would read another checkout's staged
/// state or nothing at all. `--git-path` answers relative to the cwd git ran
/// in, which is `repo_root`.
fn index_path(repo_root: &Path) -> std::path::PathBuf {
    let rel = git_lines(repo_root, &["rev-parse", "--git-path", "index"])
        .pop()
        .unwrap_or_else(|| ".git/index".to_string());
    let path = std::path::PathBuf::from(&rel);
    if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    }
}

/// The non-empty output lines of a git command run in `repo_root`.
///
/// Empty when git cannot answer at all — not a repository, no git on PATH, a
/// ref that stopped resolving. Every caller here treats git's answer as
/// advisory, so "it said nothing" and "it could not be asked" collapse into
/// the same, safe result.
pub(crate) fn git_lines(repo_root: &Path, args: &[&str]) -> Vec<String> {
    let Ok(out) = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Paths changed in the working tree relative to `base_ref`: tracked
/// edits (`git diff --name-only <ref>`) plus untracked, non-ignored
/// files (`git ls-files --others --exclude-standard`). Returned
/// repo-root-relative with `/` separators, sorted and de-duplicated.
/// Best-effort — an empty list on any git failure (callers treat the
/// changed set as advisory, never as a gate).
pub fn changed_files(repo_root: &Path, base_ref: &str) -> Vec<String> {
    let mut files: std::collections::BTreeSet<String> =
        git_lines(repo_root, &["diff", "--name-only", base_ref])
            .into_iter()
            .collect();
    files.extend(git_lines(
        repo_root,
        &["ls-files", "--others", "--exclude-standard"],
    ));
    files.into_iter().collect()
}

/// Which of `paths` git currently ignores, as the same strings passed in.
///
/// Asked of git rather than re-derived from a config, so the answer follows
/// `.gitignore`, `.git/info/exclude` and the global excludes *as they are
/// now* — a matcher built once at startup goes stale the moment someone edits
/// an ignore file, and a stale matcher that wrongly calls a file ignored is
/// how a watcher silently stops updating.
///
/// Empty when git cannot answer, which reads every path as un-ignored: more
/// work than necessary, never less than correct.
///
/// Note that git reports a *tracked* file as un-ignored even when a
/// `.gitignore` pattern matches it, where the walk's `ignore` crate would
/// drop it. That asymmetry only ever errs toward doing the work.
pub fn ignored_paths(repo_root: &Path, paths: &[String]) -> HashSet<String> {
    if paths.is_empty() {
        return HashSet::new();
    }
    let mut args = vec!["check-ignore", "--"];
    args.extend(paths.iter().map(String::as_str));
    git_lines(repo_root, &args).into_iter().collect()
}

/// Create a detached git worktree at `dir` for the given ref. Removes any
/// stale worktree at the same path first.
///
/// The checkout carries committed state alone, so the working tree's *local*
/// ignore rules do not reach it — see [`mirror_local_ignores`], which is
/// called here rather than at each of the four call sites so that no worktree
/// nao creates can be analyzed under a different ignore ruleset than the tree
/// it will be compared against.
pub fn create_worktree(repo_root: &Path, dir: &Path, git_ref: &str) -> AnyhowResult<()> {
    let dir_str = dir.to_str().unwrap();
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", dir_str])
        .current_dir(repo_root)
        .output();
    let out = Command::new("git")
        .args(["worktree", "add", "--detach", dir_str, git_ref])
        .current_dir(repo_root)
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "Failed to create worktree: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    restore_stashed_untracked(repo_root, dir, git_ref);
    mirror_local_ignores(repo_root, dir);
    Ok(())
}

/// Git's own subject for the commit a `git stash -u` puts untracked files in.
/// Matching on it is what separates a stash from an ordinary three-parent
/// octopus merge, which must not be overlaid.
const UNTRACKED_STASH_SUBJECT: &str = "untracked files on ";

/// Overlay a stash's untracked files onto a checkout of it.
///
/// `git stash --include-untracked` does not put those files in the stash
/// commit's own tree; it puts them in a *third parent*. So a
/// `git worktree add --detach <dir> stash@{0}` reproduces the stash minus
/// exactly the files a reader is most likely looking for — the new ones. A
/// stashed module simply would not exist on the canvas, and the diff would
/// report it as absent rather than as added (UI-107).
///
/// Best-effort in both directions. `git stash -u` writes a third parent even
/// when nothing untracked was there to save, so a checkout that matches
/// nothing is the ordinary case and not a failure; and a diff is still worth
/// computing without the overlay, so no outcome here fails the worktree.
///
/// A no-op for every ref that is not a stash — the third parent has to exist
/// *and* carry git's untracked-stash subject.
fn restore_stashed_untracked(repo_root: &Path, dir: &Path, git_ref: &str) {
    let Some(sha) = stashed_untracked_tree(repo_root, git_ref) else {
        return;
    };
    // Run inside the new worktree: it shares the object database, so the
    // stash's untracked tree resolves there, and `-- .` lands the files at
    // the checkout's root without disturbing what is already there.
    let _ = Command::new("git")
        .args(["checkout", &sha, "--", "."])
        .current_dir(dir)
        .output();
}

/// The commit holding `git_ref`'s stashed untracked files, if it is a stash
/// that has one.
///
/// Both halves of the test matter. A third parent alone is an ordinary
/// octopus merge, whose sides are already merged into the commit's own tree —
/// overlaying one would write files into a checkout that never held them. The
/// subject is git's own marker for the other shape, and it is the only thing
/// that tells the two apart from the outside.
fn stashed_untracked_tree(repo_root: &Path, git_ref: &str) -> Option<String> {
    let third = format!("{}^3", git_ref);
    let sha = git_lines(repo_root, &["rev-parse", "--verify", "--quiet", &third]).pop()?;
    let subject = git_lines(repo_root, &["log", "-1", "--format=%s", &sha]).pop()?;
    subject.starts_with(UNTRACKED_STASH_SUBJECT).then_some(sha)
}

/// The ignore files whose absence from a checkout changes what a walk sees.
///
/// `.git/info/exclude` is deliberately absent: a linked worktree resolves it
/// through `commondir`, so both sides already read the same file and copying
/// it would be a no-op at best.
const LOCAL_IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];

/// Copy the working tree's uncommitted ignore files into a fresh worktree, so
/// an analysis there applies the same ignore rules as one of the live tree.
///
/// A `git worktree add` reproduces committed state, which leaves out exactly
/// the ignore files a person keeps to themselves: one never committed, or one
/// committed but since edited. Analyze the two sides under two rulesets and
/// the diff reports the difference between the *rulesets* as a change to the
/// code — a file that only the head ignores reads as removed, on a tree where
/// nothing was removed.
///
/// Best-effort, and silent about failure: not copying leaves the checkout as
/// it was before this existed, which is a worse diff, not a broken one. It is
/// not silent about success — a reader comparing two trees deserves to know
/// that their uncommitted ignore rules were applied to both.
///
/// Returns how many files were copied.
pub fn mirror_local_ignores(repo_root: &Path, dir: &Path) -> usize {
    let copied = local_ignore_files(repo_root)
        .iter()
        .filter(|rel| copy_into(repo_root, dir, rel))
        .count();
    if copied > 0 {
        eprintln!(
            "    Applied {} local ignore file(s) to the checkout",
            copied
        );
    }
    copied
}

/// Copy one repo-relative file between two trees, creating the directories it
/// needs. False if any step fails — the caller counts successes and no single
/// ignore file is worth failing a diff over.
fn copy_into(from_root: &Path, to_root: &Path, rel: &str) -> bool {
    let to = to_root.join(rel);
    to.parent()
        .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
        && std::fs::copy(from_root.join(rel), &to).is_ok()
}

/// The repo-relative ignore-file paths that a checkout will not reproduce:
/// untracked ones, and tracked ones with uncommitted edits.
fn local_ignore_files(repo_root: &Path) -> Vec<String> {
    let mut paths: std::collections::BTreeSet<String> =
        git_lines(repo_root, &["ls-files", "--others", "--exclude-standard"])
            .into_iter()
            .collect();
    paths.extend(git_lines(repo_root, &["diff", "--name-only"]));
    paths
        .into_iter()
        .filter(|p| names_ignore_file(Path::new(p)))
        .collect()
}

/// Whether a path names an ignore file, at any depth. Public because the
/// file watcher needs it too: an ignore file is not itself analyzed, but
/// editing one changes which files are, so it has to be a reason to
/// re-analyze.
pub fn names_ignore_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| LOCAL_IGNORE_FILES.contains(&name.to_string_lossy().as_ref()))
}

/// A fingerprint of the ignore rules a walk would apply *beyond* what any
/// checkout of this repo carries: the uncommitted ignore files, plus the
/// repo-local `.git/info/exclude`.
///
/// Part of the key a cached base analysis is stored under, because editing an
/// ignore file changes what an analysis contains while leaving the base ref
/// alone. Without this, the first diff after such an edit re-analyzes the
/// head under the new rules and serves the base from a cache built under the
/// old ones — which is the very skew the mirroring exists to prevent, arriving
/// through the cache instead.
///
/// Not covered: the *global* gitignore, which no repo-local edit touches and
/// which changing mid-session is rare enough to leave to a server restart.
pub fn ignore_fingerprint(repo_root: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for rel in local_ignore_files(repo_root)
        .iter()
        .map(String::as_str)
        .chain([".git/info/exclude"])
    {
        rel.hash(&mut hasher);
        std::fs::read_to_string(repo_root.join(rel))
            .unwrap_or_default()
            .hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

/// The whole of what a cached analysis has to be keyed on besides its git
/// ref: the scope the config asks for, and the ignore rules in force.
pub fn analysis_fingerprint(config: &Config, repo_root: &Path) -> String {
    format!(
        "{}|{}",
        scope_fingerprint(config),
        ignore_fingerprint(repo_root)
    )
}

/// Remove a git worktree (best-effort, ignores errors).
pub fn remove_worktree(repo_root: &Path, dir: &Path) {
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", dir.to_str().unwrap()])
        .current_dir(repo_root)
        .output();
}

/// Build a Config for analyzing a directory.
///
/// The command line first, `root`'s settings file last, which is the order
/// [`apply_to_config`](crate::settings::Settings::apply_to_config) is built
/// for: it extends the pattern lists and fills only the scalars nothing has
/// chosen, so an `--include-tests` or `--language` typed on the command line
/// still outranks the file (ADR-0008).
///
/// Reading the file here rather than at each call site is what CFG-011 was:
/// this was the one config builder that never called `settings::load`, and
/// `nao mcp` is its only entry point — so an agent, the caller with no flags
/// to pass and therefore the one most dependent on the file, got the single
/// code path where the file was inert. `exclude_patterns` named in a repo's
/// `.nao/settings.json` were honoured by every CLI command and by nothing
/// served over MCP.
///
/// The file is read from `root`'s *checkout* rather than from `root` itself
/// (CFG-012), so a subdirectory gets the same settings the whole repo would —
/// which is most of what makes CFG-011 worth having, since `map` and
/// `reshape` are routinely called on a subfolder. What a subdirectory call
/// does not preserve is the entity keys a diff matches on: both sides of a
/// diff still take one config and [`rooted_at`] it, never a second call.
pub fn build_analysis_config(
    root: &Path,
    include_tests: bool,
    languages: &Option<Vec<String>>,
) -> Config {
    let mut config = Config::for_path(root).with_output_format(OutputFormat::Json);
    config.analysis.include_tests = include_tests;
    if let Some(langs) = languages {
        for lang in langs {
            if let Some(language) = Language::from_name(lang) {
                config.analysis.languages.insert(language);
            }
        }
    }
    crate::settings::load(root).apply_to_config(&mut config);
    config
}

/// An existing config pointed at a different directory.
///
/// Everything about *what* to analyze is inherited; `root_path` alone is
/// replaced, and only it, because that field is what `strip_prefix` removes
/// from every `file_path` to form the entity keys a diff matches on — inherit
/// it and every entity reads as added-and-removed (SRV-019). The base-side
/// mirror of the server's `working_head_config`.
///
/// This is how the two sides of a diff are kept to one scope. Rebuilding a
/// config for the base instead of deriving it from the head's is what let
/// `include_docs`, `spec_dir` and every settings-file pattern apply to one
/// side only, so a clean tree reported changes that were really just the two
/// sides disagreeing about what to look at.
pub fn rooted_at(config: &Config, dir: &Path) -> Config {
    let mut rooted = config.clone();
    rooted.root_path = dir.to_path_buf();
    rooted
}

/// A string that changes whenever anything about what an analysis *includes*
/// changes, for keying cached analyses.
///
/// Both halves of a scope are covered, matching what the watcher republishes
/// on a live scope change. A cache keyed on the git ref alone will happily
/// serve a base analyzed under the scope in force ten seconds ago.
pub fn scope_fingerprint(config: &Config) -> String {
    serde_json::to_string(&(&config.analysis, &config.filters))
        .unwrap_or_else(|_| format!("{:?}{:?}", config.analysis, config.filters))
}

/// Analyze `config.root_path` with a config the caller has already settled.
pub fn analyze_with(config: Config, label: &str) -> AnyhowResult<(DependencyGraph, Config)> {
    eprintln!("  Analyzing {} ...", label);
    let mut analyzer = Analyzer::new(config.clone());
    let result = analyzer.analyze()?;
    let graph = DependencyGraph::from_analysis(&result);
    eprintln!(
        "    {} entities, {} relationships",
        result.entities.len(),
        result.relationships.len()
    );
    Ok((graph, config))
}

/// Analyze code at `root_dir`, under `root_dir`'s own settings, and return
/// graph + config.
///
/// For a directory that stands alone. **Not for the base side of a diff:**
/// since [`build_analysis_config`] reads `.nao/settings.json` from the root
/// it is handed, calling this once per worktree gives each side the settings
/// committed at its own ref, and a scope the two sides disagree about reads
/// as every excluded file being added or removed. Both sides of a diff take
/// one config from the repo root and [`rooted_at`] it into each checkout,
/// then go through [`analyze_with`].
pub fn analyze_at(
    root_dir: &Path,
    include_tests: bool,
    languages: &Option<Vec<String>>,
    label: &str,
) -> AnyhowResult<(DependencyGraph, Config)> {
    analyze_with(
        build_analysis_config(root_dir, include_tests, languages),
        label,
    )
}

/// Render the before-side detail sidecar: per-entity source, and per-file
/// source read from disk.
///
/// **Call this while the base checkout still exists.** File entries are the
/// only part of a graph that is not self-contained — `render_details` reads
/// each file's text off disk at render time, so rendering against a base
/// whose worktree has been removed silently drops every file's source and
/// keeps only the files that happen to carry a doc comment. That is why the
/// server renders this once, next to the analysis, and carries the string
/// rather than the graph it came from.
pub fn render_base_details(
    base_graph: &DependencyGraph,
    base_config: &Config,
) -> AnyhowResult<String> {
    JsonRenderer::render_details(base_graph, base_config)
}

/// Write all diff-related output files to `output_dir`.
///
/// `base_details` is passed in rather than rendered here — see
/// [`render_base_details`] for why it has to be produced earlier.
pub fn write_diff_outputs(
    output_dir: &Path,
    head_graph: &DependencyGraph,
    head_config: &Config,
    base_details: &str,
    diff: &DiffResult,
) -> AnyhowResult<String> {
    let data_path = output_dir.join("data.json");
    std::fs::create_dir_all(output_dir)?;

    let head_json = output::render(head_graph, head_config)?;
    std::fs::write(&data_path, &head_json)?;

    let details_str = JsonRenderer::render_details(head_graph, head_config)?;
    std::fs::write(data_path.with_extension("details.json"), &details_str)?;

    let index_str = JsonRenderer::render_index(head_graph, head_config)?;
    std::fs::write(data_path.with_extension("index.json"), &index_str)?;

    std::fs::write(output_dir.join("data.base-details.json"), base_details)?;

    let diff_json = serde_json::to_string(diff)?;
    std::fs::write(output_dir.join("diff.json"), &diff_json)?;

    Ok(diff_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{Parameter, Relationship, Span};

    fn entity(file: &str, line: usize, name: &str, kind: EntityKind) -> CodeEntity {
        CodeEntity::new(name, kind, file, Span::from_positions(line, 0, line, 0))
    }

    /// A graph from synthetic entities plus `(source, target, kind)` edges
    /// given as indices into `entities`.
    fn graph_of(
        entities: Vec<CodeEntity>,
        edges: &[(usize, usize, RelationshipKind)],
    ) -> DependencyGraph {
        let relationships = edges
            .iter()
            .map(|&(s, t, kind)| Relationship::new(&entities[s].id, &entities[t].id, kind))
            .collect();
        DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn diff_of(base: &DependencyGraph, head: &DependencyGraph) -> DiffResult {
        let root = std::path::Path::new("/repo");
        compute_diff(base, head, root, root, "base", "head")
    }

    fn row<'a>(d: &'a DiffResult, name: &str) -> &'a EntityDiff {
        d.entities
            .iter()
            .find(|e| e.name == name)
            .expect("entity in diff")
    }

    fn fns(names: &[&str]) -> Vec<CodeEntity> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| entity("/repo/src/a.rs", i * 10 + 1, n, EntityKind::Function))
            .collect()
    }

    /// A unique-per-test checkout to stand in for a base worktree.
    ///
    /// The directory name deliberately carries no "test" in it: the walker
    /// honours `include_tests: false` by path, so a fixture under a `…-test-…`
    /// directory is skipped entirely and the analysis comes back empty.
    fn temp_checkout(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("nao-diff-details-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "//! A tiny crate.\npub fn hello() -> u32 { 41 + 1 }\n",
        )
        .unwrap();
        root
    }

    /// A repo with one commit, for the worktree tests. Same naming rule as
    /// `temp_checkout`: no "test" in the directory name.
    fn git_repo(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("nao-diff-repo-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn hello() -> u32 { 1 }\n").unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
        };
        git(&["init", "-q", "."]);
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.email=nao@example.com",
            "-c",
            "user.name=Nao",
            "commit",
            "-qm",
            "init",
        ]);
        root
    }

    /// A root carrying a repo-scope settings file, for the CFG-011 tests.
    /// Same naming rule as `temp_checkout`: no "test" in the directory name.
    fn root_with_settings(tag: &str, json: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("nao-diff-cfg-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".nao")).unwrap();
        std::fs::write(root.join(".nao/settings.json"), json).unwrap();
        root
    }

    /// CFG-011. Every other entry point settles its config through
    /// `settings::load`; this builder did not, and `nao mcp` is its only
    /// caller — so the audience with no flags to pass got the one code path
    /// where the repo's file was inert.
    #[test]
    fn a_built_config_carries_the_repo_settings_file() {
        let root = root_with_settings(
            "excludes",
            r#"{"exclude_patterns": ["**/generated/**"], "min_weight": 4}"#,
        );
        let config = build_analysis_config(&root, false, &None);
        assert!(
            config
                .analysis
                .exclude_patterns
                .contains(&"**/generated/**".to_string()),
            "the file's excludes never reached the config: {:?}",
            config.analysis.exclude_patterns
        );
        assert_eq!(config.filters.min_weight, 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The order the file is applied in, not just the fact of it: a
    /// `--language` on the `nao mcp` command line is a narrowing the caller
    /// typed, and the file underneath it must not widen it back.
    #[test]
    fn a_language_on_the_command_line_outranks_the_file() {
        let root = root_with_settings("language", r#"{"language": ["python"]}"#);
        let config = build_analysis_config(&root, false, &Some(vec!["rust".to_string()]));
        assert!(config.analysis.languages.contains(&Language::Rust));
        assert!(
            !config.analysis.languages.contains(&Language::Python),
            "the file widened a filter the command line had narrowed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The scope a diff is computed under has to reach both sides. Only the
    /// root may differ, because it is what entity keys are stripped against —
    /// inherit it and every entity reads as added-and-removed (SRV-019).
    #[test]
    fn a_re_rooted_config_keeps_everything_but_the_root() {
        let mut live = Config::for_path("/repo");
        live.analysis.include_docs = true;
        live.analysis.include_tests = true;
        live.analysis.spec_dir = Some(std::path::PathBuf::from("spec"));
        live.analysis
            .exclude_patterns
            .push("**/generated/**".to_string());

        let base = rooted_at(&live, Path::new("/tmp/nao-diff-base-abc"));

        assert_eq!(
            base.root_path,
            std::path::PathBuf::from("/tmp/nao-diff-base-abc")
        );
        assert!(
            base.analysis.include_docs,
            "docs were on for the head and must be on for the base"
        );
        assert!(base.analysis.include_tests);
        assert_eq!(base.analysis.spec_dir, live.analysis.spec_dir);
        assert!(base
            .analysis
            .exclude_patterns
            .contains(&"**/generated/**".to_string()));
    }

    /// A relative `spec_dir` has to follow the config into the checkout, or
    /// the base looks for the spec in the *head's* tree and the whole spec
    /// layer reads as added.
    #[test]
    fn a_relative_spec_dir_follows_the_re_rooted_config() {
        let mut live = Config::for_path("/repo");
        live.analysis.spec_dir = Some(std::path::PathBuf::from("docs/domain"));
        let base = rooted_at(&live, Path::new("/tmp/nao-diff-base-abc"));
        assert_eq!(
            base.spec_root(),
            Some(std::path::PathBuf::from(
                "/tmp/nao-diff-base-abc/docs/domain"
            )),
        );
    }

    /// The cache key half of the same fix. Narrowing the scope leaves the git
    /// ref alone, so a base cached under the old scope is otherwise served to
    /// a diff computed under the new one.
    #[test]
    fn narrowing_the_scope_changes_the_fingerprint() {
        let live = Config::for_path("/repo");
        let same = rooted_at(&live, Path::new("/elsewhere"));
        assert_eq!(
            scope_fingerprint(&live),
            scope_fingerprint(&same),
            "the root is not part of the scope — re-rooting alone must not evict a cached base",
        );

        for narrower in [
            |c: &mut Config| c.analysis.include_docs = true,
            |c: &mut Config| c.analysis.include_tests = true,
            |c: &mut Config| c.analysis.exclude_patterns.push("**/gen/**".to_string()),
            |c: &mut Config| {
                c.analysis.languages.insert(Language::Rust);
            },
            |c: &mut Config| c.filters.min_weight = 3,
        ] {
            let mut changed = live.clone();
            narrower(&mut changed);
            assert_ne!(
                scope_fingerprint(&live),
                scope_fingerprint(&changed),
                "a scope change that a cached base predates has to evict it",
            );
        }
    }

    #[test]
    fn an_ignore_file_is_recognised_at_any_depth() {
        for yes in [
            ".gitignore",
            "src/.gitignore",
            "a/b/c/.ignore",
            "/abs/path/.ignore",
        ] {
            assert!(
                names_ignore_file(Path::new(yes)),
                "{yes} names an ignore file"
            );
        }
        for no in [
            "src/lib.rs",
            "gitignore",
            "src/.gitignore.bak",
            ".gitattributes",
            "",
        ] {
            assert!(
                !names_ignore_file(Path::new(no)),
                "{no} does not name an ignore file"
            );
        }
    }

    /// Editing an ignore file changes what an analysis contains without
    /// touching the git ref it was taken at, so a base cached under the old
    /// rules would otherwise be served to a head re-analyzed under the new
    /// ones — the mirroring undone by the cache in front of it.
    #[test]
    fn editing_an_ignore_file_evicts_the_cached_base() {
        let repo = git_repo("ignore-fingerprint");
        let config = Config::for_path(&repo);
        let before = analysis_fingerprint(&config, &repo);

        std::fs::write(repo.join(".gitignore"), "gen/\n").unwrap();
        let with_ignore = analysis_fingerprint(&config, &repo);
        assert_ne!(
            before, with_ignore,
            "a new ignore file changes what is analyzed"
        );

        std::fs::write(repo.join(".gitignore"), "gen/\nvendor/\n").unwrap();
        assert_ne!(
            with_ignore,
            analysis_fingerprint(&config, &repo),
            "so does editing one"
        );

        std::fs::write(repo.join(".git/info/exclude"), "build/\n").unwrap();
        let with_exclude = analysis_fingerprint(&config, &repo);
        assert_ne!(
            with_ignore, with_exclude,
            "and so does the repo-local exclude file"
        );

        // Stable when nothing moved — or every save pays for a base checkout.
        assert_eq!(with_exclude, analysis_fingerprint(&config, &repo));

        let _ = std::fs::remove_dir_all(&repo);
    }

    /// The fix for the asymmetry a reader sees as phantom removals: an ignore
    /// file that was never committed is invisible to `git worktree add`, so
    /// the base gets analyzed under the repo's committed ignore rules while
    /// the head is analyzed under the author's local ones. Every file only
    /// the head ignores then reads as removed from a tree where nothing was.
    #[test]
    fn a_worktree_inherits_the_working_trees_uncommitted_ignore_files() {
        let repo = git_repo("uncommitted-ignore");
        std::fs::write(repo.join(".gitignore"), "gen/\n").unwrap();
        // Nested, and in a directory the commit does not contain at all —
        // the copy has to create the path on the way.
        std::fs::create_dir_all(repo.join("src/vendor")).unwrap();
        std::fs::write(repo.join("src/vendor/.ignore"), "*.min.js\n").unwrap();

        let work =
            std::env::temp_dir().join(format!("nao-diff-wt-{}-uncommitted", std::process::id()));
        create_worktree(&repo, &work, "HEAD").unwrap();

        assert_eq!(
            std::fs::read_to_string(work.join(".gitignore")).unwrap(),
            "gen/\n"
        );
        assert_eq!(
            std::fs::read_to_string(work.join("src/vendor/.ignore")).unwrap(),
            "*.min.js\n",
        );

        remove_worktree(&repo, &work);
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// The milder half of the same problem: an ignore file that *is*
    /// committed but has been edited since. The checkout reproduces the
    /// committed text, which is not the ruleset the head is being read under.
    #[test]
    fn a_worktree_takes_the_edited_ignore_file_over_the_committed_one() {
        let repo = git_repo("edited-ignore");
        std::fs::write(repo.join(".gitignore"), "committed/\n").unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
        };
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.email=nao@example.com",
            "-c",
            "user.name=Nao",
            "commit",
            "-qm",
            "ignore",
        ]);
        std::fs::write(repo.join(".gitignore"), "edited/\n").unwrap();

        let work = std::env::temp_dir().join(format!("nao-diff-wt-{}-edited", std::process::id()));
        create_worktree(&repo, &work, "HEAD").unwrap();

        assert_eq!(
            std::fs::read_to_string(work.join(".gitignore")).unwrap(),
            "edited/\n"
        );

        remove_worktree(&repo, &work);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_files_before_source_has_to_be_rendered_while_its_checkout_exists() {
        // The whole reason `render_base_details` is a separate step the
        // server calls next to the analysis. A graph is self-contained, but
        // the FILE half of the detail sidecar is read off disk at render
        // time — so rendering against a base whose worktree has been removed
        // (which is every refresh after the first, since the base analysis is
        // cached and its checkout is not) quietly produces a sidecar with no
        // file sources in it, and the details pane has nothing to diff.
        let root = temp_checkout("before-source");
        let (graph, config) = analyze_at(&root, false, &None, "test base").unwrap();

        let with_checkout: serde_json::Value =
            serde_json::from_str(&render_base_details(&graph, &config).unwrap()).unwrap();
        let file_source = with_checkout
            .get("src/lib.rs")
            .and_then(|e| e.get("source_code"))
            .and_then(|s| s.as_str());
        assert!(
            file_source.is_some_and(|s| s.contains("41 + 1")),
            "the file's own text belongs in the before-side sidecar"
        );

        std::fs::remove_dir_all(&root).unwrap();
        let without_checkout: serde_json::Value =
            serde_json::from_str(&render_base_details(&graph, &config).unwrap()).unwrap();
        assert!(
            without_checkout
                .get("src/lib.rs")
                .and_then(|e| e.get("source_code"))
                .is_none(),
            "same graph, no checkout, no file source — so the render cannot be deferred"
        );
    }

    #[test]
    fn a_swapped_call_is_a_change_even_though_every_metric_holds() {
        // `caller` drops its call to `old` and calls `new` instead. Same
        // fan_out, same source hash (both are None here), same everything the
        // metric comparison looks at — the edges are the only witness.
        let base = graph_of(
            fns(&["caller", "old", "new"]),
            &[(0, 1, RelationshipKind::Calls)],
        );
        let head = graph_of(
            fns(&["caller", "old", "new"]),
            &[(0, 2, RelationshipKind::Calls)],
        );
        let d = diff_of(&base, &head);

        let caller = row(&d, "caller");
        assert_eq!(caller.status, ChangeStatus::Modified);
        assert!(
            !caller.source_changed,
            "rewiring is an impact, not a core change"
        );
        assert!(caller.metric_deltas.is_empty(), "no metric moved");

        let out: Vec<_> = caller
            .rel_deltas
            .iter()
            .filter(|r| r.direction == RelDirection::Outgoing)
            .map(|r| (r.status, r.other_name.as_str(), r.label.as_str()))
            .collect();
        assert_eq!(
            out,
            vec![
                (ChangeStatus::Added, "new", "calls"),
                (ChangeStatus::Removed, "old", "calls"),
            ]
        );

        assert_eq!(d.summary.relationships_added, 1);
        assert_eq!(d.summary.relationships_removed, 1);
    }

    #[test]
    fn the_callee_learns_who_stopped_calling_it() {
        let base = graph_of(
            fns(&["caller", "old", "new"]),
            &[(0, 1, RelationshipKind::Calls)],
        );
        let head = graph_of(
            fns(&["caller", "old", "new"]),
            &[(0, 2, RelationshipKind::Calls)],
        );
        let d = diff_of(&base, &head);

        let old = row(&d, "old");
        assert_eq!(old.rel_deltas.len(), 1);
        assert_eq!(old.rel_deltas[0].status, ChangeStatus::Removed);
        assert_eq!(old.rel_deltas[0].direction, RelDirection::Incoming);
        assert_eq!(old.rel_deltas[0].label, "called by");
        assert_eq!(old.rel_deltas[0].other_name, "caller");

        let new = row(&d, "new");
        assert_eq!(new.rel_deltas[0].status, ChangeStatus::Added);
        assert_eq!(new.rel_deltas[0].direction, RelDirection::Incoming);
    }

    #[test]
    fn an_added_entity_carries_no_edge_list_of_its_own() {
        // Every edge a new function has is new. Listing them under it would
        // bury the one delta that carries information: that `caller` reaches
        // it now.
        let base = graph_of(fns(&["caller"]), &[]);
        let head = graph_of(
            fns(&["caller", "helper"]),
            &[(0, 1, RelationshipKind::Calls)],
        );
        let d = diff_of(&base, &head);

        let helper = row(&d, "helper");
        assert_eq!(helper.status, ChangeStatus::Added);
        assert!(helper.rel_deltas.is_empty());

        let caller = row(&d, "caller");
        assert_eq!(caller.rel_deltas.len(), 1);
        assert_eq!(caller.rel_deltas[0].other_name, "helper");
        assert_eq!(
            caller.rel_deltas[0].other_entity_id.as_deref(),
            Some("/repo/src/a.rs:11:helper"),
            "the far end is navigable while it exists in head"
        );
        // The edge is counted once, on the end that reports it.
        assert_eq!(d.summary.relationships_added, 1);
    }

    #[test]
    fn an_edge_whose_target_moved_down_the_file_is_the_same_edge() {
        let base = graph_of(
            vec![
                entity("/repo/src/a.rs", 1, "caller", EntityKind::Function),
                entity("/repo/src/a.rs", 20, "callee", EntityKind::Function),
            ],
            &[(0, 1, RelationshipKind::Calls)],
        );
        let head = graph_of(
            vec![
                entity("/repo/src/a.rs", 1, "caller", EntityKind::Function),
                entity("/repo/src/a.rs", 44, "callee", EntityKind::Function),
            ],
            &[(0, 1, RelationshipKind::Calls)],
        );
        let d = diff_of(&base, &head);

        assert!(row(&d, "caller").rel_deltas.is_empty());
        assert_eq!(d.summary.relationships_added, 0);
        assert_eq!(d.summary.relationships_removed, 0);
    }

    #[test]
    fn a_dead_callee_still_names_itself_on_the_caller_that_lost_it() {
        let base = graph_of(fns(&["caller", "gone"]), &[(0, 1, RelationshipKind::Calls)]);
        let head = graph_of(fns(&["caller"]), &[]);
        let d = diff_of(&base, &head);

        let caller = row(&d, "caller");
        assert_eq!(caller.rel_deltas.len(), 1);
        let lost = &caller.rel_deltas[0];
        assert_eq!(lost.status, ChangeStatus::Removed);
        assert_eq!(lost.other_name, "gone");
        assert_eq!(lost.other_file, "src/a.rs", "reported repo-relative");
        assert!(lost.other_entity_id.is_none(), "nothing to navigate to");
    }

    // ── UI-087: overloads ─────────────────────────────────────────────────

    /// A method with declared parameter types. Both `parameters` and
    /// `metrics.param_count` are set because the parsers populate both and
    /// matching reads each for a different purpose — the count keys the group,
    /// the types pick a member out of it.
    fn overload(line: usize, name: &str, params: &[&str]) -> CodeEntity {
        let mut e = entity("/repo/src/a.rs", line, name, EntityKind::Method);
        e.parameters = params
            .iter()
            .enumerate()
            .map(|(i, t)| Parameter {
                name: format!("p{i}"),
                type_name: Some((*t).to_string()),
                default_value: None,
                visibility: None,
            })
            .collect();
        e.metrics.param_count = Some(params.len() as u32);
        e.source_code = Some(format!("{name}({})", params.join(", ")));
        e
    }

    fn rows<'a>(d: &'a DiffResult, name: &str) -> Vec<&'a EntityDiff> {
        d.entities.iter().filter(|e| e.name == name).collect()
    }

    /// The bug UI-087 exists for, in miniature: three overloads, nothing
    /// edited. Before the fix the base index kept only the last of them, the
    /// other two matched against it, and their parameter lists read as edits —
    /// which is how a 7-file change drew 80 files.
    #[test]
    fn overloads_of_one_name_do_not_report_each_other_as_edits() {
        let group = || {
            vec![
                overload(10, "setState", &["Product"]),
                overload(20, "setState", &["Product", "Status"]),
                overload(30, "setState", &["Product", "Status", "User"]),
            ]
        };
        let d = diff_of(&graph_of(group(), &[]), &graph_of(group(), &[]));

        let set_state = rows(&d, "setState");
        assert_eq!(set_state.len(), 3, "three overloads, three rows");
        for r in &set_state {
            assert_eq!(r.status, ChangeStatus::Unchanged, "{:?} moved", r.entity_id);
        }
        assert_eq!(d.summary.modified, 0);
        assert_eq!(d.summary.added, 0);
        assert_eq!(d.summary.removed, 0);
        assert_eq!(
            d.summary.total_base, 3,
            "counts entities, not distinct keys"
        );
    }

    /// Same arity, so the key alone cannot separate them — the parameter
    /// types do. Reordering the declarations must not report both as edits.
    #[test]
    fn same_arity_overloads_match_by_parameter_type() {
        let base = graph_of(
            vec![
                overload(10, "get", &["Product"]),
                overload(20, "get", &["Variant"]),
            ],
            &[],
        );
        let head = graph_of(
            vec![
                overload(10, "get", &["Variant"]),
                overload(20, "get", &["Product"]),
            ],
            &[],
        );
        let d = diff_of(&base, &head);

        let get = rows(&d, "get");
        assert_eq!(get.len(), 2);
        for r in &get {
            assert_eq!(
                r.status,
                ChangeStatus::Unchanged,
                "swapped order is not an edit"
            );
        }
    }

    #[test]
    fn editing_one_overload_leaves_its_siblings_alone() {
        let base = graph_of(
            vec![
                overload(10, "set", &["Product"]),
                overload(20, "set", &["Product", "Status"]),
            ],
            &[],
        );
        let mut edited = vec![
            overload(10, "set", &["Product"]),
            overload(20, "set", &["Product", "Status"]),
        ];
        edited[1].source_code = Some("set(Product, Status) { /* rewritten */ }".into());
        let d = diff_of(&base, &graph_of(edited, &[]));

        let set = rows(&d, "set");
        let by_arity = |n: usize| {
            *set.iter()
                .find(|r| {
                    r.entity_id
                        .ends_with(&format!("{}:set", if n == 1 { 10 } else { 20 }))
                })
                .expect("row")
        };
        assert_eq!(
            by_arity(1).status,
            ChangeStatus::Unchanged,
            "untouched sibling"
        );
        assert_eq!(by_arity(2).status, ChangeStatus::Modified);
        assert!(by_arity(2).source_changed);
    }

    /// The key carries no line number, so an overload group that slid down the
    /// file is the same group — the property UI-086 protects, re-checked now
    /// that matching runs through `match_group`.
    #[test]
    fn an_overload_group_that_moved_down_the_file_is_unchanged() {
        let base = graph_of(
            vec![
                overload(10, "run", &["A"]),
                overload(20, "run", &["A", "B"]),
            ],
            &[],
        );
        let head = graph_of(
            vec![
                overload(80, "run", &["A"]),
                overload(90, "run", &["A", "B"]),
            ],
            &[],
        );
        let d = diff_of(&base, &head);

        for r in rows(&d, "run") {
            assert_eq!(r.status, ChangeStatus::Unchanged);
        }
    }

    /// Dropping one overload removes exactly that one. Before the fix the
    /// whole group shared a key, so the survivor claimed it and the removal
    /// went unreported.
    #[test]
    fn dropping_one_overload_removes_only_that_one() {
        let base = graph_of(
            vec![
                overload(10, "log", &["String"]),
                overload(20, "log", &["String", "Level"]),
            ],
            &[],
        );
        let head = graph_of(vec![overload(10, "log", &["String"])], &[]);
        let d = diff_of(&base, &head);

        let log = rows(&d, "log");
        assert_eq!(log.len(), 2);
        let statuses: Vec<_> = log.iter().map(|r| r.status).collect();
        assert!(statuses.contains(&ChangeStatus::Unchanged));
        assert!(statuses.contains(&ChangeStatus::Removed));
        assert_eq!(d.summary.removed, 1);
    }

    /// Arity in the key would otherwise turn "gained a parameter" into an
    /// addition plus an unrelated removal, losing the before/after on exactly
    /// the edit a reader opened the diff for. The second matching pass keeps
    /// it one row.
    #[test]
    fn a_method_that_gained_a_parameter_is_modified_not_replaced() {
        let base = graph_of(vec![overload(10, "save", &["Product"])], &[]);
        let head = graph_of(vec![overload(10, "save", &["Product", "User"])], &[]);
        let d = diff_of(&base, &head);

        let save = rows(&d, "save");
        assert_eq!(save.len(), 1, "one row, not an add plus a remove");
        assert_eq!(save[0].status, ChangeStatus::Modified);
        assert!(save[0].source_changed);
        assert!(
            save[0]
                .metric_deltas
                .iter()
                .any(|m| m.name == "param_count" && m.delta == 1.0),
            "the signature change is reported as the delta it is",
        );
        assert!(
            save[0].base_entity_id.is_some(),
            "the before-source is reachable"
        );
    }

    /// The second pass only sees entities that failed the first on *both*
    /// sides, so a group that merely exists can never reach it — which is what
    /// stops the fallback from resurrecting the phantom edits.
    #[test]
    fn the_loose_pass_does_not_rematch_an_intact_overload_group() {
        let group = || {
            vec![
                overload(10, "put", &["A"]),
                overload(20, "put", &["A", "B"]),
            ]
        };
        let mut head = group();
        head.push(overload(30, "put", &["A", "B", "C"]));
        let d = diff_of(&graph_of(group(), &[]), &graph_of(head, &[]));

        let put = rows(&d, "put");
        assert_eq!(put.len(), 3);
        let added: Vec<_> = put
            .iter()
            .filter(|r| r.status == ChangeStatus::Added)
            .collect();
        assert_eq!(added.len(), 1, "the new overload is an addition");
        assert_eq!(
            put.iter()
                .filter(|r| r.status == ChangeStatus::Unchanged)
                .count(),
            2,
            "its siblings are untouched, not dragged into a loose match",
        );
    }

    /// `EdgeKey` is built from `EntityKey`, so arity reaches the edge pass
    /// too: switching a call from `foo(a)` to `foo(a, b)` is a rewiring, not
    /// the same edge.
    #[test]
    fn a_call_that_switches_overload_is_a_changed_edge() {
        let target = |line, params: &[&str]| overload(line, "write", params);
        let caller = || entity("/repo/src/a.rs", 1, "caller", EntityKind::Function);
        let base = graph_of(
            vec![caller(), target(10, &["Buf"]), target(20, &["Buf", "Len"])],
            &[(0, 1, RelationshipKind::Calls)],
        );
        let head = graph_of(
            vec![caller(), target(10, &["Buf"]), target(20, &["Buf", "Len"])],
            &[(0, 2, RelationshipKind::Calls)],
        );
        let d = diff_of(&base, &head);

        assert_eq!(d.summary.relationships_added, 1);
        assert_eq!(d.summary.relationships_removed, 1);
        assert_eq!(row(&d, "caller").status, ChangeStatus::Modified);
    }

    // --------------------------------------------------------------
    //  Worktrees over real git repositories (UI-107)
    // --------------------------------------------------------------

    /// Run a git command in `dir`, asserting it succeeded — a test whose
    /// setup half-failed would otherwise assert against an unknown tree.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// A fresh repository with one commit, at a path unique to this test.
    fn repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nao-stash-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "--initial-branch=main", "."]);
        git(&dir, &["config", "user.email", "t@t.t"]);
        git(&dir, &["config", "user.name", "t"]);
        write(&dir, "tracked.txt", "one\n");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "base"]);
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The files `-u` saves live in the stash's *third parent*, not in its own
    /// tree, so a plain checkout of the stash is missing exactly the new code
    /// a reader opened the diff to look at.
    #[test]
    fn a_stash_checkout_carries_its_untracked_files() {
        let dir = repo("untracked");
        write(&dir, "tracked.txt", "two\n");
        write(&dir, "brand_new.rs", "fn added() {}\n");
        git(&dir, &["stash", "-q", "--include-untracked"]);

        let wt = dir.join("wt");
        create_worktree(&dir, &wt, "stash@{0}").unwrap();

        assert_eq!(
            std::fs::read_to_string(wt.join("tracked.txt")).unwrap(),
            "two\n"
        );
        assert!(
            wt.join("brand_new.rs").exists(),
            "an untracked file in the stash must reach the checkout"
        );
        remove_worktree(&dir, &wt);
        cleanup(&dir);
    }

    /// `git stash -u` writes a third parent even with nothing untracked to
    /// save, so the overlay runs against an empty tree on ordinary stashes and
    /// must not turn that into a failed worktree.
    #[test]
    fn a_stash_with_nothing_untracked_still_checks_out() {
        let dir = repo("empty-untracked");
        write(&dir, "tracked.txt", "two\n");
        git(&dir, &["stash", "-q", "--include-untracked"]);

        let wt = dir.join("wt");
        create_worktree(&dir, &wt, "stash@{0}").unwrap();

        assert_eq!(
            std::fs::read_to_string(wt.join("tracked.txt")).unwrap(),
            "two\n"
        );
        remove_worktree(&dir, &wt);
        cleanup(&dir);
    }

    /// A three-parent commit is not by itself a stash. An octopus merge's
    /// sides are already merged into its own tree, so overlaying one would
    /// write files into a checkout that never held them.
    ///
    /// Asserted on the decision rather than on the resulting files: the
    /// merge's tree contains every side already, so an overlay that wrongly
    /// ran would leave the checkout looking correct.
    #[test]
    fn an_octopus_merge_is_not_a_stash() {
        let dir = repo("octopus");
        let base = git(&dir, &["rev-parse", "HEAD"]);
        for branch in ["b1", "b2"] {
            git(&dir, &["checkout", "-q", "-b", branch, &base]);
            write(&dir, &format!("{}.txt", branch), "x\n");
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-qm", branch]);
        }
        // Move main off `base` first, or git fast-forwards to b1 and the
        // merge lands with two parents instead of three.
        git(&dir, &["checkout", "-q", "main"]);
        write(&dir, "main.txt", "x\n");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "main moves"]);
        git(&dir, &["merge", "-q", "--no-edit", "b1", "b2"]);

        let parents = git(&dir, &["rev-list", "--parents", "-n", "1", "HEAD"]);
        assert_eq!(
            parents.split(' ').count(),
            4,
            "three parents plus the commit itself"
        );
        assert!(
            stashed_untracked_tree(&dir, "HEAD").is_none(),
            "a third parent alone must not be read as stashed untracked files"
        );
        cleanup(&dir);
    }

    // --------------------------------------------------------------
    //  The index as a ref (UI-111)
    // --------------------------------------------------------------

    /// The point of the whole thing: what was `git add`ed reaches a checkout,
    /// and what was only saved to disk does not.
    #[test]
    fn a_staged_commit_checks_out_the_index_and_not_the_working_tree() {
        let dir = repo("staged");
        write(&dir, "tracked.txt", "staged\n");
        write(&dir, "added.rs", "fn staged() {}\n");
        git(&dir, &["add", "."]);
        // Both files move again *after* staging, and one more appears that was
        // never staged at all. None of it may reach the checkout.
        write(&dir, "tracked.txt", "working\n");
        write(&dir, "added.rs", "fn working() {}\n");
        write(&dir, "unstaged.rs", "fn never() {}\n");

        let sha = staged_commit(&dir).unwrap().expect("something is staged");
        let wt = dir.join("wt");
        create_worktree(&dir, &wt, &sha).unwrap();

        assert_eq!(
            std::fs::read_to_string(wt.join("tracked.txt")).unwrap(),
            "staged\n",
            "the checkout must be the index, not the working tree"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("added.rs")).unwrap(),
            "fn staged() {}\n"
        );
        assert!(
            !wt.join("unstaged.rs").exists(),
            "a file that was never staged is not part of the staged tree"
        );
        remove_worktree(&dir, &wt);
        cleanup(&dir);
    }

    /// `HEAD → staged` has to be the staged change alone, which is what the
    /// first parent buys. Paired against anything else, every commit in between
    /// would read as something the index did.
    #[test]
    fn a_staged_commit_is_parented_on_head() {
        let dir = repo("staged-parent");
        let head = git(&dir, &["rev-parse", "HEAD"]);
        write(&dir, "tracked.txt", "staged\n");
        git(&dir, &["add", "."]);

        let sha = staged_commit(&dir).unwrap().unwrap();
        assert_eq!(git(&dir, &["rev-parse", &format!("{}^1", sha)]), head);
        cleanup(&dir);
    }

    /// Nothing staged is the ordinary state of a repository. It has to be
    /// distinguishable from a comparison that ran and found nothing, or the
    /// reader is shown an empty overlay and told the code did not change.
    #[test]
    fn nothing_staged_is_no_commit_rather_than_an_empty_one() {
        let dir = repo("staged-empty");
        // A working-tree edit is not a staged one, so this must still be None.
        write(&dir, "tracked.txt", "working only\n");

        assert!(staged_commit(&dir).unwrap().is_none());
        cleanup(&dir);
    }

    /// nao reads the repository it watches and does not write to it. `git
    /// write-tree` updates the cache-tree extension of the index it is given,
    /// so it is given a copy — asserted on the bytes, because the tree that
    /// comes out is the same either way and would not show the difference.
    #[test]
    fn naming_the_index_does_not_write_to_it() {
        let dir = repo("staged-readonly");
        write(&dir, "tracked.txt", "staged\n");
        git(&dir, &["add", "."]);

        let index = index_path(&dir);
        let before = std::fs::read(&index).unwrap();
        staged_commit(&dir).unwrap().unwrap();
        assert_eq!(
            before,
            std::fs::read(&index).unwrap(),
            "the working repository's index must come out byte-identical"
        );
        cleanup(&dir);
    }

    #[test]
    fn a_stash_third_parent_is_recognised() {
        let dir = repo("recognised");
        write(&dir, "brand_new.rs", "fn added() {}\n");
        git(&dir, &["stash", "-q", "--include-untracked"]);

        assert!(stashed_untracked_tree(&dir, "stash@{0}").is_some());
        // The ordinary case: no third parent at all.
        assert!(stashed_untracked_tree(&dir, "HEAD").is_none());
        cleanup(&dir);
    }
}
