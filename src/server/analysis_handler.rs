//! Handler for `POST /api/analysis/scope` — narrow (or widen) the
//! language filter the analyzer applies and re-run from scratch.
//!
//! Cancellation flow:
//! 1. Set `state.cancel`. Any in-flight `analyze_with_cancel` will
//!    short-circuit at the next stage / parse-batch boundary and return
//!    `Cancelled`, which the prior caller treats as a no-op (no JSON
//!    write, no graph swap).
//! 2. Acquire `state.analysis_in_progress`. By the time the lock is
//!    held, the previous run has either finished naturally or bailed
//!    out, so it's safe to take ownership.
//! 3. Reset `state.cancel` so the new run isn't immediately aborted.
//! 4. Build a fresh `Config` with the requested languages + the rest
//!    of the originals, run `write_json_with_cancel`, swap the graph,
//!    fire the SSE reload event.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use axum::{extract::State, http::StatusCode, response::Json};

use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::file_info::Language;

use super::state::{write_json_with_cancel, AppState, ReloadKind};
use super::types::{AnalysisScopeRequest, AnalysisScopeResponse, AnalysisScopeState};

/// GET /api/analysis/scope — report the current filter so a freshly loaded
/// UI can show what is actually being analyzed rather than guessing.
pub(crate) async fn analysis_scope_state_handler(
    State(state): State<AppState>,
) -> Result<Json<AnalysisScopeState>, (StatusCode, String)> {
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {e}"),
        )
    })?;
    let mut languages: Vec<String> = config
        .analysis
        .languages
        .iter()
        .map(|l| l.filter_name().to_string())
        .collect();
    languages.sort();
    Ok(Json(AnalysisScopeState {
        // Empty means "no filter", which is what `null` means on the wire.
        languages: (!languages.is_empty()).then_some(languages),
        include_docs: config.analysis.include_docs,
        spec_dir: spec_dir_display(&config),
    }))
}

/// The spec directory as a string the panel can put in a text field, or
/// `None` for the default (every `.elv` under the root).
fn spec_dir_display(config: &Config) -> Option<String> {
    config
        .analysis
        .spec_dir
        .as_ref()
        .map(|p| p.display().to_string())
}

/// POST /api/analysis/scope — replace the analyzer's language filter
/// and re-run. Treats `languages = []` the same as `null`.
pub(crate) async fn analysis_scope_handler(
    State(state): State<AppState>,
    Json(req): Json<AnalysisScopeRequest>,
) -> Result<Json<AnalysisScopeResponse>, (StatusCode, String)> {
    // Normalize to None whenever the caller sent nothing or an empty list.
    let requested: Option<Vec<String>> = req.languages.filter(|v| !v.is_empty());
    let include_docs = req.include_docs;

    // Step 1 + 2: ask any in-flight analysis to abort, then acquire the
    // serialization mutex. The mutex guarantees we don't trample a run
    // that's already past the cancel-check points.
    state.cancel.store(true, Ordering::Relaxed);
    let mut in_progress = state.analysis_in_progress.lock().await;
    if *in_progress {
        // Another handler holds the lock and will release once it sees
        // the cancel flag. We don't queue here — return 409 so the UI
        // can decide whether to retry.
        state.cancel.store(false, Ordering::Relaxed);
        return Ok(Json(AnalysisScopeResponse {
            success: false,
            languages: requested,
            message: Some("Another analysis is already in progress".to_string()),
            entity_count: None,
            relationship_count: None,
        }));
    }
    *in_progress = true;
    state.cancel.store(false, Ordering::Relaxed);
    drop(in_progress);

    // Build a fresh Config: copy everything, then replace the languages
    // set. We don't mutate the existing `state.config` until the run
    // succeeds — that keeps readers (the live SSE clients, scope_handler)
    // observing the previous, valid scope.
    let new_config =
        match build_config_with_scope(&state, &requested, include_docs, req.spec_dir.as_deref()) {
            Ok(c) => c,
            Err(msg) => {
                let mut in_progress = state.analysis_in_progress.lock().await;
                *in_progress = false;
                return Ok(Json(AnalysisScopeResponse {
                    success: false,
                    languages: requested,
                    message: Some(msg),
                    entity_count: None,
                    relationship_count: None,
                }));
            }
        };

    let result = run_analysis_blocking(
        new_config.clone(),
        state.output_dir.clone(),
        state.cancel.clone(),
    )
    .await;

    let mut in_progress = state.analysis_in_progress.lock().await;
    *in_progress = false;
    drop(in_progress);

    match result {
        Ok((entity_count, rel_count, new_graph)) => {
            if let Ok(mut g) = state.graph.write() {
                *g = new_graph;
            }
            if let Ok(mut c) = state.config.write() {
                *c = new_config;
            }
            let _ = state.tx.send(ReloadKind::Graph);
            Ok(Json(AnalysisScopeResponse {
                success: true,
                languages: requested,
                message: Some(format!(
                    "Analyzed {} entities, {} relationships",
                    entity_count, rel_count
                )),
                entity_count: Some(entity_count),
                relationship_count: Some(rel_count),
            }))
        }
        Err(msg) => Ok(Json(AnalysisScopeResponse {
            success: false,
            languages: requested,
            message: Some(msg),
            entity_count: None,
            relationship_count: None,
        })),
    }
}

