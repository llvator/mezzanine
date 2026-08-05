use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        Json,
    },
};
use std::process::Command;
use tokio::sync::broadcast;

use crate::output::{self, JsonRenderer};

use super::state::AppState;
use super::types::{CommitInfo, RootPathResponse};

/// SSE handler: clients subscribe to reload events.
pub(crate) async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.tx.subscribe();
    let stream = async_stream::stream! {
        // Send an initial "connected" event so the client knows the stream is live.
        yield Ok(Event::default().event("connected").data("ok"));
        loop {
            match rx.recv().await {
                Ok(kind) => {
                    // Two event names, so a client can re-fetch only the
                    // overlay when only the overlay moved (UI-067).
                    yield Ok(Event::default().event(kind.event_name()).data("changed"));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

/// GET /api/commits — list recent commits.
pub(crate) async fn commits_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<CommitInfo>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    Ok(Json(git_commits(&repo_root)?))
}

/// Read the 50 most recent commits of the repository at `repo_root`.
/// Shared by watch-mode `/api/commits` and serve-mode
/// `/api/repos/{slug}/commits`.
pub(crate) fn git_commits(
    repo_root: &std::path::Path,
) -> Result<Vec<CommitInfo>, (StatusCode, String)> {
    let output = Command::new("git")
        .args([
            "log",
            "--oneline",
            "--format=%H|%h|%s|%an|%ad",
            "--date=short",
            "-50",
        ])
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to run git: {}", e),
            )
        })?;

    if !output.status.success() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Not a git repository or git command failed".to_string(),
        ));
    }

    let commits: Vec<CommitInfo> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 5 {
                Some(CommitInfo {
                    hash: parts[0].to_string(),
                    short_hash: parts[1].to_string(),
                    message: parts[2].to_string(),
                    author: parts[3].to_string(),
                    date: parts[4].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(commits)
}

/// GET /api/root — get current analyzed root path.
pub(crate) async fn get_root_handler(
    State(state): State<AppState>,
) -> Json<RootPathResponse> {
    let path = state.repo_root.read().await.display().to_string();
    Json(RootPathResponse {
        path,
        success: true,
        message: None,
        entity_count: None,
        relationship_count: None,
    })
}

/// GET /api/diff — diff result JSON.
pub(crate) async fn diff_get_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), StatusCode> {
    let data = state
        .diff_result
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match data.as_ref() {
        Some(json) => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json.clone(),
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// GET /api/details/base — base commit details (for side-by-side comparison).
pub(crate) async fn base_details_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), StatusCode> {
    let data = state
        .base_details
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match data.as_ref() {
        Some(json) => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json.clone(),
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// GET /api/graph — full graph JSON.
pub(crate) async fn graph_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), (StatusCode, String)> {
    let graph = state.graph.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Graph lock poisoned: {}", e),
        )
    })?;
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {}", e),
        )
    })?;
    let json = output::render(&graph, &config).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Render failed: {}", e),
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json,
    ))
}

/// GET /api/index — file/folder hierarchy.
pub(crate) async fn index_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), (StatusCode, String)> {
    let graph = state.graph.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Graph lock poisoned: {}", e),
        )
    })?;
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {}", e),
        )
    })?;
    let json = JsonRenderer::render_index(&graph, &config).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Render failed: {}", e),
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json,
    ))
}

/// GET /api/details — entity details + file contents.
pub(crate) async fn details_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), (StatusCode, String)> {
    let graph = state.graph.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Graph lock poisoned: {}", e),
        )
    })?;
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {}", e),
        )
    })?;
    let json = JsonRenderer::render_details(&graph, &config).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Render failed: {}", e),
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json,
    ))
}
