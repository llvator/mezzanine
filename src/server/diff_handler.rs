use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{extract::State, http::StatusCode, response::Json};

use crate::activity;
use crate::analyzer::{Analyzer, FileWalker};
use crate::config::Config;
use crate::diff;
use crate::graph::DependencyGraph;
use crate::output::JsonRenderer;

use super::files_handler::{changed_files, head_side};
use super::state::{build_config, AppState, CachedBase, LiveDiff, ReloadKind};
use super::types::{
    ChangedFile, DiffRequest, DiffResponse, DiffSummaryResponse, RootPathRequest, RootPathResponse,
};

/// The one `to_ref` that means "the tree this server is watching" rather
/// than a git ref that has to be checked out into a temp worktree first.
pub(crate) const WORKING_REF: &str = "WORKING";

/// The `to_ref` that means the git index. A sentinel rather than a sha because
/// the index is not a ref: the commit that names it is manufactured *inside*
/// this call and is unreferenced, so a client that held one would be holding a
/// commit that means whatever the index happened to be when it was made — the
/// `stash@{N}` mistake with the failure moved to the other end (UI-111).
pub(crate) const STAGED_REF: &str = "STAGED";

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

/// The two ways a comparison does not happen, which a caller answers
/// differently.
///
/// The distinction is whose problem it is. A 500 tells the reader something
/// broke and gives them nothing to do about it; `success: false` with a
/// message tells them to `git add` something, or names the files that put the
/// change outside the analysis. A ref that does not resolve, or git absent, is
/// the server failing to answer.
///
/// A type rather than the string equality against `NOTHING_STAGED` this
/// replaces: SRV-020's decline names the files it found, so it is a different
/// string every time and nothing could have recognised it by value.
enum NoDiff {
    /// The server declines, and the message says why.
    Declined(String),
    /// The server could not answer.
    Failed(String),
}

/// Every bare-string error inside the pipeline is a failure to answer. Written
/// as a conversion so that `?` on the existing `map_err(e2s)` call sites keeps
/// meaning what it did once the pipeline started returning this type.
impl From<String> for NoDiff {
    fn from(e: String) -> Self {
        NoDiff::Failed(e)
    }
}

/// What a comparison the reader stopped answers with (UI-141).
///
/// A decline, not a failure: nothing broke, and the one thing the reader must
/// not be told is that their own button produced an error.
const STOPPED: &str = "Comparison stopped.";

/// `Err(Declined)` once the reader has asked this run to stop.
///
/// Called at the phase boundaries the analyses do not cover, so that a stop
/// pressed during a worktree checkout or the structural diff is noticed at the
/// next seam rather than at the end of the run.
fn stop_requested(cancel: &Arc<AtomicBool>) -> Result<(), NoDiff> {
    if cancel.load(Ordering::Relaxed) {
        Err(NoDiff::Declined(STOPPED.to_string()))
    } else {
        Ok(())
    }
}

/// Sort an analysis that ended early: the reader's own stop, or a real
/// failure. Asked of the error rather than of the flag, so a run that raced a
/// stop it never actually saw is still reported as what it was.
fn analysis_ended(e: anyhow::Error) -> NoDiff {
    if e.downcast_ref::<crate::analyzer::Cancelled>().is_some() {
        NoDiff::Declined(STOPPED.to_string())
    } else {
        NoDiff::Failed(e.to_string())
    }
}

/// Remove the checkouts this run is responsible for.
///
/// `base_is_ours` is false for a base the previous diff analyzed and cached —
/// that worktree is already gone, and asking git to remove it again just
/// prints an error. `head_dir` is `None` for a working-tree head, which is the
/// reader's own tree and not this call's to delete.
fn drop_worktrees(repo_root: &Path, base_dir: &Path, base_is_ours: bool, head_dir: Option<&Path>) {
    if base_is_ours {
        diff::remove_worktree(repo_root, base_dir);
    }
    if let Some(dir) = head_dir {
        diff::remove_worktree(repo_root, dir);
    }
}

/// Turn a refusal into the response shape the caller returns.
fn refused(no: NoDiff) -> Result<DiffResponse, String> {
    match no {
        NoDiff::Declined(message) => Ok(DiffResponse {
            success: false,
            message,
            summary: None,
        }),
        NoDiff::Failed(e) => Err(e),
    }
}

