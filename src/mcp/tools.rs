//! Implementations of the MCP tools: `map`, `quality`, `assess_change`.
//!
//! Each returns compact markdown-ish text, not raw graph JSON — MCP tool
//! output lands in an agent's context window, so responses rank, cap, and
//! annotate rather than dump. Truncation is always stated explicitly.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::diff;
use crate::graph::DependencyGraph;
use crate::models::{CodeEntity, EntityKind, Precision, Relationship, RelationshipKind, SmellKind};
use crate::Analyzer;

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
/// substring match `is_test_path` applies to paths, so `mod tests` and
/// `mod locality_tests` classify alike.
pub(crate) fn is_test_entity(graph: &DependencyGraph, e: &CodeEntity) -> bool {
    if crate::analyzer::is_test_path(&e.file_path) {
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
            Some(x) => cur = graph.parent(&x.id),
            None => break,
        }
    }
    false
}

/// Resolve the optional `path` argument against the server root.
fn resolve_path(server: &McpServer, args: &Value) -> Result<PathBuf> {
    let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let candidate = if raw.is_empty() {
        server.root.clone()
    } else {
        let p = PathBuf::from(raw);
        if p.is_absolute() { p } else { server.root.join(p) }
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
    let gen_at_start = server.generation.load(std::sync::atomic::Ordering::Acquire);
    if let Some(hit) = server.graph_cache.lock().unwrap().get(&key) {
        if hit.gen == gen_at_start {
            eprintln!("⚡ graph cache hit for {}", path.display());
            return Ok(hit.graph.clone());
        }
    }

    let (dir, single_file) = if path.is_file() {
        (path.parent().unwrap_or(path).to_path_buf(), Some(path))
    } else {
        (path.to_path_buf(), None)
    };
    let config = diff::build_analysis_config(&dir, include_tests, &server.languages);
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
        cache.insert(key, crate::mcp::CachedGraph { graph: graph.clone(), gen: gen_at_start });
    }
    Ok(graph)
}

fn rel_path(file: &Path, base: &Path) -> String {
    // When the analyzed target is itself a file, relativize against its
    // directory so the file keeps its name instead of becoming "".
    let base = if base.is_file() {
        base.parent().unwrap_or(base)
    } else {
        base
    };
    file.strip_prefix(base).unwrap_or(file).display().to_string()
}

/// Edge label carrying its AN-004 precision marker, e.g. `calls ·exact` /
/// `calls ·heuristic`. Only call edges carry precision; every other edge
/// renders its plain label. Surfaces the exact-vs-heuristic distinction the
/// agent needs to decide whether to trust a blast radius or fall back to grep.
fn edge_label(r: &Relationship) -> String {
    match (r.kind, r.precision) {
        (RelationshipKind::Calls, Some(p)) => {
            format!("{} ·{}", r.kind.display_label(), p.marker())
        }
        _ => r.kind.display_label().to_string(),
    }
}

fn smell_labels(smells: &[SmellKind]) -> String {
    smells.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")
}

