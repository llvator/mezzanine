//! `GET` / `PUT /api/views` — the reader's **saved views**, stored at
//! `<analyzed-root>/.nao/views.json`.
//!
//! A saved view is a named record of what the canvas is drawing: a scope, an
//! aggregation level, the kind/language/file filters, the spec cross-filter.
//! It is repo-scope by ADR 0008's line — it is a set of paths, languages and
//! kinds *of this repo*, true no matter who clones it, and worth committing
//! beside the code it describes. That also puts the browser UI and the VS Code
//! webview on one list, since both talk HTTP to the same `nao watch`.
//!
//! `nao serve` deliberately does not register these routes. There the tree
//! arrived from a URL a stranger pasted, and ADR 0008 already forbids reading
//! a repo-scope file in that mode; the UI treats the resulting 404 as "this
//! server has no view store" and keeps its views in the browser instead.
//!
//! **The envelope is typed; the view is not.** Each entry carries `id`,
//! `name` and `saved_at` so the file reads and diffs as a named list in git
//! and so the server can refuse a nameless one — but `state` is opaque JSON.
//! Capturing one more toggle is a UI-only change, rather than the same field
//! added in two languages.

use std::path::{Path, PathBuf};

use axum::{extract::State, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};

use super::state::AppState;

const FILE_NAME: &str = "views.json";

/// Ceiling on how many views one repo may store. Not a resource limit — a
/// list this long has stopped being a set of readings you switch between,
/// and the number exists so a looping client cannot grow the file forever.
const MAX_VIEWS: usize = 200;

/// One saved view. `state` is whatever the UI captured, kept opaque here.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SavedView {
    pub id: String,
    pub name: String,
    /// ISO-8601, minted by the client — the browser is the only participant
    /// that knows the reader's clock, and this is never compared to anything
    /// server-side.
    pub saved_at: String,
    pub state: serde_json::Value,
}

/// The file's shape. `version` is written so a later format change can be
/// recognised rather than guessed at.
#[derive(Serialize, Deserialize)]
pub(crate) struct ViewsFile {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub views: Vec<SavedView>,
}

fn current_version() -> u32 {
    1
}

impl Default for ViewsFile {
    fn default() -> Self {
        Self { version: current_version(), views: Vec::new() }
    }
}

/// Where the views for an analyzed root live.
fn views_path(root: &Path) -> PathBuf {
    crate::settings::repo_dir(root).join(FILE_NAME)
}

/// Read the file, or the empty list when it is absent.
///
/// Absent is the normal case and says nothing. Unreadable or malformed is an
/// error rather than a silent empty list: answering "you have no views" for a
/// file we failed to parse invites the next save to overwrite it.
fn read_views(path: &Path) -> Result<ViewsFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<ViewsFile>(&text)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ViewsFile::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write the file whole, atomically.
///
/// Rename over a sibling temp file rather than truncate-and-write, so an
/// interrupted save leaves the previous list intact instead of half a JSON
/// document. Pretty-printed because this file is meant to be read and
/// reviewed in a diff.
fn write_views(path: &Path, file: &ViewsFile) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let body = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{body}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What a client may not store, checked before anything is written.
fn validate(file: &ViewsFile) -> Result<(), String> {
    if file.views.len() > MAX_VIEWS {
        return Err(format!("too many views ({}, max {MAX_VIEWS})", file.views.len()));
    }
    if let Some(v) = file.views.iter().find(|v| v.name.trim().is_empty()) {
        return Err(format!("view {} has no name", v.id));
    }
    if let Some(v) = file.views.iter().find(|v| v.id.trim().is_empty()) {
        return Err(format!("view {:?} has no id", v.name));
    }
    Ok(())
}

/// GET /api/views — the saved views for the analyzed root.
pub(crate) async fn get_views_handler(
    State(state): State<AppState>,
) -> Result<Json<ViewsFile>, (StatusCode, String)> {
    let root = state.repo_root.read().await.clone();
    read_views(&views_path(&root))
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Could not read saved views — {e}")))
}

/// PUT /api/views — replace the whole list.
///
/// Whole-list replace rather than per-view routes: the client holds the list
/// it is editing, and two clients editing one repo's views at the same instant
/// is not a case this tool has. The cost of the simpler contract is that the
/// later save wins outright, which is the same bargain `.nao/settings.json`
/// already makes.
pub(crate) async fn put_views_handler(
    State(state): State<AppState>,
    Json(file): Json<ViewsFile>,
) -> Result<Json<ViewsFile>, (StatusCode, String)> {
    validate(&file).map_err(|e| (StatusCode::BAD_REQUEST, format!("Rejected — {e}")))?;
    let root = state.repo_root.read().await.clone();
    write_views(&views_path(&root), &file)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save views — {e}")))?;
    Ok(Json(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-test analyzed root, isolated from the developer's tree and from
    /// sibling tests — same shape as `settings::tests::TempConfig`.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("nao-views-test-{}-{}", std::process::id(), tag));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> PathBuf {
            views_path(&self.0)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn view(id: &str, name: &str) -> SavedView {
        SavedView {
            id: id.to_string(),
            name: name.to_string(),
            saved_at: "2026-08-05T10:00:00.000Z".to_string(),
            state: serde_json::json!({ "scope": [{ "pattern": "src", "negate": false }] }),
        }
    }

    #[test]
    fn an_absent_file_is_an_empty_list_not_an_error() {
        let root = TempRoot::new("absent");
        let file = read_views(&root.path()).unwrap();
        assert!(file.views.is_empty());
        assert_eq!(file.version, 1);
    }

    #[test]
    fn a_written_list_reads_back_with_its_opaque_state() {
        let root = TempRoot::new("roundtrip");
        let file = ViewsFile { version: 1, views: vec![view("v1", "Parsers only")] };
        write_views(&root.path(), &file).unwrap();

        let back = read_views(&root.path()).unwrap();
        assert_eq!(back.views.len(), 1);
        assert_eq!(back.views[0].name, "Parsers only");
        assert_eq!(back.views[0].state["scope"][0]["pattern"], "src");
    }

    /// The failure this endpoint exists to avoid: a file we cannot parse must
    /// not read as "no saved views", because the next save would then replace
    /// it with an empty list.
    #[test]
    fn a_malformed_file_is_an_error_rather_than_an_empty_list() {
        let root = TempRoot::new("malformed");
        std::fs::create_dir_all(root.path().parent().unwrap()).unwrap();
        std::fs::write(root.path(), "{ not json").unwrap();
        assert!(read_views(&root.path()).is_err());
    }

    #[test]
    fn a_save_leaves_no_temp_file_behind() {
        let root = TempRoot::new("atomic");
        write_views(&root.path(), &ViewsFile { version: 1, views: vec![view("v1", "One")] })
            .unwrap();
        assert!(!root.path().with_extension("json.tmp").exists());
    }

    #[test]
    fn a_nameless_view_is_rejected() {
        let mut file = ViewsFile { version: 1, views: vec![view("v1", "  ")] };
        assert!(validate(&file).is_err());
        file.views[0].name = "Named".into();
        assert!(validate(&file).is_ok());
    }

    #[test]
    fn more_views_than_the_ceiling_are_rejected() {
        let views = (0..=MAX_VIEWS).map(|i| view(&format!("v{i}"), "x")).collect();
        assert!(validate(&ViewsFile { version: 1, views }).is_err());
    }
}
