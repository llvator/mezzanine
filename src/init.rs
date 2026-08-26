//! `mezz init`: the files a repo would otherwise hand-write.
//!
//! Every key in [`crate::settings`] is optional, so a repo works with no
//! settings file at all. What it does not get without one is a *pinned*
//! language list — and the surfaces that matter most, `mezz watch` and the VS
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
//!
//! Two more files are written on request: `.vscode/tasks.json` (`--vscode`)
//! and `.mcp.json` (`--mcp`). Both already exist in most repos and belong to
//! their owner, so both merge into what is there rather than replace it.
//!
//! `.vscode/settings.json` is deliberately *not* among them. The extension's
//! `mezz.language` and `mezz.includeTests` reach the engine as CLI flags, and a
//! flag beats `.mezz/settings.json` — so scaffolding them would hand every
//! repo a second, uncommitted copy of its own configuration that silently
//! wins. `mezz.language` is worse still: one string against the pinned list,
//! so writing it would drop every other language from the extension's graph.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::analyzer::FileWalker;
use crate::config::Config;
use crate::models::file_info::Language;
use crate::settings;

/// The share of the walked tree a language must hold to be pinned.
///
/// A repo is not "a Python repo" because one `.py` release script lives in
/// `scripts/`, and pinning it there costs every later analysis the whole
/// Python parser for four files. A twentieth of the tree is the line: mezz's
/// own checkout puts Rust, TypeScript and Svelte over it and leaves the four
/// stray `.py` files under.
const MIN_SHARE: f64 = 0.05;

/// Which optional scaffolds this run was asked for.
///
/// A struct rather than a run of `bool` parameters because `--all` has to be
/// resolved into them *somewhere*, and doing it at the call site would put
/// the two `||` in `main`, where the complexity gate counts them against a
/// function that has nothing to do with scaffolding (CI-001).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Targets {
    pub vscode: bool,
    pub mcp: bool,
    /// Write the extra VS Code task that starts the engine with
    /// `--allow-agent-spawn`. Not a file of its own — a fourth task in the
    /// file `vscode` already writes — and deliberately not part of `--all`;
    /// see [`Targets::with_agent_spawn`].
    pub agent_spawn: bool,
}

impl Targets {
    /// `--all` is not a third scaffold. It is every scaffold below turned on
    /// at once, which is why it lives here: a new one added to this struct
    /// joins `--all` by being read here, not by anyone remembering to.
    pub fn new(vscode: bool, mcp: bool, all: bool) -> Self {
        Self {
            vscode: vscode || all,
            mcp: mcp || all,
            agent_spawn: false,
        }
    }

    /// Ask for the agent-spawn task as well.
    ///
    /// Separate from [`Targets::new`] for two reasons. It is the one scaffold
    /// `--all` does not turn on: `--all` means "every optional file", and a
    /// task that starts an engine willing to run Claude Code on this machine
    /// is not something a reader should acquire by asking for everything.
    /// Turning it on also implies `--vscode`, because the task has nowhere
    /// else to live — asking for it and getting silence would be the worse
    /// answer.
    pub fn with_agent_spawn(self, on: bool) -> Self {
        Self {
            vscode: self.vscode || on,
            agent_spawn: on,
            ..self
        }
    }
}

/// Scaffold `.mezz/settings.json`, plus whichever optional files were asked for.
pub fn run(root: &Path, targets: Targets, force: bool) -> Result<()> {
    write_settings(root, force)?;
    write_scaffolds(root, targets, force)
}

