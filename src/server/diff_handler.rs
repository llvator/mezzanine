use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::{extract::State, http::StatusCode, response::Json};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::diff;
use crate::graph::DependencyGraph;
use crate::output::{self, JsonRenderer};

use super::state::{build_config, AppState, CachedBase, LiveDiff, ReloadKind};
use super::types::{DiffRequest, DiffResponse, DiffSummaryResponse, RootPathRequest, RootPathResponse};

/// The one `to_ref` that means "the tree this server is watching" rather
/// than a git ref that has to be checked out into a temp worktree first.
const WORKING_REF: &str = "WORKING";

/// Whether this request's head is the live working tree. The answer decides
/// whether the result may be adopted as live state — see the swap at the end
/// of `diff_handler` (SRV-019).
fn is_working_head(req: &DiffRequest) -> bool {
    req.to_ref == WORKING_REF
}

/// The config to diff the working tree with: the live one, re-rooted at the
/// path this server actually watches.
///
/// The language filter and test-inclusion settings are the live ones and are
/// kept. The root is not taken on trust, because `root_path` is the single
/// field a previous diff could have moved: it decides what
/// `strip_prefix` removes from every `file_path`, and a root pointing at a
/// deleted temp worktree strips nothing, so the entity keys stop matching the
/// base's and every entity reads as added-and-removed (SRV-019).
///
/// Restating it here rather than only fixing the site that broke it makes
/// this call independent of whatever ran before it.
fn working_head_config(mut config: Config, repo_root: &Path) -> Config {
    config.root_path = repo_root.to_path_buf();
    config
}

/// The cached base analysis for `sha`, or `None` when the cache holds a
/// different ref (or nothing).
///
/// Clones rather than moves: a refresh that fails partway must not have
/// consumed the cache, or the next save pays for the analysis again.
fn take_cached_base(
    cache: &Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
    sha: &str,
) -> Option<(DependencyGraph, Config)> {
    let guard = cache.as_ref()?.read().ok()?;
    let cached = guard.as_ref()?;
    if cached.sha != sha {
        return None;
    }
    Some((cached.graph.clone(), cached.config.clone()))
}

fn store_cached_base(
    cache: &Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
    sha: &str,
    graph: &DependencyGraph,
    config: &Config,
) {
    let Some(cache) = cache else { return };
    if let Ok(mut slot) = cache.write() {
        let already = slot.as_ref().is_some_and(|c| c.sha == sha);
        if !already {
            *slot = Some(CachedBase {
                sha: sha.to_string(),
                graph: graph.clone(),
                config: config.clone(),
            });
        }
    }
}

// ------------------------------------------------------------------
//  Main blocking diff pipeline
// ------------------------------------------------------------------

