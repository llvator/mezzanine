//! Push-mode: nao's structural signal arrives without being asked.
//!
//! The MCP tools are pull-only — an agent that forgets to call
//! `assess_change` ships regressions silently. This module drives the
//! two push legs that fix that:
//!
//! - **`self_review`** (MCP-007): a `nao hook self-review` subcommand
//!   for a Claude Code Stop/PostToolUse hook. It reports structural
//!   regressions of the working tree with the LSP-diagnostics token
//!   contract: silent when clean, each finding surfaced exactly once
//!   per session (fingerprint state file), a severity floor, and a
//!   hard line cap. Determinism (AN-002) is what makes the fingerprints
//!   stable across runs.
//! - **`pr_report`** (MCP-008): a `nao pr-report` subcommand that
//!   renders the full `assess_change` report as a PR comment body, or a
//!   one-liner when the PR touches nothing structural.
//!
//! Both reuse the diff machinery in [`crate::diff`] and the report
//! renderer in [`crate::mcp::tools`]; neither needs the long-lived MCP
//! server or its warm cache (a hook/CI run is a fresh process).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::diff;
use crate::graph::DependencyGraph;
use crate::mcp::tools;
use crate::models::{CodeEntity, FolderShape, OutsideVerdict, ShapePattern, SmellKind};

/// A combined `cyclomatic + max_nesting` increase below this is treated
/// as metric noise, never a finding — the "not every ±1 wiggle" floor.
const COMPLEXITY_FLOOR: f64 = 3.0;

/// Default hard cap on hook output lines (LSP-diagnostics contract).
pub const DEFAULT_LINE_CAP: usize = 10;

/// Severity ranks, ordered so `min` acts as a floor: `Low < Medium < High`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Severity> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Severity::Low),
            "medium" | "med" => Some(Severity::Medium),
            "high" => Some(Severity::High),
            _ => None,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
        })
    }
}

/// One structural regression, with a fingerprint stable across runs so
/// the state file can dedupe it, and a one-line human rendering.
#[derive(Debug, Clone)]
pub struct Finding {
    pub fingerprint: String,
    pub severity: Severity,
    pub line: String,
}

/// The analyzed base + head graphs, their entity-level diff, and the
/// git-changed path list — everything the report and findings need.
pub struct DiffArtifacts {
    pub base_graph: DependencyGraph,
    pub head_graph: DependencyGraph,
    pub result: diff::DiffResult,
    pub changed: Vec<String>,
    /// Where each side was analysed. The base lives in a throwaway
    /// worktree, so the two graphs spell the same file differently and
    /// anything joining them by path has to strip its own root first.
    pub base_root: PathBuf,
    pub head_root: PathBuf,
}

/// Analyze `base_ref` (via a throwaway worktree) and the working tree,
/// diff them, and collect the git-changed path list. Uncached — a hook
/// or CI job is a fresh process, so the MCP warm cache buys nothing.
pub fn build_artifacts(
    root: &Path,
    base_ref: &str,
    include_tests: bool,
    languages: &Option<Vec<String>>,
) -> Result<DiffArtifacts> {
    diff::verify_git_repo(root)?;
    let from_sha = diff::resolve_git_ref(root, base_ref)?;
    if from_sha.is_empty() {
        bail!("Cannot resolve git ref '{}'", base_ref);
    }

    // One scope for both sides, settled from the working tree — the base
    // checkout carries the `.nao/settings.json` committed at `base_ref`, and
    // two sides that exclude different files report the difference as
    // structural change.
    let scope = diff::build_analysis_config(root, include_tests, languages);

    let base_dir = std::env::temp_dir().join(format!("nao-push-base-{}", from_sha));
    diff::create_worktree(root, &base_dir, base_ref)?;
    let base = diff::analyze_with(
        diff::rooted_at(&scope, &base_dir),
        &format!("base ({})", from_sha),
    );
    diff::remove_worktree(root, &base_dir);
    let (base_graph, _) = base?;

    let (head_graph, _) = diff::analyze_with(scope, "working tree")?;
    let result = diff::compute_diff(
        &base_graph,
        &head_graph,
        &base_dir,
        root,
        &from_sha,
        "working",
    );
    let changed = diff::changed_files(root, base_ref);

    Ok(DiffArtifacts {
        base_graph,
        head_graph,
        result,
        changed,
        base_root: base_dir,
        head_root: root.to_path_buf(),
    })
}