/// The files that are written only on request.
///
/// Split from [`run`] so that a fourth scaffold costs a branch here rather
/// than in the function every caller of this module goes through.
fn write_scaffolds(root: &Path, targets: Targets, force: bool) -> Result<()> {
    if targets.vscode {
        write_tasks(root, targets.agent_spawn, force)?;
    }
    if targets.mcp {
        write_mcp(root, force)?;
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
/// task already present is: `mezz init --vscode` in a repo that was set up
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
    // `mezz init src` must write `src/spec` rather than `spec`.
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
/// choosing it, and mezz's own checkout has more Markdown than Rust.
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

fn write_tasks(root: &Path, agent_spawn: bool, force: bool) -> Result<()> {
    let path = TASKS_PATH
        .iter()
        .fold(root.to_path_buf(), |p, part| p.join(part));
    let port = settings::load(root).port.unwrap_or(settings::DEFAULT_PORT);
    let tasks = [mezz_tasks(port), spawn_tasks(agent_spawn)].concat();

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
        println!("✓ {} already has the Mezzanine tasks", path.display());
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

/// Add the Mezzanine tasks to a `tasks.json` that already exists, or `None` when
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
        .filter(|task| !(force && is_mezz_task(task, &tasks)))
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

fn is_mezz_task(task: &Value, ours: &[Value]) -> bool {
    ours.iter().any(|our| our["label"] == task["label"])
}

/// Start, open, stop — the three things a reader does with the browser UI,
/// each pinned to the same port so they cannot disagree with each other.
///
/// `mezz watch` has no idle shutdown, so closing the browser tab leaves the
/// engine holding the port. The stop task exists because that surprises
/// everyone once.
fn mezz_tasks(port: u16) -> Vec<Value> {
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
        "label": "Mezzanine: Start web UI",
        "detail": "Analyse this repo and serve the browser UI",
        "type": "shell",
        "command": "mezz",
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
        "label": "Mezzanine: Open web UI in browser",
        "detail": format!("Opens http://localhost:{port}, starting the engine first"),
        "type": "shell",
        "command": opener(),
        "args": [format!("http://localhost:{port}")],
        "dependsOn": "Mezzanine: Start web UI",
        "presentation": { "reveal": "silent", "panel": "dedicated" },
        "problemMatcher": []
    })
}

fn stop_task(port: u16) -> Value {
    // Kills only a listener that *is* mezz: the port may have been taken by
    // something else entirely, and a task that kills whatever it finds is a
    // task that eventually kills a database.
    let command = format!(
        "PORT={port}; if [ -z \"$(lsof -ti tcp:$PORT -sTCP:LISTEN)\" ]; then \
         echo \"No process is listening on port $PORT — nothing to stop.\"; else \
         for p in $(lsof -ti tcp:$PORT -sTCP:LISTEN); do CMD=$(ps -o comm= -p $p); \
         if [ \"$(basename \"$CMD\")\" = \"mezz\" ]; then kill $p && \
         echo \"Stopped mezz engine on port $PORT (pid $p).\"; else \
         echo \"Port $PORT is held by pid $p ($CMD), which is not mezz — left alone.\"; \
         fi; done; fi"
    );
    json!({
        "label": "Mezzanine: Stop web UI",
        "detail": "Kills the engine holding this repo's port. Closing the browser does not stop it.",
        "type": "shell",
        "command": command,
        "options": { "cwd": "${workspaceFolder}" },
        "presentation": { "reveal": "always", "panel": "shared", "focus": false },
        "problemMatcher": []
    })
}

/// The agent-spawn task, or nothing.
///
/// A `Vec` rather than an `Option` so the caller concatenates instead of
/// branching, and so a second opt-in task later joins it here.
fn spawn_tasks(agent_spawn: bool) -> Vec<Value> {
    if agent_spawn {
        vec![spawn_task()]
    } else {
        Vec::new()
    }
}

/// Start the engine with `--allow-agent-spawn`, which is what makes the
/// quality tables offer a Refactor button.
///
/// A *second* task rather than a flag added to [`start_task`]: the route it
/// opens runs Claude Code on this machine, so the plain task has to stay the
/// one a reader gets by default, and picking the other one has to be a
/// deliberate act with the reason written on the label.
///
/// Built by amending [`start_task`] rather than repeating it. The background
/// `problemMatcher` is the part that must not drift — VS Code treats a
/// background task without one as never finishing — and two literals are how
/// it would.
fn spawn_task() -> Value {
    let mut task = start_task();
    task["label"] = json!("Mezzanine: Start web UI (agent spawn)");
    task["detail"] = json!(
        "Start the browser UI with --allow-agent-spawn: the quality tables \
         gain a Refactor button that opens a Claude Code terminal on this \
         machine. Binds the same port as 'Mezzanine: Start web UI', so stop that \
         one first."
    );
    task["args"] = json!(["watch", ".", "--allow-agent-spawn"]);
    task
}

fn opener() -> &'static str {
    match std::env::consts::OS {
        "macos" => "open",
        "windows" => "explorer",
        _ => "xdg-open",
    }
}

