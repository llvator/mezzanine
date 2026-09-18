//! Implementations of the MCP tools: `map`, `quality`, `assess_change`.
//!
//! Each returns compact markdown-ish text, not raw graph JSON — MCP tool
//! output lands in an agent's context window, so responses rank, cap, and
//! annotate rather than dump. Truncation is always stated explicitly.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::analyzer::TestPaths;
use crate::diff;
use crate::graph::DependencyGraph;
use crate::models::{
    CodeEntity, EntityKind, FolderShape, Precision, Relationship, RelationshipKind,
    ShapePattern, SmellKind,
};
use crate::Analyzer;

use super::answer::{self, Answer};
use super::census::FolderCensus;
use super::chains::Direction;
use super::externals::Externals;
use super::McpServer;

/// Line budget for a single tool response body.
const MAX_BODY_LINES: usize = 400;

// ------------------------------------------------------------------
//  Shared helpers
// ------------------------------------------------------------------

/// True if the entity belongs in an agent-facing structural listing.
/// Excludes ghost entities (unresolved call targets with no real file)
/// and kinds that are synthetic visualization aids or too fine-grained.
pub(crate) fn is_listed(e: &CodeEntity) -> bool {
    !e.tags.contains("ghost")
        && !matches!(
            e.kind,
            EntityKind::Parameter
                | EntityKind::Branch
                | EntityKind::Loop
                | EntityKind::Import
                | EntityKind::Variable
                | EntityKind::File
        )
}

/// True when the entity is test code: its file matches the walker's test
/// heuristic, or (Rust inline `#[cfg(test)]` convention) an ancestor
/// module's name marks it as tests — those live inside regular source
/// files the path heuristic cannot see. The name rule is the same
/// substring match [`TestPaths`] applies to paths, so `mod tests` and
/// `mod locality_tests` classify alike.
///
/// `tests` is passed in rather than built here: it resolves the repo root
/// off the filesystem, and this is asked once per entity over graphs of
/// tens of thousands. Any path inside the checkout roots it correctly, so
/// callers hand it whatever root they already hold.
pub(crate) fn is_test_entity(graph: &DependencyGraph, e: &CodeEntity, tests: &TestPaths) -> bool {
    if tests.matches(Path::new(&e.file_path)) {
        return true;
    }
    let is_test_module = |x: &CodeEntity| {
        let name = x.name.to_lowercase();
        x.kind == EntityKind::Module && (name.contains("test") || name.contains("spec"))
    };
    let mut cur = Some(e);
    for _ in 0..16 {
        match cur {
            Some(x) if is_test_module(x) => return true,
            Some(x) => cur = parent_of(graph, x),
            None => break,
        }
    }
    false
}

/// The synthetic scope nodes a callable's body is cut into.
///
/// A call written inside an `if` or a `for` is not attached to the function
/// containing it: the analyzer reattaches it to the `Branch` or `Loop` node,
/// so the edge on the wire is `branch → callee`. Reading edges off the
/// callable alone therefore misses it, and dropping unlisted endpoints drops
/// it — which is the same fact from the two ends.
pub(super) fn is_body_scope(e: &CodeEntity) -> bool {
    matches!(e.kind, EntityKind::Branch | EntityKind::Loop)
}

/// The nearest ancestor a listing may name, or `None` when there is none.
///
/// Field report, 2026-08-31: `impact` on a function called twice from inside
/// a `for` loop reported `Used by (0)`, while its own header said `in 2` —
/// the metric counted the edges and the listing filtered their source out
/// without traversing through it. Ten of one function's twelve callees were
/// invisible for this reason, and in most real code the majority of call
/// sites are inside a branch or a loop.
///
/// The browser UI has solved this since UI-113: `liftBodies` re-routes every
/// edge with an end inside a body to the enclosing callable, and measured
/// 1 100 calls on this repo that would otherwise silently vanish. This is
/// that rule, on the MCP side, which never had it.
///
/// Bounded, because a `parent_id` chain that points at itself is cheaper to
/// cap than to prove impossible — the same reasoning and the same depth as
/// the UI's.
/// `pub(crate)` because `mezz deps --reverse` asks the same question one
/// grain coarser — which *files* depend on this one — and a caller lost to an
/// unlifted branch node is lost there too.
pub(crate) fn lifted<'g>(graph: &'g DependencyGraph, e: &'g CodeEntity) -> Option<&'g CodeEntity> {
    let mut cur = e;
    for _ in 0..64 {
        if is_listed(cur) {
            return Some(cur);
        }
        cur = parent_of(graph, cur)?;
    }
    None
}

/// The ids a call written *in* `target` actually leaves from: the target
/// itself plus the body-scope nodes nested inside it.
///
/// The outgoing half of the same defect. Lifting an endpoint fixes
/// `Used by`, because there the body node is the edge's *source* and can be
/// walked up from. It cannot fix `Uses`: those edges never touch the target's
/// id at all, so there is nothing to lift — they have to be gathered.
pub(super) fn body_scope_ids(graph: &DependencyGraph, target: &CodeEntity) -> Vec<String> {
    let mut ids = vec![target.id.clone()];
    let mut frontier = vec![target.id.clone()];
    for _ in 0..64 {
        let mut next = Vec::new();
        for id in &frontier {
            for child in graph.children(id).into_iter().filter(|c| is_body_scope(c)) {
                ids.push(child.id.clone());
                next.push(child.id.clone());
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    ids
}

/// What the target relies on, in reading order, with the targets that are
/// not code in this repo tallied beside it.
///
/// Read from the target *and* the branch/loop nodes its body is cut into: a
/// call written inside an `if` leaves from the branch, so those edges never
/// touch the target's own id. There is nothing to lift here — they have to
/// be gathered (see [`body_scope_ids`]).
///
/// The second half used to be `raw - uses.len()`, printed as *plus N
/// external/unresolved targets not listed*. That subtraction counted every
/// dropped edge, parameters and body scopes included, and merged a library
/// call with a parser miss — so it over-reported, and what it reported it
/// would not name (MCP-039). The ghosts are gathered instead, and
/// [`Externals`] places them.
fn uses_of<'g>(
    graph: &'g DependencyGraph,
    target: &CodeEntity,
) -> (Vec<(&'g CodeEntity, String)>, Externals<'g>) {
    let all: Vec<_> = body_scope_ids(graph, target)
        .iter()
        .flat_map(|id| graph.dependencies(id))
        .collect();
    let mut outside = Externals::of_file(graph, &target.file_path);
    outside.gather(&all);
    let mut uses: Vec<(&CodeEntity, String)> = all
        .into_iter()
        .filter(|(e, _)| is_listed(e))
        .map(|(e, r)| (e, edge_label(r)))
        .collect();
    uses.sort_by_key(|(e, _)| (e.file_path.clone(), e.span.start.line));
    uses.dedup_by_key(|(e, l)| (e.id.clone(), l.clone()));
    (uses, outside)
}

/// The callables that depend on the target, in reading order.
///
/// A dependent whose call sits in a branch or a loop arrives as that scope
/// node. It is lifted to the callable a reader can actually go and edit,
/// rather than dropped for not being listed — which is what produced
/// `Used by (0)` beside a header reading `in 2` (see [`lifted`]).
///
/// The target is excluded from its own dependents: a call in one arm of a
/// function to another part of itself lifts to the function, and "this
/// function depends on itself" is not a row anyone can act on.
fn dependents_of<'g>(
    graph: &'g DependencyGraph,
    target: &CodeEntity,
) -> Vec<(&'g CodeEntity, String)> {
    let mut used_by: Vec<(&CodeEntity, String)> = graph
        .dependents(&target.id)
        .into_iter()
        .filter_map(|(e, r)| Some((lifted(graph, e)?, edge_label(r))))
        .filter(|(e, _)| e.id != target.id)
        .collect();
    used_by.sort_by_key(|(e, _)| (e.file_path.clone(), e.span.start.line));
    used_by.dedup_by_key(|(e, l)| (e.id.clone(), l.clone()));
    used_by
}

/// An entity's parent, by the `parent_id` the parser recorded, falling back
/// to the `Contains` edge.
///
/// Not `graph.parent` alone. That reads the edge, and a **single-file**
/// analysis — `quality`/`map` with a file for `path`, which is how an agent
/// narrows to one file — carries the entities without their `Contains`
/// relationships, so every entity in it reports no parent. That is what
/// made `quality(path: "src/activity.rs")` list seven `#[test]` functions
/// as production smells while `quality(path: "src")` over the same file
/// counted them as test code (field report, 2026-08-31). `parent_id` is
/// populated either way, which is why `map`'s own top-level test reads it.
pub(super) fn parent_of<'g>(graph: &'g DependencyGraph, e: &CodeEntity) -> Option<&'g CodeEntity> {
    e.parent_id
        .as_deref()
        .and_then(|id| graph.get_entity(id))
        .or_else(|| graph.parent(&e.id))
}

/// The graph and the rooted heuristic together, so a caller can ask whether
/// an entity is test code without carrying two things to ask it with.
///
/// Built once per report: [`TestPaths::rooted_at`] climbs the filesystem for
/// a repo root, and this question is asked once per entity.
pub(crate) struct TestCode<'a> {
    graph: &'a DependencyGraph,
    paths: TestPaths,
}

impl<'a> TestCode<'a> {
    /// Rooted at any path inside the checkout — they all resolve to the
    /// same repo root.
    pub(crate) fn of(graph: &'a DependencyGraph, root: &Path) -> Self {
        Self {
            graph,
            paths: TestPaths::rooted_at(root),
        }
    }

    /// Whether this entity is test code. `None` — a row whose entity is in
    /// neither graph — is not test code, which keeps an unjoinable row
    /// visible rather than silently filed under the count.
    pub(crate) fn holds(&self, e: Option<&CodeEntity>) -> bool {
        e.is_some_and(|e| is_test_entity(self.graph, e, &self.paths))
    }
}

/// Why smells on test code are counted rather than listed.
///
/// A unit test exists to exercise one type, so it interacts more with that
/// type than with itself — which is the exact shape Feature Envy fires on,
/// and a shape a *correct* test cannot avoid. Two field reports (2026-08-31)
/// reached this from `quality` and from `assess_change`: seven of seven new
/// smells on `#[test]` functions in one file, none on the nineteen
/// production functions above them, each row carrying the hint "move this
/// method to the type it mostly interacts with". Followed, that instruction
/// damages a passing test suite to silence a warning about nothing; behind a
/// hook that exits 2, following it is the cheapest way out of a blocked turn.
///
/// The second-order cost is the one that decides it: a ⚠ section that is
/// mostly noise teaches the reader to skim the section where a real smell
/// would appear. And it inverts the signal — a file's smell count would rise
/// with how well it is tested.
///
/// Counted, never silently dropped. A section that quietly shrinks is the
/// failure this whole queue is otherwise about.
fn smells_aside(shown: usize, in_tests: usize, label: &str) -> String {
    match in_tests {
        0 => format!("{label} ({shown})"),
        n => format!("{label} ({shown}, plus {n} in test code — not listed)"),
    }
}

/// The `, test` a row carries when it is test code, and nothing when it is
/// not.
///
/// `quality`'s refactor-pressure ranking keeps test entities — a slow or
/// tangled test suite is worth ranking — but a row reading `⚠ Feature Envy`
/// beside a `#[test]` function is one a reader acts on wrongly. The
/// reporter asked for exactly this token, in exactly this position
/// (2026-08-31).
fn test_marker(tests: &TestCode, e: &CodeEntity) -> &'static str {
    match tests.holds(Some(e)) {
        true => ", test",
        false => "",
    }
}

/// The smelly production entities, worst composite score first, and how
/// many the test code beside them would have added — see [`smells_aside`].
pub(crate) fn production_smells<'g>(
    graph: &'g DependencyGraph,
    tests: &TestCode,
) -> (Vec<&'g CodeEntity>, usize) {
    let smelly = graph
        .entities()
        .filter(|e| is_listed(e) && !e.metrics.smells.is_empty());
    let (mut production, in_tests): (Vec<&CodeEntity>, Vec<&CodeEntity>) =
        smelly.partition(|e| !tests.holds(Some(e)));
    production.sort_by(|a, b| {
        b.metrics
            .composite_score
            .total_cmp(&a.metrics.composite_score)
    });
    (production, in_tests.len())
}

/// Resolve the optional `path` argument against the server root.
fn resolve_path(server: &McpServer, args: &Value) -> Result<PathBuf> {
    let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let candidate = if raw.is_empty() {
        server.root.clone()
    } else {
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            p
        } else {
            server.root.join(p)
        }
    };
    candidate
        .canonicalize()
        .with_context(|| format!("Path not found: {}", candidate.display()))
}

/// Analyze a directory (or single file) into a dependency graph.
pub(super) fn analyze(server: &McpServer, path: &Path) -> Result<Arc<DependencyGraph>> {
    analyze_with_tests(server, path, server.include_tests)
}

/// Like `analyze`, but with an explicit test-inclusion override —
/// `tests_for` must see test entities even in a default session.
///
/// Serves from the warm cache when the generation is unchanged
/// (MCP-006); a result computed while the tree was being edited is
/// returned but not cached, so no torn snapshot can ever be served.
fn analyze_with_tests(
    server: &McpServer,
    path: &Path,
    include_tests: bool,
) -> Result<Arc<DependencyGraph>> {
    let key = (path.to_path_buf(), include_tests);
    let (dir, single_file) = if path.is_file() {
        (path.parent().unwrap_or(path).to_path_buf(), Some(path))
    } else {
        (path.to_path_buf(), None)
    };
    // The settings file is the *repo's*, so it is read at `server.root` and
    // the result re-rooted at whatever subdirectory this call asked for.
    // Building it at `dir` instead would leave `map path="src"` — the
    // ordinary way an agent narrows — reading a `.mezz/settings.json` that
    // has to sit under `src/` to exist at all (CFG-011).
    //
    // Built before the cache is consulted rather than after, because the
    // scope it resolves to is half the cache key: `rooted_at` moves
    // `root_path` only, which `scope_fingerprint` does not read, so the
    // digest is the server's scope for this `include_tests` either way.
    let config = diff::rooted_at(
        &diff::build_analysis_config(&server.root, include_tests, &server.languages),
        &dir,
    );
    let scope = super::scope_id(&config);
    let gen_at_start = server.generation.load(std::sync::atomic::Ordering::Acquire);
    if let Some(hit) = server.graph_cache.lock().unwrap().get(&key) {
        if hit.serves(gen_at_start, &scope) {
            eprintln!("⚡ graph cache hit for {}", path.display());
            return Ok(hit.graph.clone());
        }
    }

    let mut analyzer = Analyzer::new(config);
    let result = match single_file {
        Some(f) => analyzer.analyze_file(f)?,
        None => analyzer.analyze()?,
    };
    let graph = Arc::new(DependencyGraph::from_analysis(&result));

    if server.generation.load(std::sync::atomic::Ordering::Acquire) == gen_at_start {
        let mut cache = server.graph_cache.lock().unwrap();
        if cache.len() >= 8 {
            cache.clear();
        }
        cache.insert(
            key,
            crate::mcp::CachedGraph {
                graph: graph.clone(),
                gen: gen_at_start,
                scope,
            },
        );
    }
    Ok(graph)
}

/// `pub(super)` because the push-mode findings join two graphs by
/// path and the base one lives in a throwaway worktree, so it has the
/// same root-stripping problem this solves for the reports here.
pub(crate) fn rel_path(file: &Path, base: &Path) -> String {
    // When the analyzed target is itself a file, relativize against its
    // directory so the file keeps its name instead of becoming "".
    let base = if base.is_file() {
        base.parent().unwrap_or(base)
    } else {
        base
    };
    file.strip_prefix(base)
        .unwrap_or(file)
        .display()
        .to_string()
}

/// One node, the way a chain prints it: what it is, what it is called, and
/// where to open it.
///
/// Shared by `trace` and [`super::radius`] rather than written twice. A
/// second node renderer is how two chains of the same graph come to spell
/// the same entity differently, and a reader comparing two reports reads
/// that as two entities.
pub(super) fn render_node(e: &CodeEntity, root: &Path) -> String {
    if e.tags.contains("ghost") {
        return format!("`{}` (external)", e.name);
    }
    format!(
        "{} `{}` ({}:{})",
        e.kind.display_name(),
        e.name,
        rel_path(&e.file_path, root),
        e.span.start.line + 1
    )
}

/// Edge label carrying its AN-004 precision marker, e.g. `calls ·exact` /
/// `calls ·heuristic`. Only call edges carry precision; every other edge
/// renders its plain label. Surfaces the exact-vs-heuristic distinction the
/// agent needs to decide whether to trust a blast radius or fall back to grep.
pub(super) fn edge_label(r: &Relationship) -> String {
    match (r.kind, r.precision) {
        (RelationshipKind::Calls, Some(p)) => {
            format!("{} ·{}", r.kind.display_label(), p.marker())
        }
        _ => r.kind.display_label().to_string(),
    }
}

fn smell_labels(smells: &[SmellKind]) -> String {
    smells
        .iter()
        .map(|s| s.label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Compact one-line metric annotation for an entity.
/// How much of this row's `out` mezz actually identified.
///
/// The count a reader wants and `fan_out` hides. On this repo the median
/// entity with `fan_out >= 5` has **71%** of it landing on names mezz could
/// not resolve to anything it has seen — a `std` call, a library method, a
/// link in a chain against an external type. `render_index` scores `out
/// 98`, of which 6 are things mezz identified.
///
/// Written as the identified count rather than as a warning marker,
/// because a marker needs a threshold and there is no honest one here: the
/// share runs 0% to 100% across the repo but sits near the top for
/// *every* high-pressure row, so a threshold fires on all of them and
/// discriminates nothing. The bare number varies, and it is the one that
/// answers "how many things must I understand to change this".
///
/// Reported and not corrected. Telling a builder chain from four genuine
/// dependencies on unidentified things needs the receiver, and an external
/// chain records none (AN-031) — so the correction is a parser change, and
/// a scoring change made without one would re-rank every repo while
/// leaving that exact case alone.
fn unresolved_note(e: &CodeEntity, unresolved: u32) -> String {
    if e.metrics.fan_out == 0 || unresolved == 0 {
        return String::new();
    }
    format!(
        ", {} of it identified",
        e.metrics.fan_out.saturating_sub(unresolved)
    )
}

/// `ws N` for a callable the parser measured, nothing for one it did not.
///
/// Printed for every callable, not only over the line: `ws 3` beside
/// `cog 40` is the reading that says the body is branchy but narrow, and a
/// metric shown only when it is bad cannot make that distinction.
fn working_set_part(m: &crate::models::entity::EntityMetrics) -> Option<String> {
    m.working_set.map(|w| format!("ws {w}"))
}

pub(crate) fn metric_suffix(e: &CodeEntity) -> String {
    let m = &e.metrics;
    let mut parts = vec![
        format!("L{}", e.span.start.line + 1),
        format!("loc {}", m.loc),
    ];
    if let Some(c) = m.cyclomatic {
        parts.push(format!("cx {}", c));
    }
    if let Some(c) = m.cognitive_complexity {
        parts.push(format!("cog {}", c));
    }
    parts.extend(working_set_part(m));
    if m.method_count > 0 {
        parts.push(format!("methods {}", m.method_count));
    }
    if m.fan_in > 0 {
        parts.push(format!("in {}", m.fan_in));
    }
    if m.fan_out > 0 {
        parts.push(format!("out {}", m.fan_out));
    }
    if m.in_cycle {
        parts.push("cycle".to_string());
    }
    if !e.metrics.smells.is_empty() {
        parts.push(format!("⚠ {}", smell_labels(&e.metrics.smells)));
    }
    parts.join(", ")
}

/// Collapse a type or name spanning several source lines into one line.
/// TypeScript inline object types are written multi-line and idiomatically
/// unbounded, and they reach us verbatim from the parser.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Trim a rendered fragment to `budget` characters, marking the cut. The
/// line-level counterpart of `cap_lines`: same rule that truncation is
/// always visible, applied inside a line rather than across a body.
fn cap_chars(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let kept: String = text.chars().take(budget).collect();
    format!("{}…", kept.trim_end())
}

/// Trim a body to the line budget, appending an explicit truncation note.
pub(super) fn cap_lines(body: Vec<String>, hint: &str) -> String {
    if body.len() <= MAX_BODY_LINES {
        return body.join("\n");
    }
    let shown = &body[..MAX_BODY_LINES];
    format!(
        "{}\n… truncated: {} more lines. {}",
        shown.join("\n"),
        body.len() - MAX_BODY_LINES,
        hint
    )
}

// ------------------------------------------------------------------
//  map
// ------------------------------------------------------------------

pub fn map(server: &McpServer, args: &Value) -> Result<Answer> {
    let path = resolve_path(server, args)?;
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .clamp(1, 3);

    // The whole root, then filtered to `path` — not an analysis of `path`.
    // Coupling metrics are computed over whatever graph was built, so a graph
    // built from one folder cannot see the callers outside it and reports
    // every exported function there as uncoupled. `impact` avoids this by
    // always analysing the root, and `dead_code` documents the same trap;
    // `map` had it, and `map` is the tool people are told to reach for first
    // on unfamiliar code (MCP-021).
    let graph = analyze(server, &server.root)?;

    // Counted off the disk as well as out of the graph: a listing smaller
    // than the folder that does not say so is read as the folder — see
    // [`census`].
    let census = FolderCensus::of(server, &graph, &path);
    let mut body = census.header(&path);
    body.extend(census.rows(&graph, depth));

    // Both renderings read the one census (ADR 0035). `depth` is the
    // caller's bound and so applies to both; `cap_lines` is the prose's
    // own reading budget and applies to neither the census nor the JSON.
    let data = census.as_json(&graph, &path, &server.root, depth);

    Ok(Answer::structured(
        cap_lines(
            body,
            "Call `map` again with a narrower `path` or a smaller `depth`.",
        ),
        data,
    ))
}

// ------------------------------------------------------------------
//  quality
// ------------------------------------------------------------------

pub fn quality(server: &McpServer, args: &Value) -> Result<Answer> {
    let path = resolve_path(server, args)?;
    let top = args
        .get("top")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .clamp(1, 50) as usize;

    let graph = analyze(server, &path)?;
    let tests = TestCode::of(&graph, &path);
    let entity_row = |e: &CodeEntity| {
        format!(
            "- {} `{}` — {}:{} ({}{})",
            e.kind.display_name(),
            e.name,
            rel_path(&e.file_path, &path),
            e.span.start.line + 1,
            metric_suffix(e),
            test_marker(&tests, e),
        )
    };

    let mut body = vec![format!("# Quality of {}", path.display()), String::new()];

    // Smells, worst composite score first — test code counted apart, for
    // the reasons in [`smells_aside`].
    let (smelly, smells_in_tests) = production_smells(&graph, &tests);
    body.push(format!(
        "## {}",
        smells_aside(smelly.len(), smells_in_tests, "Smells")
    ));
    let mut hints_used: HashSet<SmellKind> = HashSet::new();
    for e in smelly.iter().take(top) {
        body.push(entity_row(e));
        for s in &e.metrics.smells {
            hints_used.insert(*s);
        }
    }
    if smelly.len() > top {
        body.push(format!(
            "… and {} more smelly entities.",
            smelly.len() - top
        ));
    }
    if !hints_used.is_empty() {
        body.push(String::new());
        body.push("Remediation hints:".to_string());
        let mut hints: Vec<&SmellKind> = hints_used.iter().collect();
        hints.sort_by_key(|s| s.label());
        for s in hints {
            body.push(format!("- {}: {}", s.label(), s.hint()));
        }
    }

    // Top offenders by composite "refactor pressure" score.
    body.push(String::new());
    body.push(format!("## Top {} by refactor pressure", top));
    // Said here rather than left to the reader, because the shape section
    // below is the one with the imperative heading and gets read as the plan.
    body.push(
        "A ranking, not a verdict: this list does not say which of these are worth \
         changing. A high score can be inherent to a shape that is fine — a builder \
         chain scores its every link as a dependency — so read the entity before \
         acting on its place here. Nothing in this list is excused by the folder \
         work below, and nothing there covers this."
            .to_string(),
    );
    body.push(
        "`out N, M of it identified` is what that disclaimer looks like per row: the \
         other N−M are names mezz could not resolve to anything it has seen — a \
         library call, a `std` call, a link in a chain against an external type. The \
         score counts all N. A row whose `out` is large and whose identified count is \
         small is mentioning a lot of names, not depending on a lot of things."
            .to_string(),
    );
    let unresolved = graph.unresolved_fan_out();
    let mut ranked: Vec<&CodeEntity> = graph
        .entities()
        .filter(|e| is_listed(e) && e.metrics.composite_score > 0.0)
        .collect();
    ranked.sort_by(|a, b| {
        b.metrics
            .composite_score
            .total_cmp(&a.metrics.composite_score)
    });
    for e in ranked.iter().take(top) {
        body.push(format!(
            "- [{:.2}] {} `{}` — {}:{} ({}{}{})",
            e.metrics.composite_score,
            e.kind.display_name(),
            e.name,
            rel_path(&e.file_path, &path),
            e.span.start.line + 1,
            metric_suffix(e),
            unresolved_note(e, unresolved.get(e.id.as_str()).copied().unwrap_or(0)),
            test_marker(&tests, e),
        ));
    }

    // Dependency cycles. SCCs from the raw graph can run through synthetic
    // nodes (parameters, branches); keep only cycles with 2+ listed members.
    let cycles: Vec<Vec<&CodeEntity>> = graph
        .find_cycles()
        .iter()
        .map(|cycle| {
            cycle
                .iter()
                .filter_map(|id| graph.get_entity(id))
                .filter(|e| is_listed(e))
                .collect::<Vec<_>>()
        })
        .filter(|members| members.len() >= 2)
        .collect();
    body.push(String::new());
    body.push(format!("## Dependency cycles ({})", cycles.len()));
    for members in cycles.iter().take(SHOWN_CYCLES) {
        let names: Vec<&str> = members
            .iter()
            .take(NAMED_CYCLE_MEMBERS)
            .map(|e| e.name.as_str())
            .collect();
        let ellipsis = if members.len() > NAMED_CYCLE_MEMBERS {
            " → …"
        } else {
            ""
        };
        body.push(format!(
            "- {} → {}{}",
            names.join(" → "),
            names[0],
            ellipsis
        ));
    }
    if cycles.len() > SHOWN_CYCLES {
        body.push(format!(
            "… and {} more cycles.",
            cycles.len() - SHOWN_CYCLES
        ));
    }

    // Scored once, read by both renderings — the shape section is the one
    // part of this report whose value took its own pass over the graph.
    let shapes = folder_shapes(&graph, &path);
    body.extend(folder_shape_section(&shapes, &graph, &path, top));

    let facts = QualityFacts {
        path: &path,
        root: &server.root,
        top,
        smelly: &smelly,
        smells_in_tests,
        ranked: &ranked,
        unresolved: &unresolved,
        cycles: &cycles,
        shapes: &shapes,
        tests: &tests,
        graph: &graph,
    };
    Ok(Answer::structured(
        cap_lines(body, "Call `quality` with a narrower `path`."),
        facts.as_json(),
    ))
}

/// How many cycles the prose names, and how many members of each. Both are
/// reading budgets: a report that lists forty rings is not read, and the
/// count above them is the part that carries the message. JSON carries
/// every ring and every member (ADR 0035).
const SHOWN_CYCLES: usize = 5;
const NAMED_CYCLE_MEMBERS: usize = 8;

/// What `quality` selected, held together so both renderings read the one
/// selection (ADR 0035).
///
/// A parameter object rather than ten arguments, and borrowed throughout:
/// the value here *is* which entities were picked and in what order, not
/// a copy of them.
struct QualityFacts<'g> {
    path: &'g Path,
    root: &'g Path,
    top: usize,
    smelly: &'g [&'g CodeEntity],
    smells_in_tests: usize,
    ranked: &'g [&'g CodeEntity],
    unresolved: &'g HashMap<&'g str, u32>,
    cycles: &'g [Vec<&'g CodeEntity>],
    shapes: &'g [(String, &'g FolderShape)],
    tests: &'g TestCode<'g>,
    graph: &'g DependencyGraph,
}

