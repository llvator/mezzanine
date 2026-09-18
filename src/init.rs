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
//! - Never a key it cannot justify from the repo. `debounce_ms` is the plain
//!   case: nothing here reads it, and `300` in the file is only today's
//!   default frozen.
//!
//! `output_dir` and `port` used to sit under that second limit and no longer
//! do, because this command does not write one file. `write_tasks` bakes the
//! port into three places in `.vscode/tasks.json` and those tasks write to
//! `output_dir` — a value a scaffold depends on is justified by the scaffold,
//! whatever the tree says. Both are pinned relative to `.mezz/`, so what this
//! writes stays inside the directory it just made; the absolute `output_dir`
//! copied between checkouts is the wart this repo's own file carries, and is
//! what "relative" is guarding against. See CFG-016.
//!
//! Three more files are written on request: `.vscode/tasks.json` (`--vscode`),
//! `.mcp.json` (`--mcp`) and `.claude/settings.json` (`--hooks`). All three
//! already exist in most repos and belong to their owner, so all three merge
//! into what is there rather than replace it.
//!
//! `--hooks` is the one that changes what happens rather than what is
//! available: it wires the push-mode `Stop` hooks, so a turn that introduced a
//! structural regression is blocked once and handed the finding. `--mcp` gives
//! an agent tools it must remember to call; this is the half that arrives
//! whether or not it remembered.
//!
//! Two flags add tasks to the file `--vscode` writes rather than a file of
//! their own, and both sit outside `--all`. `--allow-agent-spawn` is held
//! back because of what its task does; `--editor-tools` because of how many
//! there are — eleven graph tools bound to the editor's open file, in a list
//! the reader shares with their own builds. See [`EDITOR_TOOLS`].
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
    pub hooks: bool,
    /// Write the extra VS Code task that starts the engine with
    /// `--allow-agent-spawn`. Not a file of its own — a fourth task in the
    /// file `vscode` already writes — and deliberately not part of `--all`;
    /// see [`Targets::with_agent_spawn`].
    pub agent_spawn: bool,
    /// Write the [`EDITOR_TOOLS`] tasks — one graph tool each, scoped to the
    /// editor's open file. Like `agent_spawn`, more tasks in the file
    /// `vscode` writes rather than a file of its own, and outside `--all`;
    /// see [`Targets::with_editor_tools`].
    pub editor_tools: bool,
}

