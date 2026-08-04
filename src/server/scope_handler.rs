use axum::{extract::State, http::StatusCode, response::Json};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::graph::DependencyGraph;
use crate::models::{RelationshipKind, Thresholds};
use petgraph::Direction;

use super::refactor_prompt::{self, PromptContext};
use super::state::AppState;
use super::types::{ScopeEntity, ScopeExports, ScopeRequest, ScopeResponse, TraversalRule};

/// Scope map: entity_id → set of reasons why it's included.
type ScopeMap = HashMap<String, HashSet<String>>;

/// Insert an entity into the scope map with a reason tag.
fn add_to_scope(scope: &mut ScopeMap, id: &str, reason: &str) {
    scope
        .entry(id.to_string())
        .or_default()
        .insert(reason.to_string());
}

// ------------------------------------------------------------------
//  Scope collection strategies
// ------------------------------------------------------------------

/// BFS traversal: walk all edge types bidirectionally up to `depth` hops.
fn collect_scope_manual(
    graph: &DependencyGraph,
    entity_id: &str,
    depth: usize,
    scope: &mut ScopeMap,
) {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(entity_id.to_string());
    let mut frontier = vec![entity_id.to_string()];

    for _ in 0..depth {
        let mut next = Vec::new();
        for id in &frontier {
            for (dep, _) in graph.dependencies(id) {
                if visited.insert(dep.id.clone()) {
                    add_to_scope(scope, &dep.id, "Reachable");
                    next.push(dep.id.clone());
                }
            }
            for (dep, _) in graph.dependents(id) {
                if visited.insert(dep.id.clone()) {
                    add_to_scope(scope, &dep.id, "Reachable");
                    next.push(dep.id.clone());
                }
            }
            for child in graph.children(id) {
                if visited.insert(child.id.clone()) {
                    add_to_scope(scope, &child.id, "Reachable");
                    next.push(child.id.clone());
                }
            }
            if let Some(parent) = graph.parent(id) {
                if visited.insert(parent.id.clone()) {
                    add_to_scope(scope, &parent.id, "Reachable");
                    next.push(parent.id.clone());
                }
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
}

/// Traversal rules for the "refactor" mode.
fn refactor_rules() -> Vec<TraversalRule> {
    vec![
        TraversalRule { kind: RelationshipKind::Calls,        direction: Direction::Incoming, reason: "Callers" },
        TraversalRule { kind: RelationshipKind::Calls,        direction: Direction::Outgoing, reason: "Callees" },
        TraversalRule { kind: RelationshipKind::UsesType,     direction: Direction::Outgoing, reason: "Types" },
        TraversalRule { kind: RelationshipKind::Returns,      direction: Direction::Outgoing, reason: "Types" },
        TraversalRule { kind: RelationshipKind::Instantiates, direction: Direction::Outgoing, reason: "Types" },
        TraversalRule { kind: RelationshipKind::Implements,   direction: Direction::Outgoing, reason: "Traits/Interfaces" },
        TraversalRule { kind: RelationshipKind::Inherits,     direction: Direction::Outgoing, reason: "Base Classes" },
    ]
}

/// Traversal rules for the "understand" mode.
fn understand_rules() -> Vec<TraversalRule> {
    vec![
        TraversalRule { kind: RelationshipKind::Calls,        direction: Direction::Outgoing, reason: "Callees" },
        TraversalRule { kind: RelationshipKind::UsesType,     direction: Direction::Outgoing, reason: "Types" },
        TraversalRule { kind: RelationshipKind::Returns,      direction: Direction::Outgoing, reason: "Types" },
        TraversalRule { kind: RelationshipKind::Instantiates, direction: Direction::Outgoing, reason: "Types" },
        TraversalRule { kind: RelationshipKind::Implements,   direction: Direction::Outgoing, reason: "Traits/Interfaces" },
        TraversalRule { kind: RelationshipKind::Imports,      direction: Direction::Outgoing, reason: "Imports" },
    ]
}

/// Directed traversal: parent, siblings (refactor only), then typed
/// relationship rules.
fn collect_scope_directed(
    graph: &DependencyGraph,
    entity_id: &str,
    parent_id: Option<&str>,
    is_refactor: bool,
    scope: &mut ScopeMap,
) {
    if let Some(parent) = graph.parent(entity_id) {
        add_to_scope(scope, &parent.id, "Parent");
    }

    // Siblings (refactor only)
    if is_refactor {
        if let Some(pid) = parent_id {
            for sibling in graph.children(pid) {
                if sibling.id != entity_id {
                    add_to_scope(scope, &sibling.id, "Siblings");
                }
            }
        }
    }

    let rules = if is_refactor { refactor_rules() } else { understand_rules() };
    for rule in &rules {
        for (related, _) in graph.related_by_kind(entity_id, rule.kind, rule.direction) {
            add_to_scope(scope, &related.id, rule.reason);
        }
    }
}

// ------------------------------------------------------------------
//  Response assembly
// ------------------------------------------------------------------

/// Result of converting a scope map into response-ready data.
pub(super) struct ScopeAssembly {
    entities: Vec<ScopeEntity>,
    reason_summary: HashMap<String, usize>,
    files: Vec<String>,
    entity_context: String,
    /// Instruction half of the refactor prompt (SRV-010). Built here rather
    /// than in `finish_scope` because it needs the selected `CodeEntity` and
    /// its metrics, which are only in reach while the graph is borrowed.
    /// `finish_scope` appends the context body to it.
    prompt_header: String,
    /// Which context body `finish_scope` should append.
    prompt_context: PromptContext,
}

/// Convert the scope map into sorted `ScopeEntity` list, file set,
/// reason summary, and concatenated entity source context.
fn build_scope_entities(
    graph: &DependencyGraph,
    scope: ScopeMap,
    selected_id: &str,
    root_path: &Path,
    excluded: &HashSet<String>,
    prompt_context: PromptContext,
) -> ScopeAssembly {
    let mut entries: Vec<_> = scope.into_iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Selected entity first, then the rest.
    let ordered: Vec<_> = entries
        .iter()
        .filter(|(id, _)| id == selected_id)
        .chain(entries.iter().filter(|(id, _)| id != selected_id))
        .collect();

    let mut entities: Vec<ScopeEntity> = Vec::new();
    let mut file_set: HashSet<String> = HashSet::new();
    let mut source_parts: Vec<String> = Vec::new();

    for (id, reasons) in ordered {
        let Some(e) = graph.get_entity(id) else { continue };
        let fp = e
            .file_path
            .strip_prefix(root_path)
            .unwrap_or(&e.file_path)
            .display()
            .to_string();
        if excluded.contains(&fp) {
            continue;
        }
        file_set.insert(fp.clone());

        let line = e.span.start.line + 1;
        let end_line = e.span.end.line + 1;
        if let Some(ref src) = e.source_code {
            source_parts.push(format!("// {}:{}-{}\n{}", fp, line, end_line, src));
        }

        entities.push(ScopeEntity {
            id: e.id.clone(),
            name: e.name.clone(),
            qualified_name: e.qualified_name.clone(),
            kind: format!("{:?}", e.kind),
            file_path: fp,
            line,
            end_line,
            source_code: e.source_code.clone(),
            reasons: reasons.iter().cloned().collect(),
        });
    }

    let mut reason_summary: HashMap<String, usize> = HashMap::new();
    for se in &entities {
        for r in &se.reasons {
            *reason_summary.entry(r.clone()).or_default() += 1;
        }
    }

    let mut files: Vec<String> = file_set.into_iter().collect();
    files.sort();

    // Built from the graph rather than from `entities`, so excluding the
    // target's file from the *context* still yields a prompt about it — the
    // file checkboxes trim what the agent reads, not what it is asked to do.
    let prompt_header = graph
        .get_entity(selected_id)
        .map(|e| {
            let rel = e
                .file_path
                .strip_prefix(root_path)
                .unwrap_or(&e.file_path)
                .display()
                .to_string();
            refactor_prompt::build_header(e, &rel, &Thresholds::default(), prompt_context)
        })
        .unwrap_or_default();

    ScopeAssembly {
        entities,
        reason_summary,
        files,
        entity_context: source_parts.join("\n\n"),
        prompt_header,
        prompt_context,
    }
}

/// Read full file contents from disk for the file-level context export.
fn read_full_files(root: &Path, files: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for fp in files {
        let abs = root.join(fp);
        if let Ok(content) = std::fs::read_to_string(&abs) {
            parts.push(format!("// {}\n{}", fp, content));
        }
    }
    parts.join("\n\n")
}

// ------------------------------------------------------------------
//  Handler
// ------------------------------------------------------------------

/// Traverse the graph for one scope request. Split out of the handler so
/// serve mode's `/api/repos/{slug}/scope` can run the same traversal
/// against a `RepoState` that holds its graph by value rather than behind
/// the watch-mode locks.
pub(super) fn collect_scope(
    graph: &DependencyGraph,
    root_path: &Path,
    req: &ScopeRequest,
) -> Result<ScopeAssembly, (StatusCode, String)> {
    let entity = graph.get_entity(&req.entity_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("Entity not found: {}", req.entity_id))
    })?;

    let mut scope: ScopeMap = HashMap::new();
    add_to_scope(&mut scope, &entity.id, "Selected");

    match req.mode.as_str() {
        "manual" => {
            collect_scope_manual(graph, &req.entity_id, req.depth, &mut scope);
        }
        "refactor" | "understand" => {
            let is_refactor = req.mode == "refactor";
            collect_scope_directed(
                graph,
                &req.entity_id,
                entity.parent_id.as_deref(),
                is_refactor,
                &mut scope,
            );
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown mode: {}", req.mode),
            ));
        }
    }

    let prompt_context = PromptContext::parse(req.prompt_context.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let excluded: HashSet<String> = req.excluded_files.iter().cloned().collect();
    Ok(build_scope_entities(
        graph, scope, &req.entity_id, root_path, &excluded, prompt_context,
    ))
}

