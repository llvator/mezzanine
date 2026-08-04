//! `spec_slice` — the project's Elevator spec, narrowed to one folder.
//!
//! `overview` renders the whole domain layer *for reading*. This tool
//! renders a subset of it back into `.elv` source: the spec entities
//! whose `cr:` claims land inside a folder, everything they contain,
//! the Concepts they use, and their ancestors as pruned context. The
//! emitted slice parses and passes `--check` on its own, so an agent
//! can drop it next to a ticket as the durable record of which part of
//! the domain a change was about — a small, diffable snapshot that
//! survives after the shared spec has moved on.
//!
//! The selection is the whole difference from `elevator --extract`,
//! which selects by entity name. Here the question is "what does the
//! spec say about *this folder*", and the answer comes from the `cr:`
//! index the spec already maintains:
//!
//! - **Claims inside the folder** are the seeds. Their subtrees come
//!   with them, so slicing `src/mcp` when a Category claims `src/mcp/`
//!   yields that Category's whole branch.
//! - **No claim inside** falls back to the *narrowest* claim above the
//!   folder, and says so. Slicing a single file usually lands here: a
//!   spec normally claims `src/mcp/tools.rs`, not the function in it.
//! - A folder no `cr:` mentions in either direction is an error, not
//!   an empty file — silence there means the spec makes no claim, and
//!   writing an empty slice would look like it made an empty one.
//!
//! Writes stay inside the project root (see [`resolve_out`]). With no
//! `out`, the slice comes back in the response instead, capped like
//! every other tool's body.
//!
//! The file is `slice.rs` rather than `spec_slice.rs` on purpose:
//! `is_test_path` classifies any path containing `spec` as test code,
//! so the obvious name would drop this module out of nao's own graph —
//! and out of the complexity gate and `dead_code` with it.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::analyzer::AnalysisResult;
use crate::graph::DependencyGraph;
use crate::models::{CodeEntity, RelationshipKind};
use crate::output::elevator_code_map::{parse_cr_attr, short_ref};
use crate::output::elevator_extract::{self, Slice};

use super::tools::{cap_lines, path_under};
use super::McpServer;

/// How many seed entities the response names before summarizing.
const MAX_LISTED_SEEDS: usize = 20;

/// `spec_slice` — extract the spec's claims about one folder into
/// standalone `.elv`, optionally written to a file.
pub fn spec_slice(server: &McpServer, args: &Value) -> Result<String> {
    let target = target_folder(server, args)?;
    let graph = super::tools::analyze(server, &server.root)?;
    let result = spec_only(&graph);
    if result.entities.is_empty() {
        bail!(
            "No Elevator (.elv) spec found under {}. `spec_slice` extracts a subset of \
             the domain layer; without .elv files there is nothing to slice. Use `map` \
             for code-level structure instead.",
            server.root.display()
        );
    }

    let selection = select(&result, &target);
    if selection.seeds.is_empty() {
        bail!(
            "No Elevator entity claims a path inside or containing `{}`. The spec makes \
             no claim about that folder — deepen it first (add a `cr:` there), or call \
             `overview` to see which paths the spec does claim.",
            target
        );
    }

    let source = server.root.display().to_string();
    let slice = elevator_extract::extract_seeds(&result, &selection.seeds, &selection.label, &source);
    let report = Report::new(&target, &selection, &slice, &result);
    match args.get("out").and_then(|v| v.as_str()).map(str::trim) {
        Some(out) if !out.is_empty() => {
            let path = resolve_out(server, out, args)?;
            write_slice(&path, &slice.text)?;
            Ok(report.written(&path, &server.root))
        }
        _ => Ok(report.inline()),
    }
}

// ------------------------------------------------------------------
//  Selection
// ------------------------------------------------------------------

/// The entities a folder selects, and the one-line reason recorded in
/// the slice header.
struct Selection {
    seeds: Vec<String>,
    label: String,
    /// True when nothing claimed a path inside the folder and the
    /// slice fell back to the narrowest claim above it.
    fallback: bool,
}

/// Seed the slice from the spec's `cr:` index.
///
/// Claims inside the folder win outright; a `cr:` *above* the folder
/// is only consulted when there are none, because expanding a broad
/// claim's subtree (a Category claiming `src/`) would return most of
/// the spec for a question about one folder inside it.
fn select(result: &AnalysisResult, target: &str) -> Selection {
    let mut inside: BTreeSet<String> = BTreeSet::new();
    let mut enclosing: Vec<(usize, String, String)> = Vec::new();

    for e in elevator_extract::elevator_entities(result) {
        for cr in code_refs(e) {
            if path_under(target, &cr) {
                inside.insert(e.id.clone());
            } else if path_under(&cr, target) {
                enclosing.push((depth(&cr), cr, e.id.clone()));
            }
        }
    }

    if !inside.is_empty() {
        return Selection {
            label: format!("path {} — {} cr: claim(s) inside it", target, inside.len()),
            seeds: inside.into_iter().collect(),
            fallback: false,
        };
    }

    let narrowest = enclosing.iter().map(|(d, _, _)| *d).max().unwrap_or(0);
    let claims: BTreeSet<&str> = enclosing
        .iter()
        .filter(|(d, _, _)| *d == narrowest)
        .map(|(_, cr, _)| cr.as_str())
        .collect();
    let tied: BTreeSet<String> = enclosing
        .iter()
        .filter(|(d, _, _)| *d == narrowest)
        .map(|(_, _, id)| id.clone())
        .collect();
    let seeds = innermost(result, tied);
    Selection {
        label: format!(
            "path {} — no claim inside; narrowest enclosing claim: {}",
            target,
            claims.into_iter().collect::<Vec<_>>().join(", ")
        ),
        seeds,
        fallback: true,
    }
}

