mod access;
mod agent_terminal;
mod analysis_handler;
mod diff_handler;
mod educator_handler;
mod handlers;
mod jobs;
mod refactor_prompt;
mod repo;
mod scope_handler;
mod settings_handler;
mod serve;
mod state;
mod types;
mod ui_dir;
mod views_handler;

pub use access::AccessOptions;
pub use repo::{default_cache_dir, parse_seed};
pub use serve::{serve, ServeOptions};


use access::AccessPolicy;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use axum::{
    routing::{get, post},
    Router,
};
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::broadcast;

use state::{build_config, write_json, AppState, ReloadKind};

/// Everything `nao watch` needs from the CLI, in one place so `main.rs`
/// doesn't grow a nine-argument call — the same reason `ServeOptions` exists.
pub struct WatchOptions {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    pub port: u16,
    pub include_tests: bool,
    /// Analyze Markdown alongside the code (see `AnalysisConfig::include_docs`).
    pub include_docs: bool,
    pub languages: Option<Vec<String>>,
    /// `--spec-dir`: where the Elevator spec lives, when it isn't simply
    /// every `.elv` under the root. Unlike the settings-file key of the same
    /// name it may point outside the tree — see
    /// [`crate::settings::Settings::spec_dir`] for why that difference
    /// exists.
    pub spec_dir: Option<PathBuf>,
    pub debounce_ms: u64,
    /// Fallback Educator content root; see the CLI flag's documentation.
    pub content_fallback: Option<PathBuf>,
    pub access: AccessOptions,
    /// Explicit location of the built browser UI. `None` falls through the
    /// rest of [`ui_dir::resolve`]'s order.
    pub ui_dir: Option<PathBuf>,
    /// `ui_dir` from the user settings file, already loaded by the caller.
    /// Ranks below `--ui-dir` and `NAO_UI_DIR`; see [`ui_dir::resolve`].
    pub settings_ui_dir: Option<PathBuf>,
    /// Both settings scopes, still unmerged. Watch analyzes a path the
    /// operator chose, so both apply — unlike `serve`, which never reads a
    /// submitted repo's own file (see [`crate::settings`]).
    ///
    /// Unmerged because `/api/settings` has to say which file a value came
    /// from, and merging is exactly where that is lost.
    pub loaded: crate::settings::Loaded,
    /// Which settings keys arrived as command-line flags, by name. See
    /// [`crate::settings::report::named`].
    pub flags_named: std::collections::BTreeSet<String>,
    /// `--allow-agent-spawn`: register the route that opens a Claude Code
    /// terminal on this machine (SRV-017). Off by default — it is the one
    /// route that executes code, so it should not exist unless asked for.
    pub allow_agent_spawn: bool,
    /// `--pin-diff`: stop a working-tree diff from following the watcher, so
    /// a comparison stays where it was put (UI-067). On by default because
    /// "compare against the last commit and watch it evolve" is the reason
    /// most people open a diff next to `nao watch`.
    pub pin_diff: bool,
}

/// Resolve the two startup flags that can fail, before anything expensive.
///
/// Both servers call this first: a mistyped `--allow-origin` or a `--ui-dir`
/// pointing at nothing should cost a second, not a full parse of the repo or
/// a clone. Shared so the two commands report the same mistake the same way.
fn resolve_startup(
    access: &AccessOptions,
    ui_dir: Option<&std::path::Path>,
    settings_ui_dir: Option<&std::path::Path>,
) -> Result<(AccessPolicy, Option<PathBuf>)> {
    let policy = AccessPolicy::new(access).map_err(|e| anyhow::anyhow!("--allow-origin: {e}"))?;
    let ui = ui_dir::resolve(ui_dir, settings_ui_dir).map_err(|e| anyhow::anyhow!(e))?;
    Ok((policy, ui))
}

/// Run the watch server: check the flags, then analyze the codebase, start a
/// file watcher, and serve a live-updating HTTP API.
///
/// Split in two so the checking happens strictly before the several seconds
/// of analysis that follow it.
pub fn run(opts: WatchOptions) -> Result<()> {
    let (policy, ui) = resolve_startup(
        &opts.access,
        opts.ui_dir.as_deref(),
        opts.settings_ui_dir.as_deref(),
    )?;
    run_watch(opts, policy, ui)
}

