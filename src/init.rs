//! `nao init`: the two files a repo would otherwise hand-write.
//!
//! Every key in [`crate::settings`] is optional, so a repo works with no
//! settings file at all. What it does not get without one is a *pinned*
//! language list — and the surfaces that matter most, `nao watch` and the VS
//! Code extension, are launched with nobody typing a flag. Pinning is
//! therefore the one thing a new repo almost always wants, and reading a
//! directory listing is a worse way to decide it than walking the tree.
//!
//! Two deliberate limits on what this writes:
//!
//! - Only keys it inferred from the tree. A scaffold that spelled out every
//!   default would freeze today's defaults into every repo that ran it, and
//!   the next change to a default would silently skip them all.
//! - Never a key it cannot justify from the repo — no `output_dir`, no
//!   `port`. Both have working defaults, and an absolute `output_dir` copied
//!   between checkouts is the wart this repo's own file carries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::analyzer::FileWalker;
use crate::config::Config;
use crate::models::file_info::Language;
use crate::settings;

/// The share of the walked tree a language must hold to be pinned.
///
/// A repo is not "a Python repo" because one `.py` release script lives in
/// `scripts/`, and pinning it there costs every later analysis the whole
/// Python parser for four files. A twentieth of the tree is the line: nao's
/// own checkout puts Rust, TypeScript and Svelte over it and leaves the four
/// stray `.py` files under.
const MIN_SHARE: f64 = 0.05;

/// Scaffold `.nao/settings.json`, and optionally the VS Code tasks.
pub fn run(root: &Path, vscode: bool, force: bool) -> Result<()> {
    write_settings(root, force)?;
    if vscode {
        write_tasks(root, force)?;
    }
    Ok(())
}

/// Every file a default analysis of this root would parse.
///
/// The real walker rather than a directory listing, so `.gitignore`,
/// `target/`, `node_modules/` and the rest are already gone — a language list
/// inferred from vendored dependencies would describe somebody else's repo.
fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let config = Config::for_path(root);
    FileWalker::new(&config)
        .walk(root)
        .with_context(|| format!("walking {}", root.display()))
}

/// Write the settings file, unless the repo already has one.
///
/// An existing file is a note rather than an error, for the same reason a
/// task already present is: `nao init --vscode` in a repo that was set up
/// last year should still add the tasks, not refuse the whole command over
/// the half that was already done.
fn write_settings(root: &Path, force: bool) -> Result<()> {
    let path = settings::repo_path(root);
    if path.exists() && !force {
        println!(
            "✓ {} already exists — left alone (--force replaces it)",
            path.display()
        );
        return Ok(());
    }

    // Walked here rather than in `run` so the repo that only wants the VS
    // Code tasks doesn't pay for a full tree walk to learn nothing.
    let files = walk(root)?;
    let languages = detect_languages(&files);
    if languages.is_empty() {
        bail!(
            "no source files found under {} — nothing to infer. Analyze the \
             right directory, or write {} by hand.",
            root.display(),
            path.display()
        );
    }

    // Against the directory the file lands in, not the one that was walked:
    // `spec_dir` is read back relative to the repo root (CFG-012), so
    // `nao init src` must write `src/spec` rather than `spec`.
    let body = settings_body(&languages, detect_spec_dir(&settings::repo_root(root), &files));
    std::fs::create_dir_all(settings::repo_dir(root))
        .with_context(|| format!("creating {}", settings::repo_dir(root).display()))?;
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;

    println!("✓ wrote {}", path.display());
    println!("{}", indent(&body));
    Ok(())
}

/// The languages this repo is written in, most-used first.
///
/// Opt-in languages — Markdown today — never appear, and the rule is stated
/// here rather than left to the walker that already filters them. A pinned
/// list is exactly where `.md` would stop being opt-in without anyone
/// choosing it, and nao's own checkout has more Markdown than Rust.
fn detect_languages(files: &[PathBuf]) -> Vec<Language> {
    // Hashed rather than ordered: `Language` has no `Ord`, and the sort
    // below — count descending, then name — is what makes the output
    // reproducible anyway.
    let mut counts: HashMap<Language, usize> = HashMap::new();
    for file in files {
        let language = crate::parser::detect_language(file);
        if language != Language::Unknown && !language.is_opt_in() {
            *counts.entry(language).or_default() += 1;
        }
    }

    // The share is of the files that *have* a language, not of everything
    // walked: a repo full of docs must not push its own source under the bar.
    let total = counts.values().sum::<usize>().max(1) as f64;
    let mut kept: Vec<(Language, usize)> = counts
        .into_iter()
        .filter(|&(language, n)| earns_a_place(language, n as f64 / total))
        .collect();
    // Descending by count, then by name so the file is reproducible when two
    // languages tie.
    kept.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.filter_name().cmp(b.0.filter_name()))
    });
    kept.into_iter().map(|(language, _)| language).collect()
}