// ---------------------------------------------------------------------------
// MCP registration
// ---------------------------------------------------------------------------

const MCP_PATH: &str = ".mcp.json";

/// The key mezz registers itself under, and the name every tool call is
/// prefixed with on the agent's side. Changing it renames the tools.
const SERVER_NAME: &str = "mezz";

/// Register `mezz mcp` in the repo's `.mcp.json`, so an agent opened here has
/// the code graph without anybody wiring it up first.
///
/// Anchored on the repo root rather than the analyzed path: `.mcp.json` is
/// read by the agent from the checkout it was started in, so `mezz init --mcp
/// src` must still register the server where that agent will look for it.
fn write_mcp(root: &Path, force: bool) -> Result<()> {
    let path = settings::repo_root(root).join(MCP_PATH);
    let entry = mezz_server_entry();

    let merged = match std::fs::read_to_string(&path) {
        Ok(text) => match merge_mcp(&text, entry.clone(), &path, force) {
            Ok(merged) => merged,
            // Same bargain as the tasks file: we refuse to rewrite it, so we
            // owe the reader the block we would have written.
            Err(e) => return Err(offer_mcp_by_hand(e, &entry)),
        },
        Err(_) => Some(mcp_file(entry)),
    };

    let Some(file) = merged else {
        println!("✓ {} already registers mezz", path.display());
        return Ok(());
    };

    let body = format!("{}\n", serde_json::to_string_pretty(&file)?);
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
    println!("✓ wrote {}", path.display());
    println!("{}", indent(&body));
    warn_unless_on_path();
    Ok(())
}

/// Print the entry the merge refused to write, and hand back the error that
/// stopped it — the command still fails, because nothing was written.
fn offer_mcp_by_hand(error: anyhow::Error, entry: &Value) -> anyhow::Error {
    let body = serde_json::to_string_pretty(&mcp_file(entry.clone())).unwrap_or_default();
    eprintln!("   Add this by hand:\n{}", indent(&body));
    error
}

fn mcp_file(entry: Value) -> Value {
    json!({ "mcpServers": { SERVER_NAME: entry } })
}

/// How the agent starts the server.
///
/// The binary by name, never the absolute path this machine happens to have
/// it at: `.mcp.json` is committed, and a path out of one developer's home
/// directory registers a server nobody else can start. `mezz mcp` defaults its
/// root to the current directory, which is the checkout the agent runs in, so
/// the entry needs no path argument either — one less thing to be wrong after
/// the repo is moved or renamed.
fn mezz_server_entry() -> Value {
    json!({ "command": SERVER_NAME, "args": ["mcp"] })
}

/// The cost of naming the binary rather than pathing to it is that it has to
/// be findable, and the moment to say so is while the reader is looking at
/// what was just written — not the first time an agent reports no mezz tools.
fn warn_unless_on_path() {
    if on_path(SERVER_NAME) {
        return;
    }
    eprintln!(
        "   ⚠ `{SERVER_NAME}` is not on PATH. The entry names it anyway, because \
         {MCP_PATH} is committed and an absolute path would register a server \
         only this machine can start — install it (`cargo install --path .`), \
         or point `command` somewhere absolute in a checkout you do not share."
    );
}

fn on_path(binary: &str) -> bool {
    let name = format!("{binary}{}", std::env::consts::EXE_SUFFIX);
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(&name).is_file()))
}

/// Add mezz to a `.mcp.json` that already exists, or `None` when it is already
/// registered.
///
/// The refusals matter more than the merge. Another server in that file is
/// somebody's working setup, and the file is the only record of it — so
/// anything we cannot read as `{"mcpServers": {…}}` is left alone rather than
/// rewritten from a guess. Unlike `tasks.json` this one is strict JSON with no
/// comments allowed, so an unparseable file here really is broken; saying so
/// is still better than replacing it.
fn merge_mcp(text: &str, entry: Value, path: &Path, force: bool) -> Result<Option<Value>> {
    let mut file: Value = serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "{}: {e}. Left untouched — the servers already registered there \
             are worth more than the one we came to add.",
            path.display()
        )
    })?;

    let servers = servers_of(&mut file, path)?;
    // An entry that is already there is left as it is: a colleague may have
    // pointed `command` at a checkout-local build on purpose, and --force is
    // how you say you meant to replace that.
    if servers.contains_key(SERVER_NAME) && !force {
        return Ok(None);
    }
    servers.insert(SERVER_NAME.to_string(), entry);
    Ok(Some(file))
}