impl Targets {
    /// `--all` is not a third scaffold. It is every scaffold below turned on
    /// at once, which is why it lives here: a new one added to this struct
    /// joins `--all` by being read here, not by anyone remembering to.
    pub fn new(vscode: bool, mcp: bool, hooks: bool, all: bool) -> Self {
        Self {
            vscode: or_all(vscode, all),
            mcp: or_all(mcp, all),
            hooks: or_all(hooks, all),
            agent_spawn: false,
            editor_tools: false,
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

    /// Ask for the editor-scoped tool tasks as well.
    ///
    /// Outside `--all` for a different reason than [`Targets::with_agent_spawn`]:
    /// nothing here runs anything the plain tasks don't, so the objection is
    /// not safety but volume. Eleven entries land in a list a reader shares
    /// with their own builds and tests, and a scaffold that triples the
    /// length of that list because it was asked for "everything" is one
    /// people stop running. Implies `--vscode` for the same reason the
    /// spawn task does: there is nowhere else for a task to live.
    pub fn with_editor_tools(self, on: bool) -> Self {
        Self {
            vscode: self.vscode || on,
            editor_tools: on,
            ..self
        }
    }
}

/// One scaffold's own flag, with `--all` folded in.
///
/// A function rather than a `||` per field: the complexity gate counts each
/// one against [`Targets::new`] and fails on any increase, so a fourth
/// scaffold has to cost a row rather than a branch (CI-001).
const fn or_all(asked: bool, all: bool) -> bool {
    asked || all
}

/// Scaffold `.mezz/settings.json`, plus whichever optional files were asked for.
pub fn run(root: &Path, targets: Targets, force: bool) -> Result<()> {
    write_settings(root, force)?;
    write_scaffolds(root, targets, force)
}

/// Every optional scaffold: whether this run asked for it, and what it writes.
///
/// A table rather than an `if` per target. In a flat run of branches the
/// cyclomatic count *is* the number of scaffolds, so each one added pushed
/// this function higher and the gate — which fails on any increase to an
/// existing function — would have made the next one unlandable. `check/rules.rs`
/// and `mcp/mod.rs` hit the same wall and answered it the same way (CI-001).
///
/// This is also the single list: [`Targets::new`] decides the flags, and a
/// scaffold joins `mezz init` by gaining a row here rather than by anyone
/// remembering to call it.
const OPTIONAL: &[(fn(&Targets) -> bool, fn(&Path, Targets, bool) -> Result<()>)] = &[
    (|t| t.vscode, write_tasks),
    (|t| t.mcp, |root, _, force| write_mcp(root, force)),
    (|t| t.hooks, |root, _, force| write_hooks(root, force)),
];

/// The files that are written only on request.
///
/// Split from [`run`] so that a fourth scaffold costs a row in [`OPTIONAL`]
/// rather than a branch in the function every caller of this module goes
/// through.
fn write_scaffolds(root: &Path, targets: Targets, force: bool) -> Result<()> {
    // Filtered rather than a `for` with an `if` inside it: that nests one
    // deeper than the loop this replaced, and the gate scores nesting too.
    OPTIONAL
        .iter()
        .filter(|(wanted, _)| wanted(&targets))
        .try_for_each(|(_, write)| write(root, targets, force))
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

    // Read before the write that replaces it, so `--force` can put back the
    // keys the reader chose rather than the tree implied.
    let previous = settings::read_raw(&path).unwrap_or_default();
    // `spec_dir` is detected against the directory the file lands in, not the
    // one that was walked: it is read back relative to the repo root
    // (CFG-012), so `mezz init src` must write `src/spec` rather than `spec`.
    let inferred = settings_body(&languages, detect_spec_dir(&settings::repo_root(root), &files));
    let body = carry_over(inferred, &previous);
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

/// The keys `--force` leaves alone.
///
/// Everything else this command writes is *inferred* — re-run it after a repo
/// changes shape and a fresh language list is the whole point. These two are
/// not inferred from anything; they are a choice, and `port` in particular is
/// one a reader has to make the moment they open a second repo beside this
/// one. Resetting it would also undo it in `.vscode/tasks.json`, which is
/// rewritten from this file in the same run — so a reader who moved the port
/// and re-scaffolded the tasks would silently get the old number back in both
/// places (CFG-016).
const CARRIED_ACROSS_FORCE: [&str; 2] = ["output_dir", "port"];

/// Put back whatever [`CARRIED_ACROSS_FORCE`] key the file being replaced
/// already had.
///
/// `previous` is the raw map of that file, and the caller reads it with
/// `unwrap_or_default` on purpose: a file that is absent, unreadable or not
/// JSON carries nothing over, which is the same answer as an empty one. This
/// runs on the way to *replacing* that file, so refusing to proceed over a
/// parse failure would strand the reader with the broken file `--force` was
/// reached for.
fn carry_over(body: String, previous: &Map<String, Value>) -> String {
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&body) else {
        return body;
    };
    for key in CARRIED_ACROSS_FORCE {
        if let Some(value) = previous.get(key) {
            map.insert(key.to_string(), value.clone());
        }
    }
    format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(map)).unwrap_or(body)
    )
}

/// Where a scaffolded repo writes its graph JSON.
///
/// Under `.mezz/`, the directory this command has just created, rather than
/// the built-in `ui/public` default: that one is a guess about the reader's
/// tree, and a repo whose root already holds a file called `ui` fails on the
/// first `mezz watch` with nothing but `ENOTDIR` to go on (CFG-016).
const SCAFFOLD_OUTPUT_DIR: &str = ".mezz/data";

