//! Per-repo state for `nao serve` (SRV-003, SRV-004).
//!
//! `nao watch` answers "which repo is this request about?" implicitly — it's
//! the path the process was launched with. `nao serve` hosts several repos at
//! once, so the answer has to be carried in the URL. This module holds the
//! analyzed-repo value type, the slot that wraps it with a job status, and
//! the registry keyed by slug that the route extractor resolves against.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

use crate::analyzer::{AnalysisResult, Analyzer};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::output::OutputFormat;

/// One analyzed repo, ready to serve.
///
/// Immutable once built: every field is fixed at analysis time, so the
/// registry can hand out `Arc<RepoState>` clones without any inner locking.
/// The not-yet-ready states live in [`RepoSlot`] *around* this type, so a
/// half-built repo is never representable here.
pub(crate) struct RepoState {
    pub slug: String,
    /// Clone URL, when the repo came from one. `None` for `--seed`ed local
    /// paths, which have no submission URL.
    pub url: Option<String>,
    /// `HEAD` at analysis time, when the path is a git checkout.
    pub sha: Option<String>,
    /// Unix epoch seconds at which analysis finished.
    pub ready_at: u64,
    pub root_path: PathBuf,
    pub graph: DependencyGraph,
    pub config: Config,
}

/// Where a repo is in the clone → analyze pipeline.
///
/// `Ready` and `Failed` are terminal; re-submitting a failed slug restarts
/// the pipeline, which is what a user retrying a fixed typo expects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JobStatus {
    Queued,
    Cloning,
    Analyzing,
    Ready,
    Failed(String),
}

impl JobStatus {
    pub fn label(&self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Cloning => "cloning",
            JobStatus::Analyzing => "analyzing",
            JobStatus::Ready => "ready",
            JobStatus::Failed(_) => "failed",
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            JobStatus::Failed(e) => Some(e),
            _ => None,
        }
    }

}

/// A slug's entry in the registry: its current pipeline status, the analyzed
/// repo once there is one, and a broadcast channel of status transitions.
///
/// `std::sync::RwLock` rather than tokio's: every hold is a field read or a
/// status swap with no `.await` inside, and the status has to be readable
/// from the blocking analysis thread.
pub(crate) struct RepoSlot {
    pub slug: String,
    /// Submission URL. `None` for `--seed`ed local paths.
    pub url: Option<String>,
    status: std::sync::RwLock<JobStatus>,
    state: std::sync::RwLock<Option<Arc<RepoState>>>,
    /// Status transitions, for `GET /api/repos/{slug}/events`. Bounded and
    /// lossy by construction — a lagging subscriber misses intermediate
    /// states but the handler re-reads the current status, so it always
    /// converges on the truth.
    events: broadcast::Sender<StatusEvent>,
}