/// The `mcpServers` object, created when absent.
///
/// Written out rather than done with `file["mcpServers"][name] = …`, which
/// reads better and panics on a file whose top level — or whose `mcpServers`
/// — is not an object. Refusing those is the entire job.
fn servers_of<'a>(file: &'a mut Value, path: &Path) -> Result<&'a mut Map<String, Value>> {
    let Value::Object(map) = file else {
        bail!(
            "{}: the top level is not a JSON object. Left untouched.",
            path.display()
        );
    };
    match map.entry("mcpServers").or_insert_with(|| json!({})) {
        Value::Object(servers) => Ok(servers),
        _ => bail!(
            "{}: `mcpServers` is not an object. Left untouched.",
            path.display()
        ),
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
        assert_eq!(detect_spec_dir(root, &paths(&["/repo/mezz.elv"])), None);
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
        let merged = merge_tasks(text, mezz_tasks(3000), Path::new("tasks.json"), false)
            .unwrap()
            .unwrap();
        let labels: Vec<&str> = merged["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels[0], "watch: extension");
        assert!(labels.contains(&"Mezzanine: Start web UI"));
    }

    #[test]
    fn merging_twice_adds_nothing() {
        let text = r#"{"version":"2.0.0","tasks":[]}"#;
        let once = merge_tasks(text, mezz_tasks(3000), Path::new("t.json"), false)
            .unwrap()
            .unwrap();
        let again = merge_tasks(
            &once.to_string(),
            mezz_tasks(3000),
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
    fn force_replaces_a_stale_mezz_task() {
        let text = r#"{"tasks":[{"label":"Mezzanine: Start web UI","command":"old"}]}"#;
        let merged = merge_tasks(text, mezz_tasks(3000), Path::new("t.json"), true)
            .unwrap()
            .unwrap();
        let tasks = merged["tasks"].as_array().unwrap();
        let start: Vec<&Value> = tasks
            .iter()
            .filter(|t| t["label"] == "Mezzanine: Start web UI")
            .collect();
        assert_eq!(
            start.len(),
            1,
            "the stale task survived alongside the new one"
        );
        assert_eq!(start[0]["command"], "mezz");
    }

    /// A commented `tasks.json` is valid to VS Code and unparseable here.
    /// Failing is the point: the alternative is deleting the comments.
    #[test]
    fn a_commented_file_is_refused_rather_than_rewritten() {
        let text = "{\n  // the build\n  \"tasks\": []\n}";
        let err = merge_tasks(text, mezz_tasks(3000), Path::new("t.json"), false).unwrap_err();
        assert!(err.to_string().contains("Left untouched"), "{err}");
    }

    #[test]
    fn every_generated_task_is_labelled_and_matcher_bearing() {
        for task in [mezz_tasks(3100), spawn_tasks(true)].concat() {
            assert!(task["label"]
                .as_str()
                .is_some_and(|l| l.starts_with("Mezzanine: ")));
            assert!(
                !task["problemMatcher"].is_null(),
                "{task} would hang the task runner"
            );
        }
    }

    /// The task that runs code on this machine is not something a reader
    /// acquires by asking for the VS Code tasks.
    #[test]
    fn the_spawn_task_is_absent_unless_asked() {
        assert!(spawn_tasks(false).is_empty());
        let labels: Vec<String> = mezz_tasks(3000)
            .iter()
            .map(|t| t["label"].to_string())
            .collect();
        assert!(
            !labels.iter().any(|l| l.contains("agent spawn")),
            "{labels:?}"
        );
    }

    /// It carries the flag, and it stays a background task VS Code can tell
    /// has finished starting — which amending `start_task` is what buys.
    #[test]
    fn the_spawn_task_carries_the_flag_and_the_matcher() {
        let task = spawn_task();
        assert_eq!(task["args"], json!(["watch", ".", "--allow-agent-spawn"]));
        assert_eq!(task["problemMatcher"], start_task()["problemMatcher"]);
        assert_ne!(task["label"], start_task()["label"]);
    }

    /// Asking for the task without `--vscode` used to write nothing at all.
    #[test]
    fn asking_for_the_spawn_task_implies_the_vscode_scaffold() {
        let t = Targets::new(false, false, false).with_agent_spawn(true);
        assert!(t.vscode && t.agent_spawn);
        // `--all` is every optional *file*, not this.
        assert!(!Targets::new(false, false, true).agent_spawn);
    }

    #[test]
    fn the_tasks_agree_on_the_port() {
        let tasks = mezz_tasks(3100);
        assert!(tasks
            .iter()
            .any(|t| t.to_string().contains("localhost:3100")));
    }

    // -----------------------------------------------------------------------
    // MCP registration
    // -----------------------------------------------------------------------

    fn merged_servers(text: &str, force: bool) -> Map<String, Value> {
        merge_mcp(text, mezz_server_entry(), Path::new(MCP_PATH), force)
            .unwrap()
            .expect("a merge was expected")["mcpServers"]
            .as_object()
            .expect("mcpServers is an object")
            .clone()
    }

    /// The file is somebody's working setup before it is ours.
    #[test]
    fn registering_preserves_the_servers_already_there() {
        let text = r#"{"mcpServers":{"github":{"command":"gh-mcp","args":[]}}}"#;
        let servers = merged_servers(text, false);
        assert!(servers.contains_key("github"), "{servers:?}");
        assert_eq!(servers["mezz"], mezz_server_entry());
    }

    /// Keys outside `mcpServers` belong to whoever put them there.
    #[test]
    fn registering_preserves_the_rest_of_the_file() {
        let text = r#"{"$schema":"https://example.test/mcp.json","mcpServers":{}}"#;
        let merged = merge_mcp(text, mezz_server_entry(), Path::new(MCP_PATH), false)
            .unwrap()
            .expect("a merge was expected");
        assert_eq!(merged["$schema"], "https://example.test/mcp.json");
    }

    #[test]
    fn a_file_without_the_servers_key_gains_one() {
        assert_eq!(merged_servers("{}", false)["mezz"], mezz_server_entry());
    }

    /// Re-running `mezz init --mcp` must be a no-op, not a second entry and
    /// not a silent overwrite of an entry someone tuned by hand.
    #[test]
    fn registering_twice_changes_nothing() {
        let text = r#"{"mcpServers":{"mezz":{"command":"/opt/mezz","args":["mcp"]}}}"#;
        let again = merge_mcp(text, mezz_server_entry(), Path::new(MCP_PATH), false).unwrap();
        assert!(again.is_none(), "{again:?}");
    }

    #[test]
    fn force_replaces_a_hand_edited_entry() {
        let text = r#"{"mcpServers":{"mezz":{"command":"/opt/mezz","args":["mcp"]}}}"#;
        assert_eq!(merged_servers(text, true)["mezz"], mezz_server_entry());
    }

    #[test]
    fn an_unparseable_file_is_refused_rather_than_rewritten() {
        let err = merge_mcp("{oops", mezz_server_entry(), Path::new(MCP_PATH), false).unwrap_err();
        assert!(err.to_string().contains("Left untouched"), "{err}");
    }

    /// The two shapes that would panic if the entry were written with
    /// `file["mcpServers"][name] = …` instead of [`servers_of`].
    #[test]
    fn a_file_that_is_not_an_object_is_refused() {
        for text in ["[]", r#"{"mcpServers":[]}"#] {
            let err = merge_mcp(text, mezz_server_entry(), Path::new(MCP_PATH), false).unwrap_err();
            assert!(err.to_string().contains("Left untouched"), "{text}: {err}");
        }
    }

    /// A committed file cannot name a path only one machine has.
    #[test]
    fn the_entry_names_the_binary_rather_than_a_path() {
        let entry = mezz_server_entry();
        assert_eq!(entry["command"], "mezz");
        assert_eq!(entry["args"], json!(["mcp"]));
    }

    #[test]
    fn all_turns_on_every_scaffold() {
        assert_eq!(
            Targets::new(false, false, true),
            Targets {
                vscode: true,
                mcp: true,
                // Every optional *file*. The agent-spawn task is not one.
                agent_spawn: false
            }
        );
        assert_eq!(Targets::new(false, false, false), Targets::default());
        assert_eq!(
            Targets::new(true, false, false),
            Targets {
                vscode: true,
                mcp: false,
                agent_spawn: false
            }
        );
    }
}