fn fingerprint(tag: &str, d: &diff::EntityDiff, detail: &str) -> String {
    // Line-number-independent: matches the diff's own stable entity key
    // (name + kind + file), so an edit that shifts an entity down the
    // file keeps the same fingerprint.
    format!("{}|{}|{}|{}|{}", tag, d.kind, d.name, d.file_path, detail)
}

/// Collect structural regressions from a diff: new smells, new cycles,
/// complexity jumps above the floor, and folders that fell a shape tier.
/// Filtered to `min` severity and up, sorted worst-first then by
/// fingerprint for deterministic output.
///
/// The first three are per-entity and read off the diff rows; the fourth
/// is per-folder and is joined by path instead, because a folder can lose
/// its shape to an edge added in a file it does not contain.
pub fn findings(a: &DiffArtifacts, min: Severity) -> Vec<Finding> {
    let head_by_id: HashMap<&str, &CodeEntity> = a
        .head_graph
        .entities()
        .map(|e| (e.id.as_str(), e))
        .collect();
    let base_by_id: HashMap<&str, &CodeEntity> = a
        .base_graph
        .entities()
        .map(|e| (e.id.as_str(), e))
        .collect();

    let mut out: Vec<Finding> = Vec::new();
    for d in &a.result.entities {
        if d.status == diff::ChangeStatus::Removed {
            continue;
        }
        let Some(head) = head_by_id.get(d.entity_id.as_str()).copied() else {
            continue;
        };
        if !tools::is_listed(head) {
            continue;
        }
        let base = d
            .base_entity_id
            .as_deref()
            .and_then(|id| base_by_id.get(id).copied());

        // New smells: present on head, absent on base.
        let base_smells: HashSet<SmellKind> = base
            .map(|e| e.metrics.smells.iter().copied().collect())
            .unwrap_or_default();
        for s in &head.metrics.smells {
            if !base_smells.contains(s) {
                out.push(Finding {
                    fingerprint: fingerprint("smell", d, s.label()),
                    severity: Severity::Medium,
                    line: format!(
                        "new smell: {} on {} {} — {}",
                        s.label(),
                        d.kind,
                        d.name,
                        d.file_path
                    ),
                });
            }
        }

        // New cycle membership.
        let base_in_cycle = base.map(|e| e.metrics.in_cycle).unwrap_or(false);
        if head.metrics.in_cycle && !base_in_cycle {
            out.push(Finding {
                fingerprint: fingerprint("cycle", d, ""),
                severity: Severity::High,
                line: format!("new cycle: {} {} — {}", d.kind, d.name, d.file_path),
            });
        }

        // Complexity jump: combined cyclomatic + max_nesting growth.
        let cx: f64 = d
            .metric_deltas
            .iter()
            .filter(|m| matches!(m.name.as_str(), "cyclomatic" | "max_nesting"))
            .map(|m| m.delta.max(0.0))
            .sum();
        if cx >= COMPLEXITY_FLOOR {
            let severity = if cx >= 10.0 {
                Severity::High
            } else if cx >= 5.0 {
                Severity::Medium
            } else {
                Severity::Low
            };
            out.push(Finding {
                fingerprint: fingerprint("cx", d, ""),
                severity,
                line: format!(
                    "complexity +{}: {} {} — {}",
                    cx as i64, d.kind, d.name, d.file_path
                ),
            });
        }
    }

    out.extend(shape_findings(a));
    out.retain(|f| f.severity >= min);
    out.sort_by(|x, y| {
        y.severity
            .cmp(&x.severity)
            .then_with(|| x.fingerprint.cmp(&y.fingerprint))
    });
    out
}

// ------------------------------------------------------------------
//  Folder shape
// ------------------------------------------------------------------

