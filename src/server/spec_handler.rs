//! `POST /api/spec/entity` — create an Elevator entity from the browser
//! (SRV-022).
//!
//! The gap this closes is the one a reader hits mid-review: you open a file
//! you don't recognise, look for the spec entity that would explain it, and
//! there isn't one. Writing it meant leaving the page, finding the right
//! `.elv`, remembering which Category the thing belongs under and what the
//! `cr:` path is — by which point the cheap moment has passed. The UI already
//! knows all four of those; this route is what lets it act on them.
//!
//! **Deliberately not a general spec editor.** It creates one entity, with
//! the description and the code refs the reader supplied, and never rewrites
//! or deletes one. A `.elv` is prose somebody wrote, and the tool that can
//! only ever *add* a definition is one you can hand a button to. Deepening
//! that prose is a job for an editor or an agent, which is why the UI's
//! second button opens a terminal rather than a bigger form.
//!
//! `mezz serve` does not register this route. There the tree arrived from a
//! URL a stranger pasted (ADR 0008), and a submitted repo must not reach a
//! path that writes to it. The bar in watch mode is
//! [`access::require_trusted_ui`] — the same one the agent-spawn route
//! clears, because both change the host rather than read it.

use std::path::{Path, PathBuf};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Json,
};
use serde::{Deserialize, Serialize};

use super::access;
use super::spec_write::{
    append_definition, insert_child_ref, render_definition, validate_name, Draft, ParentEdit,
    SpecKind,
};
use super::state::AppState;
use crate::graph::DependencyGraph;
use crate::models::CodeEntity;

type Failure = (StatusCode, String);

#[derive(Deserialize)]
pub(crate) struct CreateRequest {
    /// `c`, `f`, `fu` or `concept`.
    pub kind: String,
    /// Leaf name. A Functionality's Feature comes from `parent_id`, not from
    /// a dotted name here.
    pub name: String,
    /// Graph id of the spec entity to attach this one to.
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Root-relative paths for the `cr:` field, in display order.
    #[serde(default)]
    pub code_refs: Vec<String>,
    /// Root-relative `.elv` to write into. Ignored when `parent_id` is given:
    /// a child is written beside its parent so no `import` is needed.
    #[serde(default)]
    pub file: Option<String>,
    /// The pairing token, for a caller that is not a page this engine served.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct CreateResponse {
    /// Root-relative path of the file written.
    pub file: String,
    /// The id the next analysis will give this entity.
    pub entity_id: String,
    /// 1-based line the definition starts on, so the UI can offer to open it.
    pub line: u32,
    /// Whether the parent's body gained a child reference. False with a
    /// `note` when the parent could not be edited — the definition is still
    /// written, as an orphan.
    pub parented: bool,
    /// The `.elv` source written, so the panel can show what it did without
    /// waiting for the re-analysis.
    pub source: String,
    /// Present only when something is worth saying out loud.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where the write lands, resolved before anything is read.
struct Target {
    absolute: PathBuf,
    relative: String,
    /// The parent's definition, as `(keyword, qualified_name, first_line,
    /// last_line)` with 1-based inclusive lines. Absent for an entity that
    /// stands alone.
    parent_block: Option<(&'static str, String, u32, u32)>,
}

/// `POST /api/spec/entity`.
pub(crate) async fn create_entity_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateRequest>,
) -> Result<Json<CreateResponse>, Failure> {
    access::require_trusted_ui(
        &headers,
        req.token.as_deref(),
        state.access_token.as_deref(),
    )?;

    let kind = SpecKind::parse(&req.kind).ok_or_else(|| {
        bad(format!(
            "`{}` is not a kind this route creates — use c, f, fu or concept.",
            req.kind
        ))
    })?;
    let name = req.name.trim().to_string();
    validate_name(&name).map_err(bad)?;
    let code_refs = clean_refs(&req.code_refs)?;

    let root = read_root(&state)?;
    let (draft, target) = {
        let graph = state.graph.read().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Graph lock poisoned: {e}"),
            )
        })?;
        let parent = resolve_parent(&graph, kind, req.parent_id.as_deref())?;
        let draft = Draft {
            kind,
            name,
            parent_qualname: parent.map(|p| p.qualified_name.clone()),
            description: req.description.clone(),
            code_refs,
        };
        if let Some(existing) = graph.get_entity(&draft.entity_id()) {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "`{} {}` already exists, defined in {}. Every entity is defined exactly \
                     once — deepen that one rather than declaring a second.",
                    kind.keyword(),
                    draft.qualname(),
                    existing.file_path.display()
                ),
            ));
        }
        let target = resolve_target(&root, kind, parent, req.file.as_deref())?;
        (draft, target)
    };

    write_entity(&draft, &target).map(Json)
}

