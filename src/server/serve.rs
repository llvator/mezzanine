//! `nao serve` — multi-repo HTTP server (SRV-003).
//!
//! Where [`super::run`] (`nao watch`) serves exactly one path under a flat
//! `/api/*` namespace with a file watcher and SSE live-reload, this serves
//! any number of analyzed repos under `/api/repos/{slug}/*` with neither.
//! The two route tables are deliberately separate: watch mode's endpoints
//! are the VS Code extension's contract and must not shift under it.
//!
//! Out of scope here, per the ticket: cloning (SRV-004), diff, educator,
//! SSE, and analysis-scope. `/api/repos/{slug}/details/base` answers 404,
//! which is exactly what watch mode answers before a diff has been computed,
//! so the UI's existing handling covers it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        Json,
    },
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tokio::sync::{broadcast, RwLock, Semaphore};

use crate::output::{self, JsonRenderer};

use super::access::{self, AccessOptions, AccessPolicy};
use super::handlers::git_commits;
use super::jobs::{self, JobConfig, JobLimiter, Submission};
use super::repo::{
    analyze_repo, is_valid_slug, parse_github_url, rehydrate, unsafe_passes_allowed, RepoRegistry,
    RepoSlot, RepoState, RepoSummary,
};
use super::ui_dir;
use super::scope_handler::{collect_scope, finish_scope};
use super::types::{CommitInfo, ScopeRequest, ScopeResponse};

/// Everything `nao serve` needs from the CLI, in one place so `main.rs`
/// doesn't grow a nine-argument call.
pub struct ServeOptions {
    pub port: u16,
    pub seeds: Vec<(String, PathBuf)>,
    pub cache_dir: PathBuf,
    pub jobs: usize,
    pub clone_timeout_secs: u64,
    pub max_repo_mb: u64,
    pub include_tests: bool,
    pub languages: Option<Vec<String>>,
    pub access: AccessOptions,
    /// Explicit location of the built browser UI; see `ui_dir::resolve`.
    pub ui_dir: Option<PathBuf>,
    /// `ui_dir` from the *user* settings file. Serve never reads a submitted
    /// repo's `.nao/settings.json` — see [`crate::settings`] — so this is the
    /// operator's own preference and nothing else.
    pub settings_ui_dir: Option<PathBuf>,
}