fn run_watch(opts: WatchOptions, policy: AccessPolicy, ui_dir: Option<PathBuf>) -> Result<()> {
    let WatchOptions {
        path,
        output_dir,
        port,
        include_tests,
        include_docs,
        languages,
        spec_dir,
        debounce_ms,
        content_fallback,
        allow_agent_spawn,
        pin_diff,
        loaded,
        flags_named,
        ..
    } = opts;
    let settings = Arc::new(loaded.merged());
    let settings_view = Arc::new(state::SettingsView {
        effective: crate::settings::report::Effective {
            port: Some(port),
            debounce_ms: Some(debounce_ms),
            output_dir: Some(output_dir.clone()),
            ui_dir: ui_dir.clone(),
            content_fallback: content_fallback.clone(),
        },
        loaded: std::sync::RwLock::new(loaded),
        flags: flags_named,
    });

    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
    let config = build_config(
        &canonical_path,
        include_tests,
        include_docs,
        &languages,
        spec_dir.clone(),
        &settings,
    );

    // Initial analysis
    eprintln!("🔍 Initial analysis of {}...", path.display());
    let (entities, rels, initial_graph) = write_json(&config, &output_dir)?;
    eprintln!("   Found {} entities and {} relationships", entities, rels);

    let shared_graph = Arc::new(std::sync::RwLock::new(initial_graph));
    let shared_config = Arc::new(std::sync::RwLock::new(config.clone()));
    let (tx, _) = broadcast::channel::<ReloadKind>(16);
    let tx = Arc::new(tx);

    let rt = tokio::runtime::Runtime::new()?;
    let (watcher_stop_tx, watcher_stop_rx) = std::sync::mpsc::channel::<()>();

    let watcher_thread = spawn_file_watcher(
        path.clone(), config.clone(), output_dir.clone(),
        tx.clone(), shared_graph.clone(), shared_config.clone(),
        watcher_stop_rx, debounce_ms,
    );

    rt.block_on(run_http_server(
        HttpServer {
            path, output_dir, port, include_tests, include_docs, languages, spec_dir,
            content_fallback, policy, ui_dir, allow_agent_spawn, pin_diff, settings,
            settings_view,
        },
        tx, shared_graph, shared_config,
    ));

    let _ = watcher_stop_tx.send(());
    let _ = watcher_thread.join();
    Ok(())
}

// ------------------------------------------------------------------
//  File watcher
// ------------------------------------------------------------------

fn spawn_file_watcher(
    watch_path: PathBuf,
    config: crate::config::Config,
    output_dir: PathBuf,
    tx: Arc<broadcast::Sender<ReloadKind>>,
    graph: Arc<std::sync::RwLock<crate::graph::DependencyGraph>>,
    shared_config: Arc<std::sync::RwLock<crate::config::Config>>,
    stop_rx: std::sync::mpsc::Receiver<()>,
    debounce_ms: u64,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();
        let mut debouncer = new_debouncer(
            std::time::Duration::from_millis(debounce_ms),
            notify_tx,
        )
        .expect("Failed to create file watcher");

        debouncer
            .watcher()
            .watch(&watch_path, notify::RecursiveMode::Recursive)
            .expect("Failed to watch path");

        eprintln!("👁  Watching {} for changes...", watch_path.display());

        // A spec kept outside the analyzed tree is still a thing people edit,
        // and a spec pane that only refreshes when some *unrelated* source
        // file happens to change reads as broken. Watched as a second root
        // rather than by widening the first: the point of `--spec-dir
        // ../../` is to see one repo's specs, not to watch its code.
        watch_outside_spec(debouncer.watcher(), &config);

        loop {
            if stop_rx.try_recv().is_ok() { break; }
            match notify_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(Ok(events)) => {
                    // Asked of the *live* scope, not the watcher's startup
                    // copy, for the same reason `handle_reanalysis` analyzes
                    // with the live one: a reader who narrowed the analysis
                    // from the browser changed which files are worth waking
                    // up for.
                    let scoped = with_live_scope(&config, &shared_config);
                    let worth_it = analyzable_changes(&events, &scoped);
                    if worth_it.is_empty() { continue; }
                    log_changed_files(&worth_it);
                    handle_reanalysis(&config, &output_dir, &graph, &shared_config, &tx);
                }
                Ok(Err(e)) => eprintln!("   ⚠ Watch error: {:?}", e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    })
}