impl QualityFacts<'_> {
    /// The report as fields. Every section the prose has, and the counts
    /// it states in its headings as numbers rather than inside them.
    fn as_json(&self) -> Value {
        json!({
            "path": answer::scope_path(self.path, self.root),
            "top": self.top,
            "smells": {
                "entities": self.rows(self.smelly.iter().take(self.top)),
                "shown": self.smelly.len().min(self.top),
                "total": self.smelly.len(),
                "in_test_code": self.smells_in_tests,
            },
            "pressure": {
                "entities": self.pressure_rows(),
                "shown": self.ranked.len().min(self.top),
                "total": self.ranked.len(),
            },
            "cycles": {
                "cycles": self.cycles
                    .iter()
                    .map(|members| json!({
                        "members": self.rows(members.iter()),
                    }))
                    .collect::<Vec<_>>(),
                "total": self.cycles.len(),
            },
            "folder_shape": folder_shape_json(self.shapes, self.graph, self.path, self.top),
        })
    }

    /// Entity rows, each carrying the `, test` marker the prose puts on it.
    fn rows<'e>(&self, entities: impl Iterator<Item = &'e &'e CodeEntity>) -> Vec<Value> {
        entities
            .map(|e| {
                let mut row = answer::entity_json(e, self.path);
                row["test_code"] = json!(self.tests.holds(Some(e)));
                row
            })
            .collect()
    }

    /// The pressure rows, plus the two numbers that row's prose carries
    /// and no other row's does: the score it is ranked by, and how much of
    /// its `fan_out` mezz actually identified — see [`unresolved_note`],
    /// which says the same thing in words.
    fn pressure_rows(&self) -> Vec<Value> {
        self.ranked
            .iter()
            .take(self.top)
            .map(|e| {
                let mut row = answer::entity_json(e, self.path);
                row["test_code"] = json!(self.tests.holds(Some(e)));
                row["pressure"] = json!(e.metrics.composite_score);
                row["identified_fan_out"] = json!(e.metrics.fan_out.saturating_sub(
                    self.unresolved.get(e.id.as_str()).copied().unwrap_or(0)
                ));
                row
            })
            .collect()
    }
}

/// Folders whose shape tier moved between the two graphs.
///
/// This is what closes the loop `reshape` opens: it proposes one rung, and
/// the only honest confirmation that the rung was climbed is the tier
/// moving when measured the same way. Silent when nothing moved, which is
/// the normal case for a change that was not about organisation.
///
/// Regressions lead. A folder that fell a tier is news whether or not the
/// change set out to touch its shape, and it is the half a reader would
/// otherwise never look for.
fn shape_moves(
    base_graph: &DependencyGraph,
    head_graph: &DependencyGraph,
    roots: (&Path, &Path),
) -> Vec<String> {
    // Keyed on the path each side reports relative to its own root: the
    // base lives in a worktree, so the raw paths never match.
    let by_folder = |graph: &DependencyGraph, root: &Path| -> HashMap<String, ShapePattern> {
        graph
            .folder_metrics()
            .iter()
            .filter_map(|m| {
                let shape = m.metrics.shape.as_ref()?;
                Some((rel_path(Path::new(&m.path), root), shape.pattern))
            })
            .collect()
    };
    let before = by_folder(base_graph, roots.0);
    let after = by_folder(head_graph, roots.1);

    let mut moves: Vec<(String, ShapePattern, ShapePattern)> = after
        .iter()
        .filter_map(|(path, now)| {
            let was = *before.get(path)?;
            (was != *now).then(|| (path.clone(), was, *now))
        })
        .collect();
    if moves.is_empty() {
        return Vec::new();
    }
    // Worse first, then by how far it fell, then by path so two identical
    // runs print identically.
    moves.sort_by(|a, b| {
        let fell = |m: &(String, ShapePattern, ShapePattern)| m.2 < m.1;
        fell(b)
            .cmp(&fell(a))
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
    });

    let mut body = vec![
        String::new(),
        format!("## Folder shape moved ({})", moves.len()),
    ];
    for (path, was, now) in &moves {
        let arrow = if now < was { "⚠ fell" } else { "improved" };
        body.push(format!(
            "- {} {}: {} → {}",
            if path.is_empty() { "(root)" } else { path },
            arrow,
            was.label(),
            now.label(),
        ));
    }
    body.push(
        "_Higher is better: cyclic < tangled < hierarchical < fractal. Call \
         `reshape` on a folder to see the graph behind its verdict._"
            .to_string(),
    );
    body
}

/// How readable a picture each folder in the assessed area draws, and
/// which of them is worth sending anybody at.
///
/// Ranked lists of the folders short of `fractal`, because the ones
/// already there need no action and listing them would bury the ones that
/// do. The tally line above the list is what says how much was left out.
///
/// Split in two before it is ranked, which is the part that matters for
/// planning. Worst-first alone answers "where is the organisation worst"
/// — the question `quality` exists for — and it is the wrong order to
/// *work* in. The ladder is recursive: a folder held back by a tangled
/// subfolder cannot move until that subfolder does, so an agent sent at
/// it spends the session against a gate that will not open however well
/// it reads the parent's drawing. The folders whose blocker is in their
/// own drawing go first, deepest-first, because clearing a deep one can
/// clear a parent's child gate as well as its own.
/// The files that changed place, named as such.
///
/// Its own section because a relocation is neither a modification nor an
/// addition, and the diff deliberately reports it as neither: the entity
/// pass matches a moved file to its old self so its smells and its history
/// carry across (see `diff::match_moved`). Without this section the same
/// fix would leave `assess_change` reporting "0 changed" over a folder
/// restructure — the one change a reviewer most wants named, and the one
/// this whole family of tools exists to encourage.
///
/// Reported per *file*, not per entity: `git mv` moves files, and twelve
/// properties of one interface are one move.
///
/// Counted over the listed population only. Unfiltered, a single entity no
/// list shows — a ghost call target, an import, a field — was enough to
/// assert that a file had moved when nothing a reader could see had. A
/// claim about the tree has to rest on evidence the reader can check.
fn moved_files(
    result: &diff::DiffResult,
    skip: &dyn Fn(&diff::EntityDiff) -> bool,
) -> Vec<String> {
    let mut moves: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for d in result.entities.iter().filter(|d| !skip(d)) {
        if let Some(was) = d.moved_from.as_deref() {
            *moves
                .entry(was)
                .or_default()
                .entry(d.file_path.as_str())
                .or_default() += 1;
        }
    }
    if moves.is_empty() {
        return Vec::new();
    }
    let split = moves.values().filter(|to| to.len() > 1).count();
    let mut body = vec![
        String::new(),
        format!("## Moved ({})", moves.len()),
        String::new(),
        "Matched to their old selves — entity by entity, so anything below about \
         smells, metrics or coupling is a claim about the same entities and not \
         about their arrival."
            .to_string(),
        String::new(),
    ];
    body.extend(moves.iter().take(MAX_MOVED_FILES).map(destination_line));
    if moves.len() > MAX_MOVED_FILES {
        body.push(format!("- … and {} more.", moves.len() - MAX_MOVED_FILES));
    }
    body.extend(split_caveat(split));
    body
}

/// A removal and an addition that look like one entity under a new name.
///
/// Entity matching pairs a file's contents across a move, and stops at the
/// name: an entity *renamed* as well as relocated has nothing linking the
/// two spellings, so it reads as one deletion plus one arrival. Reported
/// from the field as the boundary the move matching now sits behind — two
/// function bodies lifted from `copy.ts` into `target.ts` under new names,
/// showing up as the only two entries in `Removed`.
///
/// Suggested, never matched. A rename is a judgement about intent and this
/// is a fingerprint; pairing them silently would be the mispairing the
/// split reporting was just fixed for. So the diff keeps calling them an
/// addition and a removal, and this points at the pair.
///
/// Three guards keep it quiet rather than clever: the fingerprint must be
/// distinctive (a one-line delegate is not), and it must be unique on both
/// sides — two candidates for one removal is no candidate at all.
fn possible_renames(
    result: &diff::DiffResult,
    base_by_id: &HashMap<&str, &CodeEntity>,
    head_by_id: &HashMap<&str, &CodeEntity>,
    skip: &dyn Fn(&diff::EntityDiff) -> bool,
) -> Vec<String> {
    let gone = side(result, diff::ChangeStatus::Removed, base_by_id, skip);
    let came = side(result, diff::ChangeStatus::Added, head_by_id, skip);
    let pairs: Vec<String> = gone
        .iter()
        .filter(|(print, _, _)| only_one(&gone, print) && only_one(&came, print))
        .filter_map(|(print, d, e)| {
            let (_, other, _) = came.iter().find(|(p, _, _)| p == print)?;
            Some(format!(
                "`{}` — {} → `{}` — {} (same {}, {} lines)",
                d.name,
                d.file_path,
                other.name,
                other.file_path,
                e.kind.display_name(),
                e.metrics.loc,
            ))
        })
        .collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let mut body = vec![
        String::new(),
        format!("## Possibly renamed ({})", pairs.len()),
        String::new(),
        "Each of these is listed below as both a removal and an addition, because \
         nothing links two names. They are paired here by shape alone — same kind, \
         same parameters, same complexity, same length — which is a hint and not a \
         match. Read them as one entity if that is what they are."
            .to_string(),
        String::new(),
    ];
    body.extend(pairs.into_iter().map(|p| format!("- {p}")));
    body
}

/// Whether exactly one row on this side carries `print`.
///
/// Required on *both* sides before a pair is suggested. Two removals and
/// one addition sharing a shape is an ambiguity, and picking either would
/// be the silent mispairing this whole hint is written to avoid.
fn only_one(
    rows: &[(Fingerprint, &diff::EntityDiff, &CodeEntity)],
    print: &Fingerprint,
) -> bool {
    rows.iter().filter(|(p, _, _)| p == print).count() == 1
}

/// What tells two bodies apart when their names cannot.
///
/// Metrics rather than source text, because a rename changes the text by
/// definition — the identifier is in it.
type Fingerprint = (String, Option<u32>, Option<u32>, Option<u32>, Option<u32>, u32);

/// One side's fingerprinted rows, dropping anything too plain to identify.
fn side<'a>(
    result: &'a diff::DiffResult,
    status: diff::ChangeStatus,
    by_id: &HashMap<&str, &'a CodeEntity>,
    skip: &dyn Fn(&diff::EntityDiff) -> bool,
) -> Vec<(Fingerprint, &'a diff::EntityDiff, &'a CodeEntity)> {
    result
        .entities
        .iter()
        .filter(|d| d.status == status && !skip(d))
        .filter_map(|d| {
            let id = if status == diff::ChangeStatus::Removed {
                d.base_entity_id.as_deref()?
            } else {
                d.entity_id.as_str()
            };
            let e = *by_id.get(id)?;
            Some((fingerprint(e)?, d, e))
        })
        .collect()
}

/// An entity's shape, or `None` when it has too little shape to identify.
///
/// A three-line delegate with no branches matches a hundred others, and a
/// hint that fires on those is one a reader learns to ignore.
fn fingerprint(e: &CodeEntity) -> Option<Fingerprint> {
    let distinctive = e.metrics.loc > 3 || e.metrics.cyclomatic.is_some_and(|c| c > 1);
    distinctive.then(|| {
        (
            e.kind.display_name().to_string(),
            e.metrics.param_count,
            e.metrics.cyclomatic,
            e.metrics.cognitive_complexity,
            e.metrics.max_nesting,
            e.metrics.loc,
        )
    })
}

/// The counts at the top, taken over the rows the lists below actually
/// show.
///
/// They used to come off `DiffResult::summary`, which counts every row the
/// diff produced, while every list under them is filtered by `skip_row` —
/// no fields, no imports, no unresolved call targets, no `File` rows. So
/// the header said "16 added, 9 removed" over an `Added (11)` and a
/// `Removed (2)` that were complete and carried no truncation marker.
/// Reported from the field once the lists got short enough for the gap to
/// be visible; it had been there all along, hidden behind lists long
/// enough to look truncated.
///
/// Counting the listed population and naming the remainder, rather than
/// the reverse: the number a reader checks is the one they can check.
fn headline(result: &diff::DiffResult, skip: &dyn Fn(&diff::EntityDiff) -> bool) -> Vec<String> {
    let mut shown = Tally::default();
    let mut hidden = Tally::default();
    for d in &result.entities {
        let tally = if skip(d) { &mut hidden } else { &mut shown };
        tally.count(d);
    }
    let mut body = vec![format!(
        "{} added, {} removed, {} modified ({} source, {} coupling-only), {} unchanged",
        shown.added,
        shown.removed,
        shown.modified,
        shown.modified_source,
        shown.modified - shown.modified_source,
        shown.unchanged,
    )];
    if hidden.total() > 0 {
        body.push(format!(
            "Counted over what the lists below show. A further {} {} — fields, \
             imports, unresolved call targets and the file rows their children \
             already stand for — which no listing here displays.",
            hidden.total(),
            if hidden.total() == 1 {
                "entity changed"
            } else {
                "entities changed"
            },
        ));
    }
    body
}

/// One population's change counts.
#[derive(Default)]
struct Tally {
    added: usize,
    removed: usize,
    modified: usize,
    modified_source: usize,
    unchanged: usize,
}

impl Tally {
    fn count(&mut self, d: &diff::EntityDiff) {
        match d.status {
            diff::ChangeStatus::Added => self.added += 1,
            diff::ChangeStatus::Removed => self.removed += 1,
            diff::ChangeStatus::Unchanged => self.unchanged += 1,
            diff::ChangeStatus::Modified => {
                self.modified += 1;
                self.modified_source += usize::from(d.source_changed);
            }
        }
    }

    /// Everything that is not "unchanged" — what a reader is being told
    /// they cannot see.
    fn total(&self) -> usize {
        self.added + self.removed + self.modified
    }
}

/// One old file and everywhere its contents ended up.
///
/// Every destination, with how many entities went to each — not the
/// busiest one. A one-into-two split has no single right answer, and the
/// first version of this picked a half, stated it as a rename and never
/// mentioned the other. Reported from the field on a case where the half
/// it picked was the one `git` disagreed with: `generatorSection.ts` was
/// renamed to `generator/generatorSection.ts` and had
/// `generator/snowflakeOptions.ts` split out of it, and this claimed the
/// second as the rename.
fn destination_line((was, to): (&&str, &BTreeMap<&str, usize>)) -> String {
    if let Some((only, _)) = to.iter().next().filter(|_| to.len() == 1) {
        return format!("- `{was}` → `{only}`");
    }
    let parts: Vec<String> = to
        .iter()
        .map(|(now, count)| format!("`{now}` ({count})"))
        .collect();
    format!("- `{was}` → split across {}", parts.join(", "))
}

/// What the counts on a split line mean, and what they do not.
fn split_caveat(split: usize) -> Vec<String> {
    if split == 0 {
        return Vec::new();
    }
    vec![
        String::new(),
        format!(
            "{} of those went to more than one place. The counts are entities \
             matched to each destination, which is what this diff knows; they are \
             not a claim about which half `git` will call the rename, and on a split \
             the two can disagree.",
            if split == 1 { "One".to_string() } else { format!("{split}") },
        ),
    ]
}

/// How many moved files get named before the list summarises. Generous,
/// because a restructure that moves thirty files is precisely the change
/// whose file list a reviewer reads.
const MAX_MOVED_FILES: usize = 40;

/// One folder in a shape listing: the path as the reader spells it, and
/// what the shape pass scored it.
type ShapeRow<'s> = (String, &'s FolderShape);

/// Every folder under `base` the shape pass scored, named the way the
/// reader spells it.
///
/// Lifted out of [`folder_shape_section`] so the prose and the JSON read
/// one scoring rather than each taking their own pass (ADR 0035).
fn folder_shapes<'g>(graph: &'g DependencyGraph, base: &Path) -> Vec<(String, &'g FolderShape)> {
    graph
        .folder_metrics()
        .iter()
        .filter_map(|m| {
            let shape = m.metrics.shape.as_ref()?;
            Some((rel_path(Path::new(&m.path), base), shape))
        })
        .collect()
}

fn folder_shape_section(
    scored: &[(String, &FolderShape)],
    graph: &DependencyGraph,
    base: &Path,
    top: usize,
) -> Vec<String> {
    if scored.is_empty() {
        return Vec::new();
    }

    let mut body = shape_tally(scored);
    body.extend(whole_tree(scored, graph, base));
    let (here, elsewhere) = shape_groups(scored);
    if here.is_empty() && elsewhere.is_empty() {
        body.push("Every folder below the top level holds its shape.".to_string());
        return body;
    }

    body.extend(start_here_group(&here, top, graph, base));
    body.extend(blocked_group(&elsewhere, graph, base));
    body.extend(verdict_hints(here.iter().chain(elsewhere.iter())));
    body
}

/// How many "answered elsewhere" folders are worth naming. Well below
/// `top`: they are not the work list, and the count plus the gates they
/// are waiting on is the whole message.
const SHAPE_BLOCKED_SHOWN: usize = 5;

/// The shape section as fields: every scored folder, with the group the
/// prose files it under.
///
/// Grouped rather than flat because the grouping *is* the finding —
/// "start here" means the blocker is in the folder's own drawing and
/// "answered elsewhere" means it is not, which is the difference between
/// a work list and a waiting list. A consumer reading only `compliance`
/// would re-derive that rule and get it wrong, as field reports about the
/// prose version already showed.
///
/// Complete, unlike the prose: `top` caps the printed work list and
/// `SHAPE_BLOCKED_SHOWN` the printed waiting list, and both are reading
/// budgets rather than anything the caller asked for. `shown` records what
/// the prose displayed so the two can be checked against each other.
fn folder_shape_json(
    scored: &[(String, &FolderShape)],
    graph: &DependencyGraph,
    base: &Path,
    top: usize,
) -> Value {
    let (here, elsewhere) = shape_groups(scored);
    json!({
        "folders": scored
            .iter()
            .map(|(dir, shape)| shape_json(dir, shape, graph, base))
            .collect::<Vec<_>>(),
        "tally": {
            "cyclic": shape_count(scored, ShapePattern::Cyclic),
            "tangled": shape_count(scored, ShapePattern::Tangled),
            "hierarchical": shape_count(scored, ShapePattern::Hierarchical),
            "fractal": shape_count(scored, ShapePattern::Fractal),
            "total": scored.len(),
        },
        "start_here": {
            "folders": here.iter().map(|(dir, _)| dir.as_str()).collect::<Vec<_>>(),
            "shown": here.len().min(top),
            "total": here.len(),
        },
        "answered_elsewhere": {
            "folders": elsewhere.iter().map(|(dir, _)| dir.as_str()).collect::<Vec<_>>(),
            "shown": elsewhere.len().min(SHAPE_BLOCKED_SHOWN),
            "total": elsewhere.len(),
        },
    })
}

/// The two lists the prose prints, in the order it prints them — the
/// partition and both sorts, so neither rendering re-decides them.
///
/// The root is dropped: it is stated above whatever it scores
/// ([`whole_tree`]), so leaving it in either list would say it twice. A
/// sub-fractal folder always carries a blocker — `folder_shape` has an
/// invariant test for exactly that — so the `None` arm of the partition is
/// unreachable rather than a judgement, and it falls to the second group,
/// which is the harmless side: nothing there is claimed to be actionable.
fn shape_groups<'s>(scored: &'s [ShapeRow<'s>]) -> (Vec<ShapeRow<'s>>, Vec<ShapeRow<'s>>) {
    let short: Vec<(String, &FolderShape)> = scored
        .iter()
        .map(|(dir, s)| (dir.clone(), *s))
        .filter(|(dir, s)| !dir.is_empty() && s.pattern < ShapePattern::Fractal)
        .collect();
    let (mut here, mut elsewhere): (Vec<_>, Vec<_>) = short
        .into_iter()
        .partition(|(_, s)| s.blocker.is_some_and(|b| b.is_own_drawing()));
    here.sort_by(work_order);
    elsewhere.sort_by(worst_first);
    (here, elsewhere)
}