fn settings_body(languages: &[Language], spec_dir: Option<PathBuf>) -> String {
    let names: Vec<&str> = languages.iter().map(|l| l.filter_name()).collect();
    // Two keys that are defaults everywhere else and pinned here, against the
    // module doc's general rule, because this command writes a *second* file
    // that reads them. `write_tasks` bakes the port into three places in
    // `tasks.json`; leaving it out of the settings file puts the number
    // somewhere the reader cannot change it. `output_dir` is where those
    // tasks write, and its default can collide with the repo (CFG-016).
    let mut body = json!({
        "language": names,
        "output_dir": SCAFFOLD_OUTPUT_DIR,
        "port": settings::DEFAULT_PORT,
    });
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

fn write_tasks(root: &Path, targets: Targets, force: bool) -> Result<()> {
    let path = TASKS_PATH
        .iter()
        .fold(root.to_path_buf(), |p, part| p.join(part));
    let port = settings::load(root).port.unwrap_or(settings::DEFAULT_PORT);
    let tasks = [
        mezz_tasks(port),
        spawn_tasks(targets.agent_spawn),
        editor_tool_tasks(targets.editor_tools),
    ]
    .concat();

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
// Editor-scoped tool tasks
// ---------------------------------------------------------------------------

/// One graph tool, bound to whatever the editor has open.
///
/// The `mezz` argv is stored whole rather than assembled from a tool name
/// and a scope kind. The sixteen tools do not take their target the same
/// way — `quality` takes a positional path, `impact` takes `--path`, and
/// `--line` turns `impact` from a file report into an entity one — so a
/// scope enum would have had a variant per spelling and bought nothing over
/// writing the arguments down.
struct EditorTool {
    /// What the reader picks from the task list, after the `Mezzanine: `
    /// prefix every task in this file carries.
    label: &'static str,
    detail: &'static str,
    /// Passed to `mezz` as-is. VS Code substitutes the `${...}` variables
    /// before the process starts, so mezz never sees one.
    args: &'static [&'static str],
}

/// The file the editor has open, relative to its workspace folder — which is
/// exactly how the tools read a path, since `Scope` resolves `path`
/// arguments against the root.
const THIS_FILE: &str = "${relativeFile}";

/// Its folder, for the tools that grade an area rather than a file.
const THIS_FOLDER: &str = "${relativeFileDirname}";

/// The cursor's line, 1-based — which is the base `--line` counts from.
const THIS_LINE: &str = "${lineNumber}";

/// The graph tools worth a keystroke, in the three families they fall into:
/// what the open file *is*, what the entity under the cursor is, and what
/// the folder around it looks like.
///
/// Four of the sixteen are missing because the editor has no answer to give
/// them. `overview`, `similar` and `trace` take a name, a query or a pair of
/// entities — a task would have to prompt for it, which is slower than
/// typing the command. `assess_change` takes a git ref and is about the
/// working tree, not about any one file. `spec_slice` writes a file and
/// needs a repo with an Elevator spec, so it is a command to run
/// deliberately rather than one to bind to the cursor.
const EDITOR_TOOLS: &[EditorTool] = &[
    EditorTool {
        label: "Map this file",
        detail: "Entities in the open file with their metrics and coupling",
        args: &["map", THIS_FILE],
    },
    EditorTool {
        label: "Quality of this file",
        detail: "Smells and refactor pressure in the open file",
        args: &["quality", THIS_FILE],
    },
    EditorTool {
        label: "Dead code in this file",
        detail: "Entities in the open file that nothing references",
        args: &["dead-code", THIS_FILE],
    },
    EditorTool {
        label: "What depends on this file",
        detail: "Who breaks if the open file changes, and what it owes the rest of the tree",
        args: &["impact", "--path", THIS_FILE],
    },
    EditorTool {
        label: "Impact of the entity at the cursor",
        detail: "Blast radius of the entity the cursor is inside",
        args: &["impact", "--path", THIS_FILE, "--line", THIS_LINE],
    },
    EditorTool {
        label: "Cost of the entity at the cursor",
        detail: "How the entity the cursor is inside scales, and what it calls — worst-case time complexity along the call chain",
        args: &["cost", "--path", THIS_FILE, "--line", THIS_LINE],
    },
    EditorTool {
        label: "Context for the entity at the cursor",
        detail: "The minimal pack needed to edit the entity the cursor is inside",
        args: &["context", "--path", THIS_FILE, "--line", THIS_LINE],
    },
    EditorTool {
        label: "Tests covering the entity at the cursor",
        detail: "Which tests reach the entity the cursor is inside, directly or transitively",
        args: &["tests-for", "--path", THIS_FILE, "--line", THIS_LINE],
    },
    EditorTool {
        label: "Reshape this file's folder",
        detail: "The one change that would improve the structure of the folder around the open file",
        args: &["reshape", THIS_FOLDER],
    },
    EditorTool {
        label: "Layout of this file's folder",
        detail: "Where this folder's files would sit if its dependency drawing decided",
        args: &["layout", THIS_FOLDER],
    },
    EditorTool {
        label: "Boundaries of this file's folder",
        detail: "Which of this folder's imports reach past another folder's door",
        args: &["boundaries", THIS_FOLDER],
    },
    EditorTool {
        label: "Hotspots in this file's folder",
        detail: "This folder ranked by git churn × complexity",
        args: &["hotspots", THIS_FOLDER],
    },
];

/// The [`EDITOR_TOOLS`] tasks, or nothing.
fn editor_tool_tasks(editor_tools: bool) -> Vec<Value> {
    if !editor_tools {
        return Vec::new();
    }
    EDITOR_TOOLS.iter().map(editor_tool_task).collect()
}

fn editor_tool_task(tool: &EditorTool) -> Value {
    json!({
        "label": format!("Mezzanine: {}", tool.label),
        "detail": tool.detail,
        "type": "shell",
        "command": "mezz",
        "args": tool.args,
        // `${fileWorkspaceFolder}` rather than the `${workspaceFolder}` the
        // web-UI tasks use, because these three variables disagree in a
        // multi-root workspace: `${relativeFile}` is relative to the folder
        // holding the open file, and `${workspaceFolder}` is the first one
        // in the workspace. Pairing them would analyze one repo and hand it
        // a path into another. It also fails loudly rather than quietly when
        // the open file belongs to no workspace folder at all.
        "options": { "cwd": "${fileWorkspaceFolder}" },
        // `clear` because every run of these replaces the last one's answer
        // rather than continuing it, and a panel holding a report about the
        // file you just navigated away from is the way to misread one.
        "presentation": {
            "reveal": "always", "panel": "dedicated", "focus": false, "clear": true
        },
        "problemMatcher": tool_matcher(tool.args[0])
    })
}

/// The Problems-panel matcher for a tool whose output carries `file:line`,
/// or the empty list every other task here uses.
///
/// Only `quality` gets one, and only over its `## Smells` section. Two
/// conditions have to hold for a matcher to be honest, and this is the one
/// place both do:
///
/// - **Every row it matches is in a file VS Code can find.** `quality`
///   prints paths relative to the scope it was asked about
///   ([tools.rs](../mcp/tools.rs) `entity_row`), and this task's scope is
///   the open file — so a bare `init.rs:370` resolves against
///   `${fileDirname}` and nowhere else. The entity-level tools print
///   root-relative paths instead, which would need a different base.
/// - **A row is a finding.** Smells are. `map`'s entity list and `quality`'s
///   own refactor-pressure ranking are not: that ranking opens by saying it
///   is "a ranking, not a verdict", and piping it into the Problems panel
///   would contradict the sentence above it. The regex requires the row to
///   end at the `⚠` marker, which the pressure rows never do — they carry a
///   `N of it identified` tail.
///
/// A drift in that row format costs an empty Problems panel and no wrong
/// answer, which is the failure direction to pick when tying one file's
/// regex to another file's `format!`.
fn tool_matcher(tool: &str) -> Value {
    if tool != "quality" {
        return json!([]);
    }
    json!({
        "owner": "mezz",
        "source": "mezz",
        "severity": "warning",
        "fileLocation": ["relative", "${fileDirname}"],
        "pattern": {
            "regexp": "^- \\w+ `[^`]+` — ([^ :]+):(\\d+) \\(.*⚠ ([^,)]+)\\)$",
            "file": 1,
            "line": 2,
            "message": 3
        }
    })
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

// ------------------------------------------------------------------
//  Claude Code Stop hooks (--hooks)
// ------------------------------------------------------------------

const CLAUDE_SETTINGS_PATH: [&str; 2] = [".claude", "settings.json"];

/// What marks a hook entry as one of ours, on a re-run and on `--force`.
///
/// The subcommand rather than the whole command line: the wrapper around it
/// is the part most likely to be edited by hand, and a reader who tuned their
/// redirection should not end up with a second copy of the same leg.
const HOOK_MARKER: &str = "mezz hook ";

/// Long enough for two full analyses of a large repo, short enough that a
/// wedged hook does not hold a session. Mezzanine's own checkout — ~16.7k
/// entities analyzed twice — takes about two seconds.
const HOOK_TIMEOUT_SECS: u64 = 120;

/// Wire the two push-mode legs into `.claude/settings.json` as `Stop` hooks.
///
/// The other scaffolds hand an agent things it must remember to use: `--mcp`
/// registers tools it can call, `--vscode` adds tasks a human can run. This
/// one is the only scaffold that arrives on its own — an agent that never
/// calls `assess_change` still cannot end a turn on a cycle it just closed.
///
/// Anchored on the repo root, like `.mcp.json`: Claude Code reads this file
/// from the checkout it was started in, so `mezz init --hooks src` must still
/// write where that agent will look.
fn write_hooks(root: &Path, force: bool) -> Result<()> {
    let path = CLAUDE_SETTINGS_PATH
        .iter()
        .fold(settings::repo_root(root), |p, part| p.join(part));
    let group = hook_group();

    let merged = match std::fs::read_to_string(&path) {
        Ok(text) => match merge_hooks(&text, group.clone(), &path, force) {
            Ok(merged) => merged,
            // Same bargain as the other two: we refuse to rewrite the file,
            // so we owe the reader the block we would have written.
            Err(e) => return Err(offer_hooks_by_hand(e, &group)),
        },
        Err(_) => Some(json!({ "hooks": { "Stop": [group] } })),
    };

    let Some(file) = merged else {
        println!("✓ {} already runs the mezz hooks", path.display());
        return Ok(());
    };

    std::fs::create_dir_all(path.parent().unwrap_or(root))
        .with_context(|| format!("creating {}", path.display()))?;
    let body = format!("{}\n", serde_json::to_string_pretty(&file)?);
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
    println!("✓ wrote {}", path.display());
    println!(
        "   A stop that introduced a new smell, cycle, complexity jump, folder-shape \
         fall or rule breach now blocks once, with the finding, so the agent can fix \
         it before the turn ends. Silent otherwise."
    );
    warn_unless_hooks_can_run();
    Ok(())
}

/// Print the block the merge refused to write, and hand back the error that
/// stopped it — the command still fails, because nothing was written.
fn offer_hooks_by_hand(error: anyhow::Error, group: &Value) -> anyhow::Error {
    let body =
        serde_json::to_string_pretty(&json!({ "hooks": { "Stop": [group] } })).unwrap_or_default();
    eprintln!("   Add this by hand:\n{}", indent(&body));
    error
}

/// The two legs, as one `Stop` group.
fn hook_group() -> Value {
    json!({
        "hooks": [
            hook_leg("self-review", "mezz self-review"),
            hook_leg("check", "mezz rule check"),
        ]
    })
}

fn hook_leg(sub: &str, status: &str) -> Value {
    json!({
        "type": "command",
        "command": hook_command(sub),
        "timeout": HOOK_TIMEOUT_SECS,
        "statusMessage": status,
    })
}

/// The shell around the hook, and why it is not just the bare command.
///
/// Three things have to happen that the binary cannot do for itself:
///
/// - **Drop the progress chatter.** `mezz` writes `Analyzing base ...` to
///   stderr and its findings to stdout, precisely so a caller can discard one
///   without the other. `2>/dev/null` is that discard.
/// - **Put the findings where a blocked stop reads them.** A `Stop` hook that
///   exits 0 sends stdout to a debug log — not the transcript, and never the
///   agent. Exit 2 blocks the stop and feeds back *stderr*, so the findings
///   are re-emitted there.
/// - **Let only exit 2 through.** A mezz that is missing, half-built or
///   erroring exits something else, and that must end the hook quietly rather
///   than wedge every stop in the repo.
fn hook_command(sub: &str) -> String {
    format!(
        "o=$(mezz hook {sub} --block 2>/dev/null); r=$?; \
         [ -n \"$o\" ] && echo \"$o\" >&2; [ $r -eq 2 ] && exit 2; exit 0"
    )
}

/// Both ways the wrapper can be written correctly and still never run.
fn warn_unless_hooks_can_run() {
    if !on_path("mezz") {
        eprintln!(
            "   ⚠ `mezz` is not on PATH. The hooks name it anyway, because \
             .claude/settings.json is committed and an absolute path out of one \
             developer's home directory is a hook only that machine can run — \
             install it with `cargo install --path .`."
        );
    }
    if std::env::consts::OS == "windows" {
        eprintln!(
            "   ⚠ The wrapper is POSIX shell. On Windows, run it under Git Bash or \
             WSL, or rewrite the command for your shell — the parts that matter are \
             `--block`, stderr for the findings, and exit 2."
        );
    }
}

/// Add the group to a `.claude/settings.json` that already exists, or `None`
/// when the hooks are already wired.
///
/// The refusals are the point, as in [`merge_mcp`]. That file holds a
/// reader's permission allowlist and whatever other hooks they run, and it is
/// the only record of both — so anything we cannot parse is left alone rather
/// than rewritten from a guess.
///
/// `--force` replaces our own group and nobody else's: the retain drops only
/// entries carrying [`HOOK_MARKER`], so a repo with its own `Stop` hook keeps
/// it either way.
fn merge_hooks(text: &str, group: Value, path: &Path, force: bool) -> Result<Option<Value>> {
    let mut file: Value = serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "{}: {e}. Left untouched — the permissions and hooks already there \
             are worth more than the ones we came to add.",
            path.display()
        )
    })?;

    let groups = stop_groups_of(&mut file, path)?;
    if groups.iter().any(is_mezz_group) && !force {
        return Ok(None);
    }
    groups.retain(|g| !is_mezz_group(g));
    groups.push(group);
    Ok(Some(file))
}