/// Sort a head-resolution failure. Nothing staged is the only declined one.
fn head_failure(e: String) -> NoDiff {
    if e == NOTHING_STAGED {
        NoDiff::Declined(e)
    } else {
        NoDiff::Failed(e)
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

/// How many paths a decline names before it starts counting instead.
const NAMED_IN_DECLINE: usize = 3;

/// What the reader is told when the two refs name the same tree.
const NO_FILE_DIFFERS: &str = "Nothing to compare: git reports no file differing between these two refs.";

/// `1 file` / `2 files`, so a decline reads as a sentence.
fn files(n: usize) -> String {
    if n == 1 {
        "1 file".to_string()
    } else {
        format!("{} files", n)
    }
}

/// The decline for a change holding nothing this analysis would open.
fn nothing_to_compare(rows: &[ChangedFile]) -> String {
    if rows.is_empty() {
        return NO_FILE_DIFFERS.to_string();
    }
    let shown = rows.len().min(NAMED_IN_DECLINE);
    let mut named: Vec<String> = rows[..shown].iter().map(|c| c.path.clone()).collect();
    if rows.len() > shown {
        named.push(format!("and {} more", rows.len() - shown));
    }
    format!(
        "Nothing to compare: {} changed and none is in a language this analysis parses ({}).",
        files(rows.len()),
        named.join(", ")
    )
}

/// Refuse, before any checkout, a comparison whose changed files the walk
/// would never open (SRV-020).
///
/// `Some(message)` declines. The cost this saves is two worktrees and two full
/// analyses — on the repository this was reported from, ten thousand parsed
/// files to discover that the change was two `.properties` files. Git answers
/// the same question in milliseconds, and the answer it gives is *why*, where
/// the analysis can only produce `+0 −0 ~0` — the string it also prints when a
/// comparison genuinely changed nothing.
///
/// Three properties keep the guard on the safe side of its own error:
///
/// - **The list is [`changed_files`]**, the one UI-134's `Changes` tab reads,
///   so what the guard calls the change and what the reader can browse are the
///   same set by construction.
/// - **The predicate is [`FileWalker::would_analyze`]**, the walk's own
///   question. It is pure and answers for a deleted path as readily as a live
///   one, and it exists precisely so that no second list of extensions can
///   drift from the walk. A rename is asked under both its names.
/// - **It declines only on unanimity.** One analysable path anywhere in the
///   change, or a git call that fails, and the diff runs exactly as before.
///
/// A `WORKING` head is never guarded. Its graph is already in memory, so the
/// only analysis at stake is the base's — which is cached across saves — and
/// `refresh_live_diff` recomputes a pinned working diff on *every* save. A
/// guard there would let a save that touched only a `.md` decline a comparison
/// the reader asked to keep.
fn analysable_change(
    repo_root: &Path,
    config: &Config,
    req: &DiffRequest,
    head: &Head,
) -> Option<String> {
    head.git_ref.as_ref()?;
    let rows = changed_files(repo_root, &req.from_ref, &head_side(&req.to_ref))?;
    let walker = FileWalker::new(config);
    let analysed = |c: &ChangedFile| {
        [Some(&c.path), c.old_path.as_ref()]
            .into_iter()
            .flatten()
            .any(|p| walker.would_analyze(&repo_root.join(p)))
    };
    if rows.iter().any(analysed) {
        return None;
    }
    Some(nothing_to_compare(&rows))
}

/// Everything decided before a diff is worth starting: the scope in force,
/// which tree the head names, and whether the change between the two holds
/// anything to analyze.
///
/// Takes the live config *lock* rather than a `Config` so that all three
/// refusals — a poisoned lock, an unresolvable head, a change with nothing in
/// it — come back through one `Err`. `run_diff` is already at the repo's
/// complexity ceiling, and every branch left here is one it does not grow.
///
/// The clone is deliberate and matches `compute_diff_blocking`'s: the scope is
/// read once, so a change landing mid-diff cannot reach the guard and the
/// analysis differently.
fn plan_diff(
    repo_root: &Path,
    config: &Arc<std::sync::RwLock<Config>>,
    req: &DiffRequest,
) -> Result<Head, NoDiff> {
    let live = config
        .read()
        .map_err(|e| NoDiff::Failed(format!("Config lock: {}", e)))?
        .clone();
    let head = resolve_head(repo_root, &req.to_ref).map_err(head_failure)?;
    match analysable_change(repo_root, &live, req, &head) {
        Some(message) => Err(NoDiff::Declined(message)),
        None => Ok(head),
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
    ctx: &DiffContext,
) -> Result<((DependencyGraph, Config, String), bool), NoDiff> {
    if let Some((graph, config, details)) = take_cached_base(&ctx.base_cache, from_sha, scope) {
        activity::step(activity::DIFF, format!("  Reusing base ({from_sha}) analysis"));
        return Ok(((graph, config, details), false));
    }
    let e2s = |e: anyhow::Error| e.to_string();
    diff::create_worktree(repo_root, base_dir, from_ref).map_err(e2s)?;
    // The checkout is this call's from here on: an analysis that stops
    // partway leaves it on disk otherwise, and a reader who stops two
    // comparisons has two of them.
    let analysed = diff::analyze_with_cancel(base_config, &format!("base ({})", from_sha), &ctx.cancel)
        .map_err(|e| {
            diff::remove_worktree(repo_root, base_dir);
            analysis_ended(e)
        })?;
    let (graph, config) = analysed;
    let details = diff::render_base_details(&graph, &config).map_err(e2s)?;
    Ok(((graph, config, details), true))
}

/// The head side: the graph, the config it was produced under, the directory
/// the diff reads its sources from, and the checkout this run must remove.
///
/// That last one is `None` for a working-tree head — that is the reader's own
/// tree, and this call neither made it nor may delete it.
#[allow(clippy::type_complexity)]
fn acquire_head(
    repo_root: &Path,
    head: &Head,
    live: Config,
    ctx: &DiffContext,
    run: &activity::Run,
) -> Result<(DependencyGraph, Config, PathBuf, Option<PathBuf>), NoDiff> {
    let Some(head_ref) = &head.git_ref else {
        run.step("   Using current working directory as head...");
        let g = ctx.current_graph.as_ref().expect("a working head has a graph");
        let graph = g.read().map_err(|e| format!("Graph lock: {}", e))?.clone();
        let config = working_head_config(live, repo_root);
        let dir = config.root_path.clone();
        return Ok((graph, config, dir, None));
    };
    checked_out_head(repo_root, head, head_ref, live, ctx)
}

/// A head that is a commit: its own throwaway checkout, analysed at the
/// subtree that corresponds to the analyzed root.
///
/// Split from [`acquire_head`] so the working-tree branch above stays the
/// short one, and so the checkout's top and the subtree analysed under it
/// are named apart — the returned `PathBuf` pair is (what `compute_diff`
/// strips, what this run must remove), and they are different directories
/// below the git top level.
///
/// The head has the same defect the base had and moves with it (SRV-021).
/// It was invisible because both sides were wrong identically, so the
/// comparison was self-consistent — of the wrong tree. Fixing one root and
/// not the other would have made it visible and worse.
fn checked_out_head(
    repo_root: &Path,
    head: &Head,
    head_ref: &str,
    live: Config,
    ctx: &DiffContext,
) -> Result<(DependencyGraph, Config, PathBuf, Option<PathBuf>), NoDiff> {
    let hdir = std::env::temp_dir().join(format!("mezz-diff-head-{}", head.label));
    diff::create_worktree(repo_root, &hdir, head_ref).map_err(|e| e.to_string())?;
    let head_root = diff::checkout_root(repo_root, &hdir);
    let (graph, config) = diff::analyze_with_cancel(
        diff::rooted_at(&live, &head_root),
        &format!("head ({})", head.label),
        &ctx.cancel,
    )
    .map_err(|e| {
        diff::remove_worktree(repo_root, &hdir);
        analysis_ended(e)
    })?;
    Ok((graph, config, head_root, Some(hdir)))
}

/// The live server handles one diff run borrows, as one thing.
///
/// Bundled rather than passed one by one: they arrive together and mean one
/// thing — this run's link back to the server that started it — and the
/// blocking pipeline was already at the argument count clippy stops reading.
struct DiffContext {
    /// The scope both sides are analyzed under, whichever refs they name. It
    /// is read once, so that a scope change landing mid-diff cannot reach one
    /// side and not the other.
    live_config: Arc<std::sync::RwLock<Config>>,
    /// The working tree's graph, for a `WORKING` head. `None` for a ref,
    /// which is checked out and analyzed instead.
    current_graph: Option<Arc<std::sync::RwLock<DependencyGraph>>>,
    /// The previous run's base analysis, reused when the ref and scope match.
    base_cache: Option<Arc<std::sync::RwLock<Option<CachedBase>>>>,
    /// Flipped when the reader asks this run to stop (UI-141).
    cancel: Arc<AtomicBool>,
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
    from_ref: &str,
    head: &Head,
    ctx: &DiffContext,
) -> Result<Landed, NoDiff> {
    let is_working = head.git_ref.is_none();
    let to_sha = head.label.clone();
    let total_start = Instant::now();
    let to_label = if is_working { "Working Tree" } else { &to_sha };
    // The guard, not a bare pair of messages: this function returns early
    // from a dozen places below, and every one of them would otherwise leave
    // the browser showing a diff that stopped as still running (UI-138).
    let run = activity::begin(
        activity::DIFF,
        format!("🔄 Starting diff: {from_ref} → {to_label}"),
    );

    let e2s = |e: anyhow::Error| e.to_string();

    let live = ctx
        .live_config
        .read()
        .map_err(|e| format!("Config lock: {}", e))?
        .clone();
    let scope = diff::analysis_fingerprint(&live, repo_root);

    let from_sha = diff::resolve_git_ref(repo_root, from_ref)
        .map_err(|e| format!("Invalid from_ref: {}", e))?;

    // Create + analyze base worktree, unless the last diff already did.
    let base_dir = std::env::temp_dir().join(format!("mezz-diff-base-{}", from_sha));
    // The checkout's top is what git made and what must be removed; the
    // subtree corresponding to the analyzed root is what gets analyzed and
    // what `compute_diff` strips (SRV-021). Below the git top level these are
    // different directories, and pairing them wrongly compares the whole
    // repository against one subtree of it.
    let base_root = diff::checkout_root(repo_root, &base_dir);
    let worktree_start = Instant::now();
    let (base, base_is_ours) = acquire_base(
        repo_root,
        &base_dir,
        from_ref,
        &from_sha,
        &scope,
        diff::rooted_at(&live, &base_root),
        ctx,
    )?;
    let (base_graph, base_config, base_details_str) = base;

    // Acquire head graph: from in-memory state (WORKING) or a new worktree.
    // The staged head goes down the worktree path like any commit — the
    // manufactured commit it checks out is an ordinary ref by then.
    let (head_graph, head_config, head_dir_for_diff, head_worktree) =
        acquire_head(repo_root, head, live, ctx, &run).inspect_err(|_| {
            // The head is where a stop most often lands — it is the side with
            // no cache to skip it — and the base checkout is this call's to
            // clean up either way.
            drop_worktrees(repo_root, &base_dir, base_is_ours, None);
        })?;

    run.step(format!(
        "   Worktree(s) created in {:.1}s",
        worktree_start.elapsed().as_secs_f32()
    ));

    // The last seam at which stopping still saves anything. Both analyses are
    // done by now, so a stop after this point cannot end the run early — it is
    // honoured instead by `discarded`, which keeps the finished answer off a
    // canvas whose reader has already said they do not want it.
    if let Err(no) = stop_requested(&ctx.cancel) {
        drop_worktrees(repo_root, &base_dir, base_is_ours, head_worktree.as_deref());
        return Err(no);
    }

    // Compute structural diff
    run.step("   Computing structural diff...");
    let diff_start = Instant::now();
    let diff_result = diff::compute_diff(
        &base_graph,
        &head_graph,
        &base_root,
        &head_dir_for_diff,
        &from_sha,
        &to_sha,
    );
    run.step(format!(
        "   Diff computed in {:.1}s: +{} -{} ~{}",
        diff_start.elapsed().as_secs_f32(),
        diff_result.summary.added,
        diff_result.summary.removed,
        diff_result.summary.modified
    ));

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
        run.step("   Cleaning up worktrees...");
    }
    drop_worktrees(repo_root, &base_dir, base_is_ours, head_worktree.as_deref());

    // Keep the base for the next refresh. Its worktree is gone either way —
    // the graph is what the next diff needs, and re-deriving it from a
    // checkout that has not moved is the cost this avoids.
    store_cached_base(
        &ctx.base_cache,
        &from_sha,
        &scope,
        &base_graph,
        &base_config,
        &base_details_str,
    );

    run.end(format!(
        "✅ Diff complete in {:.1}s total",
        total_start.elapsed().as_secs_f32()
    ));

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
        // Start from an unset flag, and clear it under this lock (UI-141). A
        // stop that landed in the moment the last run was finishing has
        // nothing left to cancel, and clearing it *here* — rather than where
        // it is set — is what stops it carrying over into this run. Under the
        // lock because `cancel_diff_handler` takes the same one before
        // setting it, which is what makes the two orderings the only two.
        state.diff_cancel.store(false, Ordering::Relaxed);
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
    // And stop the run itself, not just its result. The epoch alone keeps the
    // overlay from coming back, but the engine would still spend the minute
    // it had left computing an answer nobody will be shown — and hold the
    // in-progress lock against the next comparison for all of it (UI-141).
    state.diff_cancel.store(true, Ordering::Relaxed);

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

/// POST /api/diff/cancel — stop the comparison that is running (UI-141).
///
/// Not the DELETE above, and the difference is what the reader is left
/// looking at. Leaving diff mode drops the overlay; stopping a computation
/// says nothing about the comparison already on screen — the case this exists
/// for is a reader who asked for two commits, watched the estimate grow, and
/// wants their previous view back rather than none at all.
///
/// The flag is only set while a run is in flight, and the run clears it as it
/// starts. Between them, a stop cannot outlive the thing it stopped and abort
/// the comparison the reader asks for next.
///
/// `202` when a run was told to stop, `204` when there was nothing running.
/// Neither is a failure: pressing stop twice, or a moment after the diff
/// landed, is not an error the caller could act on.
pub(crate) async fn cancel_diff_handler(State(state): State<AppState>) -> StatusCode {
    let in_progress = state.diff_in_progress.lock().await;
    if !*in_progress {
        return StatusCode::NO_CONTENT;
    }
    state.diff_cancel.store(true, Ordering::Relaxed);
    eprintln!("⏹ Diff stop requested — the run will end at its next checkpoint");
    StatusCode::ACCEPTED
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
    // Under the lock, for the reason `diff_handler` gives at its own copy of
    // this line: a stop left over from the run before must not abort this one.
    state.diff_cancel.store(false, Ordering::Relaxed);
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

/// Why a finished run must not be published, or `None` to go ahead.
///
/// Two ways a result outlives the reason it was computed, and both are the
/// same mistake: putting something on a canvas whose reader has already said
/// they do not want it.
///
/// - **Stopped.** The stop landed after the last checkpoint, so the run
///   finished anyway. Publishing it would answer a question that was
///   withdrawn, a second after the button reported it as withdrawn (UI-141).
/// - **Diff mode was left.** The overlay would come back a moment after it
///   was dismissed, and adopting a head graph would move the canvas for a
///   comparison nobody is looking at any more (UI-100).
///
/// The stop is checked first because it is the narrower claim: `DELETE
/// /api/diff` sets both, and "you stopped it" is the more useful of the two
/// things that would then be true.
fn discarded(state: &AppState, epoch: u64) -> Option<&'static str> {
    if state.diff_cancel.load(Ordering::Relaxed) {
        return Some("Diff discarded: it was stopped while it ran");
    }
    if state.diff_epoch.load(Ordering::SeqCst) != epoch {
        return Some("Diff discarded: diff mode was left while it ran");
    }
    None
}

/// What a finished comparison hands back: the answer, the head it analyzed,
/// and the two documents to serve. Named because three functions pass it
/// between them and a bare five-tuple in each signature says nothing.
type Landed = (DiffResponse, DependencyGraph, Config, String, String);

/// One comparison, packed on the async side and run on a blocking thread.
///
/// A value rather than five locals in `run_diff`. They were all pieces of
/// one thing — what to compare, where, and with which handles back to the
/// server — and holding them separately is what put that function over the
/// working-set line the repo's own gate draws.
struct DiffJob {
    repo_root: PathBuf,
    output_dir: PathBuf,
    from_ref: String,
    head: Head,
    ctx: DiffContext,
}

impl DiffJob {
    /// Consumes the job: it owns the clones the blocking task needs, and
    /// nothing after the run has a use for them.
    fn run(self) -> Result<Landed, NoDiff> {
        compute_diff_blocking(
            &self.repo_root,
            &self.output_dir,
            &self.from_ref,
            &self.head,
            &self.ctx,
        )
    }
}

/// Everything decided before a comparison is worth starting, or the reason
/// there is nothing to start.
///
/// Plans before paying for anything. Two cases answer without a diff at all —
/// nothing staged, and a change holding no file the walk would open
/// (SRV-020) — and discovering either after a checkout and two full analyses
/// would be charging the reader for the answer.
async fn plan_job(state: &AppState, req: &DiffRequest) -> Result<DiffJob, NoDiff> {
    // Read before spawning the blocking task: it is behind an async lock.
    let repo_root = state.repo_root.read().await.clone();
    let head = plan_diff(&repo_root, &state.config, req)?;
    let ctx = DiffContext {
        // Handed over whatever the head is. Only a working-tree head *reads*
        // its graph from here; both kinds take their analysis scope from it,
        // which is what keeps a commit-to-commit diff looking at the same
        // languages, docs and patterns as the tree it was asked for from.
        live_config: state.config.clone(),
        current_graph: head.git_ref.is_none().then(|| state.graph.clone()),
        base_cache: Some(state.base_cache.clone()),
        cancel: state.diff_cancel.clone(),
    };
    Ok(DiffJob {
        repo_root,
        output_dir: state.output_dir.clone(),
        from_ref: req.from_ref.clone(),
        head,
        ctx,
    })
}

/// Compute a diff and publish it, without touching the in-progress lock or
/// the live-diff bookkeeping — both callers own those differently.
async fn run_diff(state: &AppState, req: &DiffRequest) -> Result<DiffResponse, String> {
    // Whose diff mode this result belongs to. Checked again before publishing:
    // a stop that arrives while this runs must win (UI-100).
    let epoch = state.diff_epoch.load(Ordering::SeqCst);

    let job = match plan_job(state, req).await {
        Ok(job) => job,
        Err(no) => return refused(no),
    };

    // A blocking thread, since it's CPU-intensive.
    let result = tokio::task::spawn_blocking(move || job.run())
        .await
        .map_err(|e| format!("Task failed: {}", e))?;

    match result {
        Ok(landed) => publish_if_wanted(state, req, epoch, landed),
        // A stop the reader asked for arrives here as `Declined`, which is
        // exactly what it is: a `success: false` with a message, not a 500
        // telling them their own button broke something.
        Err(no) => refused(no),
    }
}

/// Serve a finished comparison, unless nobody is waiting for it any more.
///
/// The check is [`discarded`]; the publishing is [`publish`]. What sits here
/// is the one decision between them, which is why `epoch` comes this far: the
/// question is whether the diff mode this run was started under is still the
/// one on screen.
fn publish_if_wanted(
    state: &AppState,
    req: &DiffRequest,
    epoch: u64,
    landed: Landed,
) -> Result<DiffResponse, String> {
    let (resp, head_graph, head_config, diff_json, base_details) = landed;
    if let Some(why) = resp.success.then(|| discarded(state, epoch)).flatten() {
        eprintln!("   ⚠ {why}");
        return Ok(DiffResponse {
            success: false,
            message: why.to_string(),
            summary: None,
        });
    }
    if resp.success {
        publish(state, req, head_graph, head_config, diff_json, base_details);
    }
    Ok(resp)
}

/// Make a finished comparison the one this server serves, and tell every
/// client looking at it.
///
/// Called only for a run that succeeded and that [`discarded`] still wants.
///
/// The head graph is adopted only when the head *is* what this server watches
/// — the working tree. For a commit ref, `head_config.root_path` is a temp
/// worktree that `drop_worktrees` deleted a moment ago, and adopting it
/// re-roots every subsequent response at a directory that no longer exists:
/// `/api/index` reports it, `file_path`s stop stripping to repo-relative, and
/// the scope tree renders the user's home directory as its top folder. The
/// next `→ WORKING` diff then takes its head root from the same poisoned
/// config and reports every entity as added-and-removed (SRV-019).
///
/// Nothing is lost by declining: the head commit's graph was only ever
/// visible until the next file save, which the watcher answers by publishing
/// the working tree again. A diff is an overlay on what is being watched, not
/// a checkout of something else, and `write_diff_outputs` has already written
/// the head's own `data.json` for consumers that want it.
fn publish(
    state: &AppState,
    req: &DiffRequest,
    head_graph: DependencyGraph,
    head_config: Config,
    diff_json: String,
    base_details: String,
) {
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
        *b = Some(base_details);
    }
    // `Diff`, not `Graph`: the graph is either unchanged or was just
    // published by whoever triggered this, and telling clients to re-fetch it
    // would restart a canvas that has no reason to move (UI-067).
    let _ = state.tx.send(ReloadKind::Diff);
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

    // ------------------------------------------------------------------
    //  UI-141 — stopping a comparison that is already running
    // ------------------------------------------------------------------

    fn message_of(no: NoDiff) -> String {
        match refused(no).expect("a decline is an answer, not a 500") {
            DiffResponse {
                success, message, ..
            } => {
                assert!(!success, "a stopped comparison produced no diff");
                message
            }
        }
    }

    /// The reader pressed stop. That is an answer the server gives, not a
    /// failure it reports — the one thing they must not be told is that their
    /// own button broke something.
    #[test]
    fn a_stop_is_a_decline_and_not_a_failure() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(stop_requested(&cancel).is_ok(), "nothing has been asked");

        cancel.store(true, Ordering::Relaxed);
        let no = stop_requested(&cancel).expect_err("the stop must be honoured");
        assert_eq!(message_of(no), STOPPED);
    }

    /// The two ways an analysis ends early are told apart by the error it
    /// ended with, not by reading the flag afterwards: a run that failed on
    /// its own a moment before a stop landed must still be reported as failed.
    #[test]
    fn an_analysis_that_was_stopped_is_told_apart_from_one_that_broke() {
        assert_eq!(
            message_of(analysis_ended(crate::analyzer::Cancelled.into())),
            STOPPED
        );
        match analysis_ended(anyhow::anyhow!("worktree is locked")) {
            NoDiff::Failed(e) => assert_eq!(e, "worktree is locked"),
            NoDiff::Declined(m) => panic!("a real failure was reported as a decline: {m}"),
        }
    }

    /// The whole of what a stop has to do: end the run, say why, and leave no
    /// checkout behind. A reader who stops three slow comparisons would
    /// otherwise be three worktrees deeper into their temp directory, and the
    /// next diff against the same base would find one of them already there.
    #[test]
    fn a_stopped_run_cleans_up_the_checkout_it_made() {
        let dir = repo("stopped");
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();

        let head = resolve_head(&dir, "main").expect("main resolves");
        let from_sha = diff::resolve_git_ref(&dir, "HEAD").expect("HEAD resolves");
        let base_dir = std::env::temp_dir().join(format!("mezz-diff-base-{}", from_sha));

        // Set before the run rather than raced against it: the checkpoint
        // being tested is the one inside the base analysis, which is where a
        // stop lands on any repository big enough for anyone to press stop on.
        let ctx = DiffContext {
            live_config: Arc::new(std::sync::RwLock::new(Config::for_path(&dir))),
            current_graph: None,
            base_cache: None,
            cancel: Arc::new(AtomicBool::new(true)),
        };

        let outcome = compute_diff_blocking(&dir, &out, "HEAD", &head, &ctx);
        match outcome {
            Err(no) => assert_eq!(message_of(no), STOPPED),
            Ok(_) => panic!("a run told to stop produced a diff anyway"),
        }
        assert!(
            !base_dir.exists(),
            "the base checkout outlived the run that made it: {}",
            base_dir.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    //  SRV-020 — the change git can see, before anything is checked out
    // ------------------------------------------------------------------

    fn pair(from: &str, to: &str) -> DiffRequest {
        DiffRequest {
            from_ref: from.to_string(),
            to_ref: to.to_string(),
        }
    }

    /// The live config a server watching `dir` would hold.
    fn watching(dir: &Path) -> Config {
        Config {
            root_path: dir.to_path_buf(),
            ..Default::default()
        }
    }

    /// Commit `files` on top of whatever the repo already has.
    fn commit(dir: &Path, message: &str, files: &[(&str, &str)]) {
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        git_in(dir, &["add", "-A"]);
        git_in(dir, &["commit", "-qm", message]);
    }

    /// A head resolved against `dir`, for a guard that only ever reads
    /// `git_ref`.
    fn head_of(dir: &Path, to_ref: &str) -> Head {
        resolve_head(dir, to_ref).unwrap()
    }

    /// The report this came from: two commits apart by two `.properties`
    /// files, on a repository whose analysis would have parsed ten thousand
    /// files to discover that neither is one of them.
    #[test]
    fn a_change_of_only_unparsed_files_is_declined() {
        let dir = repo("srv020-props");
        commit(
            &dir,
            "properties only",
            &[
                ("project.properties", "a=1\n"),
                ("local.properties", "b=2\n"),
            ],
        );

        let message = analysable_change(
            &dir,
            &watching(&dir),
            &pair("HEAD~1", "HEAD"),
            &head_of(&dir, "HEAD"),
        )
        .expect("nothing here would have been parsed");

        assert!(message.contains("project.properties"), "{message}");
        assert!(message.contains("local.properties"), "{message}");
        assert!(message.contains("2 files"), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unanimity, not a majority: the guard exists to save an analysis nobody
    /// needs, never to hide one somebody does.
    #[test]
    fn one_parsed_file_among_them_is_enough_to_run() {
        let dir = repo("srv020-mixed");
        commit(
            &dir,
            "properties and one source file",
            &[("project.properties", "a=1\n"), ("lib.rs", "fn f() {}\n")],
        );

        assert!(
            analysable_change(
                &dir,
                &watching(&dir),
                &pair("HEAD~1", "HEAD"),
                &head_of(&dir, "HEAD"),
            )
            .is_none(),
            "one .rs file in the change is a diff worth computing",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rename has two names and git reports both. Reading only the
    /// destination would be right here and wrong for a source file renamed
    /// *out* of the analysis.
    #[test]
    fn a_renamed_source_file_is_read_under_both_its_names() {
        let dir = repo("srv020-rename");
        commit(&dir, "add a source file", &[("lib.rs", "fn f() {}\n")]);
        std::fs::rename(dir.join("lib.rs"), dir.join("lib.properties")).unwrap();
        git_in(&dir, &["add", "-A"]);
        git_in(&dir, &["commit", "-qm", "rename it out of the analysis"]);

        assert!(
            analysable_change(
                &dir,
                &watching(&dir),
                &pair("HEAD~1", "HEAD"),
                &head_of(&dir, "HEAD"),
            )
            .is_none(),
            "the file left the analysis — that is a removal the diff should report",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pinned working diff recomputes on every save (UI-067). A guard there
    /// would let a save that touched only a `.md` decline the comparison the
    /// reader asked to keep.
    #[test]
    fn a_working_head_is_never_declined_by_this_guard() {
        let dir = repo("srv020-working");
        std::fs::write(dir.join("notes.properties"), "a=1\n").unwrap();

        assert!(
            analysable_change(
                &dir,
                &watching(&dir),
                &pair("HEAD", WORKING_REF),
                &head_of(&dir, WORKING_REF),
            )
            .is_none(),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Git failing is not an answer about the change. Anything but `None` here
    /// would turn a missing `git` binary into "nothing changed".
    #[test]
    fn a_list_git_cannot_produce_does_not_decline() {
        let outside = std::env::temp_dir().join(format!("mezz-srv020-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();

        assert!(
            analysable_change(
                &outside,
                &watching(&outside),
                &pair("HEAD~1", "HEAD"),
                &Head {
                    git_ref: Some("HEAD".to_string()),
                    label: "HEAD".to_string(),
                },
            )
            .is_none(),
        );
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn two_refs_naming_the_same_tree_say_so() {
        assert_eq!(nothing_to_compare(&[]), NO_FILE_DIFFERS);
    }

    /// Naming every path of a 200-file change would be a wall of text; naming
    /// none would be the `+0 −0 ~0` this replaces.
    #[test]
    fn a_long_change_is_named_up_to_a_point_and_then_counted() {
        let rows: Vec<ChangedFile> = (0..5)
            .map(|i| ChangedFile {
                status: "M".to_string(),
                path: format!("conf/{}.properties", i),
                old_path: None,
                additions: 1,
                deletions: 0,
                binary: false,
                untracked: false,
            })
            .collect();

        let message = nothing_to_compare(&rows);
        assert!(message.contains("5 files"), "{message}");
        assert!(message.contains("conf/0.properties"), "{message}");
        assert!(message.contains("conf/2.properties"), "{message}");
        assert!(!message.contains("conf/3.properties"), "{message}");
        assert!(message.contains("and 2 more"), "{message}");
    }

    /// The decline has to reach the reader as a message, not a 500 — they can
    /// see which files moved and pick a different pair.
    #[test]
    fn a_decline_is_an_answer_and_a_failure_is_not() {
        let answered = refused(NoDiff::Declined("nothing here".to_string())).unwrap();
        assert!(!answered.success);
        assert_eq!(answered.message, "nothing here");

        assert!(refused(NoDiff::Failed("git is gone".to_string())).is_err());
    }
}