fn bad(message: String) -> Failure {
    (StatusCode::BAD_REQUEST, message)
}

fn read_root(state: &AppState) -> Result<PathBuf, Failure> {
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {e}"),
        )
    })?;
    Ok(config.root_path.clone())
}

/// Look the parent up and check it can hold this kind.
///
/// A wrong-kind parent is refused rather than silently ignored: a Feature
/// filed under another Feature is a containment edge the language has no
/// reading for, and the reader who picked it meant something by it.
fn resolve_parent<'g>(
    graph: &'g DependencyGraph,
    kind: SpecKind,
    parent_id: Option<&str>,
) -> Result<Option<&'g CodeEntity>, Failure> {
    let Some(id) = parent_id.map(str::trim).filter(|id| !id.is_empty()) else {
        if kind.requires_parent() {
            return Err(bad(format!(
                "A {} is named by its Feature (`fu f.<feature>.<verb>`), so it needs a parent.",
                kind.keyword()
            )));
        }
        return Ok(None);
    };
    let entity = graph.get_entity(id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("No spec entity `{id}` in the current analysis."),
        )
    })?;
    let wanted = kind.parent_kind().ok_or_else(|| {
        bad(format!(
            "`{}` sits at the top of the spec and is not filed under anything.",
            kind.keyword()
        ))
    })?;
    if !matches_kind(entity, wanted) {
        return Err(bad(format!(
            "`{}` is a {:?}; a {} is declared under a {}.",
            entity.qualified_name,
            entity.kind,
            kind.keyword(),
            wanted.keyword()
        )));
    }
    Ok(Some(entity))
}

fn matches_kind(entity: &CodeEntity, kind: SpecKind) -> bool {
    use crate::models::EntityKind;
    entity.tags.contains("elevator")
        && match kind {
            SpecKind::Category => entity.kind == EntityKind::Category,
            SpecKind::Feature => entity.kind == EntityKind::Feature,
            SpecKind::Functionality => entity.kind == EntityKind::Functionality,
            SpecKind::Concept => entity.kind == EntityKind::Concept,
        }
}

/// Decide which file to write, and where the parent's body is inside it.
fn resolve_target(
    root: &Path,
    kind: SpecKind,
    parent: Option<&CodeEntity>,
    requested: Option<&str>,
) -> Result<Target, Failure> {
    if let Some(parent) = parent {
        let (absolute, relative) = parent_file(root, parent)?;
        return Ok(Target {
            absolute,
            relative,
            parent_block: Some((
                parent_keyword(kind),
                parent.qualified_name.clone(),
                // `Position::line` is 0-based; the editing here is 1-based,
                // as every line number a reader is shown must be.
                parent.span.start.line as u32 + 1,
                parent.span.end.line as u32 + 1,
            )),
        });
    }
    let relative = requested
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| bad("Name a `.elv` file to write into.".to_string()))?
        .to_string();
    Ok(Target {
        absolute: safe_elv_path(root, &relative)?,
        relative,
        parent_block: None,
    })
}

/// The keyword the parent of `kind` is written with. Only called when a
/// parent was resolved, which is exactly when `parent_kind` is `Some`.
fn parent_keyword(kind: SpecKind) -> &'static str {
    kind.parent_kind().map(SpecKind::keyword).unwrap_or("c")
}