/// Blocking helper that performs the actual diff computation.
fn compute_diff_blocking(
    repo_root: &Path,
    output_dir: &Path,
    include_tests: bool,
    languages: &Option<Vec<String>>,
    from_ref: &str,
    to_ref: &str,
    current_graph: Option<Arc<std::sync::RwLock<DependencyGraph>>>,
    current_config: Option<Arc<std::sync::RwLock<Config>>>,
    base_cache: Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
) -> Result<(DiffResponse, DependencyGraph, Config, String, String), String> {
    let is_working = to_ref == WORKING_REF;
    let total_start = Instant::now();
    let to_label = if is_working { "Working Tree" } else { to_ref };
    eprintln!("🔄 Starting diff: {} → {}", from_ref, to_label);

    let e2s = |e: anyhow::Error| e.to_string();

    // Resolve refs
    let from_sha = diff::resolve_git_ref(repo_root, from_ref)
        .map_err(|e| format!("Invalid from_ref: {}", e))?;
    let to_sha = if is_working {
        "working".to_string()
    } else {
        diff::resolve_git_ref(repo_root, to_ref)
            .map_err(|e| format!("Invalid to_ref: {}", e))?
    };

    // Create + analyze base worktree, unless the last diff already did.
    let tmp = std::env::temp_dir();
    let base_dir = tmp.join(format!("nao-diff-base-{}", from_sha));
    let worktree_start = Instant::now();
    let cached = take_cached_base(&base_cache, &from_sha);
    let base_is_ours = cached.is_none();
    let (base_graph, base_config) = match cached {
        Some((graph, config)) => {
            eprintln!("  Reusing base ({}) analysis", from_sha);
            (graph, config)
        }
        None => {
            diff::create_worktree(repo_root, &base_dir, from_ref).map_err(e2s)?;
            diff::analyze_at(&base_dir, include_tests, languages, &format!("base ({})", from_sha)).map_err(e2s)?
        }
    };

    // Acquire head graph: from in-memory state (WORKING) or a new worktree
    let (head_graph, head_config, head_dir_for_diff) = if is_working {
        eprintln!("   Using current working directory as head...");
        let g = current_graph.unwrap();
        let c = current_config.unwrap();
        let graph = g.read().map_err(|e| format!("Graph lock: {}", e))?.clone();
        let config = c.read().map_err(|e| format!("Config lock: {}", e))?.clone();
        let config = working_head_config(config, repo_root);
        let dir = config.root_path.clone();
        (graph, config, dir)
    } else {
        let hdir = tmp.join(format!("nao-diff-head-{}", to_sha));
        if let Err(e) = diff::create_worktree(repo_root, &hdir, to_ref) {
            if base_is_ours {
                diff::remove_worktree(repo_root, &base_dir);
            }
            return Err(e.to_string());
        }
        let (graph, config) =
            diff::analyze_at(&hdir, include_tests, languages, &format!("head ({})", to_sha)).map_err(e2s)?;
        (graph, config, hdir)
    };

    eprintln!(
        "   Worktree(s) created in {:.1}s",
        worktree_start.elapsed().as_secs_f32()
    );

    // Compute structural diff
    eprintln!("   Computing structural diff...");
    let diff_start = Instant::now();
    let diff_result = diff::compute_diff(
        &base_graph, &head_graph,
        &base_dir, &head_dir_for_diff,
        &from_sha, &to_sha,
    );
    eprintln!(
        "   Diff computed in {:.1}s: +{} -{} ~{}",
        diff_start.elapsed().as_secs_f32(),
        diff_result.summary.added, diff_result.summary.removed, diff_result.summary.modified
    );

    // Write output files
    let (diff_json, base_details_str) =
        diff::write_diff_outputs(output_dir, &head_graph, &head_config, &base_graph, &base_config, &diff_result).map_err(e2s)?;

    // Cleanup worktrees. Only the ones this call created — a reused base
    // was removed by whichever call analyzed it, and asking git to remove it
    // again just prints an error.
    if base_is_ours || !is_working {
        eprintln!("   Cleaning up worktrees...");
    }
    if base_is_ours {
        diff::remove_worktree(repo_root, &base_dir);
    }
    if !is_working {
        let hdir = tmp.join(format!("nao-diff-head-{}", to_sha));
        diff::remove_worktree(repo_root, &hdir);
    }

    // Keep the base for the next refresh. Its worktree is gone either way —
    // the graph is what the next diff needs, and re-deriving it from a
    // checkout that has not moved is the cost this avoids.
    store_cached_base(&base_cache, &from_sha, &base_graph, &base_config);

    eprintln!("✅ Diff complete in {:.1}s total", total_start.elapsed().as_secs_f32());

    let resp = DiffResponse {
        success: true,
        message: format!("Diff computed: {} → {}", from_sha, to_label),
        summary: Some(DiffSummaryResponse {
            added: diff_result.summary.added,
            removed: diff_result.summary.removed,
            modified: diff_result.summary.modified,
            modified_source: diff_result.summary.modified_source,
            modified_impact: diff_result.summary.modified_impact,
        }),
    };
    Ok((resp, head_graph, head_config, diff_json, base_details_str))
}