/// One `status` SSE frame.
#[derive(Clone, Serialize)]
pub(crate) struct StatusEvent {
    pub slug: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RepoSlot {
    pub fn new(slug: String, url: Option<String>, status: JobStatus) -> Self {
        let (events, _) = broadcast::channel(32);
        Self {
            slug,
            url,
            status: std::sync::RwLock::new(status),
            state: std::sync::RwLock::new(None),
            events,
        }
    }

    /// A slot for an already-analyzed repo (seeding, or restart rehydration).
    pub fn ready(state: Arc<RepoState>) -> Self {
        let slot = Self::new(state.slug.clone(), state.url.clone(), JobStatus::Ready);
        *slot.state.write().unwrap() = Some(state);
        slot
    }

    pub fn status(&self) -> JobStatus {
        self.status.read().unwrap().clone()
    }

    pub fn state(&self) -> Option<Arc<RepoState>> {
        self.state.read().unwrap().clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.events.subscribe()
    }

    /// Move to `next` and announce it. Send failures mean nobody is
    /// listening, which is the common case and not an error.
    pub fn transition(&self, next: JobStatus) {
        let event = self.event_for(&next);
        *self.status.write().unwrap() = next;
        let _ = self.events.send(event);
    }

    /// Publish the finished analysis, then flip to `Ready` so a reader that
    /// sees `ready` always finds the state behind it.
    pub fn publish(&self, state: Arc<RepoState>) {
        *self.state.write().unwrap() = Some(state);
        self.transition(JobStatus::Ready);
    }

    pub fn event_for(&self, status: &JobStatus) -> StatusEvent {
        StatusEvent {
            slug: self.slug.clone(),
            status: status.label().to_string(),
            error: status.error().map(str::to_string),
        }
    }

    pub fn summary(&self) -> RepoSummary {
        let status = self.status();
        let state = self.state();
        RepoSummary {
            slug: self.slug.clone(),
            url: self.url.clone().or_else(|| state.as_ref().and_then(|s| s.url.clone())),
            sha: state.as_ref().and_then(|s| s.sha.clone()),
            ready_at: state.as_ref().map(|s| s.ready_at).unwrap_or(0),
            entity_count: state.as_ref().map(|s| s.graph.node_count()).unwrap_or(0),
            relationship_count: state.as_ref().map(|s| s.graph.edge_count()).unwrap_or(0),
            status: status.label().to_string(),
            error: status.error().map(str::to_string),
        }
    }
}

/// The wire shape of `GET /api/repos` and `GET /api/repos/{slug}`.
#[derive(Serialize)]
pub(crate) struct RepoSummary {
    pub slug: String,
    pub url: Option<String>,
    pub sha: Option<String>,
    /// Unix epoch seconds; `0` while the repo is still being built.
    pub ready_at: u64,
    /// `queued` | `cloning` | `analyzing` | `ready` | `failed`.
    pub status: String,
    /// Present only on `failed` — the clone or analysis stderr, so a user
    /// can tell a typo'd URL from a private repo from a size rejection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub entity_count: usize,
    pub relationship_count: usize,
}

/// Slug → slot. `RwLock` because the submission handler inserts into this
/// map while requests are reading it.
pub(crate) type RepoRegistry = Arc<RwLock<HashMap<String, Arc<RepoSlot>>>>;

/// Slugs are `<owner>__<repo>`. The double underscore is the separator, so a
/// repo name containing a single one still round-trips.
///
/// The charset is `[A-Za-z0-9._-]`. SRV-003 specified `[A-Za-z0-9_-]`, but a
/// dot has to be allowed: `three.js`, `docs.rs`, `next.js` and friends are
/// ordinary GitHub repo names, and excluding the dot would make every one of
/// them unreachable.
///
/// The slug is a directory name under the cache root, so path escapes are
/// rejected explicitly rather than left to the charset: no `.`, no `..`, and
/// no `..` anywhere inside. A leading `-` or `.` is refused too — neither is
/// a legal GitHub owner name, and both are the shapes that get mistaken for
/// a flag or a hidden file.
pub(crate) fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug == "." || slug == ".." || slug.contains("..") {
        return false;
    }
    if slug.starts_with('-') || slug.starts_with('.') {
        return false;
    }
    slug.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Build the analysis config for a repo served to untrusted visitors.
///
/// The one thing that differs from [`super::state::build_config`] is
/// `allow_unsafe_passes`: AN-004's rust-analyzer tracer drives `cargo check`,
/// which executes the analyzed repo's `build.rs` and proc-macros. Under
/// `nao serve` the repo came from a URL a stranger pasted, so the pass is off
/// unless the operator sets `NAO_SERVE_ALLOW_UNSAFE_PASSES=1` for their own
/// use. Tree-sitter parsing never executes the code it reads and stays on.
pub(crate) fn build_serve_config(
    root: &Path,
    include_tests: bool,
    languages: &Option<Vec<String>>,
) -> Config {
    let mut config = Config::for_path(root).with_output_format(OutputFormat::Json);
    config.analysis.include_tests = include_tests;
    config.analysis.allow_unsafe_passes = unsafe_passes_allowed();
    if let Some(langs) = languages {
        for lang in langs {
            if let Some(language) = Language::from_name(lang) {
                config.analysis.languages.insert(language);
            }
        }
    }
    config
}

/// The `NAO_SERVE_ALLOW_UNSAFE_PASSES=1` escape hatch. Off unless explicitly
/// set — an unset or malformed value means "off", never "on".
pub(crate) fn unsafe_passes_allowed() -> bool {
    std::env::var_os("NAO_SERVE_ALLOW_UNSAFE_PASSES").is_some_and(|v| v == "1" || v == "true")
}

/// Analyze `path` into a ready-to-serve [`RepoState`].
///
/// Unlike watch mode's `write_json`, nothing is written to disk here: serve
/// mode renders each response from the in-memory graph. Persistence is a
/// separate step ([`persist`]) so `--seed`ed local paths — which the operator
/// already has on disk — don't get a cache copy they never asked for.
pub(crate) fn analyze_repo(
    slug: &str,
    url: Option<String>,
    path: &Path,
    include_tests: bool,
    languages: &Option<Vec<String>>,
) -> Result<(RepoState, AnalysisResult)> {
    let root_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let config = build_serve_config(&root_path, include_tests, languages);

    let mut analyzer = Analyzer::new(config.clone());
    let result = analyzer.analyze()?;
    let graph = DependencyGraph::from_analysis(&result);

    Ok((build_state(slug, url, root_path, graph, config), result))
}

fn build_state(
    slug: &str,
    url: Option<String>,
    root_path: PathBuf,
    graph: DependencyGraph,
    config: Config,
) -> RepoState {
    RepoState {
        slug: slug.to_string(),
        url,
        sha: head_sha(&root_path),
        ready_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        root_path,
        graph,
        config,
    }
}

/// `HEAD` of the checkout at `root`, or `None` when it isn't a git repo.
fn head_sha(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

// ------------------------------------------------------------------
//  Submission URLs
// ------------------------------------------------------------------

/// A validated submission: the canonical clone URL and its slug.
pub(crate) struct GithubRepo {
    pub url: String,
    pub slug: String,
}

/// Accept exactly `https://github.com/<owner>/<repo>` with an optional
/// `.git` suffix and optional trailing slash — nothing else.
///
/// Phase 1 is public GitHub only: no other host, no `git@`/`ssh://`/`git://`
/// scheme, no userinfo, no port, no extra path segments. Hand-rolled rather
/// than a regex because the crate isn't a direct dependency and the grammar
/// is four rules; an allowlist this narrow is also easier to audit than a
/// pattern. The output is fed straight to `git clone`, so anything that
/// could smuggle a flag or a second argument has to be excluded here.
pub(crate) fn parse_github_url(input: &str) -> Result<GithubRepo> {
    const PREFIX: &str = "https://github.com/";
    let raw = input.trim();

    let rest = raw
        .strip_prefix(PREFIX)
        .ok_or_else(|| anyhow::anyhow!("only https://github.com/<owner>/<repo> URLs are accepted"))?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);

    let (owner, repo) = rest
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("URL is missing the repository name"))?;

