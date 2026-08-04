use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::{extract::State, http::StatusCode, response::Json};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::diff;
use crate::graph::DependencyGraph;
use crate::output::{self, JsonRenderer};

use super::state::{build_config, AppState};
use super::types::{DiffRequest, DiffResponse, DiffSummaryResponse, RootPathRequest, RootPathResponse};

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
) -> Result<(DiffResponse, DependencyGraph, Config, String, String), String> {
    let is_working = to_ref == "WORKING";
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

    // Create + analyze base worktree
    let tmp = std::env::temp_dir();
    let base_dir = tmp.join(format!("nao-diff-base-{}", from_sha));
    let worktree_start = Instant::now();
    diff::create_worktree(repo_root, &base_dir, from_ref).map_err(e2s)?;
    let (base_graph, base_config) =
        diff::analyze_at(&base_dir, include_tests, languages, &format!("base ({})", from_sha)).map_err(e2s)?;

    // Acquire head graph: from in-memory state (WORKING) or a new worktree
    let (head_graph, head_config, head_dir_for_diff) = if is_working {
        eprintln!("   Using current working directory as head...");
        let g = current_graph.unwrap();
        let c = current_config.unwrap();
        let graph = g.read().map_err(|e| format!("Graph lock: {}", e))?.clone();
        let config = c.read().map_err(|e| format!("Config lock: {}", e))?.clone();
        let dir = config.root_path.clone();
        (graph, config, dir)
    } else {
        let hdir = tmp.join(format!("nao-diff-head-{}", to_sha));
        if let Err(e) = diff::create_worktree(repo_root, &hdir, to_ref) {
            diff::remove_worktree(repo_root, &base_dir);
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

    // Cleanup worktrees
    eprintln!("   Cleaning up worktrees...");
    diff::remove_worktree(repo_root, &base_dir);
    if !is_working {
        let hdir = tmp.join(format!("nao-diff-head-{}", to_sha));
        diff::remove_worktree(repo_root, &hdir);
    }

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

    // Read repo_root before spawning blocking task
    let repo_root = state.repo_root.read().await.clone();

    // Run diff in a blocking task since it's CPU-intensive
    let result = tokio::task::spawn_blocking({
        let output_dir = state.output_dir.clone();
        let include_tests = state.include_tests;
        let languages = state.languages.clone();
        let from_ref = req.from_ref.clone();
        let to_ref = req.to_ref.clone();
        let is_working = to_ref == "WORKING";
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
            )
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
        let mut in_progress = state.diff_in_progress.lock().await;
        *in_progress = false;
    }

    // Store results in memory and signal reload
    match result {
        Ok((resp, head_graph, head_config, diff_json, base_details_str)) => {
            if resp.success {
                if let Ok(mut g) = state.graph.write() {
                    *g = head_graph;
                }
                if let Ok(mut c) = state.config.write() {
                    *c = head_config;
                }
                if let Ok(mut d) = state.diff_result.write() {
                    *d = Some(diff_json);
                }
                if let Ok(mut b) = state.base_details.write() {
                    *b = Some(base_details_str);
                }
                let _ = state.tx.send(());
            }
            Ok(Json(resp))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
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
        let languages = state.languages.clone();
        let settings = state.settings.clone();
        let path = canonical_path.clone();

        move || -> Result<(usize, usize, DependencyGraph, Config), String> {
            let config = build_config(&path, include_tests, &languages, &settings);

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

            // Clear stale diff state
            if let Ok(mut d) = state.diff_result.write() {
                *d = None;
            }
            if let Ok(mut b) = state.base_details.write() {
                *b = None;
            }

            // Signal reload to all clients
            let _ = state.tx.send(());

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