/// A JSON response body plus its content type, matching the shape watch
/// mode's graph/index/details handlers return.
type JsonText = ([(axum::http::header::HeaderName, &'static str); 1], String);

fn json_body(body: String) -> JsonText {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
}

#[derive(Clone)]
struct ServeState {
    repos: RepoRegistry,
    limiter: JobLimiter,
    jobs: JobConfig,
    /// The settings this server is running under, for `GET /api/settings`.
    ///
    /// Built once at startup and never rebuilt, because nothing here can
    /// change it: serve reads the user scope only (ADR-0008 — a repo that
    /// arrived from a URL a stranger pasted does not get to configure the
    /// server analyzing it), and there is no repo-scope file to write back.
    settings: Arc<crate::settings::SettingsReport>,
}

/// Run the multi-repo server: check the flags, rehydrate the cache, analyze
/// each `--seed`ed path, then serve everything under `/api/repos/{slug}/*`
/// alongside the browser UI, accepting new submissions at `POST /api/repos`.
///
/// Split in two for the same reason watch mode is: a bad flag should be
/// reported before anything is cloned.
pub fn serve(opts: ServeOptions) -> Result<()> {
    let (policy, ui) = super::resolve_startup(
        &opts.access,
        opts.ui_dir.as_deref(),
        opts.settings_ui_dir.as_deref(),
    )?;
    serve_checked(opts, policy, ui)
}

fn serve_checked(
    opts: ServeOptions,
    policy: AccessPolicy,
    ui: Option<PathBuf>,
) -> Result<()> {
    let ServeOptions {
        port,
        seeds,
        cache_dir,
        jobs: job_slots,
        clone_timeout_secs,
        max_repo_mb,
        include_tests,
        languages,
        ..
    } = opts;

    // User scope only, and the report says so: an empty repo scope here means
    // "never looked", not "the repo had no file". Built before the seeds are
    // analyzed so a warning about the operator's own settings file is printed
    // in the same breath as the rest of startup.
    let settings = Arc::new(settings_report(port, ui.as_deref(), include_tests, &languages));

    let mut repos: HashMap<String, Arc<RepoSlot>> = HashMap::new();

    // Cached repos first, so a `--seed` of the same slug wins — the operator
    // naming a local path meant it.
    for state in rehydrate(&cache_dir, include_tests, &languages) {
        eprintln!(
            "💾 Restored {} from cache ({} entities)",
            state.slug,
            state.graph.node_count()
        );
        repos.insert(state.slug.clone(), Arc::new(RepoSlot::ready(Arc::new(state))));
    }

    for (slug, path) in &seeds {
        eprintln!("🔍 Seeding {} from {}...", slug, path.display());
        let (repo, _) = analyze_repo(slug, None, path, include_tests, &languages)?;
        eprintln!(
            "   {} entities, {} relationships",
            repo.graph.node_count(),
            repo.graph.edge_count()
        );
        repos.insert(slug.clone(), Arc::new(RepoSlot::ready(Arc::new(repo))));
    }

    let state = ServeState {
        repos: Arc::new(RwLock::new(repos)),
        limiter: Arc::new(Semaphore::new(job_slots.max(1))),
        jobs: JobConfig {
            cache_dir: cache_dir.clone(),
            clone_timeout: Duration::from_secs(clone_timeout_secs),
            max_repo_bytes: max_repo_mb.saturating_mul(1_000_000),
            include_tests,
            languages,
        },
        settings,
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let app = build_router(state, &policy, ui.as_deref(), port);
        print_startup_banner(port, &seeds, &cache_dir, &policy, ui.as_deref());

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok::<(), anyhow::Error>(())
    })
}

/// The settings this server resolved, for the panel that reports them.
///
/// `serve` has no analyzed root of its own, so the report carries no repo
/// path and `repo_scope_read: false` — the UI needs to say "never read"
/// rather than let an empty repo scope read as "your repo has no settings".
fn settings_report(
    port: u16,
    ui: Option<&std::path::Path>,
    include_tests: bool,
    languages: &Option<Vec<String>>,
) -> crate::settings::SettingsReport {
    use crate::settings::report::{Effective, Inputs};
    let loaded = crate::settings::user_scoped();
    let mut config = crate::config::Config::default();
    config.analysis.include_tests = include_tests;
    for name in languages.iter().flatten() {
        if let Some(lang) = crate::models::file_info::Language::from_name(name) {
            config.analysis.languages.insert(lang);
        }
    }
    let effective =
        Effective { port: Some(port), ui_dir: ui.map(|p| p.to_path_buf()), ..Default::default() };
    let flags = crate::settings::report::named(&[
        ("port", true),
        ("include_tests", include_tests),
        ("language", languages.is_some()),
        ("ui_dir", ui.is_some()),
    ]);
    crate::settings::SettingsReport::build(
        None,
        &Inputs { loaded: &loaded, flags: &flags, config: &config, effective: &effective, repo_scope_read: false },
    )
}

/// GET /api/settings — the user-scope settings this server is running under.
async fn settings_report_handler(
    State(state): State<ServeState>,
) -> Json<crate::settings::SettingsReport> {
    // Cloned rather than served behind the `Arc`: `serde` only serializes
    // `Arc<T>` with its `rc` feature, and a settings report is a few dozen
    // small rows on a route nobody polls.
    Json((*state.settings).clone())
}

fn build_router(
    state: ServeState,
    policy: &AccessPolicy,
    ui: Option<&std::path::Path>,
    port: u16,
) -> Router {
    let app = Router::new()
        .route("/api/repos", get(list_repos).post(submit_repo))
        .route("/api/repos/{slug}", get(repo_meta))
        .route("/api/repos/{slug}/events", get(repo_events))
        .route("/api/repos/{slug}/graph", get(repo_graph))
        .route("/api/repos/{slug}/index", get(repo_index))
        .route("/api/repos/{slug}/details", get(repo_details))
        .route("/api/repos/{slug}/details/base", get(repo_base_details))
        .route("/api/repos/{slug}/commits", get(repo_commits))
        .route("/api/repos/{slug}/scope", post(repo_scope))
        .route("/api/settings", get(settings_report_handler))
        .with_state(state);

    // Same allowlist and token as watch mode — one policy type, two callers,
    // so an origin the user allowed reaches whichever server they started.
    // Same handshake as watch mode, and the `mode` field is how the UI knows
    // to expect `/api/repos/{slug}/*` rather than the flat namespace.
    policy
        .apply(ui_dir::mount(app, ui, port))
        .route("/api/hello", access::hello_route(policy, "serve", false))
}

// ------------------------------------------------------------------
//  Slug resolution
// ------------------------------------------------------------------

/// Resolve a path slug to its registry slot.
///
/// 400 for a slug that could never be valid (so a malformed URL doesn't read
/// as "we don't host that repo"), 404 for a well-formed slug this server
/// doesn't have.
async fn slot(state: &ServeState, slug: &str) -> Result<Arc<RepoSlot>, (StatusCode, String)> {
    if !is_valid_slug(slug) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Invalid repo slug: {slug}"),
        ));
    }
    state
        .repos
        .read()
        .await
        .get(slug)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Unknown repo: {slug}")))
}

