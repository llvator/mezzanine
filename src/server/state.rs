use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::educator::Educator;
use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::output::{self, JsonRenderer, OutputFormat};
use crate::settings::Settings;

#[derive(Clone)]
pub(crate) struct AppState {
    pub tx: Arc<broadcast::Sender<()>>,
    pub output_dir: std::path::PathBuf,
    pub repo_root: Arc<RwLock<std::path::PathBuf>>,
    pub include_tests: bool,
    pub languages: Option<Vec<String>>,
    /// The merged settings file, so handlers that rebuild a config from
    /// scratch honour it exactly as the initial analysis did.
    pub settings: Arc<Settings>,
    pub diff_in_progress: Arc<Mutex<bool>>,
    pub analysis_in_progress: Arc<Mutex<bool>>,
    pub graph: Arc<std::sync::RwLock<DependencyGraph>>,
    pub config: Arc<std::sync::RwLock<Config>>,
    pub diff_result: Arc<std::sync::RwLock<Option<String>>>,
    pub base_details: Arc<std::sync::RwLock<Option<String>>>,
    /// Flipped to `true` to ask the active analyzer to abort. The
    /// analysis-scope handler sets this before acquiring the
    /// `analysis_in_progress` mutex, then resets it before launching
    /// its own run.
    pub cancel: Arc<AtomicBool>,
    /// Educator state — loaded at startup from `NAO_EDUCATOR_CONTENT` or
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

    std::fs::create_dir_all(output_dir)?;
    let data_path = output_dir.join("data.json");
    let output_str = output::render(&graph, config)?;
    std::fs::write(&data_path, &output_str)?;

    let details_str = JsonRenderer::render_details(&graph, config)?;
    std::fs::write(data_path.with_extension("details.json"), &details_str)?;

    let index_str = JsonRenderer::render_index(&graph, config)?;
    std::fs::write(data_path.with_extension("index.json"), &index_str)?;

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
    languages: &Option<Vec<String>>,
    settings: &Settings,
) -> Config {
    let mut config = Config::for_path(root).with_output_format(OutputFormat::Json);
    config.analysis.include_tests = include_tests;
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