/// Add the spec directory as a second watched root when it lies outside the
/// analyzed one. A failure here is reported and survived: the graph is still
/// correct, it just stops following spec edits, and that is not worth
/// refusing to start over.
fn watch_outside_spec(watcher: &mut dyn notify::Watcher, config: &crate::config::Config) {
    let Some(spec) = config.spec_root().filter(|_| config.spec_is_outside_root()) else {
        return;
    };
    match watcher.watch(&spec, notify::RecursiveMode::Recursive) {
        Ok(()) => eprintln!("👁  Watching spec at {} for changes...", spec.display()),
        Err(e) => eprintln!("   ⚠ Cannot watch spec dir {}: {e}", spec.display()),
    }
}

/// The paths in this batch that a re-analysis would actually look at.
///
/// Two filters, and the split between them is the point. What the *config*
/// admits — language, test files, exclude and include patterns, the spec
/// directory — is answered by the walker itself, so the watcher cannot drift
/// from the walk it is triggering. What *git* ignores is answered by git, per
/// batch, because ignore rules are edited while the server runs and an
/// answer cached at startup would be wrong by the time it mattered.
///
/// Before this, the test was the file extension alone, so every write a build
/// made under `target/`, `dist/` or `node_modules/` re-analyzed the repo —
/// and, in diff mode, re-checked-out and re-analyzed the base worktree with
/// it. The result never changed, since those paths are excluded from the
/// analysis anyway. It was pure churn on every build.
fn analyzable_changes(
    events: &[notify_debouncer_mini::DebouncedEvent],
    config: &crate::config::Config,
) -> Vec<String> {
    let walker = crate::analyzer::FileWalker::new(config);
    // An ignore file is never itself analyzed, and is exactly the thing whose
    // edit changes which files are. Left out, adding a line to `.gitignore`
    // moves nothing until the next source save — and a reader who has just
    // excluded a directory is watching for it to disappear.
    let wakes = |p: &std::path::Path| walker.would_analyze(p) || crate::diff::names_ignore_file(p);
    let candidates: Vec<String> = events
        .iter()
        .filter(|e| matches!(e.kind, DebouncedEventKind::Any))
        .filter(|e| wakes(&e.path))
        .map(|e| e.path.display().to_string())
        .collect();

    let ignored = crate::diff::ignored_paths(&config.root_path, &candidates);
    candidates.into_iter().filter(|p| !ignored.contains(p)).collect()
}

fn log_changed_files(changed: &[String]) {
    eprintln!("📝 Change detected in: {}", changed.join(", "));
}

/// Re-analyze after a file change and publish the result.
///
/// Swaps the config alongside the graph, and not as a formality: the two are
/// rendered together (`/api/graph`, `/api/index` and `/api/details` all pair
/// `state.graph` with `state.config.root_path`), so a graph from one root
/// shown against another produces absolute `file_path`s and a scope tree
/// rooted at `/Users`. A diff of two commits used to leave exactly that
/// mismatch behind, permanently, because the watcher replaced the graph on
/// every save and never the config it was analyzed with (SRV-019).
///
/// `config` here is the one the watcher analyzed with — the live, watched
/// root — so publishing it is what restores the pair.
///
/// The *scope* is taken from the published config rather than from the
/// watcher's own copy, which is the other half of the same pairing problem.
/// A reader who narrows the analysis from the browser — languages, docs, the
/// spec directory — has changed what the graph is supposed to contain, and
/// the next keystroke in any watched file would otherwise re-analyze with
/// the scope the process started with and silently undo them.
fn handle_reanalysis(
    config: &crate::config::Config,
    output_dir: &PathBuf,
    graph: &Arc<std::sync::RwLock<crate::graph::DependencyGraph>>,
    shared_config: &Arc<std::sync::RwLock<crate::config::Config>>,
    tx: &Arc<broadcast::Sender<ReloadKind>>,
) {
    let config = &with_live_scope(config, shared_config);
    match write_json(config, output_dir) {
        Ok((ents, rels, new_graph)) => {
            if let Ok(mut g) = graph.write() { *g = new_graph; }
            if let Ok(mut c) = shared_config.write() { *c = config.clone(); }
            eprintln!("   Re-analyzed: {} entities, {} relationships", ents, rels);
            let _ = tx.send(ReloadKind::Graph);
        }
        Err(e) => eprintln!("   ⚠ Re-analysis failed: {}", e),
    }
}