/// How many falls get an edge named. `folder_picture` rescans the whole
/// graph on every call and this spends two per fall, so the work is
/// bounded rather than proportional to how bad a session went — and the
/// ten-line cap means the rest would not be printed anyway.
const MAX_ATTRIBUTED_FALLS: usize = 5;

/// Folder-shape falls, each with the change to who-depends-on-what that
/// best explains it.
///
/// The one finding here that is not about an entity, and the reason it
/// has to exist: shape is a property of a *folder*, computed from the
/// whole repo's graph. An import added in one file can drop a sibling
/// folder a tier by reaching past its door, and every other finding in
/// this module fires on the entity that changed. The folder that got
/// worse may hold none of the changed files — which is exactly why
/// nothing else catches it, and why an agent that has just reached into
/// a folder it was not working in hears about it here or not at all.
///
/// Falls only. A folder that was tangled before the edit and is still
/// tangled is not news, and reporting standing shape every time an agent
/// stops would bury the findings that are.
fn shape_findings(a: &DiffArtifacts) -> Vec<Finding> {
    let base = shapes_by_folder(&a.base_graph, &a.base_root);
    let head = shapes_by_folder(&a.head_graph, &a.head_root);

    let mut falls: Vec<(&String, ShapePattern, &FolderShape)> = head
        .iter()
        .filter_map(|(folder, now)| {
            let was = base.get(folder)?.pattern;
            (now.pattern < was).then_some((folder, was, now))
        })
        .collect();
    // Worst landing first, then the longest drop, then path so two
    // identical runs order identically (AN-002). This is the order
    // attribution is spent in, so it decides which falls get an edge
    // named when a session breaks more than five folders.
    falls.sort_by(|x, y| {
        x.2.pattern
            .cmp(&y.2.pattern)
            .then(y.1.cmp(&x.1))
            .then(x.0.cmp(y.0))
    });

    falls
        .iter()
        .enumerate()
        .map(|(rank, (folder, was, now))| Finding {
            // Keyed on the transition, not just the folder: a folder that
            // falls again, further, is a new thing to say rather than one
            // the session has already reported.
            fingerprint: format!("shape|{}|{}→{}", folder, was.label(), now.pattern.label()),
            // A loop is the one rung nothing above it can be worked on
            // until it clears, so landing there outranks any other fall.
            severity: if now.pattern == ShapePattern::Cyclic {
                Severity::High
            } else {
                Severity::Medium
            },
            line: format!(
                "folder shape fell: {} {} → {}, now held back by {}{}",
                if folder.is_empty() { "(root)" } else { folder },
                was.label(),
                now.pattern.label(),
                now.blocker
                    .map_or_else(|| "nothing".to_string(), |b| b.summary()),
                (rank < MAX_ATTRIBUTED_FALLS)
                    .then(|| shape_cause(a, folder))
                    .flatten()
                    .unwrap_or_default(),
            ),
        })
        .collect()
}

fn shapes_by_folder(graph: &DependencyGraph, root: &Path) -> BTreeMap<String, FolderShape> {
    graph
        .module_metrics()
        .iter()
        .filter_map(|m| {
            let shape = m.metrics.shape.as_ref()?;
            Some((tools::rel_path(Path::new(&m.path), root), shape.clone()))
        })
        .collect()
}

/// The drawn edges of one folder's picture, split by whether they stay
/// inside it. Exits are dropped: depending outward is what a folder is
/// for, and one appearing or disappearing never moved this folder's
/// tier.
struct Crossings {
    inside: BTreeSet<(String, String)>,
    inbound: BTreeSet<(String, String)>,
}

fn picture_edges(graph: &DependencyGraph, root: &Path, folder: &str) -> Option<Crossings> {
    let picture = graph
        .folder_picture(&root.join(folder).display().to_string())?
        .relative_to(root);
    Some(Crossings {
        inside: picture
            .edges
            .iter()
            .map(|e| (trim(&e.from, folder), trim(&e.to, folder)))
            .collect(),
        inbound: picture
            .outside
            .iter()
            .filter(|o| o.verdict != OutsideVerdict::Exit)
            .map(|o| (o.outside.clone(), trim(&o.inside, folder)))
            .collect(),
    })
}