fn shape_count(scored: &[(String, &FolderShape)], pattern: ShapePattern) -> usize {
    scored.iter().filter(|(_, s)| s.pattern == pattern).count()
}

/// One folder's row: the shape scores as the pass computed them, plus the
/// two things [`shape_line`] adds in words — what it is held back by, and
/// whether its numbers were computed over a holed drawing.
fn shape_json(dir: &str, shape: &FolderShape, graph: &DependencyGraph, base: &Path) -> Value {
    json!({
        "folder": dir,
        "pattern": shape.pattern.label(),
        "blocked_by": shape.blocker.map(|b| b.summary()),
        "compliance": shape.compliance,
        "acyclicity": shape.acyclicity,
        "layering": shape.layering,
        "arborescence": shape.arborescence,
        "entry_concentration": shape.entry_concentration,
        "egress": shape.egress,
        "child_compliance": shape.child_compliance,
        "uniformity": shape.uniformity,
        "child_count": shape.child_count,
        "terms": shape.terms,
        "unresolved_imports": unresolved_imports_of(graph, base, dir),
    })
}

/// How many of this folder's imports never reached the graph — the number
/// behind [`unsound_marker`]'s ⚠, which is the only form the prose has
/// room for.
fn unresolved_imports_of(graph: &DependencyGraph, base: &Path, dir: &str) -> usize {
    let folder = if dir.is_empty() {
        base.to_path_buf()
    } else {
        base.join(dir)
    };
    graph.unresolved_imports_touching(&folder.display().to_string())
}

/// The graph the top-level folders draw between them, stated rather than
/// ranked.
///
/// The root is a folder like any other and has always been scored — its
/// children are the top-level directories, each collapsed to one node. It
/// was never *printed*, because both lists below are ranked and the root
/// loses both: `work_order` sorts deepest first, so depth zero comes last
/// of however many folders fall short, and the list is cut at `top`. A
/// repository whose top-level folders sit in a dependency loop would report
/// that loop nowhere while naming a leaf parser folder as the place to
/// start. Reported by a maintainer who could see the tangle on the canvas
/// in folder mode and could not find it in this output.
///
/// Deepest-first is right for the work list and is left alone: clearing a
/// child can clear its parent's gate, so the root is genuinely the last
/// thing to *fix*. It is the first thing to *know*, which is a different
/// question, so it gets its own line instead of a place in the ranking.
fn whole_tree(
    scored: &[(String, &FolderShape)],
    graph: &DependencyGraph,
    base: &Path,
) -> Vec<String> {
    let Some((dir, shape)) = scored.iter().find(|(dir, _)| dir.is_empty()) else {
        return Vec::new();
    };
    vec![
        String::new(),
        "### The top level".to_string(),
        "The graph drawn between the immediate children of the folder you asked \
         about — the whole repository, unless you passed a `path`. Every verdict \
         below sits inside this one, and it is stated here rather than ranked with \
         them because the work list is ordered deepest-first and would always put \
         it last."
            .to_string(),
        shape_line(dir, shape, &unsound_marker(graph, base, dir)),
    ]
}

/// The distribution across the ladder, and the two clauses a reader needs
/// before any number below it can be read the right way round.
fn shape_tally(scored: &[(String, &FolderShape)]) -> Vec<String> {
    let count = |p: ShapePattern| scored.iter().filter(|(_, s)| s.pattern == p).count();
    vec![
        String::new(),
        format!("## Folder shape ({} folders)", scored.len()),
        format!(
            "{} cyclic · {} tangled · {} hierarchical · {} fractal — \
             how readable the graph each folder draws is, over its immediate \
             children with each subfolder as one node. Higher is better here, \
             unlike the scores above. `branching` gates fractal but is not \
             part of `compliance`, so a folder can blend well and still be \
             held back by it. `uniformity` gates nothing at all: it is the \
             one number here comparing a folder against the level inside it \
             rather than against a fixed bar, and it is reported while its \
             distribution is still being learned.",
            count(ShapePattern::Cyclic),
            count(ShapePattern::Tangled),
            count(ShapePattern::Hierarchical),
            count(ShapePattern::Fractal),
        ),
    ]
}

/// Deepest first, then worst, then by path so two identical runs print
/// identically (AN-002).
///
/// Depth leads because it is the only key that encodes the ladder's
/// recursion: a folder's child gates are answered by work done below it,
/// so the deep folders are the ones whose improvement can move more than
/// themselves. Among folders at the same depth nothing links them, and
/// worst-first is the ordinary reading.
fn work_order(
    a: &(String, &FolderShape),
    b: &(String, &FolderShape),
) -> std::cmp::Ordering {
    depth(&b.0).cmp(&depth(&a.0)).then_with(|| worst_first(a, b))
}

fn worst_first(
    a: &(String, &FolderShape),
    b: &(String, &FolderShape),
) -> std::cmp::Ordering {
    a.1.pattern
        .cmp(&b.1.pattern)
        .then(a.1.compliance.total_cmp(&b.1.compliance))
        .then(a.0.cmp(&b.0))
}

/// How many folders down from the assessed root, counted in path
/// components so the assessed root itself is 0.
fn depth(rel: &str) -> usize {
    Path::new(rel).components().count()
}

/// What one folder's row says when some of its imports never reached the
/// graph.
///
/// `reshape` already refuses to give a folder a verdict without this caveat.
/// `quality`'s triage is read *first* — it is the list an agent works from
/// before calling `reshape` on anything — so a blocker misidentified from a
/// missing edge sends it to the wrong folder before the caveat is ever seen.
///
/// The direction matters and is easy to get backwards: a missing edge does not
/// only make a folder look worse, it can make it look **better**. A door that
/// nothing is recorded as reaching is not counted as a door, so
/// `entry_concentration` reads high and the folder is triaged as fine. Live
/// case from a field report: `src/uid` reported `one-door-in 0.40`, while two
/// inbound edges to `bulk.ts` were invisible and a raw read of the imports
/// found six ways in. The score was flattering it.
fn unsound_marker(graph: &DependencyGraph, base: &Path, dir: &str) -> String {
    match unresolved_imports_of(graph, base, dir) {
        0 => String::new(),
        n => format!(" · ⚠ {n} unresolved"),
    }
}

/// The note under a group holding at least one marked row, and nothing under
/// a group whose folders all resolved.
fn unsound_footnote(
    rows: &[(String, &FolderShape)],
    graph: &DependencyGraph,
    base: &Path,
) -> Vec<String> {
    let marked = rows
        .iter()
        .filter(|(dir, _)| !unsound_marker(graph, base, dir).is_empty())
        .count();
    if marked == 0 {
        return Vec::new();
    }
    vec![format!(
        "⚠ {marked} of these folders have imports that never reached the graph. Their \
         numbers are computed over an incomplete drawing and can read better than the \
         truth — an unrecorded dependency on a file is a door not counted as a door. \
         Call `reshape` on one before acting on its place in this list."
    )]
}

fn start_here_group(
    here: &[(String, &FolderShape)],
    top: usize,
    graph: &DependencyGraph,
    base: &Path,
) -> Vec<String> {
    if here.is_empty() {
        return vec![
            String::new(),
            "### Start here — none".to_string(),
            "Every folder short of fractal is waiting on something outside its own \
             drawing. Work the deepest ones on the list below first: their gates are \
             answered one level down."
                .to_string(),
        ];
    }
    let mut body = vec![
        String::new(),
        format!(
            "### Start here *for shape* — the blocker is in the folder's own \
             drawing ({} of {} shown)",
            here.len().min(top),
            here.len()
        ),
        "Deepest first: clearing one of these can also clear a parent's child gate."
            .to_string(),
        // The only imperative heading in this report, which is why it has to
        // say what it is imperative *about*. An agent reporting "no work
        // warranted" three sessions running, with the codebase's worst
        // function sitting unlabelled in the pressure list above, is what this
        // sentence exists to prevent: every folder here was a leave-it, the
        // section resolved cleanly to "nothing to do", and the search closed.
        "This covers folder shape only. Entity-level work is the refactor-pressure \
         list above — a folder can hold its shape perfectly while containing the \
         worst function in the codebase, and nothing here would say so."
            .to_string(),
    ];
    body.extend(
        here.iter()
            .take(top)
            .map(|(dir, s)| shape_line(dir, s, &unsound_marker(graph, base, dir))),
    );
    body.extend(unsound_footnote(&here[..here.len().min(top)], graph, base));
    body
}

fn blocked_group(
    elsewhere: &[(String, &FolderShape)],
    graph: &DependencyGraph,
    base: &Path,
) -> Vec<String> {
    if elsewhere.is_empty() {
        return Vec::new();
    }
    let mut body = vec![
        String::new(),
        format!(
            "### Answered elsewhere — blocked on children, callers or the blend ({})",
            elsewhere.len()
        ),
        "Nothing in these folders' own drawings is the thing holding them back. \
         Re-measure them after the work above, not before."
            .to_string(),
    ];
    body.extend(elsewhere.iter().take(SHAPE_BLOCKED_SHOWN).map(|(dir, s)| {
        format!(
            "- [{}] {} — held back by {}{}",
            s.pattern.label(),
            if dir.is_empty() { "(root)" } else { dir },
            s.blocker
                .map_or_else(|| "nothing".to_string(), |b| b.summary()),
            unsound_marker(graph, base, dir),
        )
    }));
    if elsewhere.len() > SHAPE_BLOCKED_SHOWN {
        body.push(format!(
            "… and {} more.",
            elsewhere.len() - SHAPE_BLOCKED_SHOWN
        ));
    }
    body
}

/// The blocker leads: it is the one part of the line that names something
/// to go and change, where the five numbers behind it leave the reader to
/// work out which gate failed.
fn shape_line(dir: &str, shape: &FolderShape, unsound: &str) -> String {
    format!(
        "- [{}] {} — held back by {}; compliance {:.2}, acyclic {:.2}, layered {}, \
         branching {}, one-door-in {}, uniformity {}, {} children{}",
        shape.pattern.label(),
        if dir.is_empty() { "(root)" } else { dir },
        shape
            .blocker
            .map_or_else(|| "nothing".to_string(), |b| b.summary()),
        shape.compliance,
        shape.acyclicity,
        ratio(shape.layering),
        ratio(shape.arborescence),
        ratio(shape.entry_concentration),
        ratio(shape.uniformity),
        shape.child_count,
        unsound,
    )
}

fn verdict_hints<'a>(
    listed: impl Iterator<Item = &'a (String, &'a crate::models::FolderShape)>,
) -> Vec<String> {
    let mut hints: Vec<ShapePattern> = listed
        .map(|(_, s)| s.pattern)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    hints.sort();
    let mut body = vec![String::new(), "What each verdict means:".to_string()];
    body.extend(
        hints
            .iter()
            .map(|pattern| format!("- {}: {}", pattern.label(), pattern.hint())),
    );
    body
}

/// An unmeasured ratio prints as an em dash rather than a flattering
/// zero — the same convention the coupling counts follow (UI-091).
fn ratio(value: Option<f32>) -> String {
    value.map_or_else(|| "—".to_string(), |v| format!("{v:.2}"))
}

// ------------------------------------------------------------------
//  context
// ------------------------------------------------------------------

/// Character budgets for a rendered signature. A signature sits inside a
/// ranked list, so it gets a line, not a screen: an inline object type is
/// worth a glance, never twenty lines of another entity's context.
const MAX_PARAMS_CHARS: usize = 100;
const MAX_RETURN_CHARS: usize = 40;

/// One-line signature rendered from graph data — never from re-reading
/// files. `process_order(order: Order, dry_run: bool) -> Receipt`.
/// Guaranteed single-line and length-capped, with `…` marking any cut.
fn signature(e: &CodeEntity) -> String {
    let params = e
        .parameters
        .iter()
        .map(|p| match &p.type_name {
            Some(t) => format!("{}: {}", one_line(&p.name), one_line(t)),
            None => one_line(&p.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ret = e
        .return_type
        .as_deref()
        .map(|r| format!(" -> {}", cap_chars(&one_line(r), MAX_RETURN_CHARS)))
        .unwrap_or_default();
    match e.kind {
        EntityKind::Function | EntityKind::Method => {
            format!(
                "{}({}){}",
                one_line(&e.name),
                cap_chars(&params, MAX_PARAMS_CHARS),
                ret
            )
        }
        _ => one_line(&e.name),
    }
}

pub fn context(server: &McpServer, args: &Value) -> Result<String> {
    let graph = analyze(server, &server.root)?;
    let target = match find_target(server, &graph, args)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };
    let root = &server.root;

    let sig_row = |e: &CodeEntity, rel_label: &str| {
        format!(
            "- {} {} `{}` — {}:{}",
            rel_label,
            e.kind.display_name(),
            signature(e),
            rel_path(&e.file_path, root),
            e.span.start.line + 1
        )
    };

    let parent_note = graph
        .parent(&target.id)
        .filter(|p| is_listed(p))
        .map(|p| format!(" (in {} `{}`)", p.kind.display_name(), p.name))
        .unwrap_or_default();
    let mut body = vec![format!(
        "# Context for {} `{}`{} — {}:{} ({})",
        target.kind.display_name(),
        target.name,
        parent_note,
        rel_path(&target.file_path, root),
        target.span.start.line + 1,
        metric_suffix(target)
    )];

    // The target's own source. Fenced so the agent can quote/edit it.
    body.push(String::new());
    body.push("## Source".to_string());
    match &target.source_code {
        Some(src) => {
            body.push("```".to_string());
            body.extend(src.lines().map(|l| l.to_string()));
            body.push("```".to_string());
        }
        None => body.push(format!(
            "(source not captured for this kind — read {}:{}-{})",
            rel_path(&target.file_path, root),
            target.span.start.line + 1,
            target.span.end.line + 1
        )),
    }

    // Signatures only for both directions: enough to edit the target
    // without opening the neighbors' files.
    let mut uses: Vec<(&CodeEntity, &str)> = graph
        .dependencies(&target.id)
        .into_iter()
        .filter(|(e, _)| is_listed(e))
        .map(|(e, r)| (e, r.kind.display_label()))
        .collect();
    uses.sort_by_key(|(e, _)| (e.file_path.clone(), e.span.start.line));
    uses.dedup_by_key(|(e, l)| (e.id.clone(), *l));
    body.push(String::new());
    body.push(format!("## Uses ({}) — signatures", uses.len()));
    for (e, label) in uses.iter().take(40) {
        body.push(sig_row(e, label));
    }
    if uses.len() > 40 {
        body.push(format!("… and {} more.", uses.len() - 40));
    }

    let mut used_by: Vec<(&CodeEntity, &str)> = graph
        .dependents(&target.id)
        .into_iter()
        .filter(|(e, _)| is_listed(e))
        .map(|(e, r)| (e, r.kind.display_label()))
        .collect();
    used_by.sort_by_key(|(e, _)| (e.file_path.clone(), e.span.start.line));
    used_by.dedup_by_key(|(e, l)| (e.id.clone(), *l));
    body.push(String::new());
    body.push(format!("## Used by ({}) — signatures", used_by.len()));
    for (e, label) in used_by.iter().take(40) {
        body.push(sig_row(e, label));
    }
    if used_by.len() > 40 {
        body.push(format!("… and {} more.", used_by.len() - 40));
    }

    Ok(cap_lines(
        body,
        "Target a narrower entity, or use `impact` for positions only.",
    ))
}

// ------------------------------------------------------------------
//  impact
// ------------------------------------------------------------------

/// Heading for MCP-013's hedged section. Named once so the zero-dependents
/// note and the section itself cannot drift apart.
const POSSIBLE_HEADING: &str = "Possible dependents";

/// Rows listed under that heading. Deliberately tighter than the 40 used
/// elsewhere: a name like `new` or `parse` collects unresolved references
/// from the whole tree, and a long list of maybes starts to read as evidence.
const POSSIBLE_CAP: usize = 15;

/// Entities that reference the target's *name* without the graph having
/// resolved the reference to it (MCP-013).
///
/// Unresolved targets become ghost entities keyed by the emitted name, so a
/// ghost called `foo` is where every reference to `foo` that mezz could not
/// bind ends up. Its dependents are therefore the closest thing to a reverse
/// view of the graph's blind spot — suggestive, never confirmed, and kept out
/// of the `Used by` count for that reason.
fn possible_dependents<'g>(
    graph: &'g DependencyGraph,
    target: &CodeEntity,
    known: &[(&CodeEntity, String)],
) -> Vec<(&'g CodeEntity, String)> {
    let seen: HashSet<&str> = known.iter().map(|(e, _)| e.id.as_str()).collect();
    let mut out: Vec<(&CodeEntity, String)> = graph
        .entities()
        .filter(|e| e.tags.contains("ghost") && e.name == target.name)
        .flat_map(|ghost| graph.dependents(&ghost.id))
        .filter(|(e, _)| is_listed(e) && e.id != target.id && !seen.contains(e.id.as_str()))
        .map(|(e, r)| (e, edge_label(r)))
        .collect();
    out.sort_by_key(|(e, _)| (e.file_path.clone(), e.span.start.line));
    out.dedup_by_key(|(e, l)| (e.id.clone(), l.clone()));
    out
}

/// One listing row: `- <edge label> <kind> `name` — path:line`.
fn entity_row(e: &CodeEntity, rel_label: &str, root: &Path) -> String {
    format!(
        "- {} {} `{}` — {}:{}",
        rel_label,
        e.kind.display_name(),
        e.name,
        rel_path(&e.file_path, root),
        e.span.start.line + 1
    )
}

/// The `Used by` section, plus what it cannot see (MCP-013).
///
/// A reference mezz could not resolve attaches to a ghost of the same name,
/// never to the target, so `dependents()` is a floor and not a count. The
/// `Uses` section can report its own blind spot by subtraction; this one
/// cannot see the misses at all, and so has to say so.
fn used_by_section(
    graph: &DependencyGraph,
    target: &CodeEntity,
    used_by: &[(&CodeEntity, String)],
    root: &Path,
) -> Vec<String> {
    let maybe = possible_dependents(graph, target, used_by);
    let mut out = vec![
        String::new(),
        format!(
            "## Used by ({}) — direct dependents, first to break on a contract change",
            used_by.len()
        ),
    ];
    if used_by.is_empty() {
        let pointer = if maybe.is_empty() {
            String::new()
        } else {
            format!(" See _{POSSIBLE_HEADING}_ below.")
        };
        out.push(format!(
            "_No **resolved** dependents, which is not the same as none._ \
             References mezz could not resolve are attached to a ghost of the \
             same name, and call sites in a form the parser doesn't reach \
             (Svelte markup, for one) produce no edge at all. Confirm by name \
             before treating this as unused.{pointer}"
        ));
    }
    out.extend(used_by.iter().take(40).map(|(e, l)| entity_row(e, l, root)));
    if used_by.len() > 40 {
        out.push(format!("… and {} more.", used_by.len() - 40));
    }
    if maybe.is_empty() {
        return out;
    }

    out.push(String::new());
    out.push(format!(
        "## {} ({}) — unresolved references to the name `{}`, not confirmed edges",
        POSSIBLE_HEADING,
        maybe.len(),
        target.name
    ));
    out.extend(
        maybe
            .iter()
            .take(POSSIBLE_CAP)
            .map(|(e, l)| entity_row(e, l, root)),
    );
    if maybe.len() > POSSIBLE_CAP {
        out.push(format!(
            "… and {} more not listed. A common name collects unrelated \
             references; check the listed ones before trusting the count.",
            maybe.len() - POSSIBLE_CAP
        ));
    }
    out
}

pub fn impact(server: &McpServer, args: &Value) -> Result<String> {
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .clamp(1, 5) as usize;
    let direction = super::radius::direction_of(args)?;

    // Blast radius must see every dependent, so always analyze the full root.
    let graph = analyze(server, &server.root)?;

    // `path` with no `line` and no `entity` asks about the file itself, which
    // is a different unit and not the sum of the entities in it (MCP-038).
    if let Some(file) = file_subject(server, args)? {
        return Ok(super::file_impact::report(&graph, &file, &server.root));
    }

    let target = match find_target(server, &graph, args)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };

    let root = &server.root;
    let row = |e: &CodeEntity, rel_label: &str| entity_row(e, rel_label, root);

    let context = graph
        .parent(&target.id)
        .filter(|p| is_listed(p))
        .map(|p| format!(" (in {} `{}`)", p.kind.display_name(), p.name))
        .unwrap_or_default();
    let mut body = vec![format!(
        "# Impact of {} `{}`{} — {}:{} ({})",
        target.kind.display_name(),
        target.name,
        context,
        rel_path(&target.file_path, root),
        target.span.start.line + 1,
        metric_suffix(target)
    )];

    // Outgoing: what the target relies on — its contract with the rest
    // of the code. Sorted by position for stable output.
    let (uses, outside) = uses_of(&graph, target);
    body.push(String::new());
    body.push(format!(
        "## Uses ({}) — code this entity relies on, in this repo",
        uses.len(),
    ));
    for (e, label) in uses.iter().take(40) {
        body.push(row(e, label));
    }
    if uses.len() > 40 {
        body.push(format!("… and {} more.", uses.len() - 40));
    }
    // The rest of what it relies on: named, and split, because a library
    // call and a call mezz could not bind mean opposite things (MCP-039).
    body.extend(outside.sections("this entity"));
    // And what those calls *do* — the one thing the sections above still
    // cannot say, carried `depth` hops out because the effect a reader
    // needs is usually not in the body they are editing (MCP-043).
    body.extend(
        super::effects::EffectSurface::of_entity(&graph, target, depth)
            .section("this entity", Some(depth), root),
    );

    // Incoming: direct dependents — first to break if the contract changes.
    let used_by = dependents_of(&graph, target);
    body.extend(used_by_section(&graph, target, &used_by, root));

    let children = graph.children(&target.id);
    let child_ids: HashSet<&str> = children.iter().map(|c| c.id.as_str()).collect();
    body.extend(used_via_members(&graph, target, &children, &child_ids, root));

    // The radius as routes: every entity reached, nested under the one it
    // was reached through, because "at depth 3" never said through what
    // (MCP-048). `direction: out` asks the same machinery the opposite
    // question — the call tree under the target.
    if direction == Direction::Out || depth > 1 {
        body.extend(super::radius::Radius::of(&graph, target, direction, depth).section(root));
    }

    Ok(cap_lines(
        body,
        "Lower `depth` or target a narrower entity.",
    ))
}

/// The `Used via members` section, empty for anything with no children.
///
/// Parsers emit no type-usage edges — a struct used as a parameter or a
/// field type produces no edge at all — so "who uses this type" is
/// approximated by aggregating the callers of its methods, one row per
/// caller with the members it reaches.
fn used_via_members(
    graph: &DependencyGraph,
    target: &CodeEntity,
    children: &[&CodeEntity],
    child_ids: &HashSet<&str>,
    root: &Path,
) -> Vec<String> {
    if children.is_empty() {
        return Vec::new();
    }
    let mut via: BTreeMap<(PathBuf, usize), (&CodeEntity, Vec<&str>)> = BTreeMap::new();
    for c in children.iter().filter(|c| is_listed(c)) {
        for (caller, _) in graph.dependents(&c.id) {
            if caller.id == target.id
                || child_ids.contains(caller.id.as_str())
                || !is_listed(caller)
            {
                continue;
            }
            via.entry((caller.file_path.clone(), caller.span.start.line))
                .or_insert_with(|| (caller, Vec::new()))
                .1
                .push(&c.name);
        }
    }

    let mut body = vec![
        String::new(),
        format!(
            "## Used via members ({} callers of this type's methods)",
            via.len()
        ),
    ];
    for (caller, methods) in via.values().take(40) {
        let mut methods = methods.clone();
        methods.sort();
        methods.dedup();
        body.push(format!(
            "- {} `{}` — {}:{} (uses `{}`)",
            caller.kind.display_name(),
            caller.name,
            rel_path(&caller.file_path, root),
            caller.span.start.line + 1,
            methods.join("`, `")
        ));
    }
    if via.len() > 40 {
        body.push(format!("… and {} more.", via.len() - 40));
    }
    body
}

