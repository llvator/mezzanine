//! `GET /api/cache`, `POST /api/cache/clear` — UI-153.
//!
//! The parse store is the largest thing mezz leaves on a disk and, until
//! this, the only one with no way to look at it: 11 GB on the machine that
//! asked, 3.1 GB of it a hand-made backup no code path would ever read.
//!
//! **Machine-global, not repo-scoped.** Everything else `watch` serves
//! describes the repository it was pointed at; this describes a directory
//! shared by every repository on the machine, and the panel says so. That is
//! also why these routes are watch-only: `serve` hosts repositories somebody
//! else submitted, and a submitted repo must not reach a route that deletes
//! another repository's cache.
//!
//! The report is a `stat`-only walk and never opens an entry — 1.46s over
//! 140,987 files, against 8.3 GB of JSON if it read them. It is on-demand for
//! that reason: the panel fetches when opened, not on a poll.

use axum::{http::StatusCode, response::Json};

use crate::analyzer::cache_report::{self, CacheReport, ClearTarget};

/// GET /api/cache
pub(crate) async fn cache_handler() -> Result<Json<CacheReport>, (StatusCode, String)> {
    // Off the async runtime: a cold walk of a six-figure directory is long
    // enough to matter, and holding a runtime thread through it would stall
    // the graph requests the reader is making in the same breath.
    let report = tokio::task::spawn_blocking(cache_report::report)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("cache scan: {e}")))?;
    report.map(Json).ok_or((
        StatusCode::NOT_FOUND,
        "no cache directory resolves on this machine".to_string(),
    ))
}

/// What a clear removed, so the panel can say it rather than imply it.
#[derive(serde::Serialize)]
pub(crate) struct Cleared {
    removed: Vec<String>,
}

/// POST /api/cache/clear
///
/// Safe to call at any time, including mid-analysis: by the store's
/// robustness contract a missing entry degrades to a cold parse and never to
/// an error. The cost of clearing under a running analysis is that analysis
/// re-parsing.
///
/// The names in the body are attacker-controlled path components;
/// `cache_report::clear` refuses any that is not a single directory rather
/// than sanitizing it, and re-checks containment before removing anything.
pub(crate) async fn clear_cache_handler(
    Json(target): Json<ClearTarget>,
) -> Result<Json<Cleared>, (StatusCode, String)> {
    let removed = tokio::task::spawn_blocking(move || cache_report::clear(&target))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("cache clear: {e}")))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    for path in &removed {
        crate::activity::step(crate::activity::ANALYSIS, format!("  Cleared cache {path}"));
    }
    Ok(Json(Cleared { removed }))
}