/// The watcher's root with the published scope. Only `analysis` and
/// `filters` cross over: `root_path` must stay the watched one, which is the
/// whole point of SRV-019 above.
fn with_live_scope(
    config: &crate::config::Config,
    shared: &Arc<std::sync::RwLock<crate::config::Config>>,
) -> crate::config::Config {
    let mut config = config.clone();
    if let Ok(live) = shared.read() {
        config.analysis = live.analysis.clone();
        config.filters = live.filters.clone();
    }
    config
}

// ------------------------------------------------------------------
//  HTTP server
// ------------------------------------------------------------------

/// The settled configuration the HTTP server needs, separated from the live
/// handles it shares with the file watcher. Flags kept arriving here one at a
/// time until the argument list was longer than anything could read.
struct HttpServer {
    path: PathBuf,
    output_dir: PathBuf,
    port: u16,
    include_tests: bool,
    include_docs: bool,
    languages: Option<Vec<String>>,
    spec_dir: Option<PathBuf>,
    content_fallback: Option<PathBuf>,
    policy: AccessPolicy,
    ui_dir: Option<PathBuf>,
    allow_agent_spawn: bool,
    pin_diff: bool,
    /// The merged settings file, handed to handlers that rebuild a config.
    settings: Arc<crate::settings::Settings>,
    /// The same file kept attributable, for `/api/settings`.
    settings_view: Arc<state::SettingsView>,
}