/// BFS over incoming dependency edges, for the questions that want the
/// reached *set* rather than the routes to it.
///
/// `impact` asks for routes and walks with [`super::chains`] (MCP-048).
/// `tests_for` asks which tests cover an entity, groups them by file, and
/// wants the AN-004 confidence flag this carries — the route is the obvious
/// next thing to give it, and the walker it would move to already records
/// predecessors and the edge each hop crossed.
///
/// Level 0 holds direct dependents,
/// level N holds entities N+1 hops away. Ghosts and unlisted kinds are
/// excluded from levels but still traversed through, so a dependency
/// running through a field or import does not hide the code behind it.
/// Returns the dependents reached within `max_depth` hops, level by level,
/// plus a per-entity map of whether the **shortest** discovery path reached
/// it without crossing a heuristic call edge (AN-004). The map lets
/// `tests_for` flag which tests are connected by exact edges vs a
/// heuristic-only chain the graph can't vouch for. First-visit wins, so the
/// flag reflects the shortest path; it's an honest confidence hint, not a
/// proof that no exact path exists.
fn transitive_dependents<'g>(
    graph: &'g DependencyGraph,
    seeds: &[&'g str],
    max_depth: usize,
) -> (Vec<Vec<&'g CodeEntity>>, HashMap<&'g str, bool>) {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut exact_path: HashMap<&str, bool> = HashMap::new();
    let mut frontier: Vec<&str> = Vec::new();
    for id in seeds {
        if visited.insert(id) {
            frontier.push(id);
            exact_path.insert(id, true);
        }
    }
    let mut levels = Vec::new();

    for _ in 0..max_depth {
        let mut next: Vec<&CodeEntity> = Vec::new();
        for id in &frontier {
            let src_exact = exact_path.get(id).copied().unwrap_or(false);
            for (e, r) in graph.dependents(id) {
                // Lifted at the read, so a branch or loop node does not spend
                // a level of the radius on itself and push the callable
                // behind it one hop deeper — or, at depth 1, out of the
                // report entirely. See [`lifted`].
                let Some(e) = lifted(graph, e) else { continue };
                // A hop breaks confidence only when it's a heuristic call
                // edge — the case AN-004 exists to flag. Structural/exact
                // edges preserve it.
                let hop_trusted = !(r.kind == RelationshipKind::Calls
                    && r.precision == Some(Precision::Heuristic));
                if visited.insert(&e.id) {
                    exact_path.insert(&e.id, src_exact && hop_trusted);
                    next.push(e);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next.iter().map(|e| e.id.as_str()).collect();
        levels.push(next);
    }
    (levels, exact_path)
}

pub(super) enum Found<'g> {
    One(&'g CodeEntity),
    /// Lookup produced zero or several candidates; the text explains and
    /// lists them so the agent can re-call with a disambiguated target.
    Ambiguous(String),
}

/// The file a call is asking about, or `None` when it is asking about an
/// entity (MCP-038).
///
/// `path` on its own — no `line` narrowing it to one entity, and no
/// `entity`, which keeps its precedence. A directory is refused rather than
/// answered: "what does this folder depend on" is `reshape`'s and
/// `boundaries`' question, and a file-shaped report over a folder would be
/// a different tool wearing this one's name.
fn file_subject(server: &McpServer, args: &Value) -> Result<Option<PathBuf>> {
    let named = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let asks_for_a_line = args.get("line").and_then(|v| v.as_u64()).is_some();
    let asks_for_an_entity = args
        .get("entity")
        .and_then(|v| v.as_str())
        .is_some_and(|e| !e.is_empty());
    if named.is_empty() || asks_for_a_line || asks_for_an_entity {
        return Ok(None);
    }

    let path = resolve_path(server, args)?;
    if path.is_dir() {
        bail!(
            "`{named}` is a directory. `impact` answers for one file (`path` alone) or \
             one entity (`entity`, or `path` + `line`). For a folder, call `map` for \
             its shape, `reshape` for its drawing, or `boundaries` for what its imports \
             reach past."
        );
    }
    Ok(Some(path))
}

/// Locate the target entity from `entity` (name / qualified name) or
/// `path` + `line` (1-based, innermost listed entity spanning the line).
pub(super) fn find_target<'g>(
    server: &McpServer,
    graph: &'g DependencyGraph,
    args: &Value,
) -> Result<Found<'g>> {
    let name = args.get("entity").and_then(|v| v.as_str());
    let file = args.get("path").and_then(|v| v.as_str());
    let line = args.get("line").and_then(|v| v.as_u64());

    if let (Some(file), Some(line)) = (file, line) {
        let abs = resolve_path(server, args)?;
        let line0 = (line.max(1) - 1) as usize;
        let hit = graph
            .entities()
            .filter(|e| is_listed(e) && e.file_path == abs)
            .filter(|e| e.span.start.line <= line0 && line0 <= e.span.end.line)
            .min_by_key(|e| e.span.end.line - e.span.start.line);
        return match hit {
            Some(e) => Ok(Found::One(e)),
            None => Ok(Found::Ambiguous(format!(
                "No entity spans {}:{}. Call `map` on the file to see its entities.",
                file, line
            ))),
        };
    }

    let Some(name) = name else {
        bail!("Provide either `entity` (name or qualified name) or `path` + `line`.");
    };
    find_by_name(server, graph, name)
}

/// Name/qualified-name lookup shared by `find_target` and tools taking
/// bare names (`trace`). Exact matches win; suffix matches are the
/// fallback; several survivors return the disambiguation listing.
///
/// A `<path-suffix>:<name>` form (e.g. `src/main.rs:main`) narrows by
/// file — the escape hatch when identically-named entities exist and
/// their qualified names collide too.
pub(super) fn find_by_name<'g>(
    server: &McpServer,
    graph: &'g DependencyGraph,
    name: &str,
) -> Result<Found<'g>> {
    if let Some((file_part, name_part)) = name.rsplit_once(':') {
        if file_part.contains('/') || file_part.contains('.') {
            let hits: Vec<&CodeEntity> = graph
                .entities()
                .filter(|e| {
                    is_listed(e)
                        && e.name == name_part
                        && e.file_path.to_string_lossy().ends_with(file_part)
                })
                .collect();
            if hits.len() == 1 {
                return Ok(Found::One(hits[0]));
            }
            // Zero or several: fall through to the plain-name flow so the
            // caller still gets the standard candidate listing.
        }
    }

    let exact: Vec<&CodeEntity> = graph
        .entities()
        .filter(|e| is_listed(e) && (e.name == name || e.qualified_name == name))
        .collect();
    let candidates = if exact.is_empty() {
        graph
            .entities()
            .filter(|e| is_listed(e) && e.qualified_name.ends_with(name))
            .collect()
    } else {
        exact
    };

    match candidates.len() {
        1 => Ok(Found::One(candidates[0])),
        0 => Ok(Found::Ambiguous(format!(
            "No entity named `{}` found. Call `map` to browse available entities.",
            name
        ))),
        n => {
            let mut lines = vec![format!(
                "`{}` is ambiguous ({} matches). Re-call with a qualified name, `<path-suffix>:<name>` (e.g. `src/main.rs:main`), or `path` + `line`:",
                name, n
            )];
            for e in candidates.iter().take(20) {
                lines.push(format!(
                    "- {} `{}` — {}:{}",
                    e.kind.display_name(),
                    e.qualified_name,
                    rel_path(&e.file_path, &server.root),
                    e.span.start.line + 1
                ));
            }
            Ok(Found::Ambiguous(lines.join("\n")))
        }
    }
}

// ------------------------------------------------------------------
//  hotspots
// ------------------------------------------------------------------

pub fn hotspots(server: &McpServer, args: &Value) -> Result<Answer> {
    let path = resolve_path(server, args)?;
    let days = args
        .get("days")
        .and_then(|v| v.as_u64())
        .unwrap_or(180)
        .clamp(1, 3650);
    let top = args
        .get("top")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .clamp(1, 50) as usize;

    diff::verify_git_repo(&server.root)?;

    // Churn: commits touching each file in the window. Repo-relative paths.
    let out = std::process::Command::new("git")
        .args([
            "log",
            &format!("--since={} days ago", days),
            "--name-only",
            "--pretty=format:",
        ])
        .current_dir(&server.root)
        .output()?;
    if !out.status.success() {
        bail!("git log failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let mut churn: HashMap<String, u32> = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let line = line.trim();
        if !line.is_empty() {
            *churn.entry(line.to_string()).or_default() += 1;
        }
    }

    let graph = analyze(server, &path)?;

    // Per-file rollup from listed entities: LOC-weighted composite + worst entity.
    struct FileAgg<'g> {
        weighted: f64,
        loc: u64,
        worst: &'g CodeEntity,
    }
    let mut files: BTreeMap<String, FileAgg> = BTreeMap::new();
    for e in graph.entities().filter(|e| is_listed(e)) {
        let abs = &e.file_path;
        let repo_rel = abs
            .strip_prefix(&server.root)
            .unwrap_or(abs)
            .display()
            .to_string();
        let agg = files.entry(repo_rel).or_insert(FileAgg {
            weighted: 0.0,
            loc: 0,
            worst: e,
        });
        agg.weighted += e.metrics.composite_score as f64 * e.metrics.loc.max(1) as f64;
        agg.loc += e.metrics.loc.max(1) as u64;
        if e.metrics.composite_score > agg.worst.metrics.composite_score {
            agg.worst = e;
        }
    }

    // Risk = commits × LOC-weighted average composite score.
    let mut ranked: Vec<(f64, u32, f64, &String, &FileAgg)> = files
        .iter()
        .map(|(file, agg)| {
            let commits = churn.get(file).copied().unwrap_or(0);
            let avg = agg.weighted / agg.loc.max(1) as f64;
            (commits as f64 * avg, commits, avg, file, agg)
        })
        .filter(|(risk, _, _, _, _)| *risk > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));

    // One ranking, rendered twice (ADR 0035). `top` is the caller's own
    // bound, so both renderings honour it and both carry the total.
    let shown = ranked.iter().take(top);

    let mut body = vec![
        format!("# Hotspots of {} (last {} days)", path.display(), days),
        "Risk = commits in window × LOC-weighted avg composite score. Renames count as fresh paths.".to_string(),
        String::new(),
    ];
    if ranked.is_empty() {
        body.push("No files with both churn and quality pressure in the window.".to_string());
    }
    for (risk, commits, avg, file, agg) in shown.clone() {
        body.push(format!(
            "- [risk {:.2}] {} — {} commits, avg pressure {:.2}; worst: {} `{}` ({})",
            risk,
            file,
            commits,
            avg,
            agg.worst.kind.display_name(),
            agg.worst.name,
            metric_suffix(agg.worst)
        ));
    }
    if ranked.len() > top {
        body.push(format!(
            "… and {} more files with non-zero risk.",
            ranked.len() - top
        ));
    }

    let data = json!({
        "path": answer::scope_path(&path, &server.root),
        "days": days,
        "top": top,
        "files": shown
            .map(|(risk, commits, avg, file, agg)| json!({
                "file": file,
                "risk": risk,
                "commits": commits,
                "avg_pressure": avg,
                "worst": answer::entity_json(agg.worst, &server.root),
            }))
            .collect::<Vec<_>>(),
        "total": ranked.len(),
    });

    Ok(Answer::structured(
        cap_lines(body, "Raise `top` or narrow `path`."),
        data,
    ))
}

// ------------------------------------------------------------------
//  tests_for
// ------------------------------------------------------------------

pub fn tests_for(server: &McpServer, args: &Value) -> Result<String> {
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .clamp(1, 6) as usize;

    // Force tests into the graph regardless of the server-level flag.
    let graph = analyze_with_tests(server, &server.root, true)?;
    let target = match find_target(server, &graph, args)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };
    let root = &server.root;

    // Walk dependents outward; anything living in a test file is a hit.
    // Containers seed with members, mirroring `impact`.
    let children = graph.children(&target.id);
    let mut seeds: Vec<&str> = vec![&target.id];
    seeds.extend(children.iter().map(|c| c.id.as_str()));
    let (levels, exact_path) = transitive_dependents(&graph, &seeds, depth);

    let mut by_file: BTreeMap<String, Vec<(usize, &CodeEntity)>> = BTreeMap::new();
    let mut total = 0usize;
    let mut heuristic_reached = 0usize;
    let tests = TestPaths::rooted_at(root);
    for (i, level) in levels.iter().enumerate() {
        for e in level {
            if is_test_entity(&graph, e, &tests) {
                by_file
                    .entry(rel_path(&e.file_path, root))
                    .or_default()
                    .push((i + 1, e));
                total += 1;
                if !exact_path.get(e.id.as_str()).copied().unwrap_or(false) {
                    heuristic_reached += 1;
                }
            }
        }
    }

    let mut body = vec![format!(
        "# Tests reaching {} `{}` — {}:{} (searched {} dependency hops)",
        target.kind.display_name(),
        target.name,
        rel_path(&target.file_path, root),
        target.span.start.line + 1,
        depth
    )];

    if heuristic_reached > 0 {
        body.push(format!(
            "_{} of {} reached only via a heuristic call edge — treat those as maybe, not proof._",
            heuristic_reached, total
        ));
    }

    if total == 0 {
        body.push(String::new());
        body.push(format!(
            "No tests reach this entity within {} hops. Either it is untested, \
             or coverage flows through call edges the graph cannot resolve.",
            depth
        ));
    }
    for (file, mut hits) in by_file {
        hits.sort_by_key(|(hops, e)| (*hops, e.span.start.line));
        body.push(String::new());
        body.push(format!("{} ({} tests)", file, hits.len()));
        for (hops, e) in hits {
            let directness = if hops == 1 {
                "direct".to_string()
            } else {
                format!("{} hops", hops)
            };
            // Flag tests reached only through a heuristic call edge — the
            // path may be a false positive (AN-004). Exact-path tests carry
            // no marker to keep the common case quiet.
            let confidence = if exact_path.get(e.id.as_str()).copied().unwrap_or(false) {
                ""
            } else {
                ", via heuristic edge"
            };
            body.push(format!(
                "  {} `{}` — L{} ({}{})",
                e.kind.display_name(),
                e.name,
                e.span.start.line + 1,
                directness,
                confidence
            ));
        }
    }

    Ok(cap_lines(
        body,
        "Lower `depth` to see only the closest tests.",
    ))
}

// ------------------------------------------------------------------
//  trace
// ------------------------------------------------------------------

pub fn trace(server: &McpServer, args: &Value) -> Result<String> {
    let max_hops = args
        .get("max_hops")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .clamp(1, 30) as usize;
    let from_name = args
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("`from` is required"))?;
    let to_name = args
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("`to` is required"))?;

    let graph = analyze(server, &server.root)?;
    let from = match find_by_name(server, &graph, from_name)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };
    let to = match find_by_name(server, &graph, to_name)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };
    let root = &server.root;

    let render_node = |e: &CodeEntity| render_node(e, root);

    let mut body = vec![format!(
        "# Trace: {} → {}",
        render_node(from),
        render_node(to)
    )];

    match shortest_paths(&graph, &from.id, &to.id, max_hops, 3) {
        paths if !paths.is_empty() => {
            body.push(String::new());
            for path_ids in &paths {
                let mut chain = Vec::new();
                for window in path_ids.windows(2) {
                    let label = graph
                        .dependencies(&window[0])
                        .into_iter()
                        .find(|(e, _)| e.id == window[1])
                        .map(|(_, r)| edge_label(r))
                        .unwrap_or_else(|| "→".to_string());
                    let node = graph
                        .get_entity(&window[0])
                        .map(render_node)
                        .unwrap_or_default();
                    chain.push(format!("{} —{}→", node, label));
                }
                chain.push(
                    graph
                        .get_entity(path_ids.last().unwrap())
                        .map(render_node)
                        .unwrap_or_default(),
                );
                body.push(format!("- {}", chain.join(" ")));
            }
        }
        _ => {
            body.push(String::new());
            let reverse = shortest_paths(&graph, &to.id, &from.id, max_hops, 1);
            if reverse.is_empty() {
                body.push(format!(
                    "No dependency path in either direction within {} hops.",
                    max_hops
                ));
                let near = |e: &CodeEntity| {
                    graph
                        .dependencies(&e.id)
                        .iter()
                        .filter(|(d, _)| is_listed(d))
                        .take(5)
                        .map(|(d, _)| d.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                body.push(format!("`{}` directly uses: {}", from.name, near(from)));
                body.push(format!("`{}` directly uses: {}", to.name, near(to)));
            } else {
                body.push(format!(
                    "No path from `{}` to `{}`, but the reverse direction exists:",
                    from.name, to.name
                ));
                let ids = &reverse[0];
                let names: Vec<String> = ids
                    .iter()
                    .filter_map(|id| graph.get_entity(id))
                    .map(render_node)
                    .collect();
                body.push(format!("- {}", names.join(" → ")));
            }
        }
    }

    Ok(cap_lines(
        body,
        "Raise `max_hops` if endpoints are far apart.",
    ))
}

/// Record `pred` as a predecessor of a node, unless it is already one.
///
/// The dependency graph is a multigraph: a pair joined by both a `Calls` and
/// a `UsesFn` edge is offered twice by `dependencies`. A predecessor stored
/// twice reconstructs into two byte-identical chains, so `trace` spends its
/// three-route budget printing one route repeatedly (MCP-019). Predecessor
/// lists are a handful of entries, so the linear scan is cheaper than the
/// wasted expansion it prevents.
fn record_predecessor(preds: &mut Vec<String>, pred: &str) {
    if !preds.iter().any(|p| p == pred) {
        preds.push(pred.to_string());
    }
}

/// BFS over outgoing dependency edges, reconstructing up to `max_paths`
/// shortest paths (as id chains) from `from` to `to`. Ghost nodes are
/// traversed — a path legitimately runs through externally-named hops.
fn shortest_paths(
    graph: &DependencyGraph,
    from: &str,
    to: &str,
    max_hops: usize,
    max_paths: usize,
) -> Vec<Vec<String>> {
    use std::collections::VecDeque;
    // parents: for each visited node, its distinct predecessors at minimal
    // depth — see `record_predecessor` for why distinct.
    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    let mut depth_of: HashMap<String, usize> = HashMap::new();
    depth_of.insert(from.to_string(), 0);
    let mut queue = VecDeque::from([from.to_string()]);

    while let Some(id) = queue.pop_front() {
        let d = depth_of[&id];
        if d >= max_hops || (depth_of.contains_key(to) && d + 1 > depth_of[to]) {
            continue;
        }
        for (next, _) in graph.dependencies(&id) {
            match depth_of.get(&next.id) {
                None => {
                    depth_of.insert(next.id.clone(), d + 1);
                    parents.insert(next.id.clone(), vec![id.clone()]);
                    queue.push_back(next.id.clone());
                }
                Some(&nd) if nd == d + 1 => {
                    record_predecessor(parents.entry(next.id.clone()).or_default(), &id);
                }
                _ => {}
            }
        }
    }

    if !depth_of.contains_key(to) {
        return Vec::new();
    }

    // Reconstruct up to max_paths chains by walking parents back to `from`.
    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut stack: Vec<Vec<String>> = vec![vec![to.to_string()]];
    while let Some(partial) = stack.pop() {
        if paths.len() >= max_paths {
            break;
        }
        let head = partial.last().unwrap().clone();
        if head == from {
            let mut p = partial.clone();
            p.reverse();
            paths.push(p);
            continue;
        }
        if let Some(preds) = parents.get(&head) {
            for pred in preds {
                let mut next = partial.clone();
                next.push(pred.clone());
                stack.push(next);
            }
        }
    }
    paths
}

// ------------------------------------------------------------------
//  similar
// ------------------------------------------------------------------

/// Lowercased word tokens from an identifier or free text: splits on
/// non-alphanumerics and camelCase boundaries, so `parseVisibility`,
/// `parse_visibility`, and "parse visibility" all yield {parse, visibility}.
fn tokenize(text: &str) -> HashSet<String> {
    let mut tokens = HashSet::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        let mut word = String::new();
        let mut prev_lower = false;
        for c in raw.chars() {
            if c.is_uppercase() && prev_lower {
                if word.len() > 1 {
                    tokens.insert(word.to_lowercase());
                }
                word = String::new();
            }
            prev_lower = c.is_lowercase();
            word.push(c);
        }
        if word.len() > 1 {
            tokens.insert(word.to_lowercase());
        }
    }
    tokens
}

/// English function words that carry no domain meaning on their own.
/// Suppressed by weight rather than dropped from the query: `to` must not
/// be able to lift `toRef` into the results, but it can still separate two
/// otherwise equally-matched candidates when the content words match too.
/// Words that double as code vocabulary (`get`, `set`, `new`, `all`) are
/// deliberately absent — they discriminate.
const STOPWORDS: &[&str] = &[
    "an", "and", "any", "are", "as", "at", "be", "but", "by", "can", "do", "does", "for", "from",
    "has", "have", "how", "if", "in", "into", "is", "it", "its", "may", "must", "not", "of", "on",
    "or", "should", "so", "than", "that", "the", "their", "then", "there", "these", "this",
    "those", "to", "was", "we", "what", "when", "where", "which", "will", "with", "would", "you",
    "your",
];

/// What a stopword is worth relative to a content word.
const STOPWORD_WEIGHT: f64 = 0.15;

/// Extra credit when the match lands on the entity's own name rather than
/// on a qualifier or a parameter type.
const NAME_BONUS: f64 = 0.25;

/// Minimum score a result must reach to be worth an agent's tokens.
/// Absolute, not relative to the top hit: a relative floor would always
/// return *something*, and the tool's contract is that an empty result
/// means "implementing fresh is reasonable". See the `similar` schema in
/// `mod.rs`, which documents this number to the agent.
const SIMILARITY_FLOOR: f64 = 0.40;

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

/// Weight one query token contributes to the score.
fn token_weight(token: &str) -> f64 {
    if is_stopword(token) {
        STOPWORD_WEIGHT
    } else {
        1.0
    }
}

/// Total weight of a set of tokens.
fn matched_weight<'a>(tokens: impl IntoIterator<Item = &'a String>) -> f64 {
    tokens.into_iter().map(|t| token_weight(t)).sum()
}

/// Everything an entity is searchable by: its name, its qualifiers, and
/// the types in its signature.
fn entity_vocabulary(e: &CodeEntity) -> HashSet<String> {
    let mut tokens = tokenize(&e.name);
    tokens.extend(tokenize(&e.qualified_name));
    for p in &e.parameters {
        if let Some(t) = &p.type_name {
            tokens.extend(tokenize(t));
        }
    }
    if let Some(r) = &e.return_type {
        tokens.extend(tokenize(r));
    }
    tokens
}

/// Score one entity against the query, or `None` if it is not a candidate
/// at all. `query_weight` is the query's total token weight, passed in so
/// it is summed once per call rather than once per entity.
fn score_against(
    e: &CodeEntity,
    query_tokens: &HashSet<String>,
    query_weight: f64,
) -> Option<(f64, Vec<String>)> {
    let matched: Vec<String> = query_tokens
        .intersection(&entity_vocabulary(e))
        .cloned()
        .collect();
    // A hit resting entirely on function words is noise whatever it
    // scores — `toRef` is not a match for "…ref to sha", and a query made
    // only of stopwords must match nothing at all.
    if matched.is_empty() || matched.iter().all(|t| is_stopword(t)) {
        return None;
    }
    // Fraction of the query's *weight* covered, nudged up when the name
    // itself (not just qualifiers/types) carries the match. Normalized by
    // the bonus so a match covering the whole query in the name is 1.00.
    let name_hit_weight = matched_weight(query_tokens.intersection(&tokenize(&e.name)));
    let score = (matched_weight(&matched) + NAME_BONUS * name_hit_weight)
        / (query_weight * (1.0 + NAME_BONUS));
    Some((score, matched))
}