/// Where a resolved parent is defined: the path to write, and the path to
/// report.
///
/// **The two are not the same string, and this is the bug that shipped for
/// exactly one test run.** The in-memory graph holds the analyzer's own
/// absolute paths, while `/api/graph` — the JSON the UI reads — has been
/// relativized by the renderer. So a client that echoed back the `file_path`
/// it saw would be sending a *different* path from the one the parent's
/// entity carries here. That is precisely why this route takes a parent *id*
/// and derives the file itself: the two representations never have to agree.
///
/// The path is not re-checked against the root the way a client-supplied one
/// is. It did not come from the client — it came from a file the analyzer
/// loaded, and `spec_dir` is explicitly allowed to sit outside the analyzed
/// root ([`crate::config`]). Refusing it would refuse the split-repo layout
/// the setting exists for.
fn parent_file(root: &Path, parent: &CodeEntity) -> Result<(PathBuf, String), Failure> {
    let path = &parent.file_path;
    if path.extension().is_none_or(|e| e != "elv") {
        return Err(bad(format!(
            "`{}` is not defined in a `.elv` file — it is not a spec entity.",
            parent.qualified_name
        )));
    }
    let display = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    let absolute = if path.is_absolute() {
        path.clone()
    } else {
        root.join(path)
    };
    Ok((absolute, display))
}

/// Resolve a client-supplied path to a `.elv` inside the analyzed root.
///
/// Component-wise rather than by canonicalizing and comparing, because the
/// file may not exist yet — a new spec file is a normal thing to create — and
/// `canonicalize` on a missing path fails. `..` is refused outright rather
/// than normalized away: nothing legitimate sends it, and the cost of being
/// wrong is a write outside the repo.
fn safe_elv_path(root: &Path, relative: &str) -> Result<PathBuf, Failure> {
    use std::path::Component;
    let candidate = PathBuf::from(relative.replace('\\', "/"));
    if candidate.extension().is_none_or(|e| e != "elv") {
        return Err(bad(format!("{relative:?} is not a `.elv` file.")));
    }
    let escapes = candidate
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
    if escapes {
        return Err(bad(format!(
            "{relative:?} must be a relative path inside the analyzed root."
        )));
    }
    Ok(root.join(candidate))
}

/// Reject a `cr:` path that points outside the tree, and normalize separators
/// so a Windows client's backslashes match what the analyzer indexed.
fn clean_refs(raw: &[String]) -> Result<Vec<String>, Failure> {
    let mut out = Vec::new();
    for path in raw {
        let path = path.trim().replace('\\', "/");
        if path.is_empty() {
            continue;
        }
        if path.starts_with('/') || path.contains("..") {
            return Err(bad(format!(
                "{path:?} is not a path inside the analyzed root — `cr:` is always root-relative."
            )));
        }
        if !out.contains(&path) {
            out.push(path);
        }
    }
    Ok(out)
}

/// Read, edit, write. The only place bytes move.
fn write_entity(draft: &Draft, target: &Target) -> Result<CreateResponse, Failure> {
    let existing = match std::fs::read_to_string(&target.absolute) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not read {}: {e}", target.relative),
            ))
        }
    };

    let (with_child, parented, note) = attach(&existing, draft, target);
    let block = render_definition(draft);
    let (text, line) = append_definition(&with_child, &block);
    write_atomically(&target.absolute, &text)?;

    Ok(CreateResponse {
        file: target.relative.clone(),
        entity_id: draft.entity_id(),
        line,
        parented,
        source: block,
        note,
    })
}