/// The `hooks.Stop` array, created when absent.
///
/// Written out rather than indexed into, for [`servers_of`]'s reason: a file
/// whose `hooks` is a string should be refused, not panicked on.
fn stop_groups_of<'a>(file: &'a mut Value, path: &Path) -> Result<&'a mut Vec<Value>> {
    let Value::Object(map) = file else {
        bail!(
            "{}: the top level is not a JSON object. Left untouched.",
            path.display()
        );
    };
    let hooks = match map.entry("hooks").or_insert_with(|| json!({})) {
        Value::Object(hooks) => hooks,
        _ => bail!("{}: `hooks` is not an object. Left untouched.", path.display()),
    };
    match hooks.entry("Stop").or_insert_with(|| json!([])) {
        Value::Array(groups) => Ok(groups),
        _ => bail!(
            "{}: `hooks.Stop` is not an array. Left untouched.",
            path.display()
        ),
    }
}

fn is_mezz_group(group: &Value) -> bool {
    group["hooks"].as_array().is_some_and(|legs| {
        legs.iter()
            .any(|leg| leg["command"].as_str().is_some_and(|c| c.contains(HOOK_MARKER)))
    })
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

    /// Nothing beyond the inferred language and the two keys the scaffold's
    /// own second file depends on. `debounce_ms`, `max_depth` and the rest
    /// stay absent: written out they would only freeze today's defaults.
    #[test]
    fn the_body_carries_what_was_inferred_and_what_the_scaffold_needs() {
        let body = settings_body(&[Language::Rust], None);
        assert_eq!(
            body,
            "{\n  \"language\": [\n    \"rust\"\n  ],\n  \
             \"output_dir\": \".mezz/data\",\n  \"port\": 3000\n}\n"
        );
    }

    /// The reported failure (CFG-016): the scaffolded `output_dir` must stay
    /// under `.mezz/`, the directory init just made. `ui/public` is a guess
    /// about the reader's tree, and a repo with a file called `ui` at its
    /// root fails the first `mezz watch` on it.
    #[test]
    fn the_scaffolded_output_dir_stays_inside_the_mezz_directory() {
        let body: Value = serde_json::from_str(&settings_body(&[Language::Rust], None)).unwrap();
        let dir = body["output_dir"].as_str().unwrap();
        assert!(Path::new(dir).is_relative(), "{dir} escapes the checkout");
        assert!(dir.starts_with(".mezz/"), "{dir} is outside .mezz/");
    }

    /// `write_tasks` bakes the port into `tasks.json` by reading it back from
    /// the file `write_settings` has just written. If the two disagree, the
    /// Open task points at one engine and the Stop task kills another.
    #[test]
    fn the_scaffolded_port_is_the_one_the_tasks_are_built_from() {
        let body: Value = serde_json::from_str(&settings_body(&[Language::Rust], None)).unwrap();
        assert_eq!(body["port"].as_u64(), Some(u64::from(settings::DEFAULT_PORT)));
    }

    fn previous(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    /// `--force` is for re-inferring a language list after the repo changed
    /// shape. A port the reader picked is not an inference, and resetting it
    /// would put the old number back into `tasks.json` too.
    #[test]
    fn force_keeps_the_port_and_output_dir_the_reader_chose() {
        let body = carry_over(
            settings_body(&[Language::Rust], None),
            &previous(r#"{"port":3456,"output_dir":"build/graph","language":["go"]}"#),
        );
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["port"].as_u64(), Some(3456));
        assert_eq!(parsed["output_dir"].as_str(), Some("build/graph"));
        // The language list is exactly what `--force` was reached for, so it
        // is the one thing that must *not* survive.
        assert_eq!(parsed["language"][0].as_str(), Some("rust"));
    }

    /// The first `mezz init` in a repo has nothing to carry, and must still
    /// come out with the scaffolded defaults rather than no keys at all.
    #[test]
    fn a_first_run_carries_nothing_and_keeps_the_scaffolded_values() {
        let body = carry_over(settings_body(&[Language::Rust], None), &Map::new());
        assert_eq!(body, settings_body(&[Language::Rust], None));
    }

    /// A key this command does not own is not preserved: `--force` replaces
    /// the file, and pretending otherwise would make it a merge nobody asked
    /// for.
    #[test]
    fn force_carries_nothing_beyond_the_two_named_keys() {
        let body = carry_over(
            settings_body(&[Language::Rust], None),
            &previous(r#"{"max_depth":9,"port":3456}"#),
        );
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert!(parsed.get("max_depth").is_none());
        assert_eq!(parsed["port"].as_u64(), Some(3456));
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
        for task in [
            mezz_tasks(3100),
            spawn_tasks(true),
            editor_tool_tasks(true),
        ]
        .concat()
        {
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
        let t = Targets::new(false, false, false, false).with_agent_spawn(true);
        assert!(t.vscode && t.agent_spawn);
        // `--all` is every optional *file*, not this.
        assert!(!Targets::new(false, false, false, true).agent_spawn);
    }

    /// Same two properties as the spawn task, for the same two reasons: it
    /// has nowhere to live without `--vscode`, and it is volume rather than
    /// a file, so `--all` must not pick it up.
    #[test]
    fn asking_for_the_editor_tools_implies_the_vscode_scaffold() {
        let t = Targets::new(false, false, false, false).with_editor_tools(true);
        assert!(t.vscode && t.editor_tools);
        assert!(!Targets::new(false, false, false, true).editor_tools);
        assert!(editor_tool_tasks(false).is_empty());
    }

    /// The whole point of these tasks is that they are *scoped*. One that
    /// mentioned no editor variable would silently analyze the whole
    /// repository under a label promising the open file — the failure a
    /// reader would take longest to notice, because it still prints a
    /// plausible report.
    #[test]
    fn every_editor_tool_names_the_open_file_or_its_folder() {
        for task in editor_tool_tasks(true) {
            let args = task["args"].to_string();
            assert!(
                args.contains(THIS_FILE) || args.contains(THIS_FOLDER),
                "{} is not scoped to the editor: {args}",
                task["label"]
            );
            // A path relative to the wrong root is the other way to get a
            // plausible report about the wrong tree.
            assert_eq!(task["options"]["cwd"], json!("${fileWorkspaceFolder}"));
        }
    }

    /// `--line` is what turns `impact` from a report about the file into one
    /// about the entity spanning a line, so the two tasks differ by exactly
    /// that and must not drift into being the same call under two labels.
    #[test]
    fn the_file_and_cursor_impact_tasks_ask_different_questions() {
        let tasks = editor_tool_tasks(true);
        let args = |label: &str| {
            tasks
                .iter()
                .find(|t| t["label"] == json!(format!("Mezzanine: {label}")))
                .unwrap_or_else(|| panic!("no task labelled {label}"))["args"]
                .clone()
        };
        assert_eq!(
            args("What depends on this file"),
            json!(["impact", "--path", THIS_FILE])
        );
        assert_eq!(
            args("Impact of the entity at the cursor"),
            json!(["impact", "--path", THIS_FILE, "--line", THIS_LINE])
        );
    }

    /// Samples of the two row kinds `quality` prints, copied from a real run
    /// over `src/mcp/tools.rs`. The matcher has to take the first and leave
    /// the second: a "ranking, not a verdict" in the Problems panel is a
    /// verdict, whatever the paragraph above it says.
    const A_SMELL_ROW: &str =
        "- function `trace` — tools.rs:2249 (L2249, loc 114, cx 18, cog 61, ws 18, in 1, \
         out 50, ⚠ Overfull Head)";
    const A_PRESSURE_ROW: &str =
        "- [1.44] function `trace` — tools.rs:2249 (L2249, loc 114, cx 18, cog 61, ws 18, \
         in 1, out 50, ⚠ Overfull Head, 6 of it identified)";

    /// The one test that would catch `entity_row` in `mcp/tools.rs` drifting
    /// away from the regex written here. Without it the drift shows up as an
    /// empty Problems panel, which reads exactly like a clean file.
    #[test]
    fn the_quality_matcher_takes_smells_and_leaves_the_ranking() {
        let matcher = tool_matcher("quality");
        let pattern = matcher["pattern"]["regexp"].as_str().unwrap();
        let re = regex::Regex::new(pattern).unwrap();

        let caught = re
            .captures(A_SMELL_ROW)
            .unwrap_or_else(|| panic!("{pattern} no longer matches a smell row"));
        assert_eq!(&caught[1], "tools.rs");
        assert_eq!(&caught[2], "2249");
        assert_eq!(&caught[3], "Overfull Head");

        assert!(
            !re.is_match(A_PRESSURE_ROW),
            "the refactor-pressure ranking would land in the Problems panel"
        );
    }

    /// A matcher that resolves `tools.rs:2249` has to say what folder to
    /// resolve it against, and the only folder that is right is the one
    /// holding the file the task was scoped to.
    #[test]
    fn only_quality_carries_a_matcher_and_it_is_anchored_to_the_open_folder() {
        assert_eq!(
            tool_matcher("quality")["fileLocation"],
            json!(["relative", "${fileDirname}"])
        );
        for tool in ["map", "impact", "reshape", "hotspots"] {
            assert_eq!(tool_matcher(tool), json!([]), "{tool} grew a matcher");
        }
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

    // -----------------------------------------------------------------------
    // Claude Code hooks
    // -----------------------------------------------------------------------

    fn merged_stop(text: &str, force: bool) -> Vec<Value> {
        merge_hooks(text, hook_group(), Path::new("settings.json"), force)
            .unwrap()
            .expect("a merge was expected")["hooks"]["Stop"]
            .as_array()
            .expect("Stop is an array")
            .clone()
    }

    /// The three things the wrapper has to do, none of which the binary can do
    /// for itself. Asserted on the string because the string is the contract:
    /// this is what Claude Code executes.
    #[test]
    fn the_wrapper_blocks_routes_and_fails_safe() {
        let cmd = hook_command("self-review");
        assert!(cmd.contains("--block"), "{cmd}");
        // Findings on stdout, chatter dropped, findings re-emitted on stderr —
        // the only stream a blocked stop feeds back to the agent.
        assert!(cmd.contains("2>/dev/null"), "{cmd}");
        assert!(cmd.contains(">&2"), "{cmd}");
        // Only exit 2 propagates: a missing mezz must not wedge every stop.
        assert!(cmd.contains("[ $r -eq 2 ] && exit 2; exit 0"), "{cmd}");
    }

    #[test]
    fn both_legs_are_wired() {
        let legs = hook_group()["hooks"].as_array().unwrap().clone();
        assert_eq!(legs.len(), 2);
        assert!(legs[0]["command"].as_str().unwrap().contains("self-review"));
        assert!(legs[1]["command"].as_str().unwrap().contains("hook check"));
    }

    /// The file holds a reader's permission allowlist before it holds our
    /// hooks, and it is the only record of it.
    #[test]
    fn wiring_preserves_permissions_and_foreign_hooks() {
        let text = r#"{"permissions":{"allow":["Bash(ls)"]},
                       "hooks":{"Stop":[{"hooks":[{"type":"command","command":"make lint"}]}]}}"#;
        let merged = merge_hooks(text, hook_group(), Path::new("settings.json"), false)
            .unwrap()
            .unwrap();
        assert_eq!(merged["permissions"]["allow"][0], "Bash(ls)");
        let stop = merged["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "the foreign hook must survive: {stop:?}");
        assert!(!is_mezz_group(&stop[0]));
        assert!(is_mezz_group(&stop[1]));
    }

    /// Re-running `mezz init --hooks` must be a no-op, not a second copy.
    #[test]
    fn wiring_twice_changes_nothing() {
        let text = serde_json::to_string(&json!({ "hooks": { "Stop": [hook_group()] } })).unwrap();
        let again = merge_hooks(&text, hook_group(), Path::new("settings.json"), false).unwrap();
        assert!(again.is_none(), "{again:?}");
    }

    /// `--force` replaces our leg and nobody else's — a reader who tuned the
    /// redirection gets ours back, a reader with their own Stop hook keeps it.
    #[test]
    fn force_replaces_only_our_own_group() {
        let text = r#"{"hooks":{"Stop":[
            {"hooks":[{"type":"command","command":"make lint"}]},
            {"hooks":[{"type":"command","command":"mezz hook self-review"}]}]}}"#;
        let stop = merged_stop(text, true);
        assert_eq!(stop.len(), 2, "{stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "make lint");
        assert!(stop[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--block"));
    }

    #[test]
    fn a_file_without_a_hooks_key_gains_one() {
        let stop = merged_stop(r#"{"permissions":{"allow":[]}}"#, false);
        assert_eq!(stop.len(), 1);
        assert!(is_mezz_group(&stop[0]));
    }

    #[test]
    fn an_unparseable_settings_file_is_refused_rather_than_rewritten() {
        let err = merge_hooks("{oops", hook_group(), Path::new("settings.json"), false).unwrap_err();
        assert!(err.to_string().contains("Left untouched"), "{err}");
    }

    /// The three shapes that would panic if the group were written by indexing
    /// instead of through [`stop_groups_of`].
    #[test]
    fn a_settings_file_of_the_wrong_shape_is_refused() {
        for text in ["[]", r#"{"hooks":[]}"#, r#"{"hooks":{"Stop":{}}}"#] {
            let err =
                merge_hooks(text, hook_group(), Path::new("settings.json"), false).unwrap_err();
            assert!(err.to_string().contains("Left untouched"), "{text}: {err}");
        }
    }

    #[test]
    fn all_turns_on_every_scaffold() {
        assert_eq!(
            Targets::new(false, false, false, true),
            Targets {
                vscode: true,
                mcp: true,
                hooks: true,
                // Every optional *file*. Neither of the two task-only
                // scaffolds below is one.
                agent_spawn: false,
                editor_tools: false
            }
        );
        assert_eq!(
            Targets::new(false, false, false, false),
            Targets::default()
        );
        assert_eq!(
            Targets::new(true, false, false, false),
            Targets {
                vscode: true,
                mcp: false,
                hooks: false,
                agent_spawn: false,
                editor_tools: false
            }
        );
    }
}
