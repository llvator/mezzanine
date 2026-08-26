use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use axum::{extract::State, http::StatusCode, response::Json};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::diff;
use crate::graph::DependencyGraph;
use crate::output::{self, JsonRenderer};

use super::state::{build_config, AppState, CachedBase, LiveDiff, ReloadKind};
use super::types::{
    DiffRequest, DiffResponse, DiffSummaryResponse, RootPathRequest, RootPathResponse,
};

/// The one `to_ref` that means "the tree this server is watching" rather
/// than a git ref that has to be checked out into a temp worktree first.
const WORKING_REF: &str = "WORKING";

/// The `to_ref` that means the git index. A sentinel rather than a sha because
/// the index is not a ref: the commit that names it is manufactured *inside*
/// this call and is unreferenced, so a client that held one would be holding a
/// commit that means whatever the index happened to be when it was made — the
/// `stash@{N}` mistake with the failure moved to the other end (UI-111).
const STAGED_REF: &str = "STAGED";

/// What `diff.json` calls a staged head. The literal is the discriminator the
/// UI reads to know which tree the comparison looked at, the same channel
/// `working` already uses.
const STAGED_LABEL: &str = "staged";

/// Whether this request's head is the live working tree. The answer decides
/// whether the result may be adopted as live state — see the swap at the end
/// of `diff_handler` (SRV-019).
///
/// `STAGED` is *not* a working head, deliberately: the index is checked out
/// into a temp worktree like any commit, so adopting its graph would re-root
/// the server at a directory this call deletes before it returns.
fn is_working_head(req: &DiffRequest) -> bool {
    req.to_ref == WORKING_REF
}

/// The head side of a diff once the sentinels are resolved.
#[derive(Debug)]
struct Head {
    /// The ref to check out, or `None` for the working tree the server already
    /// holds a graph of.
    git_ref: Option<String>,
    /// What the result is reported and keyed under: a literal for either
    /// sentinel, the resolved short sha for an ordinary ref.
    label: String,
}

/// What the reader is told when there is nothing in the index. Not an error:
/// nothing staged is the ordinary state of a repository, and it is the one
/// answer an empty overlay would state as "nothing changed" about the wrong
/// thing.
const NOTHING_STAGED: &str = "Nothing is staged. `git add` the changes you want to look at first.";

/// Sort a head-resolution failure into the two kinds a caller answers
/// differently: a comparison the server *declines*, and one it could not make.
///
/// Nothing staged is the only declined one, and the distinction is whose
/// problem it is. A 500 tells the reader something broke and gives them
/// nothing to do about it; `success: false` with a message tells them to
/// `git add` something. Every other failure here — a ref that does not
/// resolve, git absent — is the server failing to answer.
fn declined(e: String) -> Result<DiffResponse, String> {
    if e == NOTHING_STAGED {
        Ok(DiffResponse {
            success: false,
            message: e,
            summary: None,
        })
    } else {
        Err(e)
    }
}