/// Drop candidates that contain another candidate.
///
/// Only the fallback path needs this, and only because `cr:` depth
/// can tie: a Category claiming `src/parser/` and a Feature under it
/// claiming the same folder are equally specific by path, but seeding
/// the Category expands its every other branch. The descendant is the
/// narrower answer, and the Category still shows up as context.
///
/// Claims *inside* the folder are left alone on purpose: there the
/// union of the subtrees is exactly the part of the spec that lives in
/// the folder, ancestor pairs included.
fn innermost(result: &AnalysisResult, candidates: BTreeSet<String>) -> Vec<String> {
    let contains: Vec<(&str, &str)> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Contains)
        .map(|r| (r.source_id.as_str(), r.target_id.as_str()))
        .collect();
    candidates
        .iter()
        .filter(|id| {
            !candidates
                .iter()
                .any(|other| other != *id && is_ancestor(&contains, id, other))
        })
        .cloned()
        .collect()
}

/// True if `ancestor` reaches `descendant` through `Contains` edges.
/// Bounded by the edge count, so a cyclic spec cannot loop forever.
fn is_ancestor(contains: &[(&str, &str)], ancestor: &str, descendant: &str) -> bool {
    let mut reached: BTreeSet<&str> = BTreeSet::from([ancestor]);
    for _ in 0..contains.len() {
        let grown: Vec<&str> = contains
            .iter()
            .filter(|(s, _)| reached.contains(s))
            .map(|(_, t)| *t)
            .filter(|t| !reached.contains(t))
            .collect();
        if grown.is_empty() {
            break;
        }
        reached.extend(grown);
        if reached.contains(descendant) {
            return true;
        }
    }
    false
}

/// The normalized `cr:` paths an entity claims. Non-`cr` attributes
/// (other languages hang their own things there) and empty claims are
/// dropped.
fn code_refs(e: &CodeEntity) -> Vec<String> {
    e.attributes
        .iter()
        .filter_map(|a| parse_cr_attr(a))
        .map(|(_, p)| normalize_rel(&p))
        .filter(|p| !p.is_empty())
        .collect()
}

/// Path segments in a repo-relative path — the narrowness ranking for
/// enclosing claims, so `src/mcp` beats `src` for a question about
/// `src/mcp/tools.rs`.
fn depth(path: &str) -> usize {
    path.split('/').filter(|s| !s.is_empty()).count()
}

/// The Elevator layer of a graph, as a standalone [`AnalysisResult`].
///
/// The extractor works from an analysis, the MCP server caches a
/// graph; this is the adapter. Filtering to spec entities keeps the
/// clone small (a spec is hundreds of entities, the code graph is
/// tens of thousands) and drops every edge the extractor would ignore
/// anyway — it only ever walks `Contains` and `References` between
/// two Elevator entities.
fn spec_only(graph: &DependencyGraph) -> AnalysisResult {
    let entities: Vec<CodeEntity> = graph
        .entities()
        .filter(|e| e.tags.contains("elevator"))
        .cloned()
        .collect();
    let ids: std::collections::HashSet<String> = entities.iter().map(|e| e.id.clone()).collect();
    let relationships = graph
        .relationships()
        .filter(|r| ids.contains(&r.source_id) && ids.contains(&r.target_id))
        .cloned()
        .collect();
    AnalysisResult { entities, relationships, files: Vec::new(), warnings: Vec::new() }
}

// ------------------------------------------------------------------
//  Paths
// ------------------------------------------------------------------

/// The `path` argument as a root-relative, `/`-separated folder (or
/// file) string. Required: a slice of "the whole project" is what
/// `overview` already is.
fn target_folder(server: &McpServer, args: &Value) -> Result<String> {
    let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
    if raw.is_empty() {
        bail!("spec_slice needs a `path` — the folder whose spec claims to extract, e.g. `src/mcp`.");
    }
    let p = PathBuf::from(raw);
    let rel = if p.is_absolute() {
        let canonical = p.canonicalize().unwrap_or(p);
        canonical
            .strip_prefix(&server.root)
            .with_context(|| {
                format!(
                    "`{}` is outside the project root {}",
                    raw,
                    server.root.display()
                )
            })?
            .to_path_buf()
    } else {
        p
    };
    let normalized = normalize_rel(&rel.to_string_lossy());
    if normalized.is_empty() {
        bail!("`{}` is the project root — use `overview` for the whole spec.", raw);
    }
    Ok(normalized)
}