/// Clone the current config and swap in the requested scope: the language
/// filter, the docs switch, and the spec directory. Returns Err with a
/// human-readable message for an unknown language name or a spec directory
/// that isn't there.
///
/// Validating the directory here rather than letting the walker discover it
/// is what makes the mistake visible: the walker's fallback is a line on the
/// server's stderr, and the person who typed the path is looking at a
/// browser.
fn build_config_with_scope(
    state: &AppState,
    requested: &Option<Vec<String>>,
    include_docs: Option<bool>,
    spec_dir: Option<&str>,
) -> Result<Config, String> {
    let mut config = state
        .config
        .read()
        .map_err(|e| format!("Config lock poisoned: {}", e))?
        .clone();
    config.analysis.languages.clear();
    // Absent means "unchanged", not "off": a client that doesn't know about
    // the field must not silently undo the operator's `--include-docs`.
    if let Some(v) = include_docs {
        config.analysis.include_docs = v;
    }
    if let Some(langs) = requested {
        for lang in langs {
            match Language::from_name(lang) {
                Some(language) => {
                    config.analysis.languages.insert(language);
                }
                None => {
                    return Err(format!("Unknown language: {}", lang));
                }
            }
        }
    }
    apply_spec_dir(&mut config, spec_dir)?;
    Ok(config)
}

/// The three states of `spec_dir` on the wire — absent, empty, a path —
/// resolved against the analyzed root and checked before anything expensive
/// runs. See [`AnalysisScopeRequest::spec_dir`].
///
/// Stores the path as it was typed rather than the resolved one: a relative
/// `docs/domain` that survives as `docs/domain` still means the right thing
/// after the root moves, and is what the panel should show back.
fn apply_spec_dir(config: &mut Config, requested: Option<&str>) -> Result<(), String> {
    let Some(raw) = requested else { return Ok(()) };
    let typed = raw.trim();
    if typed.is_empty() {
        config.analysis.spec_dir = None;
        return Ok(());
    }
    let candidate = PathBuf::from(typed);
    let resolved = if candidate.is_absolute() {
        candidate.clone()
    } else {
        config.root_path.join(&candidate)
    };
    if !resolved.is_dir() {
        return Err(format!("Spec folder not found: {}", resolved.display()));
    }
    config.analysis.spec_dir = Some(candidate);
    Ok(())
}

/// Run the analysis on a blocking thread. Maps `Cancelled` to a
/// human-readable error so the UI can distinguish it from a real
/// failure.
async fn run_analysis_blocking(
    config: Config,
    output_dir: PathBuf,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(usize, usize, DependencyGraph), String> {
    tokio::task::spawn_blocking(move || {
        write_json_with_cancel(&config, &output_dir, &cancel).map_err(|e| {
            if e.is::<crate::analyzer::Cancelled>() {
                "Analysis was cancelled".to_string()
            } else {
                e.to_string()
            }
        })
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?
}