/// Turn a request's `to_ref` into the head to analyze.
///
/// Resolved on the async side, before the blocking task starts, because the
/// staged case has an answer that is not a diff at all — and finding that out
/// after a worktree checkout and a full analysis would be paying for it twice.
fn resolve_head(repo_root: &Path, to_ref: &str) -> Result<Head, String> {
    match to_ref {
        WORKING_REF => Ok(Head {
            git_ref: None,
            label: "working".to_string(),
        }),
        STAGED_REF => diff::staged_commit(repo_root)
            .map_err(|e| e.to_string())?
            .map(|sha| Head {
                git_ref: Some(sha),
                label: STAGED_LABEL.to_string(),
            })
            .ok_or_else(|| NOTHING_STAGED.to_string()),
        r => Ok(Head {
            label: diff::resolve_git_ref(repo_root, r)
                .map_err(|e| format!("Invalid to_ref: {}", e))?,
            // The ref as asked for, not its sha: a checkout of either is the
            // same tree, and the name is what the log line reads as.
            git_ref: Some(r.to_string()),
        }),
    }
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

/// The cached base analysis for `sha` under `scope`, or `None` when the cache
/// holds a different ref, a different scope, or nothing.
///
/// Clones rather than moves: a refresh that fails partway must not have
/// consumed the cache, or the next save pays for the analysis again.
fn take_cached_base(
    cache: &Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
    sha: &str,
    scope: &str,
) -> Option<(DependencyGraph, Config, String)> {
    let guard = cache.as_ref()?.read().ok()?;
    let cached = guard.as_ref()?;
    if cache_key(cached) != (sha, scope) {
        return None;
    }
    Some((
        cached.graph.clone(),
        cached.config.clone(),
        cached.details.clone(),
    ))
}

/// What identifies a cached base. Both halves, always: the ref says which
/// tree was analyzed and the scope says what was looked at in it, and an
/// answer produced under the wrong half of that pair is the wrong answer.
fn cache_key(cached: &CachedBase) -> (&str, &str) {
    (&cached.sha, &cached.scope)
}

fn store_cached_base(
    cache: &Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
    sha: &str,
    scope: &str,
    graph: &DependencyGraph,
    config: &Config,
    details: &str,
) {
    let Some(cache) = cache else { return };
    if let Ok(mut slot) = cache.write() {
        let already = slot.as_ref().is_some_and(|c| cache_key(c) == (sha, scope));
        if !already {
            *slot = Some(CachedBase {
                sha: sha.to_string(),
                scope: scope.to_string(),
                graph: graph.clone(),
                config: config.clone(),
                details: details.to_string(),
            });
        }
    }
}

/// The base side of a diff: its graph, its config, and its detail sidecar —
/// plus whether this call is the one that created the worktree, which decides
/// who cleans it up.
///
/// The sidecar is rendered *here*, next to the analysis, while the worktree it
/// reads its file sources from still exists. Deferring it to the write, or
/// re-deriving it on a later refresh from the cached graph, renders against a
/// directory that has been removed: `render_details` then finds no files, and
/// the before-side quietly loses every file's text — which left the details
/// pane with nothing to diff at file level from the second refresh onward
/// (UI-097).
///
/// `base_config` is the live config re-rooted at the checkout, not a config
/// rebuilt from the startup flags. The base has to be looked at through the
/// same lens as the head or the diff reports the two lenses disagreeing:
/// `include_docs` on one side alone turns every doc into an addition, and a
/// settings-file `exclude_patterns` entry turns everything it excludes into a
/// removal, on a working tree where nothing was touched.
#[allow(clippy::type_complexity)]
fn acquire_base(
    repo_root: &Path,
    base_dir: &Path,
    from_ref: &str,
    from_sha: &str,
    scope: &str,
    base_config: Config,
    base_cache: &Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
) -> Result<((DependencyGraph, Config, String), bool), String> {
    if let Some((graph, config, details)) = take_cached_base(base_cache, from_sha, scope) {
        eprintln!("  Reusing base ({}) analysis", from_sha);
        return Ok(((graph, config, details), false));
    }
    let e2s = |e: anyhow::Error| e.to_string();
    diff::create_worktree(repo_root, base_dir, from_ref).map_err(e2s)?;
    let (graph, config) =
        diff::analyze_with(base_config, &format!("base ({})", from_sha)).map_err(e2s)?;
    let details = diff::render_base_details(&graph, &config).map_err(e2s)?;
    Ok(((graph, config, details), true))
}

// ------------------------------------------------------------------
//  Main blocking diff pipeline
// ------------------------------------------------------------------

/// Blocking helper that performs the actual diff computation.
///
/// `live_config` is the scope both sides are analyzed under, whichever refs
/// they name. It is read once, here, so that a scope change landing mid-diff
/// cannot reach one side and not the other.
fn compute_diff_blocking(
    repo_root: &Path,
    output_dir: &Path,
    live_config: &Arc<std::sync::RwLock<Config>>,
    from_ref: &str,
    head: &Head,
    current_graph: Option<Arc<std::sync::RwLock<DependencyGraph>>>,
    base_cache: Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
) -> Result<(DiffResponse, DependencyGraph, Config, String, String), String> {
    let is_working = head.git_ref.is_none();
    let to_sha = head.label.clone();
    let total_start = Instant::now();
    let to_label = if is_working { "Working Tree" } else { &to_sha };
    eprintln!("🔄 Starting diff: {} → {}", from_ref, to_label);

    let e2s = |e: anyhow::Error| e.to_string();

    let live = live_config
        .read()
        .map_err(|e| format!("Config lock: {}", e))?
        .clone();
    let scope = diff::analysis_fingerprint(&live, repo_root);

    let from_sha = diff::resolve_git_ref(repo_root, from_ref)
        .map_err(|e| format!("Invalid from_ref: {}", e))?;

    // Create + analyze base worktree, unless the last diff already did.
    let tmp = std::env::temp_dir();
    let base_dir = tmp.join(format!("mezz-diff-base-{}", from_sha));
    let worktree_start = Instant::now();
    let (base, base_is_ours) = acquire_base(
        repo_root,
        &base_dir,
        from_ref,
        &from_sha,
        &scope,
        diff::rooted_at(&live, &base_dir),
        &base_cache,
    )?;
    let (base_graph, base_config, base_details_str) = base;

    // Acquire head graph: from in-memory state (WORKING) or a new worktree.
    // The staged head goes down the worktree path like any commit — the
    // manufactured commit it checks out is an ordinary ref by then.
    let (head_graph, head_config, head_dir_for_diff) = if let Some(head_ref) = &head.git_ref {
        let hdir = tmp.join(format!("mezz-diff-head-{}", to_sha));
        if let Err(e) = diff::create_worktree(repo_root, &hdir, head_ref) {
            if base_is_ours {
                diff::remove_worktree(repo_root, &base_dir);
            }
            return Err(e.to_string());
        }
        let (graph, config) =
            diff::analyze_with(diff::rooted_at(&live, &hdir), &format!("head ({})", to_sha))
                .map_err(e2s)?;
        (graph, config, hdir)
    } else {
        eprintln!("   Using current working directory as head...");
        let g = current_graph.unwrap();
        let graph = g.read().map_err(|e| format!("Graph lock: {}", e))?.clone();
        let config = working_head_config(live, repo_root);
        let dir = config.root_path.clone();
        (graph, config, dir)
    };

    eprintln!(
        "   Worktree(s) created in {:.1}s",
        worktree_start.elapsed().as_secs_f32()
    );

    // Compute structural diff
    eprintln!("   Computing structural diff...");
    let diff_start = Instant::now();
    let diff_result = diff::compute_diff(
        &base_graph,
        &head_graph,
        &base_dir,
        &head_dir_for_diff,
        &from_sha,
        &to_sha,
    );
    eprintln!(
        "   Diff computed in {:.1}s: +{} -{} ~{}",
        diff_start.elapsed().as_secs_f32(),
        diff_result.summary.added,
        diff_result.summary.removed,
        diff_result.summary.modified
    );

    // Write output files
    let diff_json = diff::write_diff_outputs(
        output_dir,
        &head_graph,
        &head_config,
        &base_details_str,
        &diff_result,
    )
    .map_err(e2s)?;

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
        let hdir = tmp.join(format!("mezz-diff-head-{}", to_sha));
        diff::remove_worktree(repo_root, &hdir);
    }

    // Keep the base for the next refresh. Its worktree is gone either way —
    // the graph is what the next diff needs, and re-deriving it from a
    // checkout that has not moved is the cost this avoids.
    store_cached_base(
        &base_cache,
        &from_sha,
        &scope,
        &base_graph,
        &base_config,
        &base_details_str,
    );

    eprintln!(
        "✅ Diff complete in {:.1}s total",
        total_start.elapsed().as_secs_f32()
    );

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
                        Some(LiveDiff {
                            from_ref: req.from_ref.clone(),
                        })
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

/// DELETE /api/diff — leave diff mode.
///
/// Two things have to go, and dropping either one alone is what left the
/// working-tree diff with no way out (UI-100). `live_diff` is the
/// subscription: while it is set, every file save recomputes the comparison
/// and announces it, so a client that cleared its own overlay gets it back on
/// the next keystroke it saves. `diff_result` is the answer still being
/// served: while it is set, any page that reloads asks `GET /api/diff`, is
/// handed the last comparison, and comes up in diff mode again.
///
/// `base_cache` is deliberately kept. It holds an analysis keyed by the base
/// sha and nothing reads it except a later diff against that same base, which
/// it saves a worktree checkout — dropping it would only make starting over
/// slower.
///
/// Always succeeds. "Stop showing me this" has no failure the caller could
/// act on, and leaving diff mode when there is no diff is what the caller
/// asked for either way.
pub(crate) async fn stop_diff_handler(State(state): State<AppState>) -> StatusCode {
    // Before the clears, so a diff that is mid-flight sees the new epoch when
    // it goes to publish rather than racing the writes below.
    state.diff_epoch.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut live) = state.live_diff.write() {
        *live = None;
    }
    if let Ok(mut d) = state.diff_result.write() {
        *d = None;
    }
    if let Ok(mut b) = state.base_details.write() {
        *b = None;
    }

    // Tell every connected client, not just the one that asked. The overlay
    // is server state, so a mirrored window (UI-095) that kept drawing it
    // would be drawing a comparison that no longer exists. `Diff` rather than
    // `Graph`: the code has not moved, only the overlay on it.
    let _ = state.tx.send(ReloadKind::Diff);

    eprintln!("🔀 Diff mode left — no longer following the working tree");
    StatusCode::NO_CONTENT
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
    // Whose diff mode this result belongs to. Checked again before publishing:
    // a stop that arrives while this runs must win (UI-100).
    let epoch = state.diff_epoch.load(Ordering::SeqCst);

    // Read repo_root before spawning blocking task
    let repo_root = state.repo_root.read().await.clone();

    // Resolve the head before paying for anything. The staged case can answer
    // "there is nothing to compare", and that is a message rather than a
    // failure — discovering it after a checkout and a full analysis would be
    // charging the reader for the answer.
    let head = match resolve_head(&repo_root, &req.to_ref) {
        Ok(head) => head,
        Err(e) => return declined(e),
    };

    // Run diff in a blocking task since it's CPU-intensive
    let result = tokio::task::spawn_blocking({
        let output_dir = state.output_dir.clone();
        let from_ref = req.from_ref.clone();
        let current_graph = if head.git_ref.is_none() {
            Some(state.graph.clone())
        } else {
            None
        };
        // Handed over whatever the head is. Only a working-tree head *reads*
        // its graph from here; both kinds take their analysis scope from it,
        // which is what keeps a commit-to-commit diff looking at the same
        // languages, docs and patterns as the tree it was asked for from.
        let live_config = state.config.clone();
        let base_cache = Some(state.base_cache.clone());

        move || {
            compute_diff_blocking(
                &repo_root,
                &output_dir,
                &live_config,
                &from_ref,
                &head,
                current_graph,
                base_cache,
            )
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    // Store results in memory and signal reload
    match result {
        Ok((resp, head_graph, head_config, diff_json, base_details_str)) => {
            if resp.success && state.diff_epoch.load(Ordering::SeqCst) != epoch {
                // Diff mode was left while this ran. Publishing now would put
                // the overlay back a second after the user dismissed it, and
                // adopting the head graph would move the canvas for a
                // comparison nobody is looking at any more.
                eprintln!("   ⚠ Diff discarded: diff mode was left while it ran");
                return Ok(DiffResponse {
                    success: false,
                    message: "Diff discarded: diff mode was left while it ran".to_string(),
                    summary: None,
                });
            }
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
        let spec_dir = state.spec_dir.clone();
        let settings = state.settings.clone();
        let path = canonical_path.clone();

        move || -> Result<(usize, usize, DependencyGraph, Config), String> {
            let config = build_config(
                &path,
                include_tests,
                include_docs,
                &languages,
                spec_dir,
                &settings,
            );

            // Run analysis
            let mut analyzer = Analyzer::new(config.clone());
            let result = analyzer.analyze().map_err(|e| e.to_string())?;
            let entity_count = result.entities.len();
            let rel_count = result.relationships.len();
            let graph = DependencyGraph::from_analysis(&result);

            // Write output files
            std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;
            let data_path = output_dir.join("data.json");
            let output_str = crate::output::render(&graph, &config).map_err(|e| e.to_string())?;
            std::fs::write(&data_path, &output_str).map_err(|e| e.to_string())?;

            let details_str =
                JsonRenderer::render_details(&graph, &config).map_err(|e| e.to_string())?;
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
        DiffRequest {
            from_ref: "HEAD".to_string(),
            to_ref: to_ref.to_string(),
        }
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
        // `STAGED` is in this list and not the one above: the index is checked
        // out into a temp worktree exactly like a commit, however much it reads
        // as uncommitted work (UI-111).
        for r in [
            "HEAD", "HEAD~1", "main", "c0ff6d2", "v1.2.3", "working", "STAGED", "staged",
        ] {
            assert!(
                !is_working_head(&req(r)),
                "'{r}' must not be adopted as live state"
            );
        }
    }

    /// A repository with one commit, at a path unique to this test.
    fn repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mezz-head-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q", "--initial-branch=main", "."],
            vec!["config", "user.email", "t@t.t"],
            vec!["config", "user.name", "t"],
        ] {
            assert!(std::process::Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .output()
                .unwrap()
                .status
                .success());
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-qm", "base"]] {
            assert!(std::process::Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .output()
                .unwrap()
                .status
                .success());
        }
        dir
    }

    fn git_in(dir: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success());
    }

    #[test]
    fn the_working_sentinel_resolves_to_no_ref_at_all() {
        let head = resolve_head(Path::new("/nonexistent"), WORKING_REF).unwrap();
        assert!(head.git_ref.is_none(), "nothing is checked out for WORKING");
        assert_eq!(head.label, "working");
    }

    /// The label is what reaches `diff.json`, and it has to stay the literal:
    /// a sha there would name a commit nothing references, and the UI would
    /// read a staged comparison as an ordinary commit-to-commit one.
    #[test]
    fn a_staged_head_is_a_sha_to_check_out_and_a_literal_to_report() {
        let dir = repo("staged");
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        git_in(&dir, &["add", "."]);

        let head = resolve_head(&dir, STAGED_REF).unwrap();
        assert_eq!(head.label, STAGED_LABEL);
        let sha = head.git_ref.expect("the index resolves to a commit");
        assert_ne!(sha, STAGED_REF, "the sentinel must not reach git");
        assert_eq!(sha.len(), 40, "a full commit sha: {}", sha);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing staged has to come back as this exact message, because that is
    /// what `run_diff` matches on to answer with a declined diff rather than a
    /// 500 — the reader has nothing to fix.
    #[test]
    fn nothing_staged_is_the_declined_message() {
        let dir = repo("empty");
        // An unstaged edit is not a staged one.
        std::fs::write(dir.join("a.txt"), "working only\n").unwrap();

        assert_eq!(resolve_head(&dir, STAGED_REF).unwrap_err(), NOTHING_STAGED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An ordinary ref keeps its name for the checkout and reports the sha, so
    /// `mezz-diff-head-<label>` stays one directory per tree rather than one per
    /// spelling of it.
    #[test]
    fn an_ordinary_ref_is_checked_out_by_name_and_reported_by_sha() {
        let dir = repo("named");
        let head = resolve_head(&dir, "main").unwrap();
        assert_eq!(head.git_ref.as_deref(), Some("main"));
        assert!(!head.label.is_empty() && head.label != "main");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_working_head_is_rooted_where_the_server_watches() {
        let mut live = Config::default();
        live.root_path = PathBuf::from("/tmp/mezz-diff-head-abc123");
        let rooted = working_head_config(live, Path::new("/repo"));
        assert_eq!(rooted.root_path, PathBuf::from("/repo"));
    }

    #[test]
    fn a_poisoned_root_does_not_survive_into_the_next_diff() {
        // The sequence the bug lived in: a commit diff moved the live root,
        // and the *next* working diff inherited it. Re-rooting on every call
        // is what makes the second call independent of the first, so this
        // holds however badly the config arrives.
        for poisoned in [
            "/tmp/mezz-diff-head-abc",
            "/tmp/mezz-diff-base-def",
            "relative/nonsense",
            "/",
        ] {
            let mut live = Config::default();
            live.root_path = PathBuf::from(poisoned);
            assert_eq!(
                working_head_config(live, Path::new("/repo")).root_path,
                PathBuf::from("/repo"),
                "root '{poisoned}' should have been replaced by the watched root",
            );
        }
    }

    /// The cache half of keeping the two sides to one scope. Narrowing the
    /// analysis from the browser leaves the base *ref* untouched, so a cache
    /// keyed on the sha alone hands the next refresh a base analyzed under
    /// the scope that was in force before — and the diff then reports the
    /// difference between two scopes as a change to the code.
    #[test]
    fn a_base_cached_under_another_scope_is_not_reused() {
        let cache = Some(Arc::new(std::sync::RwLock::new(Some(CachedBase {
            sha: "c0ff33".to_string(),
            scope: "wide".to_string(),
            graph: DependencyGraph::default(),
            config: Config::default(),
            details: "{}".to_string(),
        }))));

        assert!(take_cached_base(&cache, "c0ff33", "wide").is_some());
        assert!(
            take_cached_base(&cache, "c0ff33", "narrow").is_none(),
            "same ref, different scope — the analysis behind it is not the one being asked for",
        );
        assert!(take_cached_base(&cache, "0the12", "wide").is_none());
    }

    /// And storing has to replace the entry when only the scope moved, or the
    /// first scope wins for the rest of the session.
    #[test]
    fn storing_under_a_new_scope_replaces_the_cached_base() {
        let cache = Some(Arc::new(std::sync::RwLock::new(None)));
        let graph = DependencyGraph::default();
        store_cached_base(&cache, "c0ff33", "wide", &graph, &Config::default(), "{}");
        store_cached_base(&cache, "c0ff33", "narrow", &graph, &Config::default(), "{}");

        assert!(take_cached_base(&cache, "c0ff33", "narrow").is_some());
        assert!(take_cached_base(&cache, "c0ff33", "wide").is_none());
    }

    #[test]
    fn re_rooting_keeps_the_rest_of_the_live_config() {
        // Only the root is suspect. The language filter and test-inclusion
        // settings are the user's current choices and have to survive, or a
        // diff would silently widen the analysis it compares.
        let mut live = Config::default();
        live.root_path = PathBuf::from("/tmp/mezz-diff-head-abc");
        live.analysis.include_tests = !live.analysis.include_tests;
        let expected_tests = live.analysis.include_tests;
        let expected_langs = live.analysis.languages.clone();

        let rooted = working_head_config(live, Path::new("/repo"));
        assert_eq!(rooted.analysis.include_tests, expected_tests);
        assert_eq!(rooted.analysis.languages, expected_langs);
    }
}