/// Elevator is exempt from [`MIN_SHARE`]. A spec is a handful of files
/// describing a tree of thousands — nine `.elv` files against 179 `.rs` here
/// — so a share test would drop the one language whose whole purpose is to
/// be outnumbered. Having any `.elv` at all is the evidence.
fn earns_a_place(language: Language, share: f64) -> bool {
    language == Language::Elevator || share >= MIN_SHARE
}

/// The directory holding this repo's spec, when there is one place to point
/// at. Scattered `.elv` files — a fixture under `examples/` beside a real
/// spec — get no key, which is the default: every `.elv` under the root is
/// the spec. Naming one of two directories would quietly delete the other
/// from the graph.
///
/// `root` is the repo root the settings file will be written to, which is
/// what the key is resolved against on the way back in.
fn detect_spec_dir(root: &Path, files: &[PathBuf]) -> Option<PathBuf> {
    let mut dirs = files
        .iter()
        .filter(|f| crate::parser::detect_language(f) == Language::Elevator)
        .map(|f| {
            f.strip_prefix(root)
                .unwrap_or(f)
                .parent()
                .map(Path::to_path_buf)
        });

    let first = dirs.next()??;
    let all_agree = dirs.all(|dir| dir.as_deref() == Some(first.as_path()));
    // A spec at the root is the default already; saying so adds a key that
    // means nothing and one more thing to keep true after a move.
    (all_agree && first.components().next().is_some()).then_some(first)
}

fn settings_body(languages: &[Language], spec_dir: Option<PathBuf>) -> String {
    let names: Vec<&str> = languages.iter().map(|l| l.filter_name()).collect();
    let mut body = json!({ "language": names });
    if let Some(dir) = spec_dir {
        body["spec_dir"] = json!(dir.to_string_lossy());
    }
    format!(
        "{}\n",
        serde_json::to_string_pretty(&body).unwrap_or_default()
    )
}

fn indent(body: &str) -> String {
    body.trim_end()
        .lines()
        .map(|line| format!("   {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// VS Code tasks
// ---------------------------------------------------------------------------

const TASKS_PATH: [&str; 2] = [".vscode", "tasks.json"];

fn write_tasks(root: &Path, force: bool) -> Result<()> {
    let path = TASKS_PATH
        .iter()
        .fold(root.to_path_buf(), |p, part| p.join(part));
    let port = settings::load(root).port.unwrap_or(settings::DEFAULT_PORT);
    let tasks = nao_tasks(port);

    let merged = match std::fs::read_to_string(&path) {
        Ok(text) => match merge_tasks(&text, tasks.clone(), &path, force) {
            Ok(merged) => merged,
            // Refusing to rewrite the file is only half an answer. The other
            // half is handing over what we would have written, so the reader
            // is a paste away rather than back at the documentation.
            Err(e) => return Err(offer_tasks_by_hand(e, &tasks)),
        },
        Err(_) => Some(json!({ "version": "2.0.0", "tasks": tasks })),
    };

    let Some(file) = merged else {
        println!("✓ {} already has the Nao tasks", path.display());
        return Ok(());
    };

    std::fs::create_dir_all(path.parent().unwrap_or(root))
        .with_context(|| format!("creating {}", path.display()))?;
    let body = format!("{}\n", serde_json::to_string_pretty(&file)?);
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    println!("✓ wrote {} (port {port})", path.display());
    Ok(())
}

/// Print the tasks the merge refused to write, and hand back the error that
/// stopped it — the command still fails, because nothing was written.
fn offer_tasks_by_hand(error: anyhow::Error, tasks: &[Value]) -> anyhow::Error {
    let body = serde_json::to_string_pretty(&tasks).unwrap_or_default();
    eprintln!(
        "   Add these to the `tasks` array by hand:\n{}",
        indent(&body)
    );
    error
}

/// Add the Nao tasks to a `tasks.json` that already exists, or `None` when
/// every one of them is already there.
///
/// Refusing an unparseable file is the whole point of the function. VS Code
/// reads `tasks.json` as JSON *with comments*, which `serde_json` rejects —
/// so a file we cannot parse is far more likely to be a perfectly good
/// commented one than a corrupt one, and rewriting it from our own parse
/// would delete a colleague's annotations to add a task they could paste in
/// thirty seconds.
fn merge_tasks(text: &str, tasks: Vec<Value>, path: &Path, force: bool) -> Result<Option<Value>> {
    let mut file: Value = serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "{}: {e}. Left untouched — if it has comments (VS Code allows \
             them, JSON does not), add the tasks by hand.",
            path.display()
        )
    })?;

    let existing = file["tasks"].as_array().cloned().unwrap_or_default();
    let mut kept: Vec<Value> = existing
        .into_iter()
        .filter(|task| !(force && is_nao_task(task, &tasks)))
        .collect();

    let added: Vec<Value> = tasks
        .into_iter()
        .filter(|task| !kept.iter().any(|k| k["label"] == task["label"]))
        .collect();
    if added.is_empty() {
        return Ok(None);
    }

    kept.extend(added);
    file["tasks"] = Value::Array(kept);
    file["version"] = json!("2.0.0");
    Ok(Some(file))
}

