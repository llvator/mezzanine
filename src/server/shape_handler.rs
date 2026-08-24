//! HTTP handler for `GET /api/shape?path=<folder>`.
//!
//! Serves one folder's drawn graph — the evidence behind the `shape`
//! verdict the module rollups already carry. The two travel together in one
//! response on purpose: a picture fetched separately from the number could
//! be a re-analysis apart from it, and the reader would have no way to tell.
//!
//! One folder per request, because a picture is the folder's edge list and
//! only one is ever being looked at, where the four scalars behind the
//! verdict are cheap enough to ship for every folder on every graph load.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};

use super::state::AppState;
use crate::models::{FolderPicture, FolderShape};

#[derive(Deserialize)]
pub(crate) struct ShapeParams {
    /// Folder to draw, relative to the analysis root. Empty means the root
    /// itself, which is a real folder with a real shape and not a missing
    /// argument.
    #[serde(default)]
    pub path: String,
}

#[derive(Serialize)]
pub(crate) struct ShapeResponse {
    /// Echoed back root-relative, so a caller that guessed at spelling can
    /// see what it actually got.
    pub path: String,
    /// The verdict, straight off the module rollup — never recomputed here.
    /// `null` only for a folder the analysis scored no shape for.
    pub shape: Option<FolderShape>,
    /// The graph that verdict was computed over.
    pub picture: FolderPicture,
}

/// `GET /api/shape?path=<folder>` — the picture plus the verdict on it.
pub(crate) async fn shape_handler(
    State(state): State<AppState>,
    Query(params): Query<ShapeParams>,
) -> Result<Json<ShapeResponse>, (StatusCode, String)> {
    let root = {
        let config = state.config.read().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Config lock poisoned: {e}"),
            )
        })?;
        config.root_path.clone()
    };
    let graph = state.graph.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Graph lock poisoned: {e}"),
        )
    })?;

    let requested = std::path::PathBuf::from(&params.path);
    let absolute = if requested.is_absolute() {
        requested
    } else {
        root.join(&requested)
    };
    let absolute = absolute.display().to_string();

    let picture = graph.folder_picture(&absolute).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!(
                "No analysed folder at {:?}. Folders come from the files in scope, \
                 so a directory holding nothing nao parsed has no shape.",
                params.path
            ),
        )
    })?;

    // The verdict is read, not recomputed. A second derivation here would
    // be a second answer free to disagree with the one the Quality panel
    // and the MCP tools are printing.
    let shape = graph
        .module_metrics()
        .iter()
        .find(|m| m.path == absolute)
        .and_then(|m| m.metrics.shape.clone());

    let picture = picture.relative_to(&root);
    Ok(Json(ShapeResponse {
        path: picture.folder.clone(),
        shape,
        picture,
    }))
}
