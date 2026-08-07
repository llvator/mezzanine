//! `GET /api/settings` — what the settings file resolved to, and why.
//!
//! The browser had no way to see any of this. A repo's `.nao/settings.json`
//! decides which languages get parsed and how deep the traversal goes, and
//! the panel that reads "Settings" showed four client-side preferences and
//! nothing else. Asked why their graph looked the way it did, a user had
//! nowhere to look but a JSON file they had to know existed (CFG-006).
//!
//! Everything this serves is a snapshot of the *file* resolution, not of the
//! live session. Narrowing the analysis scope from the panel next door
//! changes the graph without changing the file, and reporting the narrowed
//! set here would attribute the reader's own experiment to their repo.
//!
//! `POST /api/settings/analysis` is the other direction (CFG-010): it
//! promotes the applied analysis scope to the repo's default. Both routes are
//! watch-only. `serve` gets a read route of its own — it never read a
//! submitted repo's file, so it has nothing repo-scoped to show and nothing
//! it may write.

use std::path::Path;

use axum::{extract::State, http::StatusCode, response::Json};

use crate::settings::report::{Inputs, SettingsReport};
use crate::settings::{self, Settings};

use super::state::AppState;

/// The keys "save as this repo's default" owns.
///
/// Deliberately the analysis tier only. `min_weight` and `kind` belong to the
/// filter panel and to saved views; writing them from here would give one
/// value two controls, which is how "why does the graph look like this?"
/// stops having an answer. `port` and friends need a restart and are shown
/// read-only.
const SAVED_KEYS: &[&str] = &[
    "language",
    "include_tests",
    "include_docs",
    "include_locals",
    "include_external",
    "max_depth",
    "exclude_patterns",
    "include_patterns",
    "spec_dir",
];

/// GET /api/settings
pub(crate) async fn settings_handler(
    State(state): State<AppState>,
) -> Result<Json<SettingsReport>, (StatusCode, String)> {
    let root = state.repo_root.read().await.clone();
    Ok(Json(build_report(&state, &root)?))
}

fn build_report(state: &AppState, root: &Path) -> Result<SettingsReport, (StatusCode, String)> {
    let view = &state.settings_view;
    let loaded = view
        .loaded
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Settings lock poisoned: {e}")))?;
    let config = state
        .config
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Config lock poisoned: {e}")))?;
    Ok(SettingsReport::build(
        Some(root),
        &Inputs {
            loaded: &loaded,
            flags: &view.flags,
            config: &config,
            effective: &view.effective,
            repo_scope_read: true,
        },
    ))
}

/// POST /api/settings/analysis — write the applied analysis scope into
/// `<root>/.nao/settings.json` and report the file back.
///
/// Saving does not re-analyze. The scope being saved is the one already
/// applied, so there is nothing to recompute; this is about disk.
pub(crate) async fn save_analysis_scope_handler(
    State(state): State<AppState>,
) -> Result<Json<SettingsReport>, (StatusCode, String)> {
    let root = state.repo_root.read().await.clone();
    let path = settings::repo_path(&root);

    // Read before write. A file we failed to parse is not a file we may
    // replace — answering "you had nothing" for unreadable JSON is how the
    // next save destroys it. `read_raw` distinguishes that from absent, which
    // the ordinary loader deliberately does not.
    let existing = settings::read_raw(&path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "Refusing to overwrite {} — it could not be parsed ({e}). Fix or \
                 remove it, then save again.",
                path.display()
            ),
        )
    })?;

    let merged = merge_analysis_keys(existing, &current_scope(&state)?)?;
    settings::write(&path, &merged)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save — {e}")))?;

    refresh(&state, &root)?;
    build_report(&state, &root).map(Json)
}

/// The analysis scope as this process currently has it, shaped as a settings
/// file. Read off the live `Config` rather than the startup flags: the point
/// of the button is to keep an experiment the reader just made.
fn current_scope(state: &AppState) -> Result<Settings, (StatusCode, String)> {
    let config = state
        .config
        .read()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Config lock poisoned: {e}")))?;
    Ok(Settings::analysis_scope_of(&config))
}

/// Lay the analysis keys over whatever the file already said, leaving every
/// other key exactly as the author wrote it.
///
/// A whole-file replace would be simpler and wrong: this endpoint understands
/// nine keys out of sixteen, and silently dropping a `port` the user set is
/// not a saving operation.
fn merge_analysis_keys(
    mut existing: serde_json::Map<String, serde_json::Value>,
    scope: &Settings,
) -> Result<serde_json::Map<String, serde_json::Value>, (StatusCode, String)> {
    reject_escaping_spec_dir(scope)?;
    let serde_json::Value::Object(fresh) = serde_json::to_value(scope).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Could not serialize settings — {e}"))
    })?
    else {
        unreachable!("Settings serializes as an object");
    };
    for key in SAVED_KEYS {
        match fresh.get(*key) {
            Some(v) => {
                existing.insert((*key).to_string(), v.clone());
            }
            // Absent means "no filter" for `language`, and "unset" for
            // `spec_dir`. Either way the durable answer is to remove the key,
            // not to leave a stale one behind.
            None => {
                existing.remove(*key);
            }
        }
    }
    Ok(existing)
}