    for (label, part) in [("owner", owner), ("repository", repo)] {
        if part.is_empty() {
            anyhow::bail!("URL has an empty {label}");
        }
        if part == "." || part == ".." {
            anyhow::bail!("invalid {label} `{part}`");
        }
        if !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            anyhow::bail!(
                "invalid {label} `{part}`: expected letters, digits, `.`, `_` or `-`"
            );
        }
    }

    let slug = format!("{owner}__{repo}");
    // Owner and repo are already restricted to a subset of the slug charset
    // except for `.`, which the slug rule rejects. Belt and braces: the slug
    // becomes a directory name.
    if !is_valid_slug(&slug) {
        anyhow::bail!("repository name produces an unusable slug `{slug}`");
    }

    Ok(GithubRepo {
        url: format!("{PREFIX}{owner}/{repo}"),
        slug,
    })
}

// ------------------------------------------------------------------
//  On-disk cache
// ------------------------------------------------------------------

/// Everything about a cached repo that isn't recoverable from the analysis
/// snapshot: which URL it came from and when it was analyzed.
#[derive(Serialize, Deserialize)]
struct CachedMeta {
    slug: String,
    url: Option<String>,
    sha: Option<String>,
    ready_at: u64,
    root_path: PathBuf,
}

/// The clone lives here; the JSON sits beside it.
pub(crate) fn clone_dir(cache_dir: &Path, slug: &str) -> PathBuf {
    cache_dir.join(slug).join("repo")
}