async fn run_http_server(
    opts: HttpServer,
    tx: Arc<broadcast::Sender<ReloadKind>>,
    shared_graph: Arc<std::sync::RwLock<crate::graph::DependencyGraph>>,
    shared_config: Arc<std::sync::RwLock<crate::config::Config>>,
) {
    let HttpServer {
        path,
        output_dir,
        port,
        include_tests,
        include_docs,
        languages,
        spec_dir,
        content_fallback,
        policy,
        ui_dir,
        allow_agent_spawn,
        pin_diff,
        settings,
        settings_view,
    } = opts;
    let repo_root = path.canonicalize().unwrap_or(path);

    // Resolution order: env var → workspace `content/` → CLI fallback.
    // The extension wires its bundled `content/` into `content_fallback`,
    // so educator content works in foreign workspaces but a checked-in
    // `<workspace>/content/` still overrides it.
    let resolved_content = crate::educator::Educator::resolve_content_root(&repo_root)
        .or_else(|| content_fallback.filter(|p| p.exists()));

    let educator = match resolved_content {
        Some(content_root) => match crate::educator::Educator::load(&content_root) {
            Ok(ed) => {
                eprintln!(
                    "📚 Educator: loaded {} rule(s), {} lesson(s) from {}",
                    ed.rules().len(),
                    ed.lessons().len(),
                    content_root.display()
                );
                ed
            }
            Err(e) => {
                eprintln!("⚠ Educator: failed to load from {}: {:#}", content_root.display(), e);
                crate::educator::Educator::empty()
            }
        },
        None => crate::educator::Educator::empty(),
    };

    let state = AppState {
        tx: tx.clone(),
        output_dir: output_dir.clone(),
        repo_root: Arc::new(tokio::sync::RwLock::new(repo_root)),
        include_tests,
        include_docs,
        languages,
        spec_dir,
        settings: settings.clone(),
        settings_view,
        diff_in_progress: Arc::new(tokio::sync::Mutex::new(false)),
        analysis_in_progress: Arc::new(tokio::sync::Mutex::new(false)),
        graph: shared_graph,
        config: shared_config,
        diff_result: Arc::new(std::sync::RwLock::new(None)),
        base_details: Arc::new(std::sync::RwLock::new(None)),
        live_diff: Arc::new(std::sync::RwLock::new(None)),
        base_cache: Arc::new(std::sync::RwLock::new(None)),
        diff_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        follow_diff: !pin_diff,
        cancel: Arc::new(AtomicBool::new(false)),
        educator: Arc::new(educator),
        access_token: policy.token().map(str::to_owned),
    };

    // Keep a working-tree diff current as the watcher re-analyzes (UI-067).
    // Spawned even when pinned: the task is what reads `follow_diff`, and a
    // pinned server simply never has a live diff for it to refresh.
    tokio::spawn(follow_the_watcher(state.clone()));

    let app = build_router(state, &output_dir, &policy, ui_dir.as_deref(), port, allow_agent_spawn);
    print_startup_banner(port, &policy, ui_dir.as_deref());

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Re-run the live working-tree diff whenever the watcher publishes a new
/// graph (UI-067).
///
/// Lives on the server side rather than in the watcher thread because this
/// is where `AppState` is: the refresh needs the repo root, the output dir,
/// the base cache and the diff stores, and threading all of those into a
/// `std::thread` that exists to poll a filesystem would put half the server
/// in the watcher.
///
/// Only `Graph` events are answered. The refresh publishes `Diff`, which
/// this loop must therefore ignore, or a single save would spin.
async fn follow_the_watcher(state: AppState) {
    let mut rx = state.tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(ReloadKind::Graph) => diff_handler::refresh_live_diff(&state).await,
            Ok(ReloadKind::Diff) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn build_router(
    state: AppState,
    output_dir: &std::path::Path,
    policy: &AccessPolicy,
    ui: Option<&std::path::Path>,
    port: u16,
    allow_agent_spawn: bool,
) -> Router {
    let data_service = tower_http::services::ServeDir::new(output_dir);

    let app = Router::new()
        .route("/events", get(handlers::sse_handler))
        .route("/api/commits", get(handlers::commits_handler))
        .route(
            "/api/diff",
            get(handlers::diff_get_handler)
                .post(diff_handler::diff_handler)
                .delete(diff_handler::stop_diff_handler),
        )
        .route("/api/root", get(handlers::get_root_handler).post(diff_handler::set_root_handler))
        .route("/api/graph", get(handlers::graph_handler))
        .route("/api/index", get(handlers::index_handler))
        .route("/api/details", get(handlers::details_handler))
        .route("/api/details/base", get(handlers::base_details_handler))
        .route("/api/scope", post(scope_handler::scope_handler))
        .route("/api/settings", get(settings_handler::settings_handler))
        .route(
            "/api/settings/analysis",
            post(settings_handler::save_analysis_scope_handler),
        )
        .route(
            "/api/analysis/scope",
            post(analysis_handler::analysis_scope_handler)
                .get(analysis_handler::analysis_scope_state_handler),
        )
        // Saved views (UI-082) — repo-scope, so `nao serve` never gets these
        // routes and its UI falls back to browser storage. See
        // `views_handler.rs` and ADR 0008.
        .route(
            "/api/views",
            get(views_handler::get_views_handler).put(views_handler::put_views_handler),
        )
        .route("/api/educator/position", get(educator_handler::position_handler))
        .route("/api/educator/scan", get(educator_handler::scan_handler))
        .route("/api/educator/diagnostics", get(educator_handler::diagnostics_handler))
        .nest_service("/data", data_service);

    // The only route that executes code. Absent unless asked for, rather
    // than present and refusing — a route that exists is a route that can be
    // reached by a bug. Serve mode never gets it at all (SRV-012's reasoning:
    // a submitted repo must not reach a spawn path).
    let app = if allow_agent_spawn {
        app.route("/api/agents/terminal", post(agent_terminal::terminal_handler))
    } else {
        app
    };

    let app = app.with_state(state);

    // Cross-origin access is an allowlist, not a free-for-all: the VS Code
    // webview always, anything the user named with `--allow-origin`, nothing
    // else — and a pairing token on top for origins that aren't loopback.
    // See `access.rs` for why loopback is not a boundary here.
    //
    // Wrapped around the UI mount rather than inside it, so *every* response
    // answers the same way — including the fallback's 404s. A 404 with no
    // CORS headers reads to the browser as a refused origin, which is how
    // `detectServeMode`'s probe of `/api/repos` (a route watch mode simply
    // does not have) turned into an error the UI could not tell apart from
    // a real refusal.
    // `/api/hello` is registered after the layers so neither wraps it: it is
    // the handshake that lets a refused page find out it was refused.
    policy
        .apply(ui_dir::mount(app, ui, port))
        .route("/api/hello", access::hello_route(policy, "watch", allow_agent_spawn))
}

fn print_startup_banner(port: u16, policy: &AccessPolicy, ui: Option<&std::path::Path>) {
    eprintln!();
    eprintln!("🚀 Server running at http://localhost:{}", port);
    eprintln!("   SSE endpoint:   http://localhost:{}/events", port);
    eprintln!("   Data files:     http://localhost:{}/data/data.json", port);
    eprintln!("   API:            GET  /api/commits   - list recent commits");
    eprintln!("                   POST /api/diff      - compute diff between commits");
    eprintln!("                   DEL  /api/diff      - leave diff mode");
    eprintln!("                   GET  /api/root      - get current analyzed path");
    eprintln!("                   POST /api/root      - change root path and re-analyze");
    eprintln!("                   GET  /api/analysis/scope - current analyzed languages");
    eprintln!("                   POST /api/analysis/scope - narrow analyzed languages");
    eprintln!("                   GET  /api/settings  - settings in effect, and where each came from");
    eprintln!("                   POST /api/settings/analysis - save the analysis scope as this repo's default");
    access::print_allowed_origins(policy);
    access::print_pairing_token(policy);
    ui_dir::print_banner_line(port, ui);
    eprintln!();
    eprintln!("Press Ctrl+C to stop.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_mini::DebouncedEvent;

    fn touched(paths: &[&std::path::Path]) -> Vec<DebouncedEvent> {
        paths
            .iter()
            .map(|p| DebouncedEvent { path: p.to_path_buf(), kind: DebouncedEventKind::Any })
            .collect()
    }

    /// The watcher used to test the file extension and nothing else, so a
    /// build writing generated `.rs` files under `target/` re-analyzed the
    /// whole repo — and, in diff mode, re-checked-out and re-analyzed the
    /// base worktree with it. The graph never changed: those paths are
    /// excluded from the analysis anyway. It was churn on every build.
    #[test]
    fn a_write_no_analysis_would_read_does_not_wake_the_watcher() {
        let root = std::path::Path::new("/repo");
        let config = crate::config::Config::for_path(root);

        for quiet in ["target/debug/build_script.rs", "node_modules/pkg/index.js", "notes.txt"] {
            let events = touched(&[&root.join(quiet)]);
            assert!(
                analyzable_changes(&events, &config).is_empty(),
                "{quiet} is not in the graph, so changing it cannot change the graph",
            );
        }
    }

    /// The other direction, which is the one that would break a user's
    /// session silently: a source file the analysis *does* read has to come
    /// through, even in a batch that is mostly noise.
    #[test]
    fn a_source_write_still_wakes_the_watcher() {
        let root = std::path::Path::new("/repo");
        let config = crate::config::Config::for_path(root);
        let events = touched(&[
            &root.join("target/debug/build_script.rs"),
            &root.join("src/main.rs"),
        ]);

        let woke = analyzable_changes(&events, &config);
        assert_eq!(woke, vec![root.join("src/main.rs").display().to_string()]);
    }

    /// An ignore file is not analyzed and never was — but it decides what is.
    /// A reader who has just excluded a directory is watching for it to
    /// disappear from the graph, not waiting for their next unrelated save.
    #[test]
    fn editing_an_ignore_file_wakes_the_watcher() {
        let root = std::path::Path::new("/repo");
        let config = crate::config::Config::for_path(root);

        for ignore in [".gitignore", "src/.gitignore", ".ignore"] {
            let events = touched(&[&root.join(ignore)]);
            assert!(
                !analyzable_changes(&events, &config).is_empty(),
                "{ignore} changes what an analysis contains",
            );
        }
    }
}