/// Turn a traversal result into the wire response, reading the full file
/// bodies from `repo_root`. Shared with serve mode.
pub(super) fn finish_scope(assembly: ScopeAssembly, repo_root: &Path) -> ScopeResponse {
    let token_count_entities = assembly.entity_context.len() / 4;
    let full_files = read_full_files(repo_root, &assembly.files);
    let token_count_files = full_files.len() / 4;

    let paths_export = assembly.files.join("\n");
    let ranges_export: String = assembly.entities
        .iter()
        .map(|e| format!("{}:{}-{}", e.file_path, e.line, e.end_line))
        .collect::<Vec<_>>()
        .join("\n");

    // The prompt carries its own context so it is paste-ready on its own —
    // the user should not have to copy two buttons and staple them together.
    let refactor_prompt = if assembly.prompt_header.is_empty() {
        String::new()
    } else {
        // The selected entity is ordered first by `build_scope_entities`, so
        // its rendered block is the head of the context — that is what the
        // hybrid mode keeps in full.
        let target_source = assembly.entities.first().and_then(|e| {
            e.source_code.as_ref().map(|src| {
                format!("// {}:{}-{}\n{}", e.file_path, e.line, e.end_line, src)
            })
        });
        let body = refactor_prompt::context_body(
            assembly.prompt_context,
            &assembly.entity_context,
            &ranges_export,
            target_source.as_deref(),
        );
        format!("{}\n{}\n", assembly.prompt_header, body)
    };

    ScopeResponse {
        entities: assembly.entities,
        reason_summary: assembly.reason_summary,
        files: assembly.files,
        token_count_entities,
        token_count_files,
        exports: ScopeExports {
            paths: paths_export,
            ranges: ranges_export,
            entity_context: assembly.entity_context,
            full_files,
            refactor_prompt,
        },
    }
}

/// POST /api/scope — smart scope traversal.
pub(crate) async fn scope_handler(
    State(state): State<AppState>,
    Json(req): Json<ScopeRequest>,
) -> Result<Json<ScopeResponse>, (StatusCode, String)> {
    // All graph work happens inside this block so the RwLockReadGuard
    // is dropped before any .await points.
    let assembly = {
        let graph = state.graph.read().map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Graph lock poisoned: {}", e))
        })?;
        let config = state.config.read().map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Config lock poisoned: {}", e))
        })?;
        collect_scope(&graph, &config.root_path, &req)?
    }; // graph lock dropped here

    // Full file context — read files from disk (no graph lock needed)
    let root_path = state.repo_root.read().await.clone();
    Ok(Json(finish_scope(assembly, &root_path)))
}