pub fn similar(server: &McpServer, args: &Value) -> Result<String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("`query` is required"))?;
    let kind_filter = args.get("kind").and_then(|v| v.as_str());
    let top = args
        .get("top")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .clamp(1, 50) as usize;

    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        bail!("Query yielded no tokens — use words or identifier fragments.");
    }

    let graph = analyze(server, &server.root)?;
    let root = &server.root;
    let query_weight = matched_weight(&query_tokens);

    let mut scored: Vec<(f64, Vec<String>, &CodeEntity)> = graph
        .entities()
        .filter(|e| is_listed(e))
        .filter(|e| kind_filter.is_none_or(|k| e.kind.display_name().eq_ignore_ascii_case(k)))
        .filter_map(|e| {
            score_against(e, &query_tokens, query_weight)
                .map(|(score, matched)| (score, matched, e))
        })
        .filter(|(score, _, _)| *score >= SIMILARITY_FLOOR)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.id.cmp(&b.2.id)));

    let mut body = vec![format!("# Similar to \"{}\"", query), String::new()];
    if scored.is_empty() {
        body.push(
            "Nothing sufficiently similar exists — implementing fresh is reasonable.".to_string(),
        );
    }
    for (score, mut matched, e) in scored.into_iter().take(top) {
        matched.sort();
        body.push(format!(
            "- [{:.2}] {} `{}` — {}:{} (matched: {})",
            score,
            e.kind.display_name(),
            signature(e),
            rel_path(&e.file_path, root),
            e.span.start.line + 1,
            matched.join(", ")
        ));
    }

    Ok(cap_lines(
        body,
        "Refine the query with more specific words.",
    ))
}

// ------------------------------------------------------------------
//  dead_code
// ------------------------------------------------------------------

/// Kinds a developer could actually delete. Modules and files are
/// containment rather than reference — their fan-in is structurally zero
/// and says nothing — and the Elevator/deployment kinds are not code.
/// Parameters, fields, imports and the synthetic flow kinds are already
/// gone via `is_listed`.
fn is_deletable(e: &CodeEntity) -> bool {
    matches!(
        e.kind,
        EntityKind::Function
            | EntityKind::Method
            | EntityKind::Class
            | EntityKind::Dataclass
            | EntityKind::AbstractClass
            | EntityKind::Struct
            | EntityKind::Interface
            | EntityKind::Trait
            | EntityKind::Enum
            | EntityKind::TypeAlias
            | EntityKind::Constant
            | EntityKind::Property
            | EntityKind::Macro
            | EntityKind::Component
            | EntityKind::Service
    )
}

/// Languages whose parsers read visibility off an explicit source
/// modifier (`pub`, `public`, the Python/Dart underscore convention), so `Public`
/// means "declared public" rather than "no information". Everywhere else —
/// TypeScript's `export`, Go's capitalization — every entity is parsed as
/// `Public`, and using that as a public-API filter would empty the report
/// instead of narrowing it.
fn records_visibility(lang: crate::models::file_info::Language) -> bool {
    use crate::models::file_info::Language as L;
    matches!(
        lang,
        L::Rust
            | L::Java
            | L::Kotlin
            | L::Dart
            | L::CSharp
            | L::Scala
            | L::Swift
            | L::PHP
            | L::Python
    )
}

/// True when the entity is declared visible outside its own module, so a
/// caller can live in a crate or package this analysis never sees. Absence
/// of a dependent is then no evidence of death.
fn is_public_api(e: &CodeEntity) -> bool {
    let lang = e
        .file_path
        .extension()
        .and_then(|x| x.to_str())
        .map(crate::models::file_info::Language::from_extension)
        .unwrap_or(crate::models::file_info::Language::Unknown);
    e.visibility == crate::models::Visibility::Public && records_visibility(lang)
}

/// True when something outside the graph is expected to invoke this
/// entity: a program entry point, a runtime hook (Python dunders), or a
/// member satisfying a declared contract that callers reach through the
/// abstraction rather than by name.
fn is_entry_point(graph: &DependencyGraph, e: &CodeEntity) -> bool {
    if e.kind.is_callable() && e.name == "main" {
        return true;
    }
    if e.name.starts_with("__") && e.name.ends_with("__") {
        return true;
    }
    // Contract members: Rust `impl Trait for T` methods (tagged by the
    // parser, with the trait recorded in `implements`) and Kotlin
    // `override`, which lands in `attributes`.
    if e.tags.contains("trait_impl")
        || !e.implements.is_empty()
        || e.attributes.iter().any(|a| a == "override")
    {
        return true;
    }
    graph
        .dependencies(&e.id)
        .iter()
        .any(|(_, r)| r.kind == RelationshipKind::Implements)
}

/// True when the entity's contract is declared against a type this graph
/// does not hold — a TypeScript `class P extends Plugin` where `Plugin`
/// comes from a package the analysis never walked. Whoever constructs `P`
/// and calls `onload` lives on the other side of that declaration, so
/// fan-in 0 is no more evidence of death here than it is for a Rust trait
/// impl, which `is_entry_point` already spares for exactly this reason.
///
/// Scoped to the extending class and its members. A helper function
/// sitting beside a plugin subclass is ordinary code, and so is a member
/// the source declares `private`: it cannot be reached through the base
/// type, so nothing outside the tree can be calling it and its fan-in
/// still means what it says. (Unlike a bare `Public`, which TypeScript
/// hands out by default — see `records_visibility` — an explicit
/// `private` carries information.)
fn declares_out_of_tree_contract(graph: &DependencyGraph, e: &CodeEntity) -> bool {
    if graph.has_external_supertype(&e.id) {
        return true;
    }
    if e.visibility == crate::models::Visibility::Private {
        return false;
    }
    graph
        .parent(&e.id)
        .is_some_and(|p| graph.has_external_supertype(&p.id))
}

/// Why a fan-in-0 entity is *not* reported. Counted and stated in the
/// header, so the report narrows visibly rather than silently.
enum Excluded {
    Test,
    EntryPoint,
    ExternalBase,
    PublicApi,
}

fn exclusion_reason(graph: &DependencyGraph, e: &CodeEntity, tests: &TestPaths) -> Option<Excluded> {
    if is_test_entity(graph, e, tests) {
        Some(Excluded::Test)
    } else if is_entry_point(graph, e) {
        Some(Excluded::EntryPoint)
    } else if declares_out_of_tree_contract(graph, e) {
        Some(Excluded::ExternalBase)
    } else if is_public_api(e) {
        Some(Excluded::PublicApi)
    } else {
        None
    }
}

#[derive(Default)]
struct ExcludedCounts {
    tests: usize,
    entry_points: usize,
    external_base: usize,
    public_api: usize,
    mentioned: usize,
}

/// Where each candidate name is defined, so an occurrence there can be
/// told apart from a real use. A name can be defined more than once —
/// two private helpers in different files — and any of those spans
/// disqualifies the occurrence.
type OwnSpans<'a> = HashMap<&'a str, Vec<(&'a Path, usize, usize)>>;

/// True when this occurrence of `token` sits inside the declaration or
/// body of the entity it names: a function mentioning itself, not a use.
fn in_own_definition(own: &OwnSpans, token: &str, file: &Path, line: usize) -> bool {
    own.get(token).is_some_and(|spans| {
        spans
            .iter()
            .any(|(p, from, to)| *p == file && line >= *from && line <= *to)
    })
}

/// Record every `wanted` identifier occurring in one file's text, except
/// where it occurs inside its own definition.
fn scan_mentions(
    file: &Path,
    text: &str,
    wanted: &HashSet<&str>,
    own: &OwnSpans,
    out: &mut HashSet<String>,
) {
    for (idx, line) in text.lines().enumerate() {
        for token in line.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if wanted.contains(token)
                && !out.contains(token)
                && !in_own_definition(own, token, file, idx)
            {
                out.insert(token.to_string());
            }
        }
    }
}

/// Candidate names that appear in the project source outside their own
/// definition. This is the backstop for references the graph cannot hold:
/// a Rust macro body (`format!`, `assert!`, `vec!`) reaches the parser as
/// an opaque token tree, so a helper called only from inside one has
/// fan-in 0 and would otherwise be reported as dead. The same scan
/// catches reflection and string-dispatch mentions.
///
/// It is deliberately one-directional — it only ever *suppresses* a
/// candidate. A name in a comment or a same-named symbol elsewhere costs
/// a true positive; nothing here can invent one.
fn names_mentioned_elsewhere(
    candidates: &[&CodeEntity],
    files: &BTreeSet<&Path>,
) -> HashSet<String> {
    let wanted: HashSet<&str> = candidates.iter().map(|e| e.name.as_str()).collect();
    if wanted.is_empty() {
        return HashSet::new();
    }
    let mut own: OwnSpans = HashMap::new();
    for e in candidates {
        own.entry(e.name.as_str()).or_default().push((
            e.file_path.as_path(),
            e.span.start.line,
            e.span.end.line,
        ));
    }

    let mut mentioned = HashSet::new();
    for file in files {
        if let Ok(text) = std::fs::read_to_string(file) {
            scan_mentions(file, &text, &wanted, &own, &mut mentioned);
        }
    }
    mentioned
}

/// Split the fan-in-0 entities under `scope` into reportable candidates
/// and the populations for which the absence of a dependent proves
/// nothing, counting the latter so the report can state them.
fn collect_candidates<'g>(
    graph: &'g DependencyGraph,
    scope: &Path,
    include_public: bool,
) -> (Vec<&'g CodeEntity>, ExcludedCounts) {
    let mut candidates: Vec<&CodeEntity> = Vec::new();
    let mut counts = ExcludedCounts::default();
    // Rooted once for the whole walk: any path inside the checkout resolves
    // to the same repo root, and resolving it per entity is a filesystem
    // climb per entity.
    let tests = TestPaths::rooted_at(scope);
    for e in graph.entities() {
        if !is_listed(e) || !is_deletable(e) || e.metrics.fan_in > 0 {
            continue;
        }
        if !e.file_path.starts_with(scope) {
            continue;
        }
        match exclusion_reason(graph, e, &tests) {
            Some(Excluded::Test) => counts.tests += 1,
            Some(Excluded::EntryPoint) => counts.entry_points += 1,
            Some(Excluded::ExternalBase) => counts.external_base += 1,
            Some(Excluded::PublicApi) => {
                counts.public_api += 1;
                if include_public {
                    candidates.push(e);
                }
            }
            None => candidates.push(e),
        }
    }
    (candidates, counts)
}

/// `dead_code` — entities nothing in the project references, grouped by
/// file. Fan-in 0 minus the populations for which "no dependent" is not
/// evidence of death: tests, entry points, members of a class whose base
/// type is outside the tree, public API, and names the source mentions
/// somewhere the graph cannot see.
pub fn dead_code(server: &McpServer, args: &Value) -> Result<Answer> {
    let scope = resolve_path(server, args)?;
    let include_public = args
        .get("include_public")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // The graph is always the whole project, whatever `path` says: fan-in
    // computed over a subdirectory would miss every caller outside it and
    // report the folder's entire surface as dead. `path` narrows the
    // report, never the analysis. Tests are forced in for the same reason
    // — a function whose only caller is a test is referenced, not dead
    // (and so never appears here, even with the caller filtered out).
    let graph = analyze_with_tests(server, &server.root, true)?;
    let root = &server.root;
    let (candidates, mut counts) = collect_candidates(&graph, &scope, include_public);

    // Second gate: a name the source mentions somewhere else is referenced
    // through something the graph cannot represent, not dead. Scanned over
    // every file the graph was built from — a caller outside `scope`
    // counts just as much as one inside it.
    let files: BTreeSet<&Path> = graph
        .entities()
        .filter(|e| is_listed(e))
        .map(|e| e.file_path.as_path())
        .collect();
    let mentioned = names_mentioned_elsewhere(&candidates, &files);

    let mut by_file: BTreeMap<String, Vec<&CodeEntity>> = BTreeMap::new();
    for e in candidates {
        if mentioned.contains(&e.name) {
            counts.mentioned += 1;
        } else {
            by_file
                .entry(rel_path(&e.file_path, root))
                .or_default()
                .push(e);
        }
    }

    // The grouping is the value; the report and the JSON are two readings
    // of it (ADR 0035). Nothing here is capped by the renderer, so both
    // carry every candidate.
    Ok(Answer::structured(
        render_dead_code(&by_file, &counts, &scope, root, include_public),
        dead_code_json(&by_file, &counts, &scope, root, include_public),
    ))
}

/// Files with the most candidates first, ties in path order, each file's
/// entities in source order.
///
/// Shared by both renderings (ADR 0035) rather than sorted twice: two
/// orderings of one list is how a reader comparing the prose against the
/// JSON comes to think they disagree. Stable across runs, per AN-002.
fn worst_files_first<'m, 'e>(
    by_file: &'m BTreeMap<String, Vec<&'e CodeEntity>>,
) -> Vec<(&'m String, Vec<&'e CodeEntity>)> {
    let mut files: Vec<(&'m String, Vec<&'e CodeEntity>)> = by_file
        .iter()
        .map(|(file, hits)| {
            let mut hits = hits.clone();
            hits.sort_by_key(|e| e.span.start.line);
            (file, hits)
        })
        .collect();
    files.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));
    files
}

