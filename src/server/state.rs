use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::educator::Educator;
use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::output::{self, JsonRenderer, OutputFormat};
use crate::settings::Settings;

/// What a reload event is telling clients to re-fetch.
///
/// The channel used to carry `()`, which was enough while the only thing
/// that ever changed was the graph. A working-tree diff that follows the
/// watcher (UI-067) publishes a second kind of change, and the two have to
/// be distinguishable for two reasons: a client should not re-fetch the
/// whole graph because an overlay moved, and the task that refreshes the
/// diff listens on this same channel and must not answer its own event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReloadKind {
    /// The code was re-analyzed. Re-fetch index, graph and details.
    Graph,
    /// Only the diff overlay moved. Re-fetch `/api/diff`.
    Diff,
    /// `HEAD` moved — a checkout, a new branch, a commit. No code was
    /// re-analyzed and no overlay changed; the only thing that is now wrong
    /// on screen is the label saying which branch this graph is (UI-114).
    Head,
}

impl ReloadKind {
    /// The SSE event name clients listen for.
    pub(crate) fn event_name(self) -> &'static str {
        match self {
            ReloadKind::Graph => "reload",
            ReloadKind::Diff => "diff",
            ReloadKind::Head => "head",
        }
    }
}

/// A working-tree diff the server keeps current as the watcher re-analyzes.
///
/// Only the base ref is remembered: the head is always the tree being
/// watched, which is the whole point.
#[derive(Clone, Debug)]
pub(crate) struct LiveDiff {
    pub from_ref: String,
}

/// The most recent base analysis, kept so that re-diffing an unchanged base
/// on every file save does not re-create and re-analyze its worktree.
///
/// One entry, not a map: saves come in against the same base ref for as long
/// as the user is looking at one comparison, and a full graph is not small
/// enough to accumulate copies of speculatively.
pub(crate) struct CachedBase {
    pub sha: String,
    /// The analysis scope this base was produced under, from
    /// [`crate::diff::scope_fingerprint`]. Part of the key, not decoration:
    /// narrowing the scope from the browser leaves the git ref untouched, so
    /// a cache keyed on `sha` alone answers the next refresh with a base
    /// analyzed under the *old* scope — and the difference between the two
    /// scopes is then reported as a change to the code.
    pub scope: String,
    pub graph: DependencyGraph,
    pub config: Config,
    /// The before-side detail sidecar, rendered while the base worktree was
    /// still on disk. Carried rather than re-derived: its file entries are
    /// read off disk, and the worktree is gone by the time the next diff
    /// reuses this (UI-097).
    pub details: String,
}

/// Everything `/api/settings` needs that the rest of the state does not
/// already carry (CFG-006, CFG-008).
///
/// The merged `Settings` on `AppState` cannot answer "which file did this
/// come from" — merging is where that is discarded — so the two scopes are
/// kept here unmerged. `loaded` is behind a lock because saving the analysis
/// scope rewrites the repo file and the report has to follow it without a
/// restart.
pub(crate) struct SettingsView {
    pub loaded: std::sync::RwLock<crate::settings::Loaded>,
    /// The settings keys this process was given on its command line.
    pub flags: std::collections::BTreeSet<String>,
    /// The startup values that live nowhere else — `port` and friends were
    /// consumed as the server came up.
    pub effective: crate::settings::report::Effective,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub tx: Arc<broadcast::Sender<ReloadKind>>,
    /// What the engine is doing right now, for `/api/activity` and the
    /// `activity` SSE event (UI-138). The same sink is installed process-wide
    /// in [`crate::activity`], which is how messages from inside the analyzer
    /// — several frames below anything holding this state — reach it.
    pub activity: Arc<crate::activity::Sink>,
    pub output_dir: std::path::PathBuf,
    pub repo_root: Arc<RwLock<std::path::PathBuf>>,
    pub include_tests: bool,
    /// Analyze Markdown alongside the code (see `AnalysisConfig::include_docs`).
    pub include_docs: bool,
    pub languages: Option<Vec<String>>,
    /// `--spec-dir` as the process was started, for handlers that rebuild a
    /// config from scratch. The *live* value is on `config` — this one is
    /// the startup flag, the same way `languages` is.
    pub spec_dir: Option<std::path::PathBuf>,
    /// The merged settings file, so handlers that rebuild a config from
    /// scratch honour it exactly as the initial analysis did.
    pub settings: Arc<Settings>,
    /// The same file, kept unmerged and attributable. See [`SettingsView`].
    pub settings_view: Arc<SettingsView>,
    pub diff_in_progress: Arc<Mutex<bool>>,
    pub analysis_in_progress: Arc<Mutex<bool>>,
    pub graph: Arc<std::sync::RwLock<DependencyGraph>>,
    pub config: Arc<std::sync::RwLock<Config>>,
    pub diff_result: Arc<std::sync::RwLock<Option<String>>>,
    pub base_details: Arc<std::sync::RwLock<Option<String>>>,
    /// Set while a working-tree diff should track the watcher. `None` means
    /// no diff, or one pinned to two commits — a fixed comparison, which a
    /// file save has no business moving (UI-067).
    pub live_diff: Arc<std::sync::RwLock<Option<LiveDiff>>>,
    /// Base analysis reused across refreshes. See `CachedBase`.
    pub base_cache: Arc<std::sync::RwLock<Option<CachedBase>>>,
    /// Bumped every time diff mode is left (`DELETE /api/diff`).
    ///
    /// A diff takes seconds, and the moment a user is most likely to press
    /// "stop" is while one is running. Without this the run finishes after
    /// the stop, publishes its result and broadcasts a `diff` event, and the
    /// overlay everyone just dismissed comes back. `run_diff` snapshots this
    /// before starting and declines to publish if it moved (UI-100).
    pub diff_epoch: Arc<AtomicU64>,
    /// Flipped to `true` to ask the diff that is running to stop (UI-141).
    ///
    /// Separate from `cancel`, which is the analysis-scope handler's: the two
    /// runs overlap — a diff analyzes its two sides while the watcher
    /// re-analyzes the working tree — and one flag would mean each could only
    /// be stopped by killing the other.
    ///
    /// Reset at the start of every run rather than by whoever set it, so a
    /// stop that lands in the moment a diff is finishing cannot carry over
    /// and abort the next one.
    pub diff_cancel: Arc<AtomicBool>,
    /// False when `--pin-diff` asked for the diff to stay where it was put.
    pub follow_diff: bool,
    /// Flipped to `true` to ask the active analyzer to abort. The
    /// analysis-scope handler sets this before acquiring the
    /// `analysis_in_progress` mutex, then resets it before launching
    /// its own run.
    pub cancel: Arc<AtomicBool>,
    /// Educator state — loaded at startup from `MEZZ_EDUCATOR_CONTENT` or
    /// `<workspace>/content/`. `Educator::empty()` when neither resolves.
    pub educator: Arc<Educator>,
    /// The pairing token, when one was minted. Only the agent-spawn route
    /// reads it: every other route delegates to the shared middleware, which
    /// deliberately exempts loopback origins. That exemption is fine for
    /// reading a graph and wrong for executing code (SRV-017).
    pub access_token: Option<String>,
}