/// The change to who-depends-on-what that best explains a fall, as a
/// trailing phrase. `None` when either picture cannot be drawn, in which
/// case the finding still names the tier and the gate.
///
/// Added edges lead because they are both the common cause and the
/// fixable one. A removal can lower a tier too — cutting the only edge
/// reaching a child leaves it parentless, which `arborescence` charges
/// for (ADR 0022) — so it is named when nothing was added, rather than
/// leaving the tier looking like it moved on its own.
fn shape_cause(a: &DiffArtifacts, folder: &str) -> Option<String> {
    let head = picture_edges(&a.head_graph, &a.head_root, folder)?;
    let base = picture_edges(&a.base_graph, &a.base_root, folder)?;
    let added: Vec<&(String, String)> = head.inside.difference(&base.inside).collect();
    if !added.is_empty() {
        return Some(named("new edge", &added));
    }
    let reaching: Vec<&(String, String)> = head.inbound.difference(&base.inbound).collect();
    if !reaching.is_empty() {
        return Some(named("new dependency in", &reaching));
    }
    let gone: Vec<&(String, String)> = base.inside.difference(&head.inside).collect();
    if !gone.is_empty() {
        return Some(named("edge removed", &gone));
    }
    None
}

/// At most two edges named. The hook prints ten lines in total, and one
/// finding must not spend three of them.
fn named(verb: &str, edges: &[&(String, String)]) -> String {
    let shown: Vec<String> = edges
        .iter()
        .take(2)
        .map(|(from, to)| format!("{from} → {to}"))
        .collect();
    let more = match edges.len().saturating_sub(2) {
        0 => String::new(),
        n => format!(" (+{n} more)"),
    };
    format!(" — {verb} {}{}", shown.join(", "), more)
}