/// Written **last**, and the marker startup scans for: its presence means
/// every other file in the slug directory is complete.
pub(crate) fn meta_path(cache_dir: &Path, slug: &str) -> PathBuf {
    cache_dir.join(slug).join("meta.json")
}

/// The compressed analysis snapshot — the only file rehydration reads.
fn snapshot_path(cache_dir: &Path, slug: &str) -> PathBuf {
    cache_dir.join(slug).join("analysis.json.zst")
}

/// Pre-SRV-006 caches wrote this uncompressed and also wrote three rendered
/// payloads. Both are still readable; neither is written any more.
fn legacy_snapshot_path(cache_dir: &Path, slug: &str) -> PathBuf {
    cache_dir.join(slug).join("analysis.json")
}

/// Rendered payloads SRV-004 wrote and nothing ever read — every response
/// renders from the in-memory graph. Deleted from old caches on startup.
const LEGACY_RENDERED: [&str; 3] = ["graph.json", "details.json", "index.json"];

/// zstd level. 3 is the default for a reason: on this data it reaches 31x,
/// and level 19 only gets to 33x for two orders of magnitude more CPU.
const ZSTD_LEVEL: i32 = 3;

/// Write a finished analysis to `<cache-dir>/<slug>/` so a restart doesn't
/// re-analyze.
///
/// Two files. `analysis.json.zst` is the serialized `AnalysisResult`, which
/// rebuilds the `DependencyGraph` exactly — the rendered JSON the UI consumes
/// is a lossy projection (relative paths, lowercased kinds, flattened
/// metrics) and cannot reconstruct the graph `/scope` traverses, which is why
/// it isn't the cache format. `meta.json` carries what the snapshot doesn't:
/// the submission URL and when it was analyzed.
///
/// Both are written to a temporary name and renamed, which is atomic within a
/// directory on POSIX, and `meta.json` lands last. A process killed at any
/// point therefore leaves either a complete entry or one without its marker,
/// never a torn snapshot that startup would try to parse.
pub(crate) fn persist(cache_dir: &Path, state: &RepoState, result: &AnalysisResult) -> Result<()> {
    let dir = cache_dir.join(&state.slug);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let snapshot = snapshot_path(cache_dir, &state.slug);
    write_atomic(&snapshot, &zstd::encode_all(&*serde_json::to_vec(result)?, ZSTD_LEVEL)?)
        .context("writing analysis.json.zst")?;

    let meta = CachedMeta {
        slug: state.slug.clone(),
        url: state.url.clone(),
        sha: state.sha.clone(),
        ready_at: state.ready_at,
        root_path: state.root_path.clone(),
    };
    write_atomic(&meta_path(cache_dir, &state.slug), &serde_json::to_vec(&meta)?)
        .context("writing meta.json")?;
    Ok(())
}

/// Write via `<path>.tmp` + rename so a reader never observes a partial file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read the snapshot, preferring the compressed form and falling back to the
/// uncompressed one a pre-SRV-006 nao wrote.
fn read_snapshot(cache_dir: &Path, slug: &str) -> Result<AnalysisResult> {
    let compressed = snapshot_path(cache_dir, slug);
    if compressed.exists() {
        let raw = zstd::decode_all(&*std::fs::read(&compressed)?)
            .context("decompressing analysis.json.zst")?;
        return serde_json::from_slice(&raw).context("parsing analysis.json.zst");
    }
    let plain = std::fs::read(legacy_snapshot_path(cache_dir, slug))
        .context("no analysis snapshot in this cache entry")?;
    serde_json::from_slice(&plain).context("parsing analysis.json")
}