fn is_nao_task(task: &Value, ours: &[Value]) -> bool {
    ours.iter().any(|our| our["label"] == task["label"])
}

/// Start, open, stop — the three things a reader does with the browser UI,
/// each pinned to the same port so they cannot disagree with each other.
///
/// `nao watch` has no idle shutdown, so closing the browser tab leaves the
/// engine holding the port. The stop task exists because that surprises
/// everyone once.
fn nao_tasks(port: u16) -> Vec<Value> {
    let mut tasks = vec![start_task(), open_task(port)];
    // The stop task is a POSIX shell one-liner. On Windows the default shell
    // is PowerShell and `lsof` does not exist, so emitting it there would
    // ship a task that only ever fails.
    if std::env::consts::OS != "windows" {
        tasks.push(stop_task(port));
    }
    tasks
}

fn start_task() -> Value {
    json!({
        "label": "Nao: Start web UI",
        "detail": "Analyse this repo and serve the browser UI",
        "type": "shell",
        "command": "nao",
        "args": ["watch", "."],
        "options": { "cwd": "${workspaceFolder}" },
        "isBackground": true,
        "presentation": { "reveal": "always", "panel": "dedicated", "focus": false },
        // Without a background matcher VS Code treats the task as never
        // finishing and refuses to run anything that `dependsOn` it.
        "problemMatcher": {
            "pattern": { "regexp": "^\\s+⚠\\s+(.*)$", "message": 1, "file": 1 },
            "background": {
                "activeOnStart": true,
                "beginsPattern": "^🔍 Initial analysis",
                "endsPattern": "^👁  Watching"
            }
        }
    })
}

fn open_task(port: u16) -> Value {
    json!({
        "label": "Nao: Open web UI in browser",
        "detail": format!("Opens http://localhost:{port}, starting the engine first"),
        "type": "shell",
        "command": opener(),
        "args": [format!("http://localhost:{port}")],
        "dependsOn": "Nao: Start web UI",
        "presentation": { "reveal": "silent", "panel": "dedicated" },
        "problemMatcher": []
    })
}

fn stop_task(port: u16) -> Value {
    // Kills only a listener that *is* nao: the port may have been taken by
    // something else entirely, and a task that kills whatever it finds is a
    // task that eventually kills a database.
    let command = format!(
        "PORT={port}; if [ -z \"$(lsof -ti tcp:$PORT -sTCP:LISTEN)\" ]; then \
         echo \"No process is listening on port $PORT — nothing to stop.\"; else \
         for p in $(lsof -ti tcp:$PORT -sTCP:LISTEN); do CMD=$(ps -o comm= -p $p); \
         if [ \"$(basename \"$CMD\")\" = \"nao\" ]; then kill $p && \
         echo \"Stopped nao engine on port $PORT (pid $p).\"; else \
         echo \"Port $PORT is held by pid $p ($CMD), which is not nao — left alone.\"; \
         fi; done; fi"
    );
    json!({
        "label": "Nao: Stop web UI",
        "detail": "Kills the engine holding this repo's port. Closing the browser does not stop it.",
        "type": "shell",
        "command": command,
        "options": { "cwd": "${workspaceFolder}" },
        "presentation": { "reveal": "always", "panel": "shared", "focus": false },
        "problemMatcher": []
    })
}

