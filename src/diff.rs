//! Structural diff engine: compares two analysis results at the entity level.
//!
//! Matches entities by `(name, kind, file_path_relative)` so that line-number
//! shifts from unrelated edits don't create false "removed + added" pairs.
//! For each matched entity, compares source code hash and metrics to classify
//! it as unchanged or modified. Unmatched entities are added or removed.

use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::{CodeEntity, EntityKind, EntityMetrics};
use crate::models::file_info::Language;
use crate::output::{self, JsonRenderer, OutputFormat};

/// Stable key for matching entities across commits. Uses name + kind +
/// relative file path (not the raw entity ID, which includes line numbers
/// that shift on every edit).
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct EntityKey {
    name: String,
    kind: EntityKind,
    file_path: String,
    /// Parent name (for methods inside a class/struct — disambiguates
    /// `Foo.bar` from `Baz.bar` in the same file).
    parent_name: Option<String>,
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
    }
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
    check("cyclomatic", base.cyclomatic.map(|v| v as f64), head.cyclomatic.map(|v| v as f64), true);
    check("max_nesting", base.max_nesting.map(|v| v as f64), head.max_nesting.map(|v| v as f64), true);
    check("loc", Some(base.loc as f64), Some(head.loc as f64), true);
    check("param_count", base.param_count.map(|v| v as f64), head.param_count.map(|v| v as f64), true);
    check("field_count", base.field_count.map(|v| v as f64), head.field_count.map(|v| v as f64), true);
    check("method_count", Some(base.method_count as f64), Some(head.method_count as f64), true);
    check("public_field_ratio",
        base.public_field_ratio.map(|v| v as f64),
        head.public_field_ratio.map(|v| v as f64),
        true,
    );

    // Relational metrics: change when OTHER entities change their relationships
    check("fan_in", Some(base.fan_in as f64), Some(head.fan_in as f64), false);
    check("fan_out", Some(base.fan_out as f64), Some(head.fan_out as f64), false);

    (deltas, intrinsic_changed)
}

/// Simple hash of source code for quick equality check.
fn source_hash(e: &CodeEntity) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    e.source_code.hash(&mut hasher);
    hasher.finish()
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
    // Index base entities by stable key.
    let mut base_by_key: HashMap<EntityKey, &CodeEntity> = HashMap::new();
    for e in base_graph.entities() {
        if e.kind == EntityKind::Parameter { continue; }
        let key = entity_key(e, base_root);
        base_by_key.insert(key, e);
    }

    // Walk head entities, match against base.
    let mut diffs = Vec::new();
    let mut matched_base_keys = HashSet::new();

    for e in head_graph.entities() {
        if e.kind == EntityKind::Parameter { continue; }
        let key = entity_key(e, head_root);
        let file_path = e
            .file_path
            .strip_prefix(head_root)
            .unwrap_or(&e.file_path)
            .display()
            .to_string();

        if let Some(base_e) = base_by_key.get(&key) {
            matched_base_keys.insert(key);
            // Check if anything changed.
            let source_code_changed = source_hash(e) != source_hash(base_e);
            let (metric_deltas, intrinsic_metrics_changed) = compare_metrics(&base_e.metrics, &e.metrics);

            // Core change = source code changed OR intrinsic metrics changed.
            // Impact-only change = only relational metrics (fan_in/fan_out) changed.
            let is_core_change = source_code_changed || intrinsic_metrics_changed;
            let status = if is_core_change || !metric_deltas.is_empty() {
                ChangeStatus::Modified
            } else {
                ChangeStatus::Unchanged
            };
            diffs.push(EntityDiff {
                entity_id: e.id.clone(),
                name: e.name.clone(),
                kind: e.kind.display_name().to_string(),
                file_path,
                status,
                source_changed: is_core_change,
                metric_deltas,
                base_entity_id: Some(base_e.id.clone()),
            });
        } else {
            diffs.push(EntityDiff {
                entity_id: e.id.clone(),
                name: e.name.clone(),
                kind: e.kind.display_name().to_string(),
                file_path,
                status: ChangeStatus::Added,
                source_changed: true, // Added = core change
                metric_deltas: Vec::new(),
                base_entity_id: None,
            });
        }
    }

    // Removed entities: in base but not matched by any head entity.
    for (key, base_e) in &base_by_key {
        if matched_base_keys.contains(key) { continue; }
        let file_path = base_e
            .file_path
            .strip_prefix(base_root)
            .unwrap_or(&base_e.file_path)
            .display()
            .to_string();
        diffs.push(EntityDiff {
            entity_id: base_e.id.clone(),
            name: base_e.name.clone(),
            kind: base_e.kind.display_name().to_string(),
            file_path,
            status: ChangeStatus::Removed,
            source_changed: true, // Removed = core change
            metric_deltas: Vec::new(),
            base_entity_id: Some(base_e.id.clone()),
        });
    }

    let modified_count = diffs.iter().filter(|d| d.status == ChangeStatus::Modified).count();
    let modified_source = diffs.iter().filter(|d| d.status == ChangeStatus::Modified && d.source_changed).count();
    let modified_impact = modified_count - modified_source;

    let summary = DiffSummary {
        total_base: base_by_key.len(),
        total_head: head_graph.entities().filter(|e| e.kind != EntityKind::Parameter).count(),
        added: diffs.iter().filter(|d| d.status == ChangeStatus::Added).count(),
        removed: diffs.iter().filter(|d| d.status == ChangeStatus::Removed).count(),
        modified: modified_count,
        modified_source,
        modified_impact,
        unchanged: diffs.iter().filter(|d| d.status == ChangeStatus::Unchanged).count(),
    };

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