/// Declare the containment in the parent's body, or explain why not.
///
/// A refusal is never fatal. The definition is written either way, because an
/// unparented Feature is a `--check` hint the reader can file by hand, and
/// losing what they just typed to protect them from a hint would be the worse
/// trade.
fn attach(source: &str, draft: &Draft, target: &Target) -> (String, bool, Option<String>) {
    let Some((keyword, qualname, first, last)) = target.parent_block.as_ref() else {
        return (source.to_string(), false, None);
    };
    match insert_child_ref(
        source,
        (*first, *last),
        (keyword, qualname),
        &draft.child_ref(),
    ) {
        ParentEdit::Inserted(text) => (text, true, None),
        ParentEdit::AlreadyPresent => (source.to_string(), true, None),
        ParentEdit::Moved => (
            source.to_string(),
            false,
            Some(format!(
                "`{keyword} {qualname}` is no longer at lines {first}-{last} of {} — the file \
                 has been edited since the last analysis. The definition was written, but you \
                 will need to add `{}` to that body yourself.",
                target.relative,
                draft.child_ref()
            )),
        ),
    }
}

/// Rename over a sibling temp file, the same bargain `views_handler` makes:
/// an interrupted save leaves the previous spec intact rather than half a
/// definition in a file that no longer parses.
fn write_atomically(path: &Path, text: &str) -> Result<(), Failure> {
    let fail = |e: std::io::Error, what: &Path| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Could not write {}: {e}", what.display()),
        )
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(e, dir))?;
    }
    let tmp = path.with_extension("elv.tmp");
    std::fs::write(&tmp, text).map_err(|e| fail(e, &tmp))?;
    std::fs::rename(&tmp, path).map_err(|e| fail(e, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/repo")
    }

    #[test]
    fn a_relative_elv_resolves_under_the_root() {
        let path = safe_elv_path(&root(), "spec/server.elv").unwrap();
        assert_eq!(path, PathBuf::from("/repo/spec/server.elv"));
    }

    #[test]
    fn a_path_that_leaves_the_root_is_refused() {
        for attempt in ["../outside.elv", "/etc/passwd.elv", "spec/../../x.elv"] {
            assert!(
                safe_elv_path(&root(), attempt).is_err(),
                "{attempt} should be refused"
            );
        }
    }

    #[test]
    fn only_elv_files_are_written() {
        assert!(safe_elv_path(&root(), "spec/server.rs").is_err());
        assert!(safe_elv_path(&root(), "spec/server").is_err());
    }

    /// The regression that made every parented write fail: the graph holds
    /// absolute paths and the strict root-relative check refused them all.
    #[test]
    fn a_parents_absolute_path_is_written_and_reported_relative() {
        use crate::models::{EntityKind, Position, Span};
        let span = Span::new(Position::new(0, 0, 0), Position::new(3, 1, 40));
        let mut parent = CodeEntity::new(
            "server",
            EntityKind::Category,
            "/repo/spec/server.elv",
            span,
        );
        parent.qualified_name = "server".to_string();

        let (absolute, display) = parent_file(&root(), &parent).unwrap();
        assert_eq!(absolute, PathBuf::from("/repo/spec/server.elv"));
        assert_eq!(
            display, "spec/server.elv",
            "the reader is shown a path they recognise"
        );
    }

    /// A spec that lives outside the analyzed root is what `spec_dir` is for,
    /// so a parent there is written rather than refused.
    #[test]
    fn a_parent_outside_the_root_is_still_writable() {
        use crate::models::{EntityKind, Position, Span};
        let span = Span::new(Position::new(0, 0, 0), Position::new(3, 1, 40));
        let parent = CodeEntity::new(
            "server",
            EntityKind::Category,
            "/docs/spec/server.elv",
            span,
        );
        let (absolute, display) = parent_file(&root(), &parent).unwrap();
        assert_eq!(absolute, PathBuf::from("/docs/spec/server.elv"));
        assert_eq!(display, "/docs/spec/server.elv");
    }

    #[test]
    fn a_parent_not_defined_in_an_elv_is_refused() {
        use crate::models::{EntityKind, Position, Span};
        let span = Span::new(Position::new(0, 0, 0), Position::new(3, 1, 40));
        let parent = CodeEntity::new("Server", EntityKind::Struct, "/repo/src/lib.rs", span);
        assert!(parent_file(&root(), &parent).is_err());
    }

    #[test]
    fn refs_are_normalized_and_deduped() {
        let refs = clean_refs(&[
            "src\\server\\mod.rs".to_string(),
            " src/server/mod.rs ".to_string(),
            "  ".to_string(),
            "ui/src/App.svelte".to_string(),
        ])
        .unwrap();
        assert_eq!(refs, ["src/server/mod.rs", "ui/src/App.svelte"]);
    }

    #[test]
    fn a_ref_outside_the_root_is_refused() {
        assert!(clean_refs(&["/etc/passwd".to_string()]).is_err());
        assert!(clean_refs(&["../secrets".to_string()]).is_err());
    }

    /// The whole write, against a real file — the parent gains a child line
    /// and the definition lands at the end, both in one save.
    #[test]
    fn a_feature_is_written_and_filed_under_its_category() {
        let dir = std::env::temp_dir().join(format!("mezz-spec-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("server.elv");
        std::fs::write(
            &file,
            "c server {\n    d: \"The engine.\"\n    f hello\n}\n",
        )
        .unwrap();

        let draft = Draft {
            kind: SpecKind::Feature,
            name: "spec_authoring".to_string(),
            parent_qualname: None,
            description: Some("Writes a stub from the panel.".to_string()),
            code_refs: vec!["src/server/spec_handler.rs".to_string()],
        };
        let target = Target {
            absolute: file.clone(),
            relative: "server.elv".to_string(),
            parent_block: Some(("c", "server".to_string(), 1, 4)),
        };

        let response = write_entity(&draft, &target).unwrap();
        assert!(response.parented);
        assert_eq!(response.note, None);
        assert_eq!(response.entity_id, "elevator::f.spec_authoring");

        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            written,
            "c server {\n    d: \"The engine.\"\n    f hello\n    f spec_authoring\n}\n\n\
             f spec_authoring {\n    d: \"Writes a stub from the panel.\"\n    \
             cr: \"src/server/spec_handler.rs\"\n}\n"
        );
        assert_eq!(
            written.lines().nth(response.line as usize - 1).unwrap(),
            "f spec_authoring {"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stale span writes the definition anyway and says what it could not
    /// do, rather than editing a body it can no longer identify.
    #[test]
    fn a_stale_parent_span_still_writes_the_definition() {
        let dir = std::env::temp_dir().join(format!("mezz-spec-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("server.elv");
        std::fs::write(
            &file,
            "# a line the analysis had not seen\nc server {\n    f hello\n}\n",
        )
        .unwrap();

        let draft = Draft {
            kind: SpecKind::Feature,
            name: "late".to_string(),
            parent_qualname: None,
            description: None,
            code_refs: Vec::new(),
        };
        let target = Target {
            absolute: file.clone(),
            relative: "server.elv".to_string(),
            // Where `c server` used to be, before the comment was added.
            parent_block: Some(("c", "server".to_string(), 1, 3)),
        };

        let response = write_entity(&draft, &target).unwrap();
        assert!(!response.parented);
        let note = response.note.expect("a refusal should say so");
        assert!(note.contains("f late"), "{note}");

        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.ends_with("f late\n"), "{written}");
        assert!(
            !written.contains("    f late"),
            "the body must be untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that does not exist yet is a normal thing to create.
    #[test]
    fn a_new_file_is_created_with_the_definition_at_line_one() {
        let dir = std::env::temp_dir().join(format!("mezz-spec-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("nested").join("concepts.elv");

        let draft = Draft {
            kind: SpecKind::Concept,
            name: "pairing".to_string(),
            parent_qualname: None,
            description: None,
            code_refs: Vec::new(),
        };
        let target = Target {
            absolute: file.clone(),
            relative: "nested/concepts.elv".to_string(),
            parent_block: None,
        };

        let response = write_entity(&draft, &target).unwrap();
        assert_eq!(response.line, 1);
        assert!(!response.parented);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "concept pairing\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