/// Resolve a slug all the way to its analyzed repo.
///
/// A slug that exists but is still cloning/analyzing gets **409**, not 404:
/// the distinction matters to the UI, which should keep waiting on the event
/// stream rather than report the repo missing.
async fn lookup(state: &ServeState, slug: &str) -> Result<Arc<RepoState>, (StatusCode, String)> {
    let slot = slot(state, slug).await?;
    slot.state().ok_or_else(|| {
        let status = slot.status();
        (
            StatusCode::CONFLICT,
            match status.error() {
                Some(e) => format!("Repo {slug} failed: {e}"),
                None => format!("Repo {slug} is not ready yet (status: {})", status.label()),
            },
        )
    })
}

/// Map a renderer failure onto a 500 with the underlying message.
fn render_error(what: &str, e: impl std::fmt::Display) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("{what} render failed: {e}"),
    )
}

// ------------------------------------------------------------------
//  Handlers
// ------------------------------------------------------------------

/// GET /api/repos — every repo this server knows about, in-flight ones
/// included, so a client can see its own submission appear.
async fn list_repos(State(state): State<ServeState>) -> Json<Vec<RepoSummary>> {
    let repos = state.repos.read().await;
    let mut summaries: Vec<RepoSummary> = repos.values().map(|r| r.summary()).collect();
    summaries.sort_by(|a, b| a.slug.cmp(&b.slug));
    Json(summaries)
}

/// GET /api/repos/{slug} — metadata + current pipeline status, 404 if
/// unknown. Answers for in-flight repos too; that's how a submitter polls.
async fn repo_meta(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
) -> Result<Json<RepoSummary>, (StatusCode, String)> {
    Ok(Json(slot(&state, &slug).await?.summary()))
}

#[derive(Deserialize)]
struct SubmitRequest {
    url: String,
}

/// POST /api/repos — submit a public GitHub URL for cloning and analysis.
///
/// `200` when the repo is already loaded, `202` when a job is running or was
/// just started, `400` when the URL isn't an accepted GitHub repo URL. The
/// body carries the slug either way so the client can go straight to the
/// event stream.
async fn submit_repo(
    State(state): State<ServeState>,
    Json(req): Json<SubmitRequest>,
) -> Result<(StatusCode, Json<RepoSummary>), (StatusCode, String)> {
    let repo = parse_github_url(&req.url)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e:#}")))?;

    let code = match jobs::submit(
        &state.repos,
        &state.limiter,
        &state.jobs,
        repo.url,
        repo.slug.clone(),
    )
    .await
    {
        Submission::AlreadyReady => StatusCode::OK,
        Submission::Accepted => StatusCode::ACCEPTED,
    };

    // Read back through the registry rather than synthesizing a response, so
    // the body always reflects the slot's real state — including a job that
    // finished between `submit` returning and this line.
    let summary = slot(&state, &repo.slug).await?.summary();
    Ok((code, Json(summary)))
}

/// GET /api/repos/{slug}/events — SSE stream of pipeline transitions.
///
/// Emits the *current* status immediately so a client that subscribes after
/// a transition isn't left waiting on one that already happened, then every
/// subsequent change. The stream stays open after a terminal status; the
/// client closes it.
async fn repo_events(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
) -> Result<
    Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>>,
    (StatusCode, String),