/// The same grouping as fields.
///
/// `excluded` mirrors the "_Also fan-in 0, not reported_" line exactly:
/// those counts are what keeps a short list from reading as a clean tree,
/// and a consumer comparing two runs needs them as much as a reader does.
fn dead_code_json(
    by_file: &BTreeMap<String, Vec<&CodeEntity>>,
    counts: &ExcludedCounts,
    scope: &Path,
    root: &Path,
    include_public: bool,
) -> Value {
    json!({
        "path": answer::scope_path(scope, root),
        "include_public": include_public,
        "total": by_file.values().map(|v| v.len()).sum::<usize>(),
        "files": worst_files_first(by_file)
            .into_iter()
            .map(|(file, hits)| json!({
                "file": file,
                "count": hits.len(),
                "entities": hits
                    .iter()
                    .map(|e| {
                        let mut row = answer::entity_json(e, root);
                        row["public"] = json!(is_public_api(e));
                        row
                    })
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
        "excluded": {
            "tests": counts.tests,
            "entry_points": counts.entry_points,
            "external_base": counts.external_base,
            "public_api": counts.public_api,
            "mentioned_elsewhere": counts.mentioned,
        },
    })
}

/// Render the grouped report: files worst-first, each with its count.
fn render_dead_code(
    by_file: &BTreeMap<String, Vec<&CodeEntity>>,
    counts: &ExcludedCounts,
    scope: &Path,
    root: &Path,
    include_public: bool,
) -> String {
    let total: usize = by_file.values().map(|v| v.len()).sum();
    let where_ = match rel_path(scope, root).as_str() {
        "" => "the project".to_string(),
        p => p.to_string(),
    };
    let mut body = vec![
        format!(
            "# Dead-code candidates in {} — {} in {} file{}",
            where_,
            total,
            by_file.len(),
            if by_file.len() == 1 { "" } else { "s" }
        ),
        "_No entity in the project graph depends on these, and their names appear \
         nowhere else in the source. Still verify before deleting: no static graph \
         sees FFI, dynamic dispatch by string, or a name assembled at runtime._"
            .to_string(),
    ];

    let mut excluded = vec![
        format!("{} test", counts.tests),
        format!("{} entry point", counts.entry_points),
        format!(
            "{} in a class extending a base outside the tree",
            counts.external_base
        ),
        format!(
            "{} public API{}",
            counts.public_api,
            if include_public { " (shown)" } else { "" }
        ),
        format!("{} mentioned elsewhere in the source", counts.mentioned),
    ];
    if !include_public && counts.public_api > 0 {
        excluded.push("pass `include_public: true` to include those".to_string());
    }
    body.push(format!(
        "_Also fan-in 0, not reported: {}._",
        excluded.join(", ")
    ));
    // Visibility is only honoured where the parser records it — see
    // `records_visibility`. TypeScript and JavaScript exports are never
    // excluded as public API, so for those files the mention scan is the only
    // thing between an export and this list. Worth saying out loud: a reader
    // seeing a `.ts` export here would otherwise assume public API was
    // filtered for it, as it is for Rust and Java.
    body.push(
        "_Public-API exclusion needs a language that records visibility; TypeScript \
         and JavaScript exports are not excluded, only mention-scanned._"
            .to_string(),
    );

    if by_file.is_empty() {
        body.push(String::new());
        body.push(
            "None — every entity in scope is referenced, or is a test, an entry point, \
             or public API."
                .to_string(),
        );
        return body.join("\n");
    }

    for (file, hits) in worst_files_first(by_file) {
        body.push(String::new());
        body.push(format!("{} ({})", file, hits.len()));
        for e in hits {
            let public = if is_public_api(e) { ", public" } else { "" };
            body.push(format!(
                "  {} `{}` — {}{}",
                e.kind.display_name(),
                e.name,
                metric_suffix(e),
                public
            ));
        }
    }

    cap_lines(body, "Narrow with `path` to a subdirectory.")
}

// ------------------------------------------------------------------
//  assess_change
// ------------------------------------------------------------------

pub fn assess_change(server: &McpServer, args: &Value) -> Result<String> {
    let base_ref = args
        .get("base_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("HEAD");
    let repo_root = &server.root;

    diff::verify_git_repo(repo_root)?;
    let from_sha = diff::resolve_git_ref(repo_root, base_ref)?;
    if from_sha.is_empty() {
        bail!("Cannot resolve git ref '{}'", base_ref);
    }

    // Base side: a resolved SHA pins content, so its graph is immutable
    // and cached across calls with no generation check (MCP-006). The
    // worktree is only materialized on a miss and removed right after
    // analysis — cached file paths still carry the worktree prefix,
    // which `compute_diff` only uses for string stripping.
    let base_key = (from_sha.clone(), server.include_tests);
    let cached = server
        .base_cache
        .lock()
        .unwrap()
        .get(&base_key)
        .map(|(g, d)| (g.clone(), d.clone()));
    let (base_graph, base_dir) = match cached {
        Some(hit) => {
            eprintln!("⚡ base graph cache hit for {}", from_sha);
            hit
        }
        None => {
            let base_dir = std::env::temp_dir().join(format!("mezz-mcp-base-{}", from_sha));
            diff::create_worktree(repo_root, &base_dir, base_ref)?;
            // The working tree's settings decide the scope of both sides.
            // Analyzing the checkout on its own terms would read the
            // `.mezz/settings.json` committed at `base_ref`, and a base that
            // excludes a different set of files than the head reports every
            // file the two disagree about as added or removed.
            let scope =
                diff::build_analysis_config(repo_root, server.include_tests, &server.languages);
            // The subtree corresponding to the analyzed root, not the
            // checkout's top — see [`diff::checkout_root`]. Cached in place
            // of the worktree dir because it is also the prefix
            // `compute_diff` strips, and the two must be the same path.
            let base_root = diff::checkout_root(repo_root, &base_dir);
            let analyzed = diff::analyze_with(
                diff::rooted_at(&scope, &base_root),
                &format!("base ({})", from_sha),
            );
            diff::remove_worktree(repo_root, &base_dir);
            let graph = Arc::new(analyzed?.0);
            let mut cache = server.base_cache.lock().unwrap();
            if cache.len() >= 4 {
                cache.clear();
            }
            cache.insert(base_key, (graph.clone(), base_root.clone()));
            (graph, base_root)
        }
    };

    // Head side: the working tree — shares the warm root-graph cache.
    let head_graph = analyze(server, repo_root)?;
    let result = diff::compute_diff(
        &base_graph,
        &head_graph,
        &base_dir,
        repo_root,
        &from_sha,
        "working",
    );
    let changed = diff::changed_files(repo_root, base_ref);
    Ok(render_change_report(
        &result,
        &base_graph,
        &head_graph,
        base_ref,
        &changed,
        (&base_dir, repo_root),
    ))
}

/// `overview` — the project's domain-level shape from its Elevator
/// (`.elv`) specs: Categories → Features → Functionalities plus
/// cross-cutting Concepts, rendered as the same compact text artifact
/// the `elevator` CLI produces. With `focus`, returns the context
/// bundle for one entity (ancestors, target subtree, siblings,
/// relevant Concepts, `cr:` code pointers) — the onboarding ground
/// floor an agent should read before `map`ping the code.
pub fn overview(server: &McpServer, args: &Value) -> Result<String> {
    let path = resolve_path(server, args)?;
    let graph = analyze(server, &path)?;

    if !graph.entities().any(|e| e.tags.contains("elevator")) {
        bail!(
            "No Elevator (.elv) spec found under {}. `overview` reads the project's \
             domain layer; without .elv files there is nothing to render. Use `map` \
             for code-level structure instead.",
            path.display()
        );
    }

    let body = match args.get("focus").and_then(|v| v.as_str()).map(str::trim) {
        Some(focus) if !focus.is_empty() => {
            crate::output::elevator_text_renderer::render_focus(&graph, focus)
        }
        _ => crate::output::elevator_text_renderer::render_elevator_text(&graph, None),
    };

    let full = format!(
        "{}\n{}",
        crate::output::elevator_text_renderer::LEGEND,
        body
    );
    let lines: Vec<String> = full.lines().map(String::from).collect();
    Ok(cap_lines(
        lines,
        "Pass focus=<entity> to narrow to one Feature or Functionality.",
    ))
}

/// Format a metric value that is conceptually an integer for most metrics.
fn fmt_num(v: f64) -> String {
    if v.fract().abs() < 1e-9 {
        format!("{}", v as i64)
    } else {
        format!("{:.2}", v)
    }
}

/// True if changed path `changed` falls under the `cr:` reference
/// `cr` — exact file match, or `cr` is a directory prefix of it.
/// Both are repo-root-relative with `/` separators. A bare directory
/// prefix like `src/proto` must not swallow `src/protocol/x` — hence
/// the trailing-slash boundary on the prefix branch.
pub(super) fn path_under(cr: &str, changed: &str) -> bool {
    let cr = cr.trim_start_matches("./").trim_end_matches('/');
    let changed = changed.trim_start_matches("./");
    !cr.is_empty() && (changed == cr || changed.starts_with(&format!("{cr}/")))
}

/// MCP-009 — join the diff's changed paths against the Elevator spec's
/// `cr:` references: one line per spec entity whose claimed path the
/// change touched, nudging the agent to re-check the spec description
/// while the knowledge is fresh. Silent when no spec exists or the
/// change touches no claimed path. Deterministic regardless of graph
/// enumeration order (lines collected into a sorted set).
fn spec_claims(head_graph: &DependencyGraph, changed: &[String]) -> Vec<String> {
    use crate::output::elevator_code_map::{parse_cr_attr, short_ref};
    let mut lines: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in head_graph
        .entities()
        .filter(|e| e.tags.contains("elevator"))
    {
        for attr in &e.attributes {
            let Some((_, cr_path)) = parse_cr_attr(attr) else {
                continue;
            };
            if changed.iter().any(|c| path_under(&cr_path, c)) {
                lines.insert(format!(
                    "- {} claims {} — is its description still true?",
                    short_ref(e),
                    cr_path
                ));
            }
        }
    }
    lines.into_iter().collect()
}

/// What the informational section means, said once instead of per row.
///
/// The per-smell hint is written in the imperative — correct advice when you
/// already decided the smell is a defect, and wrong here, where the finding is
/// an observation about a threshold.
const INFORMATIONAL_NOTE: &str =
    "Observations, not regressions. A create-spec, a row mirror or a config \
     record is meant to be flat — Introduce Parameter Object produces one — so \
     group the fields only where they have a natural hierarchy.";

/// Fields and methods behind a container smell.
///
/// A count next to the finding lets a reader see the rule is a threshold and
/// not a judgement, which is the difference between a fact they note and a
/// verdict they have to argue with.
fn container_shape(e: Option<&CodeEntity>) -> String {
    let Some(fields) = e.and_then(|e| e.metrics.field_count) else {
        return String::new();
    };
    let methods = e.map(|e| e.metrics.method_count).unwrap_or(0);
    format!(
        " — {} field{}, {} method{}",
        fields,
        if fields == 1 { "" } else { "s" },
        methods,
        if methods == 1 { "" } else { "s" }
    )
}

/// The smell lists a change report keeps, and what it deliberately left
/// out of them.
///
/// One collector rather than three loose vectors so the fourth thing — the
/// count of smells on test code — has somewhere to live that does not cost
/// `render_change_report` another local.
#[derive(Default)]
struct SmellChurn {
    red: Vec<String>,
    informational: Vec<String>,
    resolved: Vec<String>,
    /// New smells on test code: counted here, listed nowhere. See
    /// [`smells_aside`].
    in_tests: usize,
}

impl SmellChurn {
    /// File one new smell under the heading its severity belongs to.
    ///
    /// Informational rows carry their counts and drop the imperative hint;
    /// the section carries the caveat for all of them. Test code takes
    /// neither heading and is counted instead — a correct unit test cannot
    /// help looking envious of the type it exercises.
    fn file_new(&mut self, smell: SmellKind, loc: &str, head: Option<&CodeEntity>, is_test: bool) {
        if is_test {
            self.in_tests += 1;
        } else if smell.is_informational() {
            self.informational.push(format!(
                "- {} on {}{}",
                smell.label(),
                loc,
                container_shape(head)
            ));
        } else {
            self.red
                .push(format!("- {} on {} — {}", smell.label(), loc, smell.hint()));
        }
    }
}

/// One titled section, omitted when it has no rows.
fn titled_rows(out: &mut Vec<String>, title: String, rows: Vec<String>, note: Option<&str>) {
    if rows.is_empty() {
        return;
    }
    out.push(String::new());
    out.push(format!("## {}", title));
    out.extend(rows);
    if let Some(note) = note {
        out.push(String::new());
        out.push(note.to_string());
    }
}

/// The smell churn of a change report, red flags and observations apart.
///
/// They used to share one `⚠ New smells` list, one marker and one imperative
/// voice. A field report caught what that costs: `assess_change` recorded a
/// function going from ten parameters to three, and eleven lines later told the
/// reader to break up the parameter object that did it — advice a reviewer on a
/// cold context would have complied with, undoing the improvement the same
/// report measured.
fn smell_sections(churn: SmellChurn) -> Vec<String> {
    let mut out = Vec::new();
    let heading = smells_aside(churn.red.len(), churn.in_tests, "⚠ New smells");
    // A count with no rows still gets its heading. `⚠ New smells (0, plus 7
    // in test code)` is the *whole* finding for a change that added a
    // well-tested module, and `titled_rows` would drop it as empty — taking
    // with it the one line that explains why the reader is not being shown
    // seven Feature Envy warnings about their own tests.
    match churn.red.is_empty() && churn.in_tests > 0 {
        true => out.extend([String::new(), format!("## {heading}")]),
        false => titled_rows(&mut out, heading, churn.red, None),
    }
    titled_rows(
        &mut out,
        format!("Informational ({})", churn.informational.len()),
        churn.informational,
        Some(INFORMATIONAL_NOTE),
    );
    titled_rows(
        &mut out,
        format!("Resolved smells ({})", churn.resolved.len()),
        churn.resolved,
        None,
    );
    out
}

const MAX_MODIFIED_LISTED: usize = 40;
const MAX_STATUS_LISTED: usize = 30;

/// The file a row's base half came from, when the two ends being compared
/// are not the same file.
///
/// A cross-file pair is the one case where a metric delta need not be an
/// edit. The move pass matches on everything but the path, so two same-named
/// callables relocated in one commit are one candidate group, and in a
/// language that does not annotate its parameters nothing in the signature
/// separates them — `diff::match_relocated` settles it on the file name, but
/// a file renamed as well as moved still falls through to a positional
/// tie-break. When that goes wrong the difference between two untouched
/// bodies is reported as a regression on both, and the row gives a reader no
/// hint that a move was involved at all.
///
/// Naming the other end makes the pairing checkable: `+30 … (was bodymap.py)`
/// reads as a match, where the same row unmarked sent an agent to "fix" a
/// function that had not changed since the base.
pub(crate) fn moved_note(d: &diff::EntityDiff) -> String {
    match d.moved_from.as_deref() {
        Some(was) => format!(" (was {was})"),
        None => String::new(),
    }
}

/// Where a row sits, in the one spelling every listing uses.
fn row_loc(d: &diff::EntityDiff) -> String {
    format!("{} `{}` — {}{}", d.kind, d.name, d.file_path, moved_note(d))
}

/// The listed rows of one status, in the order the diff produced them.
fn of_status<'d>(
    result: &'d diff::DiffResult,
    status: diff::ChangeStatus,
    skip: &dyn Fn(&diff::EntityDiff) -> bool,
) -> Vec<&'d diff::EntityDiff> {
    result
        .entities
        .iter()
        .filter(|d| d.status == status && !skip(d))
        .collect()
}

/// How much worse the change left one entity, for ordering the triage queue.
///
/// Growth is summed per metric with negatives clamped away, so an improvement
/// in one metric cannot net off a regression in another: a function that shed
/// branching while deepening its nesting still has to be looked at, and would
/// sort last if the two were allowed to cancel.
fn regression_score(d: &diff::EntityDiff) -> f64 {
    d.metric_deltas
        .iter()
        .filter(|m| matches!(m.name.as_str(), "cyclomatic" | "max_nesting"))
        .map(|m| m.delta.max(0.0))
        .sum()
}

/// A modified row's metric movements, or a note that none moved — an empty
/// delta list reads as missing data rather than as an entity that held still.
fn metric_movements(d: &diff::EntityDiff) -> String {
    let moved = d
        .metric_deltas
        .iter()
        .map(|m| {
            format!(
                "{} {}→{} ({}{})",
                m.name,
                m.old.map(fmt_num).unwrap_or_else(|| "-".into()),
                m.new.map(fmt_num).unwrap_or_else(|| "-".into()),
                if m.delta > 0.0 { "+" } else { "" },
                fmt_num(m.delta)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if moved.is_empty() {
        "source changed, metrics stable".to_string()
    } else {
        moved
    }
}

/// One listing: a heading carrying the whole population's count, the first
/// `cap` rows, and a note naming what the cap dropped.
///
/// The heading's count and the note's count are the two numbers a reader
/// checks against each other, so they are produced together here rather than
/// at each call site — a cap that silently drops rows reads as a clean bill
/// of health.
fn listing(
    rows: &[&diff::EntityDiff],
    title: &str,
    cap: usize,
    noun: &str,
    row: impl Fn(&diff::EntityDiff) -> String,
) -> Vec<String> {
    let mut body = vec![String::new(), format!("## {} ({})", title, rows.len())];
    body.extend(rows.iter().take(cap).map(|d| row(d)));
    if rows.len() > cap {
        body.push(format!("… and {} more {}.", rows.len() - cap, noun));
    }
    body
}

/// Entities the change moved without touching: fan-in or fan-out shifted,
/// source did not. A heading only when something sits under it — an empty
/// section reads as a check that was made and came back clean, which is not
/// what happened.
fn ripple_heading(rippled: &[&diff::EntityDiff]) -> Vec<String> {
    if rippled.is_empty() {
        return Vec::new();
    }
    vec![
        String::new(),
        format!(
            "## Coupling ripples ({} entities with fan-in/out shifts, no source change)",
            rippled.len()
        ),
    ]
}

/// The change's entities, one listing per status, in the order a reader
/// triages them: what got worse, what arrived, what left, and what shifted
/// underneath without being edited.
///
/// The modified rows split in two — those whose source changed lead the
/// report worst-growth-first, and the remainder are the coupling ripples that
/// close it — so the split happens once here rather than in two passes that
/// could disagree about which rows are listed at all.
fn entity_listings(
    result: &diff::DiffResult,
    head_by_id: &HashMap<&str, &CodeEntity>,
    skip: &dyn Fn(&diff::EntityDiff) -> bool,
) -> Vec<String> {
    let (mut edited, rippled): (Vec<_>, Vec<_>) =
        of_status(result, diff::ChangeStatus::Modified, skip)
            .into_iter()
            .partition(|d| d.source_changed);
    edited.sort_by(|a, b| regression_score(b).total_cmp(&regression_score(a)));

    let mut body = listing(
        &edited,
        "Modified",
        MAX_MODIFIED_LISTED,
        "modified entities",
        |d| format!("- {}: {}", row_loc(d), metric_movements(d)),
    );
    // Added rows carry their metrics, so an oversized newcomer stands out at
    // a glance — but only where the head graph resolves the row. One it
    // cannot place still appears, because dropping it would under-report.
    body.extend(listing(
        &of_status(result, diff::ChangeStatus::Added, skip),
        "Added",
        MAX_STATUS_LISTED,
        "added entities",
        |d| match head_by_id.get(d.entity_id.as_str()) {
            Some(e) => format!("- {} ({})", row_loc(d), metric_suffix(e)),
            None => format!("- {}", row_loc(d)),
        },
    ));
    // Removed rows are names only: the entity a metric would describe is gone
    // from the head graph, and any number printed beside it would be the base's.
    body.extend(listing(
        &of_status(result, diff::ChangeStatus::Removed, skip),
        "Removed",
        MAX_STATUS_LISTED,
        "removed entities",
        |d| format!("- {}", row_loc(d)),
    ));
    body.extend(ripple_heading(&rippled));
    body
}

pub(crate) fn render_change_report(
    result: &diff::DiffResult,
    base_graph: &DependencyGraph,
    head_graph: &DependencyGraph,
    base_ref: &str,
    changed_files: &[String],
    // Where each side was analysed, base first. The base graph comes from a
    // throwaway worktree, so joining the two by path means stripping each
    // root before comparing.
    roots: (&Path, &Path),
) -> String {
    let base_by_id: HashMap<&str, &CodeEntity> =
        base_graph.entities().map(|e| (e.id.as_str(), e)).collect();
    let head_by_id: HashMap<&str, &CodeEntity> =
        head_graph.entities().map(|e| (e.id.as_str(), e)).collect();

    // Keep only rows whose underlying entity belongs in an agent-facing
    // listing (compute_diff itself only excludes Parameters): drops File
    // rows (they duplicate their children), ghosts, fields, imports.
    let skip_row = |d: &diff::EntityDiff| {
        head_by_id
            .get(d.entity_id.as_str())
            .or_else(|| {
                d.base_entity_id
                    .as_deref()
                    .and_then(|id| base_by_id.get(id))
            })
            .map(|e| !is_listed(e))
            .unwrap_or(d.kind == "File")
    };

    let mut body = vec![format!(
        "# Change assessment: {} ({}) → working tree",
        base_ref, result.from_ref
    )];
    body.extend(headline(result, &skip_row));

    // Smell churn, joined across the two graphs by entity id.
    let mut churn = SmellChurn::default();
    let test_code = TestCode::of(head_graph, roots.1);
    for d in &result.entities {
        if d.status == diff::ChangeStatus::Removed || skip_row(d) {
            continue;
        }
        let head_smells: HashSet<SmellKind> = head_by_id
            .get(d.entity_id.as_str())
            .map(|e| e.metrics.smells.iter().copied().collect())
            .unwrap_or_default();
        let base_smells: HashSet<SmellKind> = d
            .base_entity_id
            .as_deref()
            .and_then(|id| base_by_id.get(id))
            .map(|e| e.metrics.smells.iter().copied().collect())
            .unwrap_or_default();
        let head = head_by_id.get(d.entity_id.as_str()).copied();
        for smell in head_smells.difference(&base_smells) {
            churn.file_new(*smell, &row_loc(d), head, test_code.holds(head));
        }
        for smell in base_smells.difference(&head_smells) {
            churn
                .resolved
                .push(format!("- {} on {}", smell.label(), row_loc(d)));
        }
    }
    body.extend(shape_moves(base_graph, head_graph, roots));
    body.extend(moved_files(result, &skip_row));
    body.extend(possible_renames(result, &base_by_id, &head_by_id, &skip_row));

    body.extend(smell_sections(churn));

    body.extend(entity_listings(result, &head_by_id, &skip_row));

    // Spec claims (MCP-009): entities whose `cr:`-claimed paths the
    // change touched — the reminder lands at the one moment the domain
    // knowledge is fresh. Silent when no spec or no claimed path is hit.
    let claims = spec_claims(head_graph, changed_files);
    if !claims.is_empty() {
        body.push(String::new());
        body.push(format!("## Spec claims ({})", claims.len()));
        for line in claims.iter().take(25) {
            body.push(line.clone());
        }
        if claims.len() > 25 {
            body.push(format!(
                "… and {} more claimed entities.",
                claims.len() - 25
            ));
        }
    }

    cap_lines(
        body,
        "Narrow the change or assess per-folder with `quality`.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            Self::with_prefix("mezz-mcp-test", name)
        }

        /// `dead_code` classifies by path, and the default prefix contains
        /// "test" — every entity in such a fixture would be filtered as
        /// test code. Fixtures that must read as production source pick
        /// their own prefix.
        fn with_prefix(prefix: &str, name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}-{}",
                prefix,
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ));
            fs::create_dir_all(&path).unwrap();
            TmpDir(path)
        }

        fn write(&self, rel: &str, body: &str) {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&full, body).unwrap();
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn server_for(dir: &TmpDir) -> McpServer {
        McpServer {
            root: dir.0.canonicalize().unwrap(),
            include_tests: false,
            languages: None,
            graph_cache: std::sync::Mutex::new(HashMap::new()),
            base_cache: std::sync::Mutex::new(HashMap::new()),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            shape_baselines: Default::default(),
            rules_spelled_out: Default::default(),
            layout_caveat_spelled_out: Default::default(),
        }
    }

    fn shaped(pattern: ShapePattern, blocker: crate::models::ShapeBlocker) -> FolderShape {
        FolderShape {
            pattern,
            compliance: 0.80,
            acyclicity: 1.0,
            layering: Some(0.80),
            arborescence: Some(0.80),
            entry_concentration: Some(0.80),
            egress: None,
            child_compliance: Some(0.90),
            uniformity: None,
            child_count: 4,
            blocker: Some(blocker),
            terms: crate::models::ShapeTerms::default(),
        }
    }

    /// Depth beats severity, which is the whole reordering. A tangled
    /// leaf leads a cyclic root because the root's child gates are
    /// answered by work done below it, and an agent handed the root first
    /// spends the session against a gate that will not open.
    #[test]
    fn the_work_list_leads_with_the_deepest_folder() {
        let leaf = shaped(ShapePattern::Tangled, crate::models::ShapeBlocker::Layering(0.5));
        let root = shaped(ShapePattern::Cyclic, crate::models::ShapeBlocker::Cycles(0.2));
        let mut folders = [
            (String::new(), &root),
            ("src/parser/rust".to_string(), &leaf),
            ("src".to_string(), &leaf),
        ];
        folders.sort_by(work_order);
        let order: Vec<&str> = folders.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(order, vec!["src/parser/rust", "src", ""]);
    }

    /// Among folders nothing links, the ordinary reading returns.
    #[test]
    fn siblings_fall_back_to_worst_first() {
        let cyclic = shaped(ShapePattern::Cyclic, crate::models::ShapeBlocker::Cycles(0.2));
        let tangled = shaped(ShapePattern::Tangled, crate::models::ShapeBlocker::Layering(0.5));
        let mut folders = [
            ("src/b".to_string(), &tangled),
            ("src/a".to_string(), &cyclic),
        ];
        folders.sort_by(work_order);
        assert_eq!(folders[0].0, "src/a", "cyclic outranks tangled at equal depth");
    }

    /// An analysis with no entities and no imports — enough for the
    /// unresolved-import marker, which is all `whole_tree` asks a graph.
    fn bare_graph() -> DependencyGraph {
        DependencyGraph::from_analysis(&crate::analyzer::AnalysisResult {
            entities: Vec::new(),
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    /// The root has always been scored and was never printed. Both lists
    /// under the tally are ranked; `work_order` sorts deepest-first, so
    /// depth zero comes last of however many folders fall short, and the
    /// list is then cut at `top`. A repository whose top-level folders sat
    /// in a dependency loop reported that loop nowhere and sent the reader
    /// to a leaf parser folder instead.
    #[test]
    fn the_top_level_is_stated_even_though_the_work_list_ranks_it_last() {
        let root = shaped(ShapePattern::Cyclic, crate::models::ShapeBlocker::Cycles(0.43));
        let leaf = shaped(ShapePattern::Tangled, crate::models::ShapeBlocker::Layering(0.5));
        let scored = [
            (String::new(), &root),
            ("src/parser/rust".to_string(), &leaf),
        ];

        let out = whole_tree(&scored, &bare_graph(), Path::new("")).join("\n");
        assert!(out.contains("### The top level"), "{out}");
        assert!(out.contains("(root)"), "{out}");
        assert!(out.contains("cyclic"), "{out}");
        assert!(out.contains("a loop among its children"), "{out}");
    }

    /// A scope whose folder metrics do not reach a common root — nothing to
    /// state, and a heading over nothing is worse than no heading.
    #[test]
    fn a_scope_without_a_root_folder_states_no_top_level() {
        let leaf = shaped(ShapePattern::Tangled, crate::models::ShapeBlocker::Layering(0.5));
        let scored = [("src".to_string(), &leaf)];
        assert!(whole_tree(&scored, &bare_graph(), Path::new("")).is_empty());
    }

    /// Stated above and ranked below would say it twice, and the second
    /// telling would be the one that reads as the plan.
    #[test]
    fn the_top_level_is_not_repeated_in_the_ranked_work_list() {
        let root = shaped(ShapePattern::Cyclic, crate::models::ShapeBlocker::Cycles(0.43));
        let leaf = shaped(ShapePattern::Tangled, crate::models::ShapeBlocker::Layering(0.5));
        let scored = [
            (String::new(), &root),
            ("src/parser/rust".to_string(), &leaf),
        ];
        let short: Vec<&str> = scored
            .iter()
            .filter(|(dir, s)| !dir.is_empty() && s.pattern < ShapePattern::Fractal)
            .map(|(dir, _)| dir.as_str())
            .collect();
        assert_eq!(short, vec!["src/parser/rust"]);
    }

    /// The root is depth 0 and must not be mistaken for a leaf by a
    /// component count that counts the empty string as one.
    #[test]
    fn the_assessed_root_is_depth_zero() {
        assert_eq!(depth(""), 0);
        assert_eq!(depth("src"), 1);
        assert_eq!(depth("src/parser/rust"), 3);
    }

    /// The triage is the list an agent works from *before* it calls
    /// `reshape`, so a folder whose numbers came off an incomplete drawing
    /// has to say so here — and the note has to name the direction, because a
    /// missing edge can make a folder read *better*: a door nothing is
    /// recorded as reaching is not counted as a door.
    #[test]
    fn a_folder_with_unresolved_imports_is_marked_in_the_triage() {
        let entities = vec![
            crate::models::CodeEntity::new(
                "usesTwo",
                EntityKind::Function,
                "src/a/one.ts",
                crate::models::Span::from_positions(1, 0, 1, 0),
            ),
            crate::models::CodeEntity::new(
                "fromTwo",
                EntityKind::Function,
                "src/b/two.ts",
                crate::models::Span::from_positions(1, 0, 1, 0),
            ),
        ];
        let graph = DependencyGraph::from_analysis(&crate::analyzer::AnalysisResult {
            entities,
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: vec![crate::models::ImportSite {
                from: std::path::PathBuf::from("src/a/one.ts"),
                to: std::path::PathBuf::from("src/b/two.ts"),
                line: 0,
                is_reexport: false,
                is_type_only: false,
            }],
            warnings: Vec::new(),
        });

        assert_eq!(
            unsound_marker(&graph, Path::new(""), "src/a"),
            " · ⚠ 1 unresolved"
        );
        // A folder the unresolved import does not touch is left alone, or the
        // marker would mean nothing.
        assert_eq!(unsound_marker(&graph, Path::new(""), "src/z"), "");

        let note = unsound_footnote(&[("src/a".to_string(), &shape_of())], &graph, Path::new(""))
            .join("\n");
        assert!(note.contains("read better than the truth"), "{note}");
    }

    fn shape_of() -> FolderShape {
        FolderShape {
            pattern: ShapePattern::Tangled,
            compliance: 0.5,
            acyclicity: 1.0,
            layering: Some(0.5),
            arborescence: Some(0.5),
            entry_concentration: Some(0.5),
            egress: None,
            child_compliance: None,
            uniformity: None,
            child_count: 2,
            blocker: None,
            terms: crate::models::ShapeTerms::default(),
        }
    }

    /// An empty work list has to say so rather than print a heading over
    /// nothing: "every remaining folder is waiting on something else" is
    /// a different situation from "there is no work", and an agent that
    /// cannot tell them apart invents work.
    #[test]
    fn an_empty_work_list_says_where_the_work_went() {
        let out = start_here_group(&[], 10, &DependencyGraph::default(), Path::new("")).join("\n");
        assert!(out.contains("Start here — none"), "{out}");
        assert!(out.contains("waiting on something outside"), "{out}");
    }

    /// The blocked group is a count and a sample, never the whole list —
    /// it is not the work list and must not compete with it for lines.
    #[test]
    fn the_blocked_group_truncates_and_says_by_how_much() {
        let blocked = shaped(ShapePattern::Hierarchical, crate::models::ShapeBlocker::Entry(0.5));
        let folders: Vec<(String, &FolderShape)> = (0..SHAPE_BLOCKED_SHOWN + 3)
            .map(|i| (format!("src/f{i}"), &blocked))
            .collect();
        let out = blocked_group(&folders, &DependencyGraph::default(), Path::new("")).join("\n");
        assert!(out.contains(&format!("({})", SHAPE_BLOCKED_SHOWN + 3)), "{out}");
        assert!(out.contains("… and 3 more."), "{out}");
        assert!(!out.contains("src/f7"), "past the cap must not print: {out}");
    }

    const LONG_DESC: &str = "Structured prompt with enforced step sequence and typed responses, callable mid-conversation by an author.";

    const SPEC: &str = r#"
c library {
    d: "Reusable building blocks."
    f protocol
}

f protocol {
    d: "Structured prompt with enforced step sequence and typed responses, callable mid-conversation by an author."
    cr: "src/protocol/"
    fu creation
}

fu f.protocol.creation {
    d: "Create a protocol."
}
"#;

    /// The same two reports, end to end through `quality` on a real tree:
    /// a type, its methods, and a `#[cfg(test)] mod tests` exercising it.
    /// Inline test modules live inside admitted source files, so
    /// `include_tests: false` never sees them and the path heuristic
    /// cannot either — the module name is the only thing that can.
    #[test]
    fn quality_counts_inline_test_module_smells_apart_from_the_code() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "inline-module");
        dir.write(
            "src/lib.rs",
            r#"
pub struct Ring { items: Vec<u32>, cap: usize }

impl Ring {
    pub fn new(cap: usize) -> Self { Ring { items: Vec::new(), cap } }
    pub fn push(&mut self, v: u32) { self.items.push(v); if self.items.len() > self.cap { self.items.remove(0); } }
    pub fn len(&self) -> usize { self.items.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_ring_is_bounded() {
        let mut r = Ring::new(2);
        r.push(1);
        r.push(2);
        r.push(3);
        assert_eq!(r.len(), 2);
    }
}
"#,
        );

        // Both spellings of the same question. The reporter narrowed to one
        // file, which takes the single-file analysis path — no `Contains`
        // edges, so an ancestor walk over them finds nothing. See
        // [`parent_of`].
        for path in ["src", "src/lib.rs"] {
            assert_test_smells_are_set_aside(
                quality(&server_for(&dir), &json!({ "path": path }))
                    .unwrap()
                    .text(),
            );
        }
    }

    /// The shape both `quality` calls above have to produce.
    fn assert_test_smells_are_set_aside(out: &str) {
        let (smells, ranking) = out.split_once("## Top").expect("both sections render");
        assert!(
            smells.contains("## Smells (0, plus 1 in test code — not listed)"),
            "the test function is still filed as a production smell:\n{smells}"
        );
        assert!(
            !smells.contains("Move this method"),
            "a #[test] function is still being handed a remediation hint:\n{smells}"
        );
        // The ranking keeps it — a slow test suite is worth ranking — but
        // says what it is, so `⚠ Feature Envy` is not read as an instruction.
        assert!(
            ranking.contains("`the_ring_is_bounded`") && ranking.contains("⚠ Feature Envy, test"),
            "the ranked test row is unmarked:\n{ranking}"
        );
        assert!(
            !ranking.contains("`push` — lib.rs:6 (L6, loc 1, cx 2, cog 1, ws 3, in 1, out 3, 0 of it identified, test"),
            "production code was marked as test code:\n{ranking}"
        );
    }

    /// Field reports, 2026-08-31, from `quality` and from `assess_change`:
    /// seven of seven new smells on `#[test]` functions inside one
    /// `#[cfg(test)] mod tests`, none on the nineteen production functions
    /// above them. A unit test exercises one type and so interacts more
    /// with that type than with itself, which is the exact shape Feature
    /// Envy fires on — the remediation hint told the reader to move their
    /// test into the production type.
    ///
    /// The count is the other half. A section that quietly shrank would
    /// trade one silence for another.
    #[test]
    fn new_smells_on_test_code_are_counted_and_never_listed() {
        let mut churn = SmellChurn::default();
        churn.file_new(
            SmellKind::FeatureEnvy,
            "function `the_ring_is_bounded` — src/activity.rs",
            None,
            true,
        );
        churn.file_new(
            SmellKind::FeatureEnvy,
            "function `since_returns_only_what_came_after` — src/activity.rs",
            None,
            true,
        );

        let body = smell_sections(churn).join("\n");
        assert!(
            body.contains("## ⚠ New smells (0, plus 2 in test code — not listed)"),
            "the count went missing with the rows:\n{body}"
        );
        assert!(
            !body.contains("the_ring_is_bounded") && !body.contains("Move this method"),
            "a test function is still being handed a remediation hint:\n{body}"
        );
    }

    /// The production half is untouched, and the aside is silent when
    /// there is nothing to set aside — a caveat on every report is one
    /// nobody reads.
    #[test]
    fn a_report_with_no_test_smells_reads_exactly_as_before() {
        let mut churn = SmellChurn::default();
        churn.file_new(
            SmellKind::FeatureEnvy,
            "function `run_diff` — src/server/diff_handler.rs",
            None,
            false,
        );
        let body = smell_sections(churn).join("\n");
        assert!(
            body.contains("## ⚠ New smells (1)") && !body.contains("test code"),
            "{body}"
        );
    }

    /// Field report, 2026-08-27: `assess_change` recorded `param_count 10→3`
    /// and, eleven lines later, filed the parameter object that did it under
    /// `⚠ New smells` with "group related fields into nested sub-structs" —
    /// the standard fix for the thing it had just measured, reported as the
    /// defect. Red flags and observations must not share a heading.
    #[test]
    fn a_data_bag_is_an_observation_and_not_a_new_smell() {
        let mut bag = CodeEntity::new(
            "NewRow".to_string(),
            EntityKind::Struct,
            "src/lib.rs".to_string(),
            crate::models::Span::default(),
        );
        bag.metrics.field_count = Some(10);
        bag.metrics.method_count = 1;

        let mut churn = SmellChurn::default();
        churn.file_new(
            SmellKind::DataBag,
            "struct `NewRow` — src/lib.rs",
            Some(&bag),
            false,
        );
        churn.file_new(
            SmellKind::ShotgunSurgery,
            "struct `McpServer` — src/mcp/mod.rs",
            None,
            false,
        );

        let body = smell_sections(churn).join("\n");
        assert!(
            body.contains("## ⚠ New smells (1)"),
            "only the red smell is counted as new:\n{body}"
        );
        assert!(
            body.contains("## Informational (1)"),
            "the data bag gets its own heading:\n{body}"
        );
        let (alarm, note) = body.split_once("## Informational").unwrap();
        assert!(
            !alarm.contains("Data Bag"),
            "the data bag must not appear under the alarm heading:\n{alarm}"
        );
        assert!(
            note.contains("10 fields, 1 method"),
            "the counts the threshold fired on travel with the row:\n{note}"
        );
        assert!(
            !note.contains("Group related fields into nested sub-structs"),
            "the imperative remediation is not the voice for an observation:\n{note}"
        );
    }

    #[test]
    fn overview_renders_full_domain_map_with_legend() {
        let dir = TmpDir::new("overview-map");
        dir.write("spec.elv", SPEC);
        let out = overview(&server_for(&dir), &json!({})).unwrap();
        assert!(
            out.contains("# Elevator spec format"),
            "legend missing:\n{out}"
        );
        assert!(out.contains("c library"), "category missing:\n{out}");
        assert!(out.contains("f protocol"), "feature missing:\n{out}");
    }

    #[test]
    fn overview_focus_marks_target_and_keeps_full_descriptions() {
        let dir = TmpDir::new("overview-focus");
        dir.write("spec.elv", SPEC);
        let out = overview(&server_for(&dir), &json!({"focus": "f.protocol"})).unwrap();
        assert!(out.contains("← target"), "target marker missing:\n{out}");
        assert!(
            out.contains(LONG_DESC),
            "focus must render `d:` unclipped:\n{out}"
        );
        assert!(
            out.contains("[cr: src/protocol/]"),
            "code reference missing:\n{out}"
        );
    }

    #[test]
    fn overview_without_spec_says_so() {
        let dir = TmpDir::new("overview-empty");
        dir.write("main.rs", "fn main() {}\n");
        let err = overview(&server_for(&dir), &json!({})).unwrap_err();
        assert!(
            err.to_string().contains("No Elevator"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn path_under_matches_files_and_dir_prefixes_not_bare_prefixes() {
        // Exact file claim.
        assert!(path_under("src/main.rs", "src/main.rs"));
        // Directory prefix, with and without trailing slash on the cr ref.
        assert!(path_under("src/protocol/", "src/protocol/x.rs"));
        assert!(path_under("src/protocol", "src/protocol/nested/deep.rs"));
        // `./` prefixes on either side are normalized away.
        assert!(path_under("./src/protocol/", "src/protocol/x.rs"));
        // A bare string prefix that is not a path boundary must not match.
        assert!(!path_under("src/proto", "src/protocol/x.rs"));
        // Unrelated path.
        assert!(!path_under("src/protocol/", "docs/readme.md"));
        // Empty cr claims nothing (degenerate).
        assert!(!path_under("", "anything.rs"));
    }

    // `f protocol` claims `src/protocol/` in SPEC; use that for the join.
    #[test]
    fn spec_claims_flags_entity_when_change_touches_claimed_path() {
        let dir = TmpDir::new("claims-hit");
        dir.write("spec.elv", SPEC);
        let graph = analyze(&server_for(&dir), &dir.0.canonicalize().unwrap()).unwrap();

        let changed = vec!["src/protocol/builder.rs".to_string()];
        let lines = spec_claims(&graph, &changed);
        assert_eq!(lines.len(), 1, "expected one claim line:\n{lines:#?}");
        assert!(
            lines[0].contains("f protocol claims src/protocol/"),
            "wrong claim line: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("is its description still true?"),
            "missing nudge: {}",
            lines[0]
        );
    }

    #[test]
    fn spec_claims_silent_when_change_touches_no_claimed_path() {
        let dir = TmpDir::new("claims-miss");
        dir.write("spec.elv", SPEC);
        let graph = analyze(&server_for(&dir), &dir.0.canonicalize().unwrap()).unwrap();

        let changed = vec!["docs/readme.md".to_string(), "src/other/x.rs".to_string()];
        assert!(spec_claims(&graph, &changed).is_empty());
        // And a repo with no spec at all yields nothing.
        let bare = TmpDir::new("claims-nospec");
        bare.write("main.rs", "fn main() {}\n");
        let bare_graph = analyze(&server_for(&bare), &bare.0.canonicalize().unwrap()).unwrap();
        assert!(spec_claims(&bare_graph, &changed).is_empty());
    }

    // ---- settings (CFG-011) ---------------------------------------

    /// CFG-011, end to end. `mezz mcp` was the one entry point that never
    /// opened `.mezz/settings.json`, with no warning and no way to tell from
    /// a response which configuration produced it — so a repo that had asked
    /// mezz to leave a directory out got a `map` listing it.
    #[test]
    fn map_leaves_out_what_the_repo_settings_file_excludes() {
        let dir = TmpDir::new("settings-excludes");
        dir.write("src/core/tree.rs", "pub fn kept_by_the_scope() {}\n");
        dir.write(
            "src/generated/schema.rs",
            "pub fn excluded_by_the_file() {}\n",
        );
        dir.write(
            ".mezz/settings.json",
            r#"{"exclude_patterns": ["**/generated/**"]}"#,
        );

        let out = map(&code_server_for(&dir), &json!({})).unwrap().into_text();
        assert!(
            out.contains("kept_by_the_scope"),
            "the fixture never analyzed at all:\n{out}"
        );
        assert!(
            !out.contains("excluded_by_the_file"),
            "the settings file was ignored — the excluded file is in the map:\n{out}"
        );
    }

    /// The same file, reached through a subdirectory `path` — the ordinary
    /// way an agent narrows. The settings file is the repo's, so it is read
    /// at the server root; looked for under the analyzed path instead, it
    /// would have to sit inside `src/` to exist at all.
    #[test]
    fn a_subdirectory_map_is_held_to_the_same_scope() {
        let dir = TmpDir::new("settings-excludes-subdir");
        dir.write("src/core/tree.rs", "pub fn kept_by_the_scope() {}\n");
        dir.write(
            "src/generated/schema.rs",
            "pub fn excluded_by_the_file() {}\n",
        );
        dir.write(
            ".mezz/settings.json",
            r#"{"exclude_patterns": ["**/generated/**"]}"#,
        );

        let out = map(&code_server_for(&dir), &json!({"path": "src"})).unwrap().into_text();
        assert!(
            out.contains("kept_by_the_scope"),
            "the subdirectory never analyzed at all:\n{out}"
        );
        assert!(
            !out.contains("excluded_by_the_file"),
            "narrowing to a subdirectory dropped the repo's scope:\n{out}"
        );
    }

    // ---- similar (MCP-012) ----------------------------------------

    /// Every fixture lives under a temp path containing "test", which the
    /// walker's test heuristic excludes wholesale. A fixture made of real
    /// source has to opt back in or the graph comes back empty.
    fn code_server_for(dir: &TmpDir) -> McpServer {
        McpServer {
            include_tests: true,
            ..server_for(dir)
        }
    }

    /// The review fixture: one right answer, plus the `toRef`-class
    /// entities that used to ride in behind it on the word "to".
    fn similar_fixture(name: &str) -> TmpDir {
        let dir = TmpDir::new(name);
        dir.write(
            "diff.rs",
            r#"
use std::path::Path;

pub fn resolve_git_ref(repo_root: &Path, git_ref: &str) -> String {
    String::new()
}

pub fn resolve_child_qualname(parent: &str, child: &str) -> String {
    String::new()
}
"#,
        );
        dir.write(
            "panel.ts",
            r#"
export const toRef = "HEAD";

export function reportDiff(state: {
    fromRef: string;
    toRef: string;
    files: string[];
    additions: number;
    deletions: number;
}): void {}
"#,
        );
        dir
    }

    #[test]
    fn similar_ranks_the_match_and_drops_stopword_only_hits() {
        let dir = similar_fixture("similar-stopwords");
        let out = similar(
            &code_server_for(&dir),
            &json!({"query": "resolve git ref to sha", "top": 6}),
        )
        .unwrap();
        assert!(
            out.contains("resolve_git_ref"),
            "lost the real match:\n{out}"
        );
        assert!(
            !out.contains("toRef"),
            "a hit carried by the word `to` survived:\n{out}"
        );
        // `top: 6` is a ceiling, not a quota — the tail is not padded.
        let hits = out.lines().filter(|l| l.starts_with("- [")).count();
        assert!(
            hits <= 2,
            "expected the match and at most one other:\n{out}"
        );
        // The trace still names the tokens responsible.
        assert!(
            out.contains("matched: git, ref, resolve"),
            "trace lost:\n{out}"
        );
    }

    #[test]
    fn similar_returns_empty_when_nothing_is_close() {
        let dir = similar_fixture("similar-empty");
        let out = similar(
            &code_server_for(&dir),
            &json!({"query": "schedule recurring newsletter delivery", "top": 10}),
        )
        .unwrap();
        assert!(
            out.contains("implementing fresh is reasonable"),
            "floor never fires:\n{out}"
        );
    }

    #[test]
    fn similar_query_of_only_stopwords_matches_nothing() {
        let dir = similar_fixture("similar-allstop");
        let out = similar(&code_server_for(&dir), &json!({"query": "to the for of"})).unwrap();
        assert!(
            out.contains("implementing fresh is reasonable"),
            "a query with no content words returned hits:\n{out}"
        );
    }

    // ---- dead_code -------------------------------------------------

    /// One of each population the tool has to tell apart, in a directory
    /// whose path does not read as test code.
    fn dead_code_fixture(name: &str) -> TmpDir {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", name);
        dir.write(
            "app.rs",
            r#"
use std::fmt;

pub fn exported_but_unused(x: i32) -> i32 { x }

fn orphan(x: i32) -> i32 { x }

fn only_used_in_a_macro(x: i32) -> i32 { x }

fn live(x: i32) -> i32 { x }

struct Report;

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", only_used_in_a_macro(1))
    }
}

fn main() {
    live(1);
}
"#,
        );
        dir
    }

    #[test]
    fn dead_code_reports_the_orphan_and_spares_the_rest() {
        let dir = dead_code_fixture("dead-code-basic");
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap().into_text();

        assert!(
            out.contains("`orphan`"),
            "the one dead function is missing:\n{out}"
        );
        // A caller exists — the graph resolves it.
        assert!(
            !out.contains("`live`"),
            "a called function was reported:\n{out}"
        );
        // `main` is an entry point, `fmt` satisfies a trait contract.
        assert!(
            !out.contains("`main`"),
            "the program entry was reported:\n{out}"
        );
        assert!(
            !out.contains("`fmt`"),
            "a trait impl method was reported:\n{out}"
        );
        // `pub` means a caller can live outside this analysis entirely.
        assert!(
            !out.contains("`exported_but_unused`"),
            "public API was reported by default:\n{out}"
        );
        // The header states each population it filtered rather than
        // shrinking the list silently.
        assert!(
            out.contains("entry point"),
            "exclusion counts missing:\n{out}"
        );
    }

    #[test]
    fn dead_code_spares_a_name_only_a_macro_body_mentions() {
        let dir = dead_code_fixture("dead-code-macro");
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap().into_text();
        // The only call site is inside `write!`, which reaches the parser
        // as an opaque token tree — fan-in is 0 and the textual scan is
        // the only thing standing between it and a false positive.
        assert!(
            !out.contains("`only_used_in_a_macro`"),
            "a macro-body call site was missed:\n{out}"
        );
        assert!(
            out.contains("mentioned elsewhere in the source"),
            "the textual gate is not reported:\n{out}"
        );
    }

    /// The Obsidian shape. `Plugin` is imported from a package that is
    /// not in the tree, so the host calling `onload` is invisible to the
    /// graph and every lifecycle method on the subclass has fan-in 0.
    /// Five of five candidates on a real plugin were this.
    #[test]
    fn dead_code_spares_overrides_of_a_base_class_outside_the_tree() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "dead-code-external-base");
        dir.write(
            "main.ts",
            r#"
import { Plugin } from "obsidian";

export default class MyPlugin extends Plugin {
    async onload() {
        this.wire();
    }

    async onunload() {}

    private wire() {}

    private neverCalled() {}
}

function looseHelper() {}
"#,
        );
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap().into_text();

        // The entry point of the whole plugin, and its teardown.
        assert!(
            !out.contains("`onload`"),
            "the plugin entry point was reported dead:\n{out}"
        );
        assert!(
            !out.contains("`onunload`"),
            "a lifecycle hook was reported dead:\n{out}"
        );
        // The class itself is handed to the host by the same declaration.
        assert!(
            !out.contains("`MyPlugin`"),
            "the subclass itself was reported dead:\n{out}"
        );
        // `private` cannot be reached through the base type, so fan-in 0
        // still means what it says — the report must not go silent.
        assert!(
            out.contains("`neverCalled`"),
            "a private method nothing calls was swallowed:\n{out}"
        );
        // The rule is scoped to the class, not to the file.
        assert!(
            out.contains("`looseHelper`"),
            "a free function beside the subclass was swallowed:\n{out}"
        );
        // Narrowed visibly, like every other exclusion.
        assert!(
            out.contains("in a class extending a base outside the tree"),
            "the exclusion is not stated in the header:\n{out}"
        );
    }

    #[test]
    fn dead_code_include_public_reveals_the_unused_export() {
        let dir = dead_code_fixture("dead-code-public");
        let out = dead_code(&code_server_for(&dir), &json!({"include_public": true})).unwrap().into_text();
        assert!(
            out.contains("`exported_but_unused`"),
            "include_public did not reveal public API:\n{out}"
        );
        assert!(
            out.contains(", public"),
            "public rows are not marked:\n{out}"
        );
    }

    #[test]
    fn dead_code_groups_by_file_with_a_per_file_count() {
        let dir = dead_code_fixture("dead-code-grouping");
        dir.write("extra.rs", "fn first_orphan() {}\nfn second_orphan() {}\n");
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap().into_text();
        assert!(
            out.contains("extra.rs (2)"),
            "per-file count missing:\n{out}"
        );
        assert!(out.contains("app.rs (1)"), "per-file count missing:\n{out}");
        // Worst file first, so the heaviest cleanup target leads.
        let extra = out.find("extra.rs (2)").unwrap();
        let app = out.find("app.rs (1)").unwrap();
        assert!(extra < app, "files are not ranked by count:\n{out}");
    }

    #[test]
    fn dead_code_says_so_when_there_is_nothing_to_report() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "dead-code-clean");
        dir.write(
            "app.rs",
            "fn live(x: i32) -> i32 { x }\n\nfn main() {\n    live(1);\n}\n",
        );
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap().into_text();
        assert!(out.contains("None —"), "clean run is not stated:\n{out}");
    }

    /// A test living in a regular source file under `mod locality_tests`
    /// Field report, 2026-08-31: a function called twice from inside a `for`
    /// loop reported `Used by (0)` while its own header read `in 2` — the
    /// metric counted the edges, the listing filtered their source out
    /// without traversing through it. The analyzer attaches a call written
    /// in a branch or a loop to that scope node, so the edge on the wire is
    /// `branch → callee` and dropping unlisted endpoints drops the call.
    ///
    /// The reporter checked all twelve of one function's callees by hand:
    /// ten were invisible, and the two that resolved were the two with a
    /// top-level call site. `calledAtTopLevel` is their control — it must
    /// keep working, or the fix has only moved the blind spot.
    ///
    /// Both directions, because they fail for different reasons. Incoming,
    /// the body node is the edge's source and can be walked up from.
    /// Outgoing, the edges never touch the caller's id at all.
    #[test]
    fn a_call_inside_a_loop_or_a_branch_belongs_to_the_function_around_it() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "body-scope");
        dir.write(
            "src/helpers.ts",
            "export function calledAtTopLevel(a: string): string { return a; }\n\
             export function calledInsideALoop(a: string): string { return a; }\n\
             export function calledInsideAnIf(a: string): string { return a; }\n",
        );
        dir.write(
            "src/plan.ts",
            r#"import { calledAtTopLevel, calledInsideALoop, calledInsideAnIf } from './helpers';

export function compute(xs: string[], flag: boolean): string[] {
  const out: string[] = [];
  out.push(calledAtTopLevel('once'));
  for (const x of xs) {
    out.push(calledInsideALoop(x));
  }
  if (flag) {
    out.push(calledInsideAnIf('yes'));
  }
  return out;
}
"#,
        );
        let server = code_server_for(&dir);

        // Incoming: every callee names the function around the call, not the
        // synthetic `l1` / `c1` the edge is anchored to.
        for callee in [
            "calledAtTopLevel",
            "calledInsideALoop",
            "calledInsideAnIf",
        ] {
            let out = impact(&server, &json!({ "entity": callee })).unwrap();
            assert!(
                out.contains("## Used by (1)") && out.contains("`compute`"),
                "{callee} lost its caller to a body scope:\n{out}"
            );
        }

        // Outgoing: the caller sees all three, not only the unnested one.
        let out = impact(&server, &json!({ "entity": "compute" })).unwrap();
        assert!(
            out.contains("## Uses (3)")
                && out.contains("`calledInsideALoop`")
                && out.contains("`calledInsideAnIf`"),
            "the caller's own calls are hidden behind its body scopes:\n{out}"
        );
    }

    /// is test code: the ancestor-module rule matches names the way
    /// `is_test_path` matches paths.
    #[test]
    fn test_entities_are_recognized_in_any_test_named_module() {
        // The fixture directory must not itself read as test code — the
        // point is to exercise the ancestor-module rule, not the path one.
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "dead-code-inline-cases");
        dir.write(
            "app.rs",
            r#"
fn main() {}

#[cfg(test)]
mod locality_tests {
    #[test]
    fn a_case_nothing_calls() {}
}
"#,
        );
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap().into_text();
        assert!(
            !out.contains("`a_case_nothing_calls`"),
            "an inline test was reported as dead:\n{out}"
        );
        assert!(out.contains("1 test"), "the test was not counted:\n{out}");
    }

    /// MCP-013 fixture: a Rust struct nothing in Rust references, plus a
    /// TypeScript file naming it. The TS reference cannot resolve across the
    /// language boundary (AN-014), so it lands on a ghost called
    /// `SessionItem` — which is exactly the blind spot `impact` has to own up
    /// to rather than print "Used by (0)" and stop.
    fn ghost_join_fixture(name: &str, consumers: usize) -> TmpDir {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", name);
        dir.write(
            "backend/session.rs",
            "pub struct SessionItem { pub id: String }\n",
        );
        let mut ts = String::new();
        for i in 0..consumers {
            ts.push_str(&format!(
                "export class ViewModel{i} {{\n  get bySide{i}(): SessionItem {{ return null; }}\n}}\n"
            ));
        }
        dir.write("frontend/vm.ts", &ts);
        dir
    }

    #[test]
    fn impact_zero_dependents_says_what_it_does_not_know() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-zero");
        dir.write("app.rs", "pub fn lonely(x: i32) -> i32 { x }\n");
        let out = impact(&code_server_for(&dir), &json!({"entity": "lonely"})).unwrap();

        assert!(
            out.contains("## Used by (0)"),
            "expected a zero count:\n{out}"
        );
        assert!(
            out.contains("not the same as none"),
            "a bare zero was reported with no caveat:\n{out}"
        );
    }

    #[test]
    fn impact_surfaces_dependents_of_a_name_matched_ghost() {
        let dir = ghost_join_fixture("impact-ghost", 1);
        let out = impact(&code_server_for(&dir), &json!({"entity": "SessionItem"})).unwrap();

        assert!(
            out.contains(POSSIBLE_HEADING),
            "the unresolved reference was not surfaced:\n{out}"
        );
        assert!(
            out.contains("`bySide0`"),
            "the referring entity is missing:\n{out}"
        );
        // Suggestive, never counted: the confirmed number stays honest.
        assert!(
            out.contains("## Used by (0)"),
            "a possible dependent leaked into the confirmed count:\n{out}"
        );
    }

    #[test]
    fn impact_caps_possible_dependents_and_says_so() {
        let dir = ghost_join_fixture("impact-ghost-flood", POSSIBLE_CAP + 5);
        let out = impact(&code_server_for(&dir), &json!({"entity": "SessionItem"})).unwrap();

        let listed = out.lines().filter(|l| l.contains("`bySide")).count();
        assert!(
            listed <= POSSIBLE_CAP,
            "listed {listed} rows, cap is {POSSIBLE_CAP}:\n{out}"
        );
        assert!(out.contains("more not listed"), "the cap is silent:\n{out}");
    }

    #[test]
    fn impact_with_real_dependents_gains_neither_note() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-normal");
        dir.write(
            "app.rs",
            "pub fn helper(x: i32) -> i32 { x }\npub fn caller() -> i32 { helper(1) }\n",
        );
        let out = impact(&code_server_for(&dir), &json!({"entity": "helper"})).unwrap();

        assert!(
            out.contains("`caller`"),
            "the real dependent is missing:\n{out}"
        );
        assert!(
            !out.contains("not the same as none"),
            "the zero-case note fired on a non-zero count:\n{out}"
        );
        assert!(
            !out.contains(POSSIBLE_HEADING),
            "a hedged section appeared with nothing to hedge:\n{out}"
        );
    }

    /// MCP-038. The same `path`, with and without a `line`, are two
    /// questions: the entity spanning the line, and the file as a unit.
    /// Both have to arrive intact — the file view was added beside the
    /// entity one, not in front of it.
    ///
    /// The fixture is the shape the ticket is about: `here` and `there`
    /// call each other inside `app.rs`, so a per-entity walk reports each
    /// as a dependent of the other. Neither is a dependent of the *file*,
    /// and summing their fan-in would say the file has two callers when it
    /// has one.
    #[test]
    fn a_path_without_a_line_asks_about_the_file_and_with_one_about_the_entity() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-file-view");
        dir.write(
            "app.rs",
            "pub fn here(x: i32) -> i32 { there(x) }\n\
             pub fn there(x: i32) -> i32 { here(x) + 1 }\n",
        );
        dir.write(
            "main.rs",
            "use crate::app;\npub fn run() -> i32 { app::here(1) }\n",
        );
        let server = code_server_for(&dir);

        let file = impact(&server, &json!({"path": "app.rs"})).unwrap();
        assert!(
            file.contains("# Impact of app.rs — the file"),
            "`path` alone did not ask about the file:\n{file}"
        );
        assert!(
            file.contains("## Depended on by (1 entity in 1 file)")
                && file.contains("`run` L2 → `here`"),
            "the outside dependent is missing, or the file's own wiring leaked \
             into the section:\n{file}"
        );
        assert!(
            file.contains("## Internal (2 edges)"),
            "the file's own edges were not counted apart:\n{file}"
        );

        // The entity view is untouched, and it is the one that still sees
        // `there` as a dependent of `here`.
        let entity = impact(&server, &json!({"path": "app.rs", "line": 1})).unwrap();
        assert!(
            entity.contains("# Impact of function `here`") && entity.contains("`there`"),
            "`path` + `line` no longer answers for the entity:\n{entity}"
        );
    }

    /// MCP-039. The targets that are not code in this repo used to be one
    /// number — *plus N external/unresolved targets not listed* — over two
    /// populations that mean opposite things. Both views now name them,
    /// and keep them apart.
    ///
    /// `Regex::new` is a call on a type nothing here declares: a
    /// dependency. `whatever_this_is` binds to nothing at all: a hole. The
    /// same file answers for both, so one fixture settles both views.
    #[test]
    fn external_and_unresolved_targets_are_named_and_never_merged() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-externals");
        dir.write(
            "app.rs",
            "pub fn build(pattern: &str) -> String {\n\
            \x20   let re = Regex::new(pattern);\n\
            \x20   let mut out = Vec::new();\n\
            \x20   out.push(whatever_this_is(re));\n\
            \x20   out.join(\",\")\n\
             }\n",
        );
        let server = code_server_for(&dir);

        for out in [
            impact(&server, &json!({"entity": "build"})).unwrap(),
            impact(&server, &json!({"path": "app.rs"})).unwrap(),
        ] {
            assert!(
                out.contains("- third-party `Regex` — `new` (1×)"),
                "the library call was not named:\n{out}"
            );
            assert!(
                out.contains("- stdlib `Vec` — ") && out.contains("`push` (1×)"),
                "the standard-library call was not told apart from it:\n{out}"
            );
            assert!(
                out.contains("## Unresolved (1 call to 1 name)")
                    && out.contains("`whatever_this_is` (1×)"),
                "the unbindable call was not reported as a hole:\n{out}"
            );
            assert!(
                out.contains("floor, not a total"),
                "the unresolved section did not caveat the counts above it:\n{out}"
            );
        }
    }

    /// A folder is somebody else's question, and answering it here with a
    /// file-shaped report would be a different tool wearing this one's name.
    #[test]
    fn impact_refuses_a_directory_and_says_which_tool_takes_one() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-dir");
        dir.write("src/app.rs", "pub fn here(x: i32) -> i32 { x }\n");
        let err = impact(&code_server_for(&dir), &json!({"path": "src"})).unwrap_err();

        let said = err.to_string();
        assert!(
            said.contains("is a directory") && said.contains("`reshape`"),
            "the refusal does not point anywhere: {said}"
        );
    }

    /// A chain three deep: the blast radius is what the direct dependents
    /// hide, and MCP-048 is about *through what*. `top` is two hops out and
    /// has to hang under `mid`, the frame a reader would have to open to
    /// get there — which the old flat `[depth 2]` row could not say.
    #[test]
    fn the_blast_radius_hangs_each_dependent_under_the_route_to_it() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-blast");
        dir.write(
            "app.rs",
            "pub fn leaf(x: i32) -> i32 { x }\n\
             pub fn mid() -> i32 { leaf(1) }\n\
             pub fn top() -> i32 { mid() }\n",
        );
        let out = impact(&code_server_for(&dir), &json!({"entity": "leaf"})).unwrap();

        assert!(
            out.contains("## Blast radius to depth 2 — 2 entities reach this"),
            "the reached population was not counted:\n{out}"
        );
        let blast = out.split("## Blast radius").nth(1).unwrap_or("");
        assert!(
            blast.contains("- `1` function `mid` (app.rs:2)")
                && blast.contains("  - `1.1` function `top` (app.rs:3)"),
            "the outline does not say which route reaches `top`:\n{out}"
        );
        assert!(
            blast.contains(
                "**Furthest** — 2 hops: function `leaf` (app.rs:1) ← function `mid` (app.rs:2) \
                 ← function `top` (app.rs:3)"
            ),
            "the chain rendering is missing or does not start at the target:\n{out}"
        );
    }

    /// `direction: out` is the same walk with the arrows reversed: the call
    /// tree under an entity, which nothing else in the tool set draws.
    #[test]
    fn direction_out_draws_the_call_tree_under_the_entity() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-out");
        dir.write(
            "app.rs",
            "pub fn leaf(x: i32) -> i32 { x }\n\
             pub fn mid() -> i32 { leaf(1) }\n\
             pub fn top() -> i32 { mid() }\n",
        );
        let server = code_server_for(&dir);

        let out = impact(&server, &json!({"entity": "top", "direction": "out"})).unwrap();
        assert!(
            out.contains("## Call tree to depth 2 — 2 callables run under this"),
            "the outgoing direction was not walked:\n{out}"
        );
        assert!(
            out.contains("- `1` function `mid` (app.rs:2)")
                && out.contains("  - `1.1` function `leaf` (app.rs:1)"),
            "the call tree is not nested:\n{out}"
        );
        assert!(
            out.contains("→ function `leaf`"),
            "the outgoing chain still points inwards:\n{out}"
        );

        let bad = impact(&server, &json!({"entity": "top", "direction": "sideways"}))
            .unwrap_err()
            .to_string();
        assert!(
            bad.contains("`in`") && bad.contains("`out`"),
            "an unknown direction was not refused with its two values: {bad}"
        );
    }

    /// Depth 1 asks only for direct dependents, and "beyond direct
    /// dependents" is empty by definition there. The section is omitted
    /// rather than printed with a zero, because a zero here would read as
    /// a walk that ran and found nothing.
    #[test]
    fn depth_one_asks_for_no_blast_radius_at_all() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-blast-depth1");
        dir.write(
            "app.rs",
            "pub fn leaf(x: i32) -> i32 { x }\n\
             pub fn mid() -> i32 { leaf(1) }\n\
             pub fn top() -> i32 { mid() }\n",
        );
        let server = code_server_for(&dir);

        let deep = impact(&server, &json!({"entity": "leaf", "depth": 2})).unwrap();
        assert!(deep.contains("## Blast radius"), "{deep}");

        let shallow = impact(&server, &json!({"entity": "leaf", "depth": 1})).unwrap();
        assert!(
            !shallow.contains("Blast radius"),
            "depth 1 still printed a blast radius:\n{shallow}"
        );
    }

    /// The outline is capped, the heading counts the whole population, and
    /// the note underneath says what the reader is not seeing. Chains are
    /// wide, so the cap is stricter than the forty flat rows it replaced —
    /// and it counts the routes, ancestors included.
    #[test]
    fn the_blast_radius_caps_its_routes_and_says_how_many_it_dropped() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-blast-flood");
        let mut src = String::from(
            "pub fn leaf(x: i32) -> i32 { x }\npub fn mid() -> i32 { leaf(1) }\n",
        );
        for i in 0..45 {
            src.push_str(&format!("pub fn top{i:02}() -> i32 {{ mid() }}\n"));
        }
        dir.write("app.rs", &src);
        let out = impact(&code_server_for(&dir), &json!({"entity": "leaf"})).unwrap();

        assert!(
            out.contains("## Blast radius to depth 2 — 46 entities reach this"),
            "the heading does not count the whole population:\n{out}"
        );
        let blast = out.split("## Blast radius").nth(1).unwrap_or("");
        let listed = blast.lines().filter(|l| l.trim_start().starts_with("- `")).count();
        assert_eq!(listed, 25, "the cap did not hold:\n{out}");
        assert!(
            blast.contains("… and 21 more reached, not shown."),
            "the cap dropped routes silently:\n{out}"
        );
    }

    /// An entity in a ring with the target is reached, labelled, and not
    /// expanded — rather than quietly arriving by the long way round and
    /// reading as a third party.
    #[test]
    fn a_dependent_that_closes_a_ring_is_labelled_rather_than_walked() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-ring");
        dir.write(
            "app.rs",
            "pub fn a(x: i32) -> i32 { b(x) }\npub fn b(x: i32) -> i32 { a(x) }\n",
        );
        let out = impact(&code_server_for(&dir), &json!({"entity": "a", "depth": 3})).unwrap();

        let blast = out.split("## Blast radius").nth(1).unwrap_or("");
        assert!(
            blast.contains("- `1` function `b` (app.rs:2)"),
            "the direct dependent is missing:\n{out}"
        );
        assert!(
            blast.contains("**a ring**"),
            "the re-entry was not labelled:\n{out}"
        );
    }

    /// The ordering rule the report states has to be the one it follows:
    /// siblings in the order their call sites were written, file then line.
    #[test]
    fn siblings_come_out_in_the_order_the_call_sites_were_written() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "impact-order");
        dir.write("leaf.rs", "pub fn leaf(x: i32) -> i32 { x }\n");
        dir.write(
            "b_late.rs",
            "use crate::leaf;\npub fn zebra() -> i32 { leaf::leaf(1) }\n",
        );
        dir.write(
            "a_early.rs",
            "use crate::leaf;\n\
             pub fn alpha() -> i32 { leaf::leaf(1) }\n\
             pub fn beta() -> i32 { leaf::leaf(2) }\n",
        );
        let out = impact(&code_server_for(&dir), &json!({"entity": "leaf"})).unwrap();

        let blast = out.split("## Blast radius").nth(1).unwrap_or("");
        let names: Vec<&str> = blast
            .lines()
            .filter(|l| l.starts_with("- `"))
            .filter_map(|l| l.split('`').nth(3))
            .collect();
        assert_eq!(
            names,
            vec!["alpha", "beta", "zebra"],
            "siblings are not in written order:\n{out}"
        );
    }

    #[test]
    fn signature_stays_on_one_line_and_within_budget() {
        let inline_object = "{\n    fromRef: string;\n    toRef: string;\n    files: string[];\n    additions: number;\n    deletions: number;\n    renames: Array<{ from: string; to: string }>;\n}";
        let mut e = CodeEntity::new(
            "reportDiff",
            EntityKind::Function,
            PathBuf::from("panel.ts"),
            crate::models::Span::from_positions(125, 0, 140, 1),
        );
        e.parameters.push(crate::models::Parameter {
            name: "state".to_string(),
            type_name: Some(inline_object.to_string()),
            default_value: None,
            visibility: None,
        });
        e.return_type = Some("void".to_string());

        let sig = signature(&e);
        assert!(!sig.contains('\n'), "signature spans lines: {sig}");
        assert!(sig.contains('…'), "truncation is not marked: {sig}");
        assert!(sig.len() < 160, "signature is not capped: {sig}");
        assert!(sig.starts_with("reportDiff(state: {"), "head lost: {sig}");
        // A short signature is left exactly as it was.
        let mut small = CodeEntity::new(
            "resolve_git_ref",
            EntityKind::Function,
            PathBuf::from("diff.rs"),
            crate::models::Span::from_positions(1, 0, 3, 1),
        );
        small.parameters.push(crate::models::Parameter {
            name: "git_ref".to_string(),
            type_name: Some("&str".to_string()),
            default_value: None,
            visibility: None,
        });
        small.return_type = Some("String".to_string());
        assert_eq!(
            signature(&small),
            "resolve_git_ref(git_ref: &str) -> String"
        );
    }

    /// MCP-019: `shapes::widen` is both handed to a dispatcher (`UsesFn`)
    /// and called (`Calls`) by `middle`, so the multigraph holds two edges
    /// for one pair. The BFS recorded `middle` as a predecessor of `widen`
    /// once per edge, and reconstruction expanded each independently — the
    /// three-chain budget was spent printing one route twice.
    #[test]
    fn trace_does_not_repeat_a_chain_when_two_edges_join_a_pair() {
        let dir = TmpDir::with_prefix("mezz-mcp-fixture", "trace-parallel-edges");
        dir.write(
            "app.rs",
            r#"
pub mod shapes {
    pub fn widen(x: i32) -> i32 { x + 1 }
}

pub fn dispatch(f: fn(i32) -> i32) -> i32 { f(1) }

pub fn middle() -> i32 {
    dispatch(shapes::widen) + shapes::widen(2)
}

pub fn other() -> i32 { shapes::widen(3) }

pub fn entry() -> i32 { middle() + other() }
"#,
        );
        let out = trace(
            &code_server_for(&dir),
            &json!({"from": "entry", "to": "widen"}),
        )
        .unwrap();

        let chains: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("- ") && l.contains('\u{2192}'))
            .collect();
        let distinct: HashSet<&str> = chains.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            chains.len(),
            "the same chain was printed more than once:\n{out}"
        );
        // And the dedup collapses repeats only: the two real routes through
        // `middle` and through `other` are both still offered.
        assert_eq!(chains.len(), 2, "a distinct route was lost:\n{out}");
        assert!(
            chains.iter().any(|c| c.contains("`middle`"))
                && chains.iter().any(|c| c.contains("`other`")),
            "both routes should be listed:\n{out}"
        );
    }

    // ------------------------------------------------------------------
    //  render_change_report
    //
    //  Characterisation. This is the body of `assess_change` — the report
    //  an agent reads to decide whether its own edit made things worse —
    //  and until these tests it had no coverage at all. The listing rules
    //  below (what is ordered how, what is capped where, what stays
    //  silent) are the part a reader trusts without re-deriving, so they
    //  are pinned before the function is touched.
    // ------------------------------------------------------------------

    /// The counts under the headline are recomputed from the rows, so the
    /// summary a fixture carries is never read.
    fn no_summary() -> diff::DiffSummary {
        diff::DiffSummary {
            total_base: 0,
            total_head: 0,
            added: 0,
            removed: 0,
            modified: 0,
            modified_source: 0,
            modified_impact: 0,
            unchanged: 0,
            relationships_added: 0,
            relationships_removed: 0,
        }
    }

    /// One change row, with the id `CodeEntity::new` would mint for the
    /// same name and file — so a fixture can choose whether the head graph
    /// resolves the row by whether it holds that entity.
    fn row_of(name: &str, kind: &str, status: diff::ChangeStatus) -> diff::EntityDiff {
        diff::EntityDiff {
            entity_id: format!("src/{name}.rs:0:{name}"),
            name: name.to_string(),
            kind: kind.to_string(),
            file_path: format!("src/{name}.rs"),
            status,
            source_changed: status == diff::ChangeStatus::Modified,
            metric_deltas: Vec::new(),
            rel_deltas: Vec::new(),
            base_entity_id: None,
            moved_from: None,
        }
    }

    fn grew(mut d: diff::EntityDiff, metric: &str, delta: f64) -> diff::EntityDiff {
        d.metric_deltas.push(diff::MetricDelta {
            name: metric.to_string(),
            old: Some(0.0),
            new: Some(delta),
            delta,
        });
        d
    }

    fn report_of(entities: Vec<diff::EntityDiff>, head: &DependencyGraph) -> String {
        let result = diff::DiffResult {
            from_ref: "abc1234".to_string(),
            to_ref: "working tree".to_string(),
            summary: no_summary(),
            entities,
        };
        render_change_report(
            &result,
            &bare_graph(),
            head,
            "HEAD",
            &[],
            (Path::new(""), Path::new("")),
        )
    }

    /// The modified list is a triage queue, so it leads with whichever
    /// entity the change made worst. Growth is summed per metric with
    /// negatives clamped away, which means an improvement in one metric
    /// does not net off a regression in another: a function that shed
    /// branching while deepening its nesting still has to be looked at,
    /// and would sort last if the two were allowed to cancel.
    #[test]
    fn the_modified_list_leads_with_the_worst_complexity_growth() {
        let rows = vec![
            grew(
                row_of("mild", "Function", diff::ChangeStatus::Modified),
                "cyclomatic",
                1.0,
            ),
            grew(
                row_of("worst", "Function", diff::ChangeStatus::Modified),
                "cyclomatic",
                9.0,
            ),
            grew(
                grew(
                    row_of("mixed", "Function", diff::ChangeStatus::Modified),
                    "cyclomatic",
                    -5.0,
                ),
                "max_nesting",
                3.0,
            ),
        ];

        let out = report_of(rows, &bare_graph());
        let order: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("- Function "))
            .collect();
        assert_eq!(order.len(), 3, "{out}");
        assert!(order[0].contains("`worst`"), "{out}");
        assert!(
            order[1].contains("`mixed`"),
            "deepened nesting outranks mild branching even beside a big \
             cyclomatic improvement:\n{out}"
        );
        assert!(order[2].contains("`mild`"), "{out}");
    }

    /// Every listing is capped, and a cap that silently drops rows reads
    /// as a clean bill of health. The count in the heading is the whole
    /// population; the note underneath is what the reader is not seeing.
    #[test]
    fn the_modified_list_caps_at_forty_and_says_how_many_it_dropped() {
        let rows: Vec<diff::EntityDiff> = (0..43)
            .map(|i| {
                row_of(
                    &format!("f{i:02}"),
                    "Function",
                    diff::ChangeStatus::Modified,
                )
            })
            .collect();

        let out = report_of(rows, &bare_graph());
        let listed = out.lines().filter(|l| l.starts_with("- Function ")).count();
        assert_eq!(listed, 40, "{out}");
        assert!(out.contains("## Modified (43)"), "{out}");
        assert!(out.contains("… and 3 more modified entities."), "{out}");
        // A row whose source changed without moving a metric says so,
        // rather than printing an empty delta list.
        assert!(out.contains("source changed, metrics stable"), "{out}");
    }

    /// An added entity is printed with its metrics so an oversized
    /// newcomer stands out at a glance — but only when the head graph
    /// resolves it. A row the graph cannot place still has to appear,
    /// because dropping it would under-report the change.
    #[test]
    fn an_added_entity_shows_its_metrics_only_when_the_graph_knows_it() {
        let mut known = crate::models::CodeEntity::new(
            "known",
            EntityKind::Function,
            "src/known.rs",
            crate::models::Span::from_positions(0, 0, 0, 0),
        );
        known.metrics.loc = 42;
        known.metrics.cyclomatic = Some(7);
        let head = DependencyGraph::from_analysis(&crate::analyzer::AnalysisResult {
            entities: vec![known],
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        });

        let out = report_of(
            vec![
                row_of("known", "Function", diff::ChangeStatus::Added),
                row_of("stranger", "Function", diff::ChangeStatus::Added),
            ],
            &head,
        );

        assert!(out.contains("## Added (2)"), "{out}");
        let known_line = out.lines().find(|l| l.contains("`known`")).unwrap_or("");
        assert!(
            known_line.contains("loc 42") && known_line.contains("cx 7"),
            "{out}"
        );
        let stranger_line = out.lines().find(|l| l.contains("`stranger`")).unwrap_or("");
        assert!(
            !stranger_line.contains("loc "),
            "an unresolved row carries no invented metrics:\n{out}"
        );
    }

    /// The ripple list names entities the change moved without touching —
    /// fan-in or fan-out shifted, source did not. It is a heading only
    /// when something sits under it: an empty section reads as a finding
    /// that was made and came back clean, which is not what happened.
    #[test]
    fn coupling_ripples_are_a_heading_only_when_there_are_ripples() {
        let quiet = report_of(
            vec![row_of("touched", "Function", diff::ChangeStatus::Modified)],
            &bare_graph(),
        );
        assert!(!quiet.contains("Coupling ripples"), "{quiet}");

        let mut rippled = row_of("rippled", "Function", diff::ChangeStatus::Modified);
        rippled.source_changed = false;
        let loud = report_of(vec![rippled], &bare_graph());
        assert!(loud.contains("## Coupling ripples (1 "), "{loud}");
    }

    /// A File row stands for the entities inside it, so printing both
    /// says everything twice. It is dropped from the listings and from
    /// the headline tally — but counted in the note beneath, because a
    /// change with nothing else in it would otherwise report as no
    /// change at all.
    #[test]
    fn file_rows_are_dropped_from_the_listings_and_counted_apart() {
        let out = report_of(
            vec![
                row_of("lib", "File", diff::ChangeStatus::Modified),
                row_of("real", "Function", diff::ChangeStatus::Modified),
            ],
            &bare_graph(),
        );

        assert!(out.contains("## Modified (1)"), "{out}");
        assert!(!out.contains("File `lib`"), "{out}");
        assert!(out.contains("1 entity changed"), "{out}");
        assert!(out.contains("Function `real`"), "{out}");
    }

    /// A modified row whose two halves live in different files says so.
    ///
    /// Field report, 2026-09-07: a `git mv` of two files each holding a
    /// `state(x)` had each head entity paired against its namesake, and the
    /// difference between the two untouched bodies was reported as a
    /// complexity jump on both. `diff::match_relocated` is what stops that
    /// pairing; this is the row that lets a reader check it, because nothing
    /// in the old wording suggested a move was involved at all.
    #[test]
    fn a_modified_row_matched_across_files_names_the_file_it_came_from() {
        let mut relocated = grew(
            row_of("state", "Function", diff::ChangeStatus::Modified),
            "cyclomatic",
            5.0,
        );
        relocated.moved_from = Some("pkg/beta.py".to_string());
        let out = report_of(vec![relocated], &bare_graph());
        assert!(out.contains("(was pkg/beta.py)"), "{out}");

        // An entity that did not move carries no such note.
        let stayed = grew(
            row_of("still", "Function", diff::ChangeStatus::Modified),
            "cyclomatic",
            5.0,
        );
        let out = report_of(vec![stayed], &bare_graph());
        assert!(!out.contains("(was "), "{out}");
    }

    /// Removed entities are names only — no metrics, because the entity
    /// they would describe is gone from the head graph and any number
    /// printed beside it would be the base's.
    #[test]
    fn removed_entities_are_listed_by_name_without_metrics() {
        let out = report_of(
            vec![row_of("gone", "Function", diff::ChangeStatus::Removed)],
            &bare_graph(),
        );

        assert!(out.contains("## Removed (1)"), "{out}");
        let line = out.lines().find(|l| l.contains("`gone`")).unwrap_or("");
        assert_eq!(line, "- Function `gone` — src/gone.rs", "{out}");
    }
}