/// Compact one-line metric annotation for an entity.
fn metric_suffix(e: &CodeEntity) -> String {
    let m = &e.metrics;
    let mut parts = vec![format!("L{}", e.span.start.line + 1), format!("loc {}", m.loc)];
    if let Some(c) = m.cyclomatic {
        parts.push(format!("cx {}", c));
    }
    if let Some(c) = m.cognitive_complexity {
        parts.push(format!("cog {}", c));
    }
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

pub fn map(server: &McpServer, args: &Value) -> Result<String> {
    let path = resolve_path(server, args)?;
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .clamp(1, 3);

    let graph = analyze(server, &path)?;
    let by_id: HashMap<&str, &CodeEntity> =
        graph.entities().map(|e| (e.id.as_str(), e)).collect();

    // Top-level entities grouped by file, ordered by path then line.
    let is_top_level = |e: &CodeEntity| match &e.parent_id {
        None => true,
        Some(pid) => by_id
            .get(pid.as_str())
            .map(|p| p.kind == EntityKind::File)
            .unwrap_or(true), // unresolvable parent → treat as top-level
    };

    let mut files: BTreeMap<String, Vec<&CodeEntity>> = BTreeMap::new();
    for e in graph.entities() {
        if !is_listed(e) || !is_top_level(e) {
            continue;
        }
        files.entry(rel_path(&e.file_path, &path)).or_default().push(e);
    }
    for entities in files.values_mut() {
        entities.sort_by_key(|e| e.span.start.line);
    }

    // Header: totals and a kind breakdown.
    let mut kind_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in graph.entities() {
        if is_listed(e) {
            *kind_counts.entry(e.kind.display_name()).or_default() += 1;
        }
    }
    let breakdown = kind_counts
        .iter()
        .map(|(k, n)| format!("{} {}", n, k))
        .collect::<Vec<_>>()
        .join(", ");

    let mut body = vec![
        format!("# Map of {}", path.display()),
        format!("{} files — {}", files.len(), breakdown),
        String::new(),
    ];

    for (file, entities) in &files {
        body.push(format!("{} ({} entities)", file, entities.len()));
        if depth >= 2 {
            for e in entities {
                body.push(format!(
                    "  {} {} ({})",
                    e.kind.display_name(),
                    e.name,
                    metric_suffix(e)
                ));
                if depth >= 3 {
                    let mut members = graph.children(&e.id);
                    members.retain(|c| is_listed(c));
                    members.sort_by_key(|c| c.span.start.line);
                    for m in members {
                        body.push(format!(
                            "    {} {} ({})",
                            m.kind.display_name(),
                            m.name,
                            metric_suffix(m)
                        ));
                    }
                }
            }
        }
    }

    Ok(cap_lines(
        body,
        "Call `map` again with a narrower `path` or a smaller `depth`.",
    ))
}

// ------------------------------------------------------------------
//  quality
// ------------------------------------------------------------------

pub fn quality(server: &McpServer, args: &Value) -> Result<String> {
    let path = resolve_path(server, args)?;
    let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(10).clamp(1, 50) as usize;

    let graph = analyze(server, &path)?;
    let entity_row = |e: &CodeEntity| {
        format!(
            "- {} `{}` — {}:{} ({})",
            e.kind.display_name(),
            e.name,
            rel_path(&e.file_path, &path),
            e.span.start.line + 1,
            metric_suffix(e)
        )
    };

    let mut body = vec![format!("# Quality of {}", path.display()), String::new()];

    // Smells, worst composite score first.
    let mut smelly: Vec<&CodeEntity> = graph
        .entities()
        .filter(|e| is_listed(e) && !e.metrics.smells.is_empty())
        .collect();
    smelly.sort_by(|a, b| b.metrics.composite_score.total_cmp(&a.metrics.composite_score));
    body.push(format!("## Smells ({})", smelly.len()));
    let mut hints_used: HashSet<SmellKind> = HashSet::new();
    for e in smelly.iter().take(top) {
        body.push(entity_row(e));
        for s in &e.metrics.smells {
            hints_used.insert(*s);
        }
    }
    if smelly.len() > top {
        body.push(format!("… and {} more smelly entities.", smelly.len() - top));
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
    let mut ranked: Vec<&CodeEntity> = graph
        .entities()
        .filter(|e| is_listed(e) && e.metrics.composite_score > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.metrics.composite_score.total_cmp(&a.metrics.composite_score));
    for e in ranked.iter().take(top) {
        body.push(format!(
            "- [{:.2}] {} `{}` — {}:{} ({})",
            e.metrics.composite_score,
            e.kind.display_name(),
            e.name,
            rel_path(&e.file_path, &path),
            e.span.start.line + 1,
            metric_suffix(e)
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
    for members in cycles.iter().take(5) {
        let names: Vec<&str> = members.iter().take(8).map(|e| e.name.as_str()).collect();
        let ellipsis = if members.len() > 8 { " → …" } else { "" };
        body.push(format!("- {} → {}{}", names.join(" → "), names[0], ellipsis));
    }
    if cycles.len() > 5 {
        body.push(format!("… and {} more cycles.", cycles.len() - 5));
    }

    Ok(cap_lines(body, "Call `quality` with a narrower `path`."))
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
            format!("{}({}){}", one_line(&e.name), cap_chars(&params, MAX_PARAMS_CHARS), ret)
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

    Ok(cap_lines(body, "Target a narrower entity, or use `impact` for positions only."))
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
/// ghost called `foo` is where every reference to `foo` that nao could not
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
/// A reference nao could not resolve attaches to a ghost of the same name,
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
             References nao could not resolve are attached to a ghost of the \
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

    // Blast radius must see every dependent, so always analyze the full root.
    let graph = analyze(server, &server.root)?;
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
    let mut body = vec![
        format!(
            "# Impact of {} `{}`{} — {}:{} ({})",
            target.kind.display_name(),
            target.name,
            context,
            rel_path(&target.file_path, root),
            target.span.start.line + 1,
            metric_suffix(target)
        ),
    ];

    // Outgoing: what the target relies on — its contract with the rest
    // of the code. Sorted by position for stable output.
    let all_deps = graph.dependencies(&target.id);
    let raw_dep_count = all_deps.len();
    let mut uses: Vec<(&CodeEntity, String)> = all_deps
        .into_iter()
        .filter(|(e, _)| is_listed(e))
        .map(|(e, r)| (e, edge_label(r)))
        .collect();
    uses.sort_by_key(|(e, _)| (e.file_path.clone(), e.span.start.line));
    uses.dedup_by_key(|(e, l)| (e.id.clone(), l.clone()));
    let external = raw_dep_count - uses.len();
    let external_note = if external > 0 {
        format!(", plus {} external/unresolved targets not listed", external)
    } else {
        String::new()
    };
    body.push(String::new());
    body.push(format!(
        "## Uses ({}) — code this entity relies on{}",
        uses.len(),
        external_note
    ));
    for (e, label) in uses.iter().take(40) {
        body.push(row(e, label));
    }
    if uses.len() > 40 {
        body.push(format!("… and {} more.", uses.len() - 40));
    }

    // Incoming: direct dependents — first to break if the contract changes.
    let mut used_by: Vec<(&CodeEntity, String)> = graph
        .dependents(&target.id)
        .into_iter()
        .filter(|(e, _)| is_listed(e))
        .map(|(e, r)| (e, edge_label(r)))
        .collect();
    used_by.sort_by_key(|(e, _)| (e.file_path.clone(), e.span.start.line));
    used_by.dedup_by_key(|(e, l)| (e.id.clone(), l.clone()));
    body.extend(used_by_section(&graph, target, &used_by, root));

    // Container targets: parsers emit no type-usage edges (a struct used
    // as a parameter/field type gets no edge), so approximate "who uses
    // this type" by aggregating callers of its methods.
    let children = graph.children(&target.id);
    let child_ids: HashSet<&str> = children.iter().map(|c| c.id.as_str()).collect();
    if !children.is_empty() {
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
        body.push(String::new());
        body.push(format!(
            "## Used via members ({} callers of this type's methods)",
            via.len()
        ));
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
    }

    // Transitive dependents: the full blast radius, level by level. For
    // containers, seed with the members too so method-mediated dependents
    // are reached.
    let mut seeds: Vec<&str> = vec![&target.id];
    seeds.extend(child_ids.iter().copied());
    let (levels, _exact_path) = transitive_dependents(&graph, &seeds, depth);
    let total: usize = levels.iter().skip(1).map(|l| l.len()).sum();
    if depth > 1 {
        body.push(String::new());
        body.push(format!(
            "## Blast radius to depth {} ({} entities beyond direct dependents)",
            depth, total
        ));
        let mut listed = 0;
        for (d, level) in levels.iter().enumerate().skip(1) {
            for e in level {
                if listed >= 40 {
                    break;
                }
                body.push(format!(
                    "- [depth {}] {} `{}` — {}:{}",
                    d + 1,
                    e.kind.display_name(),
                    e.name,
                    rel_path(&e.file_path, root),
                    e.span.start.line + 1
                ));
                listed += 1;
            }
        }
        if total > listed {
            body.push(format!("… and {} more.", total - listed));
        }
    }

    Ok(cap_lines(body, "Lower `depth` or target a narrower entity."))
}

/// BFS over incoming dependency edges. Level 0 holds direct dependents,
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
                // A hop breaks confidence only when it's a heuristic call
                // edge — the case AN-004 exists to flag. Structural/exact
                // edges preserve it.
                let hop_trusted =
                    !(r.kind == RelationshipKind::Calls && r.precision == Some(Precision::Heuristic));
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
        levels.push(next.into_iter().filter(|e| is_listed(e)).collect());
    }
    (levels, exact_path)
}

enum Found<'g> {
    One(&'g CodeEntity),
    /// Lookup produced zero or several candidates; the text explains and
    /// lists them so the agent can re-call with a disambiguated target.
    Ambiguous(String),
}

/// Locate the target entity from `entity` (name / qualified name) or
/// `path` + `line` (1-based, innermost listed entity spanning the line).
fn find_target<'g>(
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
fn find_by_name<'g>(
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

pub fn hotspots(server: &McpServer, args: &Value) -> Result<String> {
    let path = resolve_path(server, args)?;
    let days = args.get("days").and_then(|v| v.as_u64()).unwrap_or(180).clamp(1, 3650);
    let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(10).clamp(1, 50) as usize;

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
        let agg = files.entry(repo_rel).or_insert(FileAgg { weighted: 0.0, loc: 0, worst: e });
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

    let mut body = vec![
        format!("# Hotspots of {} (last {} days)", path.display(), days),
        "Risk = commits in window × LOC-weighted avg composite score. Renames count as fresh paths.".to_string(),
        String::new(),
    ];
    if ranked.is_empty() {
        body.push("No files with both churn and quality pressure in the window.".to_string());
    }
    for (risk, commits, avg, file, agg) in ranked.iter().take(top) {
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
        body.push(format!("… and {} more files with non-zero risk.", ranked.len() - top));
    }

    Ok(cap_lines(body, "Raise `top` or narrow `path`."))
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
    for (i, level) in levels.iter().enumerate() {
        for e in level {
            if is_test_entity(&graph, e) {
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
            let directness = if hops == 1 { "direct".to_string() } else { format!("{} hops", hops) };
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

    Ok(cap_lines(body, "Lower `depth` to see only the closest tests."))
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

    let render_node = |e: &CodeEntity| {
        if e.tags.contains("ghost") {
            format!("`{}` (external)", e.name)
        } else {
            format!(
                "{} `{}` ({}:{})",
                e.kind.display_name(),
                e.name,
                rel_path(&e.file_path, root),
                e.span.start.line + 1
            )
        }
    };

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
                    let node = graph.get_entity(&window[0]).map(render_node).unwrap_or_default();
                    chain.push(format!("{} —{}→", node, label));
                }
                chain.push(graph.get_entity(path_ids.last().unwrap()).map(render_node).unwrap_or_default());
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

    Ok(cap_lines(body, "Raise `max_hops` if endpoints are far apart."))
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
    // parents: for each visited node, all predecessors at minimal depth.
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
                    parents.get_mut(&next.id).unwrap().push(id.clone());
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
    if is_stopword(token) { STOPWORD_WEIGHT } else { 1.0 }
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
    let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(10).clamp(1, 50) as usize;

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
            score_against(e, &query_tokens, query_weight).map(|(score, matched)| (score, matched, e))
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

    Ok(cap_lines(body, "Refine the query with more specific words."))
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
/// modifier (`pub`, `public`, Python's underscore convention), so `Public`
/// means "declared public" rather than "no information". Everywhere else —
/// TypeScript's `export`, Go's capitalization — every entity is parsed as
/// `Public`, and using that as a public-API filter would empty the report
/// instead of narrowing it.
fn records_visibility(lang: crate::models::file_info::Language) -> bool {
    use crate::models::file_info::Language as L;
    matches!(
        lang,
        L::Rust | L::Java | L::Kotlin | L::CSharp | L::Scala | L::Swift | L::PHP | L::Python
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

/// Why a fan-in-0 entity is *not* reported. Counted and stated in the
/// header, so the report narrows visibly rather than silently.
enum Excluded {
    Test,
    EntryPoint,
    PublicApi,
}

fn exclusion_reason(graph: &DependencyGraph, e: &CodeEntity) -> Option<Excluded> {
    if is_test_entity(graph, e) {
        Some(Excluded::Test)
    } else if is_entry_point(graph, e) {
        Some(Excluded::EntryPoint)
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
        spans.iter().any(|(p, from, to)| *p == file && line >= *from && line <= *to)
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
fn names_mentioned_elsewhere(candidates: &[&CodeEntity], files: &BTreeSet<&Path>) -> HashSet<String> {
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
    for e in graph.entities() {
        if !is_listed(e) || !is_deletable(e) || e.metrics.fan_in > 0 {
            continue;
        }
        if !e.file_path.starts_with(scope) {
            continue;
        }
        match exclusion_reason(graph, e) {
            Some(Excluded::Test) => counts.tests += 1,
            Some(Excluded::EntryPoint) => counts.entry_points += 1,
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
/// evidence of death: tests, entry points, public API, and names the
/// source mentions somewhere the graph cannot see.
pub fn dead_code(server: &McpServer, args: &Value) -> Result<String> {
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
            by_file.entry(rel_path(&e.file_path, root)).or_default().push(e);
        }
    }

    Ok(render_dead_code(by_file, &counts, &scope, root, include_public))
}

/// Render the grouped report: files worst-first, each with its count.
fn render_dead_code(
    by_file: BTreeMap<String, Vec<&CodeEntity>>,
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

    // Files with the most candidates first; ties keep path order so the
    // output is stable across runs (AN-002).
    let mut files: Vec<(String, Vec<&CodeEntity>)> = by_file.into_iter().collect();
    files.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    for (file, mut hits) in files {
        hits.sort_by_key(|e| e.span.start.line);
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
            let base_dir = std::env::temp_dir().join(format!("nao-mcp-base-{}", from_sha));
            diff::create_worktree(repo_root, &base_dir, base_ref)?;
            let analyzed = diff::analyze_at(
                &base_dir,
                server.include_tests,
                &server.languages,
                &format!("base ({})", from_sha),
            );
            diff::remove_worktree(repo_root, &base_dir);
            let graph = Arc::new(analyzed?.0);
            let mut cache = server.base_cache.lock().unwrap();
            if cache.len() >= 4 {
                cache.clear();
            }
            cache.insert(base_key, (graph.clone(), base_dir.clone()));
            (graph, base_dir)
        }
    };

    // Head side: the working tree — shares the warm root-graph cache.
    let head_graph = analyze(server, repo_root)?;
    let result = diff::compute_diff(
        &base_graph, &head_graph, &base_dir, repo_root, &from_sha, "working",
    );
    let changed = diff::changed_files(repo_root, base_ref);
    Ok(render_change_report(&result, &base_graph, &head_graph, base_ref, &changed))
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

    let full = format!("{}\n{}", crate::output::elevator_text_renderer::LEGEND, body);
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
    for e in head_graph.entities().filter(|e| e.tags.contains("elevator")) {
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

pub(crate) fn render_change_report(
    result: &diff::DiffResult,
    base_graph: &DependencyGraph,
    head_graph: &DependencyGraph,
    base_ref: &str,
    changed_files: &[String],
) -> String {
    let s = &result.summary;
    let base_by_id: HashMap<&str, &CodeEntity> =
        base_graph.entities().map(|e| (e.id.as_str(), e)).collect();
    let head_by_id: HashMap<&str, &CodeEntity> =
        head_graph.entities().map(|e| (e.id.as_str(), e)).collect();

    let mut body = vec![
        format!("# Change assessment: {} ({}) → working tree", base_ref, result.from_ref),
        format!(
            "{} added, {} removed, {} modified ({} source, {} coupling-only), {} unchanged",
            s.added, s.removed, s.modified, s.modified_source, s.modified_impact, s.unchanged
        ),
    ];

    // Keep only rows whose underlying entity belongs in an agent-facing
    // listing (compute_diff itself only excludes Parameters): drops File
    // rows (they duplicate their children), ghosts, fields, imports.
    let skip_row = |d: &diff::EntityDiff| {
        head_by_id
            .get(d.entity_id.as_str())
            .or_else(|| d.base_entity_id.as_deref().and_then(|id| base_by_id.get(id)))
            .map(|e| !is_listed(e))
            .unwrap_or(d.kind == "File")
    };
    let row_loc = |d: &diff::EntityDiff| format!("{} `{}` — {}", d.kind, d.name, d.file_path);

    // Smell churn, joined across the two graphs by entity id.
    let mut new_smells: Vec<String> = Vec::new();
    let mut resolved_smells: Vec<String> = Vec::new();
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
        for smell in head_smells.difference(&base_smells) {
            new_smells.push(format!("- {} on {} — {}", smell.label(), row_loc(d), smell.hint()));
        }
        for smell in base_smells.difference(&head_smells) {
            resolved_smells.push(format!("- {} on {}", smell.label(), row_loc(d)));
        }
    }
    if !new_smells.is_empty() {
        body.push(String::new());
        body.push(format!("## ⚠ New smells ({})", new_smells.len()));
        body.append(&mut new_smells);
    }
    if !resolved_smells.is_empty() {
        body.push(String::new());
        body.push(format!("## Resolved smells ({})", resolved_smells.len()));
        body.append(&mut resolved_smells);
    }

    // Modified entities with source changes, worst complexity growth first.
    let regression_score = |d: &diff::EntityDiff| -> f64 {
        d.metric_deltas
            .iter()
            .filter(|m| matches!(m.name.as_str(), "cyclomatic" | "max_nesting"))
            .map(|m| m.delta.max(0.0))
            .sum()
    };
    let mut modified: Vec<&diff::EntityDiff> = result
        .entities
        .iter()
        .filter(|d| d.status == diff::ChangeStatus::Modified && d.source_changed && !skip_row(d))
        .collect();
    modified.sort_by(|a, b| regression_score(b).total_cmp(&regression_score(a)));

    body.push(String::new());
    body.push(format!("## Modified ({})", modified.len()));
    for d in modified.iter().take(40) {
        let deltas = d
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
        let deltas = if deltas.is_empty() { "source changed, metrics stable".to_string() } else { deltas };
        body.push(format!("- {}: {}", row_loc(d), deltas));
    }
    if modified.len() > 40 {
        body.push(format!("… and {} more modified entities.", modified.len() - 40));
    }

    // Added entities, with metrics so oversized newcomers stand out.
    let added: Vec<&diff::EntityDiff> = result
        .entities
        .iter()
        .filter(|d| d.status == diff::ChangeStatus::Added && !skip_row(d))
        .collect();
    body.push(String::new());
    body.push(format!("## Added ({})", added.len()));
    for d in added.iter().take(30) {
        match head_by_id.get(d.entity_id.as_str()) {
            Some(e) => body.push(format!("- {} ({})", row_loc(d), metric_suffix(e))),
            None => body.push(format!("- {}", row_loc(d))),
        }
    }
    if added.len() > 30 {
        body.push(format!("… and {} more added entities.", added.len() - 30));
    }

    // Removed entities, names only.
    let removed: Vec<&diff::EntityDiff> = result
        .entities
        .iter()
        .filter(|d| d.status == diff::ChangeStatus::Removed && !skip_row(d))
        .collect();
    body.push(String::new());
    body.push(format!("## Removed ({})", removed.len()));
    for d in removed.iter().take(30) {
        body.push(format!("- {}", row_loc(d)));
    }
    if removed.len() > 30 {
        body.push(format!("… and {} more removed entities.", removed.len() - 30));
    }

    // Coupling ripples: entities whose fan-in/out shifted without source edits.
    let impact: Vec<&diff::EntityDiff> = result
        .entities
        .iter()
        .filter(|d| d.status == diff::ChangeStatus::Modified && !d.source_changed && !skip_row(d))
        .collect();
    if !impact.is_empty() {
        body.push(String::new());
        body.push(format!(
            "## Coupling ripples ({} entities with fan-in/out shifts, no source change)",
            impact.len()
        ));
    }

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
            body.push(format!("… and {} more claimed entities.", claims.len() - 25));
        }
    }

    cap_lines(body, "Narrow the change or assess per-folder with `quality`.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            Self::with_prefix("nao-mcp-test", name)
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
        }
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

    #[test]
    fn overview_renders_full_domain_map_with_legend() {
        let dir = TmpDir::new("overview-map");
        dir.write("spec.elv", SPEC);
        let out = overview(&server_for(&dir), &json!({})).unwrap();
        assert!(out.contains("# Elevator spec format"), "legend missing:\n{out}");
        assert!(out.contains("c library"), "category missing:\n{out}");
        assert!(out.contains("f protocol"), "feature missing:\n{out}");
    }

    #[test]
    fn overview_focus_marks_target_and_keeps_full_descriptions() {
        let dir = TmpDir::new("overview-focus");
        dir.write("spec.elv", SPEC);
        let out = overview(&server_for(&dir), &json!({"focus": "f.protocol"})).unwrap();
        assert!(out.contains("← target"), "target marker missing:\n{out}");
        assert!(out.contains(LONG_DESC), "focus must render `d:` unclipped:\n{out}");
        assert!(out.contains("[cr: src/protocol/]"), "code reference missing:\n{out}");
    }

    #[test]
    fn overview_without_spec_says_so() {
        let dir = TmpDir::new("overview-empty");
        dir.write("main.rs", "fn main() {}\n");
        let err = overview(&server_for(&dir), &json!({})).unwrap_err();
        assert!(err.to_string().contains("No Elevator"), "unexpected error: {err:#}");
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
        let bare_graph =
            analyze(&server_for(&bare), &bare.0.canonicalize().unwrap()).unwrap();
        assert!(spec_claims(&bare_graph, &changed).is_empty());
    }

    // ---- similar (MCP-012) ----------------------------------------

    /// Every fixture lives under a temp path containing "test", which the
    /// walker's test heuristic excludes wholesale. A fixture made of real
    /// source has to opt back in or the graph comes back empty.
    fn code_server_for(dir: &TmpDir) -> McpServer {
        McpServer { include_tests: true, ..server_for(dir) }
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
        assert!(out.contains("resolve_git_ref"), "lost the real match:\n{out}");
        assert!(
            !out.contains("toRef"),
            "a hit carried by the word `to` survived:\n{out}"
        );
        // `top: 6` is a ceiling, not a quota — the tail is not padded.
        let hits = out.lines().filter(|l| l.starts_with("- [")).count();
        assert!(hits <= 2, "expected the match and at most one other:\n{out}");
        // The trace still names the tokens responsible.
        assert!(out.contains("matched: git, ref, resolve"), "trace lost:\n{out}");
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
        let dir = TmpDir::with_prefix("nao-mcp-fixture", name);
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
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap();

        assert!(out.contains("`orphan`"), "the one dead function is missing:\n{out}");
        // A caller exists — the graph resolves it.
        assert!(!out.contains("`live`"), "a called function was reported:\n{out}");
        // `main` is an entry point, `fmt` satisfies a trait contract.
        assert!(!out.contains("`main`"), "the program entry was reported:\n{out}");
        assert!(!out.contains("`fmt`"), "a trait impl method was reported:\n{out}");
        // `pub` means a caller can live outside this analysis entirely.
        assert!(
            !out.contains("`exported_but_unused`"),
            "public API was reported by default:\n{out}"
        );
        // The header states each population it filtered rather than
        // shrinking the list silently.
        assert!(out.contains("entry point"), "exclusion counts missing:\n{out}");
    }

    #[test]
    fn dead_code_spares_a_name_only_a_macro_body_mentions() {
        let dir = dead_code_fixture("dead-code-macro");
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap();
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

    #[test]
    fn dead_code_include_public_reveals_the_unused_export() {
        let dir = dead_code_fixture("dead-code-public");
        let out = dead_code(&code_server_for(&dir), &json!({"include_public": true})).unwrap();
        assert!(
            out.contains("`exported_but_unused`"),
            "include_public did not reveal public API:\n{out}"
        );
        assert!(out.contains(", public"), "public rows are not marked:\n{out}");
    }

    #[test]
    fn dead_code_groups_by_file_with_a_per_file_count() {
        let dir = dead_code_fixture("dead-code-grouping");
        dir.write(
            "extra.rs",
            "fn first_orphan() {}\nfn second_orphan() {}\n",
        );
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap();
        assert!(out.contains("extra.rs (2)"), "per-file count missing:\n{out}");
        assert!(out.contains("app.rs (1)"), "per-file count missing:\n{out}");
        // Worst file first, so the heaviest cleanup target leads.
        let extra = out.find("extra.rs (2)").unwrap();
        let app = out.find("app.rs (1)").unwrap();
        assert!(extra < app, "files are not ranked by count:\n{out}");
    }

    #[test]
    fn dead_code_says_so_when_there_is_nothing_to_report() {
        let dir = TmpDir::with_prefix("nao-mcp-fixture", "dead-code-clean");
        dir.write("app.rs", "fn live(x: i32) -> i32 { x }\n\nfn main() {\n    live(1);\n}\n");
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap();
        assert!(out.contains("None —"), "clean run is not stated:\n{out}");
    }

    /// A test living in a regular source file under `mod locality_tests`
    /// is test code: the ancestor-module rule matches names the way
    /// `is_test_path` matches paths.
    #[test]
    fn test_entities_are_recognized_in_any_test_named_module() {
        // The fixture directory must not itself read as test code — the
        // point is to exercise the ancestor-module rule, not the path one.
        let dir = TmpDir::with_prefix("nao-mcp-fixture", "dead-code-inline-cases");
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
        let out = dead_code(&code_server_for(&dir), &json!({})).unwrap();
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
        let dir = TmpDir::with_prefix("nao-mcp-fixture", name);
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
        let dir = TmpDir::with_prefix("nao-mcp-fixture", "impact-zero");
        dir.write("app.rs", "pub fn lonely(x: i32) -> i32 { x }\n");
        let out = impact(&code_server_for(&dir), &json!({"entity": "lonely"})).unwrap();

        assert!(out.contains("## Used by (0)"), "expected a zero count:\n{out}");
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
        assert!(out.contains("`bySide0`"), "the referring entity is missing:\n{out}");
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
        let dir = TmpDir::with_prefix("nao-mcp-fixture", "impact-normal");
        dir.write(
            "app.rs",
            "pub fn helper(x: i32) -> i32 { x }\npub fn caller() -> i32 { helper(1) }\n",
        );
        let out = impact(&code_server_for(&dir), &json!({"entity": "helper"})).unwrap();

        assert!(out.contains("`caller`"), "the real dependent is missing:\n{out}");
        assert!(
            !out.contains("not the same as none"),
            "the zero-case note fired on a non-zero count:\n{out}"
        );
        assert!(
            !out.contains(POSSIBLE_HEADING),
            "a hedged section appeared with nothing to hedge:\n{out}"
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
        assert_eq!(signature(&small), "resolve_git_ref(git_ref: &str) -> String");
    }
}