> {
    let slot = slot(&state, &slug).await?;
    let mut rx = slot.subscribe();
    let initial = slot.event_for(&slot.status());

    let stream = async_stream::stream! {
        if let Ok(json) = serde_json::to_string(&initial) {
            yield Ok(Event::default().event("status").data(json));
        }
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Ok(json) = serde_json::to_string(&event) {
                        yield Ok(Event::default().event("status").data(json));
                    }
                }
                // Dropped frames only cost intermediate states; the client
                // can re-read `GET /api/repos/{slug}` for the truth.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

/// GET /api/repos/{slug}/graph — full graph JSON.
async fn repo_graph(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
) -> Result<JsonText, (StatusCode, String)> {
    let repo = lookup(&state, &slug).await?;
    let json = output::render(&repo.graph, &repo.config).map_err(|e| render_error("Graph", e))?;
    Ok(json_body(json))
}

/// GET /api/repos/{slug}/index — file/folder hierarchy.
async fn repo_index(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
) -> Result<JsonText, (StatusCode, String)> {
    let repo = lookup(&state, &slug).await?;
    let json = JsonRenderer::render_index(&repo.graph, &repo.config)
        .map_err(|e| render_error("Index", e))?;
    Ok(json_body(json))
}

/// GET /api/repos/{slug}/details — entity details + file contents.
async fn repo_details(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
) -> Result<JsonText, (StatusCode, String)> {
    let repo = lookup(&state, &slug).await?;
    let json = JsonRenderer::render_details(&repo.graph, &repo.config)
        .map_err(|e| render_error("Details", e))?;
    Ok(json_body(json))
}

/// GET /api/repos/{slug}/details/base — always 404 in serve mode.
///
/// Diff is out of scope for Phase 1, and 404 is what watch mode answers when
/// no diff has been computed, so the UI already treats it as "no base".
/// Still validates the slug so a typo reports the same way it does elsewhere.
async fn repo_base_details(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
) -> Result<JsonText, (StatusCode, String)> {
    slot(&state, &slug).await?;
    Err((
        StatusCode::NOT_FOUND,
        "No base details: diff is not available in serve mode".to_string(),
    ))
}

/// GET /api/repos/{slug}/commits — recent commits of the analyzed checkout.
/// A shallow clone has one; that's a truthful answer, not an error.
async fn repo_commits(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
) -> Result<Json<Vec<CommitInfo>>, (StatusCode, String)> {
    let repo = lookup(&state, &slug).await?;
    Ok(Json(git_commits(&repo.root_path)?))
}

/// POST /api/repos/{slug}/scope — smart scope traversal.
async fn repo_scope(
    State(state): State<ServeState>,
    AxumPath(slug): AxumPath<String>,
    Json(req): Json<ScopeRequest>,
) -> Result<Json<ScopeResponse>, (StatusCode, String)> {
    let repo = lookup(&state, &slug).await?;
    let assembly = collect_scope(&repo.graph, &repo.config.root_path, &req)?;
    Ok(Json(finish_scope(assembly, &repo.root_path)))
}

// ------------------------------------------------------------------
//  Startup
// ------------------------------------------------------------------

fn print_startup_banner(
    port: u16,
    seeds: &[(String, PathBuf)],
    cache_dir: &std::path::Path,
    policy: &AccessPolicy,
    ui: Option<&std::path::Path>,
) {
    eprintln!();
    eprintln!("🚀 nao serve running at http://localhost:{port}");
    eprintln!("   Cache: {}", cache_dir.display());
    if unsafe_passes_allowed() {
        eprintln!(
            "   ⚠ NAO_SERVE_ALLOW_UNSAFE_PASSES is set — analysis passes that"
        );
        eprintln!(
            "     execute code from the analyzed repo (rust-analyzer/build.rs)"
        );
        eprintln!("     are ENABLED. Do not use this on repos you don't trust.");
    } else {
        eprintln!("   Code-executing analysis passes: off (tree-sitter only)");
    }
    eprintln!("   API:  GET  /api/repos");
    eprintln!("         POST /api/repos            {{\"url\": \"https://github.com/o/r\"}}");
    eprintln!("         GET  /api/repos/{{slug}}");
    eprintln!("         GET  /api/repos/{{slug}}/events   (SSE job progress)");
    eprintln!("         GET  /api/repos/{{slug}}/graph | /index | /details");
    eprintln!("         GET  /api/repos/{{slug}}/commits");
    eprintln!("         POST /api/repos/{{slug}}/scope");
    access::print_allowed_origins(policy);
    access::print_pairing_token(policy);
    ui_dir::print_banner_line(port, ui);
    if !seeds.is_empty() {
        for (slug, path) in seeds {
            eprintln!("   Loaded: {} ← {}", slug, path.display());
        }
    }
    eprintln!();
    eprintln!("Press Ctrl+C to stop.");
}