/// Run full analysis, write JSON output files, and return the graph.
pub(crate) fn write_json(
    config: &Config,
    output_dir: &Path,
) -> Result<(usize, usize, DependencyGraph)> {
    let cancel = Arc::new(AtomicBool::new(false));
    write_json_with_cancel(config, output_dir, &cancel)
}

/// `fs::write`, with the path in the error.
///
/// A helper rather than a `.with_context` closure at each call site: a
/// closure is a nesting level, and the gate fails a function whose metrics
/// rise at all — the caller is already doing the work of a whole analysis
/// run (CI-001).
fn write_named(path: &Path, body: &str) -> Result<()> {
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

/// `create_dir_all`, with the path in the error. Same reason as
/// [`write_named`], and the failure this exists for: `output_dir` defaults to
/// the repo-relative `ui/public`, so it lands on whatever the reader happens
/// to keep under that name (CFG-016).
fn make_output_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("creating output directory {}", path.display()))
}

/// Cancellable variant of `write_json`. Returns `Err` (downcastable to
/// `analyzer::Cancelled`) without touching disk if the flag is flipped
/// during analysis — the caller decides whether that's an error or a
/// graceful shutdown.
pub(crate) fn write_json_with_cancel(
    config: &Config,
    output_dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<(usize, usize, DependencyGraph)> {
    let mut analyzer = Analyzer::new(config.clone());
    let result = analyzer.analyze_with_cancel(cancel)?;
    let entity_count = result.entities.len();
    let rel_count = result.relationships.len();
    let graph = DependencyGraph::from_analysis(&result);

    // Every disk operation below names its path. The default `output_dir` is
    // a repo-relative guess (`ui/public`), so the common failure here is a
    // collision with something the reader already has — and a bare
    // `Not a directory (os error 20)` says nothing about which path collided
    // with what (CFG-016).
    make_output_dir(output_dir)?;
    let data_path = output_dir.join("data.json");
    let output_str = output::render(&graph, config)?;
    write_named(&data_path, &output_str)?;

    let details_str = JsonRenderer::render_details(&graph, config)?;
    write_named(&data_path.with_extension("details.json"), &details_str)?;

    let index_str = JsonRenderer::render_index(&graph, config)?;
    write_named(&data_path.with_extension("index.json"), &index_str)?;

    Ok((entity_count, rel_count, graph))
}

/// Build a Config for the given root path with JSON output format.
///
/// `settings` is applied last and only fills what the caller left alone —
/// the flags reaching this function have already won (see
/// [`crate::settings::Settings::apply_to_config`]).
pub(crate) fn build_config(
    root: &Path,
    include_tests: bool,
    include_docs: bool,
    languages: &Option<Vec<String>>,
    spec_dir: Option<std::path::PathBuf>,
    settings: &Settings,
) -> Config {
    let mut config = Config::for_path(root).with_output_format(OutputFormat::Json);
    config.analysis.include_tests = include_tests;
    config.analysis.include_docs = include_docs;
    config.analysis.spec_dir = spec_dir;
    if let Some(langs) = languages {
        for lang in langs {
            if let Some(language) = Language::from_name(lang) {
                config.analysis.languages.insert(language);
            }
        }
    }
    settings.apply_to_config(&mut config);
    config
}