/// Paths inside the folder read better without the folder's own name on
/// the front: the line has already said which folder fell.
fn trim(path: &str, folder: &str) -> String {
    Path::new(path)
        .strip_prefix(folder)
        .map(|rest| rest.display().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Session state: fingerprint → the finding's one-line rendering at the
/// time it was surfaced (kept so a resolved finding can be named).
type SessionState = BTreeMap<String, String>;

/// The pure delta: given the current findings and the previously-seen
/// state, decide what to print (new findings, then resolved lines, under
/// the line cap) and what the next state should be. Returns `(output,
/// next_state)`; `output` is empty exactly when nothing new or resolved.
///
/// The four contract properties live here:
/// - **silent when clean**: no new + no resolved ⇒ empty output.
/// - **once per session**: a finding already in `seen` is not reprinted;
///   only findings actually printed enter the next state, so a new
///   finding pushed past the cap resurfaces next run rather than being
///   silently swallowed.
/// - **resolved once**: a fingerprint in `seen` but gone from current is
///   printed once and dropped; if the cap truncates it, it stays in
///   state to retry.
/// - **hard cap**: output never exceeds `cap` lines; overflow replaces
///   the last line with a pointer to `assess_change`.
fn diff_against_state(
    current: &[Finding],
    seen: &SessionState,
    cap: usize,
) -> (String, SessionState) {
    let current_fps: BTreeSet<&str> = current.iter().map(|f| f.fingerprint.as_str()).collect();

    // Candidates in priority order: new findings (already worst-first),
    // then resolved lines. Each carries its fingerprint for state math.
    let mut candidates: Vec<(String, String, bool)> = Vec::new(); // (fp, line, is_new)
    for f in current {
        if !seen.contains_key(&f.fingerprint) {
            candidates.push((f.fingerprint.clone(), format!("⚠ {}", f.line), true));
        }
    }
    for (fp, line) in seen {
        if !current_fps.contains(fp.as_str()) {
            candidates.push((fp.clone(), format!("✓ resolved: {}", line), false));
        }
    }

    if candidates.is_empty() {
        return (String::new(), seen.clone());
    }

    // Apply the cap, reserving one line for the overflow pointer.
    let (emitted, overflow): (Vec<&(String, String, bool)>, bool) = if candidates.len() <= cap {
        (candidates.iter().collect(), false)
    } else {
        (
            candidates.iter().take(cap.saturating_sub(1)).collect(),
            true,
        )
    };

    let mut lines: Vec<String> = emitted.iter().map(|(_, l, _)| l.clone()).collect();
    if overflow {
        lines.push("… more findings — run `assess_change` for the full report.".to_string());
    }
    let emitted_fps: BTreeSet<&str> = emitted.iter().map(|(fp, _, _)| fp.as_str()).collect();

    // Next state: persistent findings (still current AND previously seen),
    // plus the new findings we actually printed. Un-emitted new findings
    // are excluded so they resurface. Resolved findings not emitted (cap
    // truncation) are kept so they retry.
    let mut next: SessionState = BTreeMap::new();
    for f in current {
        if seen.contains_key(&f.fingerprint) || emitted_fps.contains(f.fingerprint.as_str()) {
            next.insert(f.fingerprint.clone(), f.line.clone());
        }
    }
    for (fp, line) in seen {
        let resolved = !current_fps.contains(fp.as_str());
        if resolved && !emitted_fps.contains(fp.as_str()) {
            next.insert(fp.clone(), line.clone());
        }
    }

    (lines.join("\n"), next)
}

/// Where the session state lives by default: a temp-dir file keyed by
/// the repo and the resolved base SHA. Keying on the SHA gives a natural
/// session boundary — a new commit moves HEAD, so the state resets and
/// findings that were surfaced against the old base start fresh.
fn default_state_path(root: &Path, base_sha: &str) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy().hash(&mut hasher);
    let repo = hasher.finish();
    std::env::temp_dir()
        .join("nao-self-review")
        .join(format!("{:016x}-{}.json", repo, base_sha))
}

fn load_state(path: &Path) -> SessionState {
    match std::fs::read_to_string(path) {
        Ok(body) => serde_json::from_str(&body).unwrap_or_default(),
        Err(_) => SessionState::default(),
    }
}

fn save_state(path: &Path, state: &SessionState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(state) {
        Ok(body) => {
            if let Err(e) = std::fs::write(path, body) {
                eprintln!(
                    "nao: could not persist self-review state to {}: {}",
                    path.display(),
                    e
                );
            }
        }
        Err(e) => eprintln!("nao: could not serialize self-review state: {}", e),
    }
}

/// MCP-007 entry point. Analyzes the working tree vs `base_ref`, diffs
/// its findings against the session state file, prints only the delta
/// (capped), and updates the state. Returns the text to print on stdout
/// — empty when there is nothing new to say.
pub fn self_review(
    root: &Path,
    base_ref: &str,
    include_tests: bool,
    languages: &Option<Vec<String>>,
    state_path: Option<PathBuf>,
    min: Severity,
    cap: usize,
) -> Result<String> {
    let artifacts = build_artifacts(root, base_ref, include_tests, languages)?;
    let current = findings(&artifacts, min);

    let from_sha = diff::resolve_git_ref(root, base_ref).unwrap_or_default();
    let state_file = state_path.unwrap_or_else(|| default_state_path(root, &from_sha));
    let seen = load_state(&state_file);

    let (output, next) = diff_against_state(&current, &seen, cap);
    if next != seen {
        save_state(&state_file, &next);
    }
    Ok(output)
}

/// MCP-008 entry point. Renders the `assess_change` report as a PR
/// comment body — a one-liner when the PR touches nothing structural,
/// otherwise the full report. The leading HTML marker lets the CI job
/// find and edit its own comment in place instead of posting a new one.
pub const PR_COMMENT_MARKER: &str = "<!-- nao-pr-report -->";

pub fn pr_report(
    root: &Path,
    base_ref: &str,
    include_tests: bool,
    languages: &Option<Vec<String>>,
) -> Result<String> {
    let artifacts = build_artifacts(root, base_ref, include_tests, languages)?;
    let s = &artifacts.result.summary;
    let structural = s.added + s.removed + s.modified_source;

    let mut out = String::from(PR_COMMENT_MARKER);
    out.push('\n');
    if structural == 0 {
        out.push_str(&format!(
            "**nao:** no structural change vs `{}` — metrics steady, nothing to report.",
            base_ref
        ));
        return Ok(out);
    }
    out.push_str(&tools::render_change_report(
        &artifacts.result,
        &artifacts.base_graph,
        &artifacts.head_graph,
        base_ref,
        &artifacts.changed,
        (&artifacts.base_root, &artifacts.head_root),
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(fp: &str, sev: Severity, line: &str) -> Finding {
        Finding {
            fingerprint: fp.to_string(),
            severity: sev,
            line: line.to_string(),
        }
    }

    /// A temp tree whose path must not contain "test": `analyze_at` with
    /// `include_tests: false` drops entities under a test-looking path,
    /// and a fixture filtered away scores no shape at all.
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nao-push-shape-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ));
            std::fs::create_dir_all(&path).unwrap();
            TmpDir(path)
        }

        fn write(&self, rel: &str, body: &str) {
            let full = self.0.join(rel);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, body).unwrap();
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn artifacts_of(base: &TmpDir, head: &TmpDir) -> DiffArtifacts {
        let (base_graph, _) = diff::analyze_at(&base.0, false, &None, "base").unwrap();
        let (head_graph, _) = diff::analyze_at(&head.0, false, &None, "head").unwrap();
        let result = diff::compute_diff(&base_graph, &head_graph, &base.0, &head.0, "base", "head");
        DiffArtifacts {
            base_graph,
            head_graph,
            result,
            changed: Vec::new(),
            base_root: base.0.clone(),
            head_root: head.0.clone(),
        }
    }

    /// A chain of three files, then one edge closing it into a loop.
    fn chain_then_loop() -> (TmpDir, TmpDir) {
        let base = TmpDir::new("base");
        let head = TmpDir::new("head");
        for dir in [&base, &head] {
            dir.write("src/pkg/mod.rs", "pub mod a;\npub mod b;\npub mod c;\n");
            dir.write(
                "src/pkg/a.rs",
                "use super::b::from_b;\npub fn from_a() -> i32 { from_b() }\n",
            );
            dir.write(
                "src/pkg/b.rs",
                "use super::c::from_c;\npub fn from_b() -> i32 { from_c() }\n",
            );
        }
        base.write("src/pkg/c.rs", "pub fn from_c() -> i32 { 1 }\n");
        // The single difference between the two trees.
        head.write(
            "src/pkg/c.rs",
            "use super::a::from_a;\npub fn from_c() -> i32 { from_a() }\n",
        );
        (base, head)
    }

    #[test]
    fn a_folder_that_gained_a_loop_is_reported_with_the_edge_that_closed_it() {
        let (base, head) = chain_then_loop();
        let found = shape_findings(&artifacts_of(&base, &head));
        let pkg = found
            .iter()
            .find(|f| f.line.contains("src/pkg "))
            .unwrap_or_else(|| panic!("no finding for src/pkg in {found:#?}"));
        assert_eq!(
            pkg.severity,
            Severity::High,
            "landing on cyclic is the rung nothing above it can be worked on"
        );
        assert!(pkg.line.contains("→ cyclic"), "{}", pkg.line);
        assert!(
            pkg.line.contains("c.rs → a.rs"),
            "the edge that closed the loop has to be named: {}",
            pkg.line
        );
    }

    /// The fingerprint carries the transition, so the session state can
    /// tell "fell again, further" from "already reported".
    #[test]
    fn the_fingerprint_names_the_transition_not_just_the_folder() {
        let (base, head) = chain_then_loop();
        let found = shape_findings(&artifacts_of(&base, &head));
        let pkg = found.iter().find(|f| f.line.contains("src/pkg ")).unwrap();
        assert!(pkg.fingerprint.starts_with("shape|src/pkg|"), "{}", pkg.fingerprint);
        assert!(pkg.fingerprint.ends_with("→cyclic"), "{}", pkg.fingerprint);
    }

    /// Silence is the contract. An unchanged tree must produce no shape
    /// finding at all — a folder that was tangled before the edit and is
    /// still tangled is not news, and printing standing shape on every
    /// stop would bury the findings that are.
    #[test]
    fn an_unchanged_tree_reports_no_shape_finding() {
        let (base, _) = chain_then_loop();
        let same = TmpDir::new("same");
        for rel in ["src/pkg/mod.rs", "src/pkg/a.rs", "src/pkg/b.rs", "src/pkg/c.rs"] {
            same.write(rel, &std::fs::read_to_string(base.0.join(rel)).unwrap());
        }
        let found = shape_findings(&artifacts_of(&base, &same));
        assert!(found.is_empty(), "identical trees must be silent: {found:#?}");
    }

    /// A tier that *rises* is not a finding. The hook reports
    /// regressions; an improvement reaching it would spend one of ten
    /// lines telling an agent its work went well.
    #[test]
    fn a_folder_that_improved_is_not_a_finding() {
        let (base, head) = chain_then_loop();
        // Swapped: the loop is the base and the chain is the head.
        let found = shape_findings(&artifacts_of(&head, &base));
        assert!(found.is_empty(), "an improvement is not a regression: {found:#?}");
    }

    #[test]
    fn silent_when_no_findings_and_none_seen() {
        let (out, next) = diff_against_state(&[], &SessionState::new(), DEFAULT_LINE_CAP);
        assert_eq!(out, "", "clean run must be zero bytes");
        assert!(next.is_empty());
    }

    #[test]
    fn finding_reported_once_then_silent_on_repeat() {
        let current = vec![f(
            "cx|Function|foo|src/a.rs|",
            Severity::High,
            "complexity +12: Function foo — src/a.rs",
        )];
        // First run: new finding is printed, enters state.
        let (out1, state1) = diff_against_state(&current, &SessionState::new(), DEFAULT_LINE_CAP);
        assert!(
            out1.contains("⚠ complexity +12"),
            "first run must print it:\n{out1}"
        );
        assert!(state1.contains_key("cx|Function|foo|src/a.rs|"));
        // Second run, same finding still present: silent.
        let (out2, state2) = diff_against_state(&current, &state1, DEFAULT_LINE_CAP);
        assert_eq!(out2, "", "unchanged finding must not reprint:\n{out2}");
        assert_eq!(state1, state2);
    }

    #[test]
    fn resolved_finding_prints_once() {
        let seen: SessionState = [(
            "cycle|Struct|Bar|src/b.rs|".to_string(),
            "new cycle: Struct Bar — src/b.rs".to_string(),
        )]
        .into_iter()
        .collect();
        // Finding gone from current → one resolved line, dropped from state.
        let (out, next) = diff_against_state(&[], &seen, DEFAULT_LINE_CAP);
        assert!(
            out.contains("✓ resolved: new cycle: Struct Bar"),
            "resolved line missing:\n{out}"
        );
        assert!(next.is_empty(), "resolved finding must leave state");
        // Next run: nothing to say.
        let (out2, _) = diff_against_state(&[], &next, DEFAULT_LINE_CAP);
        assert_eq!(out2, "");
    }

    #[test]
    fn output_never_exceeds_cap_and_points_to_assess_change() {
        let current: Vec<Finding> = (0..20)
            .map(|i| {
                f(
                    &format!("cx|Function|f{i:02}|src/a.rs|"),
                    Severity::High,
                    &format!("complexity +{}: f{i}", 20 - i),
                )
            })
            .collect();
        let cap = 10;
        let (out, next) = diff_against_state(&current, &SessionState::new(), cap);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), cap, "must cap at exactly {cap} lines");
        assert!(
            lines.last().unwrap().contains("assess_change"),
            "overflow must point to assess_change"
        );
        // Only the emitted (cap-1) findings enter state; the rest resurface.
        assert_eq!(
            next.len(),
            cap - 1,
            "un-emitted findings must not be marked seen"
        );
    }

    #[test]
    fn severity_parse_roundtrip() {
        assert_eq!(Severity::parse("LOW"), Some(Severity::Low));
        assert_eq!(Severity::parse("medium"), Some(Severity::Medium));
        assert_eq!(Severity::parse("High"), Some(Severity::High));
        assert_eq!(Severity::parse("bogus"), None);
        assert!(Severity::Low < Severity::Medium && Severity::Medium < Severity::High);
    }
}