/// Repo-relative path in the form `cr:` uses: forward slashes, no
/// `./` prefix, no trailing slash.
fn normalize_rel(path: &str) -> String {
    let cleaned = path
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string();
    // A lone `.` is the root, which every caller here treats as "no
    // path" — the target argument rejects it, a `cr:` claiming it is
    // too wide to seed anything.
    if cleaned == "." {
        String::new()
    } else {
        cleaned
    }
}

/// Resolve `out` to an absolute path under the project root.
///
/// Writes are the one thing this tool does that the others don't, so
/// they are fenced: no absolute paths, nothing that climbs out of the
/// root with `..`, and no silent overwrite. An agent that wants the
/// slice somewhere else can omit `out` and write the returned text
/// with its own file tools — where the user can see the write.
fn resolve_out(server: &McpServer, out: &str, args: &Value) -> Result<PathBuf> {
    let candidate = PathBuf::from(out);
    if candidate.is_absolute() {
        bail!(
            "`out` must be relative to the project root ({}); got an absolute path. \
             Omit `out` to get the slice text back and write it yourself.",
            server.root.display()
        );
    }
    let path = normalize_lexically(&server.root.join(&candidate));
    if !path.starts_with(&server.root) {
        bail!(
            "`out` must stay inside the project root ({}); `{}` escapes it.",
            server.root.display(),
            out
        );
    }
    let overwrite = args.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false);
    if path.exists() && !overwrite {
        bail!(
            "{} already exists. Pass overwrite=true to replace it, or choose another `out`.",
            out
        );
    }
    Ok(path)
}

/// Resolve `.` and `..` textually. The target usually doesn't exist
/// yet — that's the point of writing it — so `canonicalize` is not
/// available for the containment check above.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn write_slice(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Could not create {}", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("Could not write {}", path.display()))
}

// ------------------------------------------------------------------
//  Rendering
// ------------------------------------------------------------------

/// Everything the response says about a slice, minus where it went.
struct Report<'a> {
    target: &'a str,
    selection: &'a Selection,
    slice: &'a Slice,
    /// Seed entities by short ref, so the caller sees *what* the
    /// folder selected without re-reading the slice.
    seeds: Vec<String>,
}

impl Report<'_> {
    fn new<'a>(
        target: &'a str,
        selection: &'a Selection,
        slice: &'a Slice,
        result: &AnalysisResult,
    ) -> Report<'a> {
        let mut seeds: Vec<String> = result
            .entities
            .iter()
            .filter(|e| selection.seeds.contains(&e.id))
            .map(short_ref)
            .collect();
        seeds.sort();
        Report { target, selection, slice, seeds }
    }

    /// Rendering for a slice that was written to `path`: provenance
    /// only, never the body — the file is the artifact.
    fn written(&self, path: &Path, root: &Path) -> String {
        let where_to = path.strip_prefix(root).unwrap_or(path).display().to_string();
        let mut body = self.provenance();
        body.insert(0, format!("# Spec slice for {} → {}", self.target, where_to));
        body.push(String::new());
        body.push(format!(
            "Standalone .elv: `elevator {} --check` validates it on its own.",
            where_to
        ));
        body.join("\n")
    }

    /// Rendering with no `out`: provenance, then the slice itself,
    /// capped like every other tool body.
    fn inline(&self) -> String {
        let mut body = self.provenance();
        body.insert(0, format!("# Spec slice for {}", self.target));
        body.push(String::new());
        body.extend(self.slice.text.lines().map(String::from));
        cap_lines(body, "Pass out=<file> to write the full slice to disk instead.")
    }

    /// The provenance block both renderings share: what was selected,
    /// why, and what the slice contains.
    fn provenance(&self) -> Vec<String> {
        let mut out = vec![String::new(), format!("Selection: {}", self.selection.label)];
        if self.selection.fallback {
            out.push(format!(
                "  ⚠ nothing in the spec claims a path inside {} — this is the branch above it.",
                self.target
            ));
        }
        out.push(format!(
            "Slice:     {} entity(ies), {} ancestor(s) as context, {} name-only reference(s)",
            self.slice.members, self.slice.ancestors, self.slice.name_only
        ));
        if self.slice.dropped_unresolved > 0 {
            out.push(format!(
                "  ⚠ {} edge(s) dropped — their targets are undefined in the source spec.",
                self.slice.dropped_unresolved
            ));
        }
        out.push("Seeded from:".to_string());
        for name in self.seeds.iter().take(MAX_LISTED_SEEDS) {
            out.push(format!("  {}", name));
        }
        if let Some(extra) = self.seeds.len().checked_sub(MAX_LISTED_SEEDS).filter(|n| *n > 0) {
            out.push(format!("  … and {} more", extra));
        }
        out
    }
}

#[cfg(test)]
mod tests;