/// Paths changed in the working tree relative to `base_ref`: tracked
/// edits (`git diff --name-only <ref>`) plus untracked, non-ignored
/// files (`git ls-files --others --exclude-standard`). Returned
/// repo-root-relative with `/` separators, sorted and de-duplicated.
/// Best-effort — an empty list on any git failure (callers treat the
/// changed set as advisory, never as a gate).
pub fn changed_files(repo_root: &Path, base_ref: &str) -> Vec<String> {
    let mut files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut absorb = |args: &[&str]| {
        if let Ok(out) = Command::new("git").args(args).current_dir(repo_root).output() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let l = line.trim();
                if !l.is_empty() {
                    files.insert(l.to_string());
                }
            }
        }
    };
    absorb(&["diff", "--name-only", base_ref]);
    absorb(&["ls-files", "--others", "--exclude-standard"]);
    files.into_iter().collect()
}

/// Create a detached git worktree at `dir` for the given ref. Removes any
/// stale worktree at the same path first.
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
        anyhow::bail!("Failed to create worktree: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Remove a git worktree (best-effort, ignores errors).
pub fn remove_worktree(repo_root: &Path, dir: &Path) {
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", dir.to_str().unwrap()])
        .current_dir(repo_root)
        .output();
}

/// Build a Config for analyzing a directory.
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
    config
}

/// Analyze code at `root_dir` and return graph + config.
pub fn analyze_at(
    root_dir: &Path,
    include_tests: bool,
    languages: &Option<Vec<String>>,
    label: &str,
) -> AnyhowResult<(DependencyGraph, Config)> {
    eprintln!("  Analyzing {} ...", label);
    let config = build_analysis_config(root_dir, include_tests, languages);
    let mut analyzer = Analyzer::new(config.clone());
    let result = analyzer.analyze()?;
    let graph = DependencyGraph::from_analysis(&result);
    eprintln!("    {} entities, {} relationships", result.entities.len(), result.relationships.len());
    Ok((graph, config))
}

/// Write all diff-related output files to `output_dir`.
pub fn write_diff_outputs(
    output_dir: &Path,
    head_graph: &DependencyGraph,
    head_config: &Config,
    base_graph: &DependencyGraph,
    base_config: &Config,
    diff: &DiffResult,
) -> AnyhowResult<(String, String)> {
    let data_path = output_dir.join("data.json");
    std::fs::create_dir_all(output_dir)?;

    let head_json = output::render(head_graph, head_config)?;
    std::fs::write(&data_path, &head_json)?;

    let details_str = JsonRenderer::render_details(head_graph, head_config)?;
    std::fs::write(data_path.with_extension("details.json"), &details_str)?;

    let index_str = JsonRenderer::render_index(head_graph, head_config)?;
    std::fs::write(data_path.with_extension("index.json"), &index_str)?;

    let base_details_str = JsonRenderer::render_details(base_graph, base_config)?;
    std::fs::write(output_dir.join("data.base-details.json"), &base_details_str)?;

    let diff_json = serde_json::to_string(diff)?;
    std::fs::write(output_dir.join("diff.json"), &diff_json)?;

    Ok((diff_json, base_details_str))
}