/// Delete rendered payloads left by a pre-SRV-006 cache. They were never
/// read, and on a tinygrad-sized repo they are ~345 MB per entry.
fn drop_legacy_rendered(cache_dir: &Path, slug: &str) {
    for name in LEGACY_RENDERED {
        let path = cache_dir.join(slug).join(name);
        if path.exists() && std::fs::remove_file(&path).is_ok() {
            eprintln!("   🧹 cache: removed unused {slug}/{name}");
        }
    }
}

/// Rewrite an uncompressed snapshot in the compressed form, once, at boot.
///
/// Without this an upgraded cache keeps paying the old price until every repo
/// happens to be re-submitted — for tinygrad that's 494 MB sitting there
/// indefinitely. The compressed file is written and renamed into place before
/// the original is removed, so an interruption leaves the entry readable
/// either way.
fn compress_legacy_snapshot(cache_dir: &Path, slug: &str, result: &AnalysisResult) {
    let legacy = legacy_snapshot_path(cache_dir, slug);
    if !legacy.exists() || snapshot_path(cache_dir, slug).exists() {
        return;
    }
    let encoded = serde_json::to_vec(result)
        .map_err(anyhow::Error::from)
        .and_then(|json| Ok(zstd::encode_all(&*json, ZSTD_LEVEL)?));
    let Ok(bytes) = encoded else {
        eprintln!("   ⚠ cache: could not compress {slug}, leaving it as-is");
        return;
    };
    if write_atomic(&snapshot_path(cache_dir, slug), &bytes).is_ok()
        && std::fs::remove_file(&legacy).is_ok()
    {
        eprintln!("   🗜 cache: compressed {slug}/analysis.json → .zst");
    }
}

/// Rebuild one cached repo. `Err` means the cache entry is unusable — the
/// caller skips it, leaving the slug free to be re-submitted.
fn rehydrate_one(cache_dir: &Path, slug: &str, include_tests: bool, languages: &Option<Vec<String>>) -> Result<RepoState> {
    let meta: CachedMeta = serde_json::from_slice(&std::fs::read(meta_path(cache_dir, slug))?)
        .context("reading meta.json")?;
    let result = read_snapshot(cache_dir, slug)?;
    // Only after a successful parse — never replace a snapshot we couldn't read.
    compress_legacy_snapshot(cache_dir, slug, &result);

    let graph = DependencyGraph::from_analysis(&result);
    let config = build_serve_config(&meta.root_path, include_tests, languages);
    Ok(RepoState {
        slug: meta.slug,
        url: meta.url,
        sha: meta.sha,
        ready_at: meta.ready_at,
        root_path: meta.root_path,
        graph,
        config,
    })
}

/// Scan `<cache-dir>/*/meta.json` and rebuild every completed repo.
///
/// Best-effort per entry: a cache written by an older nao whose snapshot no
/// longer deserializes is reported and skipped, not fatal. Starting with a
/// smaller map is always recoverable — the user re-submits — whereas
/// refusing to boot is not.
pub(crate) fn rehydrate(
    cache_dir: &Path,
    include_tests: bool,
    languages: &Option<Vec<String>>,
) -> Vec<RepoState> {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(slug) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_valid_slug(&slug) || !meta_path(cache_dir, &slug).exists() {
            continue;
        }
        drop_legacy_rendered(cache_dir, &slug);
        match rehydrate_one(cache_dir, &slug, include_tests, languages) {
            Ok(state) => out.push(state),
            Err(e) => eprintln!("   ⚠ cache: skipping {slug}: {e:#}"),
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

/// Default cache root: `$XDG_CACHE_HOME/nao/serve`, else `~/.cache/nao/serve`,
/// else a temp directory. Overridable with `--cache-dir`.
pub fn default_cache_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("nao").join("serve");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home).join(".cache").join("nao").join("serve");
    }
    std::env::temp_dir().join("nao-serve")
}