/// A `--spec-dir` outside the tree is a legitimate thing for an operator to
/// pass and an illegitimate thing to write down: the loader refuses it on the
/// next start (a cloned file does not get to pick which directories nao
/// reads), so saving it would produce a file that silently stops working.
fn reject_escaping_spec_dir(scope: &Settings) -> Result<(), (StatusCode, String)> {
    let escapes = scope.spec_dir.as_ref().is_some_and(|dir| {
        dir.is_absolute() || dir.components().any(|c| c == std::path::Component::ParentDir)
    });
    if escapes {
        return Err((
            StatusCode::BAD_REQUEST,
            "This spec directory points outside the repo, and a settings file \
             may only name one inside it — the loader would refuse it on the \
             next start. Keep passing it with --spec-dir."
                .to_string(),
        ));
    }
    Ok(())
}

/// Re-read both scopes so the report reflects what is now on disk, warnings
/// included. Cheaper than it looks and far safer than patching the cached
/// copy: what the loader makes of the file is the only thing worth reporting.
fn refresh(state: &AppState, root: &Path) -> Result<(), (StatusCode, String)> {
    let fresh = settings::load_scoped(root);
    let mut slot = state
        .settings_view
        .loaded
        .write()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Settings lock poisoned: {e}")))?;
    *slot = fresh;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;
    use std::path::PathBuf;

    fn existing(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => m,
            _ => unreachable!("test fixture is an object"),
        }
    }

    fn scope() -> Settings {
        Settings::analysis_scope_of(&Config::default())
    }

    /// The regression that matters most here: this endpoint understands nine
    /// keys out of sixteen, and a save that silently dropped the other seven
    /// would not be a save.
    #[test]
    fn keys_this_endpoint_does_not_own_survive_a_save() {
        let before = existing(json!({ "port": 3300, "debounce_ms": 50, "min_weight": 2 }));
        let after = merge_analysis_keys(before, &scope()).unwrap();
        assert_eq!(after.get("port"), Some(&json!(3300)));
        assert_eq!(after.get("debounce_ms"), Some(&json!(50)));
        assert_eq!(after.get("min_weight"), Some(&json!(2)));
    }

    /// Narrowing to "no filter" has to remove the key, not leave the previous
    /// language list behind — the next start would read the stale one.
    #[test]
    fn clearing_a_filter_removes_the_key_rather_than_stranding_it() {
        let before = existing(json!({ "language": ["rust", "python"] }));
        let after = merge_analysis_keys(before, &scope()).unwrap();
        assert!(!after.contains_key("language"), "a stale filter survived the save");
    }

    #[test]
    fn the_analysis_keys_are_written() {
        let mut config = Config::default();
        config.analysis.include_docs = true;
        config.analysis.max_depth = 7;
        let after = merge_analysis_keys(Default::default(), &Settings::analysis_scope_of(&config))
            .unwrap();
        assert_eq!(after.get("include_docs"), Some(&json!(true)));
        assert_eq!(after.get("max_depth"), Some(&json!(7)));
    }

    /// A `--spec-dir` outside the tree is legitimate to pass and illegitimate
    /// to write: the loader refuses it on the next start, so saving it would
    /// produce a file that silently stops working.
    #[test]
    fn a_spec_dir_outside_the_repo_is_refused_rather_than_written() {
        for outside in ["/etc", "../../elsewhere"] {
            let mut config = Config::default();
            config.analysis.spec_dir = Some(PathBuf::from(outside));
            let scope = Settings::analysis_scope_of(&config);
            let err = merge_analysis_keys(Default::default(), &scope).unwrap_err();
            assert_eq!(err.0, StatusCode::BAD_REQUEST, "{outside} was written");
        }
    }

    #[test]
    fn a_spec_dir_inside_the_repo_is_written() {
        let mut config = Config::default();
        config.analysis.spec_dir = Some(PathBuf::from("docs/domain"));
        let after =
            merge_analysis_keys(Default::default(), &Settings::analysis_scope_of(&config)).unwrap();
        assert_eq!(after.get("spec_dir"), Some(&json!("docs/domain")));
    }

    /// The capability-granting keys are not fields of `Settings`, so no
    /// serializer can emit them. This is the test that keeps that true if
    /// someone ever swaps the writer for a hand-built JSON object.
    #[test]
    fn a_save_can_never_introduce_a_privileged_key() {
        let after = merge_analysis_keys(Default::default(), &scope()).unwrap();
        for key in ["allow_agent_spawn", "no_token", "allow_origin", "allow_unsafe_passes"] {
            assert!(!after.contains_key(key), "{key} reached a written file");
        }
    }

    /// A privileged key an author put there by hand is *not* removed. This
    /// endpoint owns nine keys; policing the rest of the file is the loader's
    /// job, and it already refuses this one loudly on every read.
    #[test]
    fn a_privileged_key_already_in_the_file_is_left_for_the_loader_to_refuse() {
        let before = existing(json!({ "allow_agent_spawn": true }));
        let after = merge_analysis_keys(before, &scope()).unwrap();
        assert_eq!(after.get("allow_agent_spawn"), Some(&json!(true)));
    }
}
