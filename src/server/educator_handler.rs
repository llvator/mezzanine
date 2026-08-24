//! HTTP handler for `GET /api/educator/position`.
//!
//! Translates a `(file, line, col)` triple into the ancestor-stack response
//! the VS Code hover provider consumes. File paths are resolved relative to
//! the configured repo root so the extension can pass workspace-relative paths
//! without leaking absolute filesystem layout.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;

use super::state::AppState;
use crate::educator::PositionResponse;

#[derive(Deserialize)]
pub(crate) struct PositionParams {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

pub(crate) async fn position_handler(
    State(state): State<AppState>,
    Query(params): Query<PositionParams>,
) -> Result<Json<PositionResponse>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    let raw = std::path::PathBuf::from(&params.file);
    let absolute = if raw.is_absolute() {
        raw
    } else {
        repo_root.join(&raw)
    };

    let response = state
        .educator
        .query_position(&absolute, params.line, params.col)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e)))?;

    Ok(Json(response))
}

#[derive(Deserialize)]
pub(crate) struct ScanParams {
    pub file: String,
}

/// `GET /api/educator/scan?file=<path>` — full-file scan used by the VS Code
/// extension to populate the Problems view (one diagnostic per scan hit).
pub(crate) async fn scan_handler(
    State(state): State<AppState>,
    Query(params): Query<ScanParams>,
) -> Result<Json<crate::educator::ScanResponse>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    let raw = std::path::PathBuf::from(&params.file);
    let absolute = if raw.is_absolute() {
        raw
    } else {
        repo_root.join(&raw)
    };

    let response = state
        .educator
        .scan_file(&absolute)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e)))?;

    Ok(Json(response))
}

/// `GET /api/educator/diagnostics` — surfaces the load-time issues for tooling
/// (the VS Code extension renders these as a notification).
pub(crate) async fn diagnostics_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    let issues = state.educator.issues();
    let error_count = issues
        .iter()
        .filter(|i| matches!(i.severity, crate::educator::LoadIssueSeverity::Error))
        .count();
    let warning_count = issues.len() - error_count;
    Json(serde_json::json!({
        "error_count": error_count,
        "warning_count": warning_count,
        "issues": issues,
    }))
}