/// Parse one `--seed <slug>=<path>` argument.
pub fn parse_seed(arg: &str) -> Result<(String, PathBuf)> {
    let (slug, path) = arg
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("--seed expects <slug>=<path>, got `{arg}`"))?;
    if !is_valid_slug(slug) {
        anyhow::bail!("invalid seed slug `{slug}`: expected [A-Za-z0-9_-]+");
    }
    if path.is_empty() {
        anyhow::bail!("--seed `{arg}` has an empty path");
    }
    Ok((slug.to_string(), PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_accepts_the_owner_repo_form() {
        assert!(is_valid_slug("tinygrad__tinygrad"));
        assert!(is_valid_slug("llvator__nao"));
        assert!(is_valid_slug("some-org__some_repo-2"));
        // Dotted repo names are ordinary on GitHub and must round-trip.
        assert!(is_valid_slug("mrdoob__three.js"));
        assert!(is_valid_slug("vercel__next.js"));
    }

    #[test]
    fn slug_rejects_path_traversal_and_separators() {
        for bad in [
            "", ".", "..", "a/b", "../etc", "a b", "a%2Fb", "café",
            // A dot is allowed, but never in a shape that walks the cache
            // root or hides the directory.
            "a..b", "..a", ".hidden", "-flag", "a\\b", "a\0b",
        ] {
            assert!(!is_valid_slug(bad), "should reject `{bad}`");
        }
    }

    #[test]
    fn seed_parses_slug_and_path() {
        let (slug, path) = parse_seed("tinygrad__tinygrad=/tmp/tinygrad").unwrap();
        assert_eq!(slug, "tinygrad__tinygrad");
        assert_eq!(path, PathBuf::from("/tmp/tinygrad"));
    }

    /// A Windows-style path keeps its drive colon; only the *first* `=`
    /// separates, so paths containing `=` survive too.
    #[test]
    fn seed_splits_on_the_first_equals_only() {
        let (slug, path) = parse_seed("a=/tmp/x=y").unwrap();
        assert_eq!(slug, "a");
        assert_eq!(path, PathBuf::from("/tmp/x=y"));
    }

    #[test]
    fn seed_rejects_missing_separator_and_bad_slugs() {
        assert!(parse_seed("no-equals-sign").is_err());
        assert!(parse_seed("../evil=/tmp/x").is_err());
        assert!(parse_seed("ok=").is_err());
    }

    /// The safety property SRV-003 exists to guarantee: serve mode must not
    /// let AN-004 execute build scripts from an untrusted tree. Checked at
    /// the config layer, so it holds whatever `NAO_LSP_EXACT` says.
    #[test]
    fn serve_config_disables_unsafe_passes_by_default() {
        // The escape hatch is process-wide env, so only assert the default
        // path when the hatch isn't set in the test environment.
        if !unsafe_passes_allowed() {
            let config = build_serve_config(Path::new("."), false, &None);
            assert!(!config.analysis.allow_unsafe_passes);
        }
    }

    /// …while every other entry point is unchanged.
    #[test]
    fn non_serve_config_still_allows_unsafe_passes() {
        let config = Config::for_path(Path::new("."));
        assert!(config.analysis.allow_unsafe_passes);
    }

    // ---- submission URLs (SRV-004) ----

    #[test]
    fn github_url_accepts_the_canonical_forms() {
        for input in [
            "https://github.com/tinygrad/tinygrad",
            "https://github.com/tinygrad/tinygrad/",
            "https://github.com/tinygrad/tinygrad.git",
            "https://github.com/tinygrad/tinygrad.git/",
            "  https://github.com/tinygrad/tinygrad  ",
        ] {
            let repo = parse_github_url(input).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(repo.slug, "tinygrad__tinygrad");
            assert_eq!(repo.url, "https://github.com/tinygrad/tinygrad");
        }
    }

    #[test]
    fn github_url_keeps_dots_and_dashes_in_names() {
        let repo = parse_github_url("https://github.com/some-org/my.repo_v2").unwrap();
        assert_eq!(repo.slug, "some-org__my.repo_v2");
    }

    /// Phase 1 is public GitHub over https and nothing else. Each of these
    /// would otherwise reach `git clone` as an argument.
    #[test]
    fn github_url_rejects_everything_else() {
        for bad in [
            "http://github.com/a/b",                  // not https
            "https://gitlab.com/a/b",                 // wrong host
            "https://bitbucket.org/a/b",
            "git@github.com:a/b.git",                 // ssh
            "ssh://git@github.com/a/b",
            "git://github.com/a/b",
            "https://github.com/a",                   // no repo
            "https://github.com/a/b/tree/main",       // extra segments
            "https://github.com//b",                  // empty owner
            "https://github.com/a/",                  // empty repo
            "https://github.com/../b",                // traversal
            "https://github.com/a/..",
            "https://user:pw@github.com/a/b",         // userinfo
            "https://github.com:22/a/b",              // port
            "https://github.com.evil.test/a/b",       // suffix-confusion host
            "https://github.com/a/b --upload-pack=x", // argument smuggling
            "--upload-pack=x",
            "",
        ] {
            assert!(
                parse_github_url(bad).is_err(),
                "should have rejected `{bad}`"
            );
        }
    }

    /// The slug is a directory name under the cache root, so nothing that
    /// escapes it may survive validation.
    #[test]
    fn every_accepted_url_yields_a_path_safe_slug() {
        for good in [
            "https://github.com/tinygrad/tinygrad",
            "https://github.com/some-org/my.repo_v2",
            "https://github.com/a/b.git",
        ] {
            let slug = parse_github_url(good).unwrap().slug;
            assert!(!slug.contains('/') && !slug.contains(".."), "unsafe: {slug}");
        }
    }

    // ---- job status ----

    #[test]
    fn failed_status_carries_its_reason_into_the_summary() {
        let slot = RepoSlot::new("a__b".into(), None, JobStatus::Queued);
        slot.transition(JobStatus::Failed("clone timed out after 300s".into()));
        let summary = slot.summary();
        assert_eq!(summary.status, "failed");
        assert_eq!(summary.error.as_deref(), Some("clone timed out after 300s"));
        assert_eq!(summary.entity_count, 0);
    }

    #[test]
    fn in_flight_slot_has_no_state_but_reports_its_status() {
        let slot = RepoSlot::new("a__b".into(), Some("u".into()), JobStatus::Cloning);
        assert!(slot.state().is_none());
        assert_eq!(slot.summary().status, "cloning");
        assert_eq!(slot.summary().url.as_deref(), Some("u"));
    }

    // ---- cache ----

    /// A cache directory that doesn't exist is an empty cache, not a crash —
    /// this is the very first run.
    #[test]
    fn rehydrate_tolerates_a_missing_cache_dir() {
        let missing = std::env::temp_dir().join("nao-cache-does-not-exist-xyz");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(rehydrate(&missing, false, &None).is_empty());
    }

    /// `meta.json` is the completion marker and is written last: a slug
    /// directory without one is an interrupted write and must be skipped.
    #[test]
    fn rehydrate_skips_a_slug_without_its_marker() {
        let root = std::env::temp_dir().join("nao-cache-marker-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a__b")).unwrap();
        std::fs::write(root.join("a__b").join("analysis.json.zst"), b"partial").unwrap();
        assert!(rehydrate(&root, false, &None).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A marker with an unreadable snapshot beside it is skipped rather than
    /// aborting the whole boot.
    #[test]
    fn rehydrate_skips_a_corrupt_snapshot() {
        let root = std::env::temp_dir().join("nao-cache-corrupt-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a__b")).unwrap();
        std::fs::write(root.join("a__b").join("meta.json"), br#"{"slug":"a__b","url":null,"sha":null,"ready_at":0,"root_path":"/tmp"}"#).unwrap();
        std::fs::write(root.join("a__b").join("analysis.json.zst"), b"not zstd").unwrap();
        assert!(rehydrate(&root, false, &None).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A full write/read round-trip: analyze this repo's own `src/server`,
    /// persist it, and rebuild — the restored graph must match, since that's
    /// the whole point of not re-analyzing on restart.
    #[test]
    fn persisted_analysis_rehydrates_to_the_same_graph() {
        let root = std::env::temp_dir().join("nao-cache-roundtrip-test");
        let _ = std::fs::remove_dir_all(&root);

        let (state, result) =
            analyze_repo("a__b", Some("u".into()), Path::new("src/server"), false, &None).unwrap();
        // The submission path analyzes through this function, so this is
        // where the AN-004 guarantee has to hold for cloned repos too.
        if !unsafe_passes_allowed() {
            assert!(!state.config.analysis.allow_unsafe_passes);
        }
        let (nodes, edges) = (state.graph.node_count(), state.graph.edge_count());
        persist(&root, &state, &result).unwrap();

        let restored = rehydrate(&root, false, &None);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].slug, "a__b");
        assert_eq!(restored[0].url.as_deref(), Some("u"));
        assert_eq!(restored[0].graph.node_count(), nodes);
        assert_eq!(restored[0].graph.edge_count(), edges);

        // Exactly two files, and the snapshot is compressed. The rendered
        // payloads SRV-004 wrote were never read back; every response
        // renders from the in-memory graph.
        let mut names: Vec<String> = std::fs::read_dir(root.join("a__b"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["analysis.json.zst", "meta.json"]);

        // …and it really is smaller than the JSON it encodes.
        let on_disk = std::fs::metadata(root.join("a__b").join("analysis.json.zst"))
            .unwrap()
            .len();
        let raw = serde_json::to_vec(&result).unwrap().len() as u64;
        assert!(on_disk < raw / 2, "expected >2x, got {raw} → {on_disk}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A cache written before SRV-006 must keep working after an upgrade,
    /// and its write-only rendered payloads must be reclaimed.
    #[test]
    fn rehydrate_reads_a_legacy_cache_and_reclaims_its_dead_weight() {
        let root = std::env::temp_dir().join("nao-cache-legacy-test");
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("a__b");
        std::fs::create_dir_all(&dir).unwrap();

        let (state, result) =
            analyze_repo("a__b", None, Path::new("src/server"), false, &None).unwrap();
        let nodes = state.graph.node_count();

        // Exactly what SRV-004 left behind: uncompressed snapshot, meta, and
        // three rendered files nothing reads.
        std::fs::write(dir.join("analysis.json"), serde_json::to_vec(&result).unwrap()).unwrap();
        let meta = CachedMeta {
            slug: "a__b".into(),
            url: None,
            sha: None,
            ready_at: 1,
            root_path: state.root_path.clone(),
        };
        std::fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        for f in LEGACY_RENDERED {
            std::fs::write(dir.join(f), b"rendered payload").unwrap();
        }

        let restored = rehydrate(&root, false, &None);
        assert_eq!(restored.len(), 1, "legacy cache should still load");
        assert_eq!(restored[0].graph.node_count(), nodes);
        for f in LEGACY_RENDERED {
            assert!(!dir.join(f).exists(), "{f} should have been reclaimed");
        }
        // The snapshot is migrated in place, so an upgraded cache stops
        // paying the old price without waiting to be re-submitted.
        assert!(!dir.join("analysis.json").exists(), "should have been compressed");
        assert!(dir.join("analysis.json.zst").exists());

        // And the migrated entry still loads on the next boot.
        let again = rehydrate(&root, false, &None);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].graph.node_count(), nodes);

        let _ = std::fs::remove_dir_all(&root);
    }
}