/// POST /api/diff — compute diff between two commits.
pub(crate) async fn diff_handler(
    State(state): State<AppState>,
    Json(req): Json<DiffRequest>,
) -> Result<Json<DiffResponse>, (StatusCode, String)> {
    // Check if diff is already in progress
    {
        let mut in_progress = state.diff_in_progress.lock().await;
        if *in_progress {
            return Ok(Json(DiffResponse {
                success: false,
                message: "A diff computation is already in progress".to_string(),
                summary: None,
            }));
        }
        *in_progress = true;
    }

    let resp = run_diff(&state, &req).await;

    // Release the lock
    {
        let mut in_progress = state.diff_in_progress.lock().await;
        *in_progress = false;
    }

    match resp {
        Ok(resp) => {
            // Remember (or forget) what the watcher should keep current.
            // A user who asks for two commits has pinned a comparison; a
            // file save must not move it out from under them (UI-067).
            if resp.success {
                if let Ok(mut live) = state.live_diff.write() {
                    *live = if state.follow_diff && is_working_head(&req) {
                        Some(LiveDiff { from_ref: req.from_ref.clone() })
                    } else {
                        None
                    };
                }
            }
            Ok(Json(resp))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// Re-run the live working-tree diff, if there is one, against the graph the
/// watcher just published (UI-067).
///
/// Skips rather than queues when a diff is already running: the next save
/// brings another event, and a queue of refreshes for a tree that has moved
/// on is work nobody is waiting for.
pub(crate) async fn refresh_live_diff(state: &AppState) {
    let Some(live) = state.live_diff.read().ok().and_then(|l| l.clone()) else {
        return;
    };

    let mut in_progress = match state.diff_in_progress.try_lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if *in_progress {
        return;
    }
    *in_progress = true;
    drop(in_progress);

    let req = DiffRequest {
        from_ref: live.from_ref,
        to_ref: WORKING_REF.to_string(),
    };
    let outcome = run_diff(state, &req).await;

    {
        let mut in_progress = state.diff_in_progress.lock().await;
        *in_progress = false;
    }

    match outcome {
        Ok(resp) if resp.success => {}
        Ok(resp) => eprintln!("   ⚠ Live diff refresh declined: {}", resp.message),
        Err(e) => {
            // A base ref that stopped resolving (a rebase, a deleted branch)
            // would otherwise fail on every save for the rest of the session.
            eprintln!("   ⚠ Live diff refresh failed, unpinning: {}", e);
            if let Ok(mut l) = state.live_diff.write() {
                *l = None;
            }
        }
    }
}

/// Compute a diff and publish it, without touching the in-progress lock or
/// the live-diff bookkeeping — both callers own those differently.
async fn run_diff(state: &AppState, req: &DiffRequest) -> Result<DiffResponse, String> {
    // Read repo_root before spawning blocking task
    let repo_root = state.repo_root.read().await.clone();

    // Run diff in a blocking task since it's CPU-intensive
    let result = tokio::task::spawn_blocking({
        let output_dir = state.output_dir.clone();
        let include_tests = state.include_tests;
        let languages = state.languages.clone();
        let from_ref = req.from_ref.clone();
        let to_ref = req.to_ref.clone();
        let is_working = to_ref == WORKING_REF;
        let current_graph = if is_working {
            Some(state.graph.clone())
        } else {
            None
        };
        let current_config = if is_working {
            Some(state.config.clone())
        } else {
            None
        };
        let base_cache = Some(state.base_cache.clone());

        move || {
            compute_diff_blocking(
                &repo_root,
                &output_dir,
                include_tests,
                &languages,
                &from_ref,
                &to_ref,
                current_graph,
                current_config,
                base_cache,
            )
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    // Store results in memory and signal reload
    match result {
        Ok((resp, head_graph, head_config, diff_json, base_details_str)) => {
            if resp.success {
                // Adopt the head graph only when the head *is* what this
                // server watches — i.e. the working tree.
                //
                // For a commit ref, `head_config.root_path` is a temp
                // worktree that `remove_worktree` deleted a few lines ago,
                // and adopting it re-roots every subsequent response at a
                // directory that no longer exists: `/api/index` reports it,
                // `file_path`s stop stripping to repo-relative, and the
                // scope tree renders the user's home directory as its top
                // folder. The next `→ WORKING` diff then takes its head root
                // from the same poisoned config and reports every entity as
                // added-and-removed (SRV-019).
                //
                // Nothing is lost by declining: the head commit's graph was
                // only ever visible until the next file save, which the
                // watcher answers by publishing the working tree again. A
                // diff is an overlay on what is being watched, not a
                // checkout of something else. `write_diff_outputs` has
                // already written the head's own `data.json` for consumers
                // that want it.
                if is_working_head(req) {
                    if let Ok(mut g) = state.graph.write() {
                        *g = head_graph;
                    }
                    if let Ok(mut c) = state.config.write() {
                        *c = head_config;
                    }
                }
                if let Ok(mut d) = state.diff_result.write() {
                    *d = Some(diff_json);
                }
                if let Ok(mut b) = state.base_details.write() {
                    *b = Some(base_details_str);
                }
                // `Diff`, not `Graph`: the graph is either unchanged or was
                // just published by whoever triggered this, and telling
                // clients to re-fetch it would restart a canvas that has no
                // reason to move (UI-067).
                let _ = state.tx.send(ReloadKind::Diff);
            }
            Ok(resp)
        }
        Err(e) => Err(e),
    }
}

/// POST /api/root — change the analyzed root path and re-analyze.
pub(crate) async fn set_root_handler(
    State(state): State<AppState>,
    Json(req): Json<RootPathRequest>,
) -> Result<Json<RootPathResponse>, (StatusCode, String)> {
    // Check if analysis is in progress
    {
        let in_progress = state.analysis_in_progress.lock().await;
        if *in_progress {
            return Ok(Json(RootPathResponse {
                path: req.path.clone(),
                success: false,
                message: Some("Analysis already in progress".to_string()),
                entity_count: None,
                relationship_count: None,
            }));
        }
    }

    // Validate the path
    let new_path = PathBuf::from(&req.path);
    let canonical_path = new_path
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;

    if !canonical_path.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Path is not a directory".to_string(),
        ));
    }

    // Mark analysis as in progress
    {
        let mut in_progress = state.analysis_in_progress.lock().await;
        *in_progress = true;
    }

    // Run analysis in a blocking task
    let result = tokio::task::spawn_blocking({
        let output_dir = state.output_dir.clone();
        let include_tests = state.include_tests;
        let include_docs = state.include_docs;
        let languages = state.languages.clone();
        let settings = state.settings.clone();
        let path = canonical_path.clone();

        move || -> Result<(usize, usize, DependencyGraph, Config), String> {
            let config = build_config(&path, include_tests, include_docs, &languages, &settings);

            // Run analysis
            let mut analyzer = Analyzer::new(config.clone());
            let result = analyzer.analyze().map_err(|e| e.to_string())?;
            let entity_count = result.entities.len();
            let rel_count = result.relationships.len();
            let graph = DependencyGraph::from_analysis(&result);

            // Write output files
            std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;
            let data_path = output_dir.join("data.json");
            let output_str =
                crate::output::render(&graph, &config).map_err(|e| e.to_string())?;
            std::fs::write(&data_path, &output_str).map_err(|e| e.to_string())?;

            let details_str = JsonRenderer::render_details(&graph, &config)
                .map_err(|e| e.to_string())?;
            std::fs::write(data_path.with_extension("details.json"), &details_str)
                .map_err(|e| e.to_string())?;

            let index_str =
                JsonRenderer::render_index(&graph, &config).map_err(|e| e.to_string())?;
            std::fs::write(data_path.with_extension("index.json"), &index_str)
                .map_err(|e| e.to_string())?;

            // Clear any existing diff.json since it's no longer valid
            let _ = std::fs::remove_file(output_dir.join("diff.json"));

            Ok((entity_count, rel_count, graph, config))
        }
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?;

    // Release the lock
    {
        let mut in_progress = state.analysis_in_progress.lock().await;
        *in_progress = false;
    }

    match result {
        Ok((entity_count, rel_count, new_graph, new_config)) => {
            // Update the in-memory graph and config
            if let Ok(mut g) = state.graph.write() {
                *g = new_graph;
            }
            if let Ok(mut c) = state.config.write() {
                *c = new_config;
            }

            // Update the repo_root
            {
                let mut root = state.repo_root.write().await;
                *root = canonical_path.clone();
            }

            // Clear stale diff state. The live-diff pin goes with it: it
            // named a ref in the old repo, and the base cache holds an
            // analysis of a tree this server is no longer watching.
            if let Ok(mut d) = state.diff_result.write() {
                *d = None;
            }
            if let Ok(mut b) = state.base_details.write() {
                *b = None;
            }
            if let Ok(mut l) = state.live_diff.write() {
                *l = None;
            }
            if let Ok(mut c) = state.base_cache.write() {
                *c = None;
            }

            // Signal reload to all clients
            let _ = state.tx.send(ReloadKind::Graph);

            Ok(Json(RootPathResponse {
                path: canonical_path.display().to_string(),
                success: true,
                message: Some(format!(
                    "Analyzed {} entities, {} relationships",
                    entity_count, rel_count
                )),
                entity_count: Some(entity_count),
                relationship_count: Some(rel_count),
            }))
        }
        Err(e) => Ok(Json(RootPathResponse {
            path: req.path,
            success: false,
            message: Some(format!("Analysis failed: {}", e)),
            entity_count: None,
            relationship_count: None,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(to_ref: &str) -> DiffRequest {
        DiffRequest { from_ref: "HEAD".to_string(), to_ref: to_ref.to_string() }
    }

    #[test]
    fn a_working_diff_may_become_the_servers_live_state() {
        assert!(is_working_head(&req(WORKING_REF)));
    }

    #[test]
    fn a_commit_diff_may_not() {
        // Each of these analyzes into a temp worktree that is deleted before
        // the response is written. Adopting one leaves the server rendering
        // its graph against a root that no longer exists (SRV-019).
        for r in ["HEAD", "HEAD~1", "main", "c0ff6d2", "v1.2.3", "working"] {
            assert!(!is_working_head(&req(r)), "'{r}' must not be adopted as live state");
        }
    }

    #[test]
    fn the_working_head_is_rooted_where_the_server_watches() {
        let mut live = Config::default();
        live.root_path = PathBuf::from("/tmp/nao-diff-head-abc123");
        let rooted = working_head_config(live, Path::new("/repo"));
        assert_eq!(rooted.root_path, PathBuf::from("/repo"));
    }

    #[test]
    fn a_poisoned_root_does_not_survive_into_the_next_diff() {
        // The sequence the bug lived in: a commit diff moved the live root,
        // and the *next* working diff inherited it. Re-rooting on every call
        // is what makes the second call independent of the first, so this
        // holds however badly the config arrives.
        for poisoned in ["/tmp/nao-diff-head-abc", "/tmp/nao-diff-base-def", "relative/nonsense", "/"] {
            let mut live = Config::default();
            live.root_path = PathBuf::from(poisoned);
            assert_eq!(
                working_head_config(live, Path::new("/repo")).root_path,
                PathBuf::from("/repo"),
                "root '{poisoned}' should have been replaced by the watched root",
            );
        }
    }

    #[test]
    fn re_rooting_keeps_the_rest_of_the_live_config() {
        // Only the root is suspect. The language filter and test-inclusion
        // settings are the user's current choices and have to survive, or a
        // diff would silently widen the analysis it compares.
        let mut live = Config::default();
        live.root_path = PathBuf::from("/tmp/nao-diff-head-abc");
        live.analysis.include_tests = !live.analysis.include_tests;
        let expected_tests = live.analysis.include_tests;
        let expected_langs = live.analysis.languages.clone();

        let rooted = working_head_config(live, Path::new("/repo"));
        assert_eq!(rooted.analysis.include_tests, expected_tests);
        assert_eq!(rooted.analysis.languages, expected_langs);
    }
}