fn opener() -> &'static str {
    match std::env::consts::OS {
        "macos" => "open",
        "windows" => "explorer",
        _ => "xdg-open",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_dominant_languages_are_pinned_most_used_first() {
        let mut files = vec![PathBuf::from("ui/app.ts"); 10];
        files.extend(vec![PathBuf::from("src/main.rs"); 20]);
        assert_eq!(
            detect_languages(&files),
            vec![Language::Rust, Language::TypeScript]
        );
    }

    /// The rule that keeps a language list describing the repo rather than
    /// its scripts directory.
    #[test]
    fn a_stray_script_does_not_make_it_a_python_repo() {
        let mut files = vec![PathBuf::from("src/main.rs"); 99];
        files.push(PathBuf::from("scripts/release.py"));
        assert_eq!(detect_languages(&files), vec![Language::Rust]);
    }

    /// A spec is outnumbered by design; the share test must not be what
    /// decides whether the repo has one.
    #[test]
    fn a_spec_is_pinned_however_few_files_it_has() {
        let mut files = vec![PathBuf::from("src/main.rs"); 999];
        files.push(PathBuf::from("spec/domain.elv"));
        assert!(detect_languages(&files).contains(&Language::Elevator));
    }

    #[test]
    fn markdown_is_never_pinned() {
        let files = paths(&["README.md", "docs/a.md", "src/main.rs"]);
        assert!(!detect_languages(&files).contains(&Language::Markdown));
    }

    #[test]
    fn one_spec_directory_is_named() {
        let root = Path::new("/repo");
        let files = paths(&["/repo/spec/a.elv", "/repo/spec/b.elv", "/repo/src/main.rs"]);
        assert_eq!(detect_spec_dir(root, &files), Some(PathBuf::from("spec")));
    }

    /// Naming one of two directories would delete the other from the graph
    /// — `spec_dir` excludes as well as includes.
    #[test]
    fn scattered_specs_get_no_key() {
        let root = Path::new("/repo");
        let files = paths(&["/repo/spec/a.elv", "/repo/examples/b.elv"]);
        assert_eq!(detect_spec_dir(root, &files), None);
    }

    #[test]
    fn a_spec_at_the_root_gets_no_key() {
        let root = Path::new("/repo");
        assert_eq!(detect_spec_dir(root, &paths(&["/repo/nao.elv"])), None);
    }

    #[test]
    fn the_body_carries_only_what_was_inferred() {
        let body = settings_body(&[Language::Rust], None);
        assert_eq!(body, "{\n  \"language\": [\n    \"rust\"\n  ]\n}\n");
    }

    #[test]
    fn the_body_names_a_spec_directory_when_there_is_one() {
        let body = settings_body(&[Language::Rust], Some(PathBuf::from("spec")));
        assert!(body.contains("\"spec_dir\": \"spec\""));
    }

    /// The merge exists to leave a colleague's tasks alone.
    #[test]
    fn merging_preserves_the_tasks_already_there() {
        let text = r#"{"version":"2.0.0","tasks":[{"label":"watch: extension"}]}"#;
        let merged = merge_tasks(text, nao_tasks(3000), Path::new("tasks.json"), false)
            .unwrap()
            .unwrap();
        let labels: Vec<&str> = merged["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels[0], "watch: extension");
        assert!(labels.contains(&"Nao: Start web UI"));
    }

    #[test]
    fn merging_twice_adds_nothing() {
        let text = r#"{"version":"2.0.0","tasks":[]}"#;
        let once = merge_tasks(text, nao_tasks(3000), Path::new("t.json"), false)
            .unwrap()
            .unwrap();
        let again = merge_tasks(
            &once.to_string(),
            nao_tasks(3000),
            Path::new("t.json"),
            false,
        );
        assert!(
            again.unwrap().is_none(),
            "a second init duplicated the tasks"
        );
    }

    /// `--force` is how a reader picks up a changed task body without
    /// hand-editing, so the old one has to go rather than sit beside it.
    #[test]
    fn force_replaces_a_stale_nao_task() {
        let text = r#"{"tasks":[{"label":"Nao: Start web UI","command":"old"}]}"#;
        let merged = merge_tasks(text, nao_tasks(3000), Path::new("t.json"), true)
            .unwrap()
            .unwrap();
        let tasks = merged["tasks"].as_array().unwrap();
        let start: Vec<&Value> = tasks
            .iter()
            .filter(|t| t["label"] == "Nao: Start web UI")
            .collect();
        assert_eq!(
            start.len(),
            1,
            "the stale task survived alongside the new one"
        );
        assert_eq!(start[0]["command"], "nao");
    }

    /// A commented `tasks.json` is valid to VS Code and unparseable here.
    /// Failing is the point: the alternative is deleting the comments.
    #[test]
    fn a_commented_file_is_refused_rather_than_rewritten() {
        let text = "{\n  // the build\n  \"tasks\": []\n}";
        let err = merge_tasks(text, nao_tasks(3000), Path::new("t.json"), false).unwrap_err();
        assert!(err.to_string().contains("Left untouched"), "{err}");
    }

    #[test]
    fn every_generated_task_is_labelled_and_matcher_bearing() {
        for task in nao_tasks(3100) {
            assert!(task["label"]
                .as_str()
                .is_some_and(|l| l.starts_with("Nao: ")));
            assert!(
                !task["problemMatcher"].is_null(),
                "{task} would hang the task runner"
            );
        }
    }

    #[test]
    fn the_tasks_agree_on_the_port() {
        let tasks = nao_tasks(3100);
        assert!(tasks
            .iter()
            .any(|t| t.to_string().contains("localhost:3100")));
    }
}
