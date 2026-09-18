//! MCP (Model Context Protocol) server over stdio.
//!
//! Exposes Mezzanine's graph to AI agents as callable tools — `overview`
//! (domain-level Elevator shape), `map` (structural map), `quality`
//! (smells + complexity offenders), `assess_change` (metric deltas of
//! the working tree vs a git ref), and the rest listed in
//! `tool_definitions`.
//!
//! Tools-only server, so the protocol surface is five JSON-RPC methods.
//! Hand-rolled rather than an SDK dependency: serde_json is already in
//! the tree and the message loop fits in this file. Protocol JSON goes
//! to stdout exclusively; all diagnostics go to stderr.

mod answer;
mod baseline;
mod boundaries;
mod census;
mod chains;
pub(crate) mod cost;
mod effects;
pub(crate) mod externals;
mod file_impact;
mod format;
mod layout;
pub mod push;
mod radius;
mod recipes;
mod reshape;
mod slice;
/// `pub(crate)` for the three helpers `mezz monitor` counts with — the smell
/// split, the test-code predicate and the path spelling. The dashboard has to
/// agree with `quality` about the same tree, and the only way to guarantee
/// that is to count with the same code.
pub(crate) mod tools;

/// What an entity listing shows: no ghosts, no parameters, no fields.
///
/// Re-exported because `mezz check` grades the same entities the listings
/// show, and answering "what counts as an entity here" a second time is how
/// two surfaces of one tool come to disagree (CHK-001).
pub(crate) use tools::is_listed;

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{json, Value};

use answer::Answer;

use crate::config::Config;
use crate::graph::DependencyGraph;

/// Protocol revisions this server can speak. `initialize` echoes the
/// client's requested revision when supported, else the newest here.
/// Tools-only servers are unaffected by the differences between these.
const SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// A cached analysis, valid only while `gen` matches the server's
/// current generation (the watcher bumps it on source changes) and
/// `scope` matches the scope the next call resolves to.
pub struct CachedGraph {
    pub graph: Arc<DependencyGraph>,
    pub gen: u64,
    /// The scope this graph was analysed under — see [`scope_id`].
    pub scope: String,
}

impl CachedGraph {
    /// Whether this entry still answers for what a call resolved to.
    ///
    /// Two conditions rather than one because an analysis goes stale for
    /// two reasons and the watcher only sees the first: it bumps the
    /// generation on *source* changes, so a `.mezz/settings.json` edited
    /// between two calls leaves it untouched. Serving a hit on the
    /// generation alone would answer under the scope in force when the
    /// entry was stored while the response footer named the current one
    /// — the one disagreement that footer exists to make impossible.
    fn serves(&self, generation: u64, scope: &str) -> bool {
        self.gen == generation && self.scope == scope
    }
}

pub struct McpServer {
    pub root: PathBuf,
    pub include_tests: bool,
    pub languages: Option<Vec<String>>,
    /// Warm graphs per (canonical path, include_tests) — MCP-006. Safe
    /// because analysis is deterministic (AN-002): a cached graph can be
    /// stale-old, never stale-wrong.
    pub graph_cache: Mutex<HashMap<(PathBuf, bool), CachedGraph>>,
    /// Base-ref graphs for `assess_change`, keyed by (resolved SHA,
    /// include_tests). SHAs pin content, so no generation check; the
    /// stored path is the worktree dir the file paths were rooted at.
    pub base_cache: Mutex<HashMap<(String, bool), (Arc<DependencyGraph>, PathBuf)>>,
    /// Bumped by the watcher thread on every relevant source change.
    pub generation: Arc<AtomicU64>,
    /// What `reshape` last reported per folder — see
    /// [`baseline::ShapeBaselines`], which owns both the record and the
    /// reason the server is the one holding it.
    pub(crate) shape_baselines: baseline::ShapeBaselines,
    /// Whether `reshape` has already spelled out its rules this session.
    ///
    /// The allowed/forbidden list and the closing instruction are the same
    /// six hundred words on every call, and an agent working through four
    /// folders reported that they were most of what the tool returned.
    /// Repetition does not make a rule more binding — it makes the finding
    /// above it harder to find — so the second call and after get the
    /// forbidden moves as a checklist and nothing else.
    ///
    /// Per process rather than per folder, and deliberately *not* off the
    /// baseline record, which now outlives the process: a fresh session is
    /// a fresh reader, and the full text is cheap once.
    pub(crate) rules_spelled_out: std::sync::atomic::AtomicBool,
    /// Whether `layout` has already explained what a relayout cannot do.
    ///
    /// Same reasoning as [`Self::rules_spelled_out`], and the same agent
    /// reported that fixing one while adding the other left the total
    /// boilerplate higher than before: `layout`'s "What a layout cannot
    /// do" arrived beside `reshape`'s "What counts as a fix", both
    /// verbatim on every call. A caveat is worth its length once.
    pub(crate) layout_caveat_spelled_out: std::sync::atomic::AtomicBool,
}

impl McpServer {
    /// The scope this server is answering under *right now*.
    ///
    /// Re-read rather than remembered, because `.mezz/settings.json` is
    /// re-read on every analysis: a value captured at startup would name
    /// the file as it stood when the process began and quietly disagree
    /// with the analysis it was printed beside. `include_tests` and
    /// `languages` come off the command line and cannot move, and that is
    /// the point — they are in the digest so a reader comparing two
    /// sessions can see that they differ.
    pub(crate) fn scope_id(&self) -> String {
        scope_id(&crate::diff::build_analysis_config(
            &self.root,
            self.include_tests,
            &self.languages,
        ))
    }
}

/// The scope an answer was produced under, short enough to sit in a footer.
///
/// A digest of [`crate::diff::scope_fingerprint`] rather than a second
/// notion of scope: that string already changes whenever anything about
/// what an analysis *includes* changes, and it is already the key a cached
/// base analysis is stored under — so two answers printing the same six
/// characters were produced under the same configuration, or there is a
/// bug worth finding. Six hex characters because this is read by a person
/// noticing a change, not compared by a machine.
pub(crate) fn scope_id(config: &Config) -> String {
    let digest = blake3::hash(crate::diff::scope_fingerprint(config).as_bytes()).to_hex();
    digest[..6].to_string()
}

pub fn run(root: PathBuf, include_tests: bool, languages: Option<Vec<String>>) -> Result<()> {
    let root = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Cannot resolve root path {}: {}", root.display(), e))?;
    eprintln!("mezz MCP server on stdio — root: {}", root.display());

    let generation = Arc::new(AtomicU64::new(0));
    spawn_invalidation_watcher(root.clone(), generation.clone());

    let server = McpServer {
        shape_baselines: baseline::ShapeBaselines::rooted_at(&root),
        rules_spelled_out: std::sync::atomic::AtomicBool::new(false),
        layout_caveat_spelled_out: std::sync::atomic::AtomicBool::new(false),
        root,
        include_tests,
        languages,
        graph_cache: Mutex::new(HashMap::new()),
        base_cache: Mutex::new(HashMap::new()),
        generation,
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = error_response(Value::Null, -32700, &format!("Parse error: {}", e));
                writeln!(stdout, "{}", resp)?;
                stdout.flush()?;
                continue;
            }
        };

        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();

        // Messages without a method are responses to server-initiated
        // requests; we never send any, so ignore. Messages without an id
        // are notifications (initialized, cancelled, …) — nothing to do.
        let (Some(id), false) = (id, method.is_empty()) else {
            continue;
        };

        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let response = match method {
            "initialize" => ok_response(id, initialize_result(&server, &params)),
            "ping" => ok_response(id, json!({})),
            "tools/list" => ok_response(id, json!({ "tools": tool_definitions() })),
            "tools/call" => handle_tools_call(&server, id, &params),
            _ => error_response(id, -32601, &format!("Method not found: {}", method)),
        };
        writeln!(stdout, "{}", response)?;
        stdout.flush()?;
    }
    Ok(())
}

/// Debounced recursive watcher that bumps the cache generation whenever
/// a parseable source file changes (MCP-006). Invalidation only — the
/// re-analysis itself happens lazily on the next tool call. Watcher
/// setup failure downgrades to cold-per-call behavior, never an error.
fn spawn_invalidation_watcher(root: PathBuf, generation: Arc<AtomicU64>) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer =
            match notify_debouncer_mini::new_debouncer(std::time::Duration::from_millis(300), tx) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("⚠ cache watcher disabled ({}); analyses stay cold", e);
                    return;
                }
            };
        if let Err(e) = debouncer
            .watcher()
            .watch(&root, notify::RecursiveMode::Recursive)
        {
            eprintln!("⚠ cache watcher disabled ({}); analyses stay cold", e);
            return;
        }
        for events in rx.into_iter().flatten() {
            if events.iter().any(|e| is_relevant_source_change(&e.path)) {
                generation.fetch_add(1, Ordering::Release);
                eprintln!("♻ source change detected — graph cache invalidated");
            }
        }
    });
}

/// True for files whose change should invalidate cached graphs: a
/// parseable language extension (same by-construction rule as `mezz
/// watch`), outside build/VCS directories that churn without changing
/// source truth (cargo writes generated `.rs` under `target/`).
fn is_relevant_source_change(path: &Path) -> bool {
    let in_ignored_dir = path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("target" | "node_modules" | ".git" | "dist" | "build")
        )
    });
    if in_ignored_dir {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            !matches!(
                crate::models::file_info::Language::from_extension(ext),
                crate::models::file_info::Language::Unknown
            )
        })
        .unwrap_or(false)
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result(server: &McpServer, params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let version = if SUPPORTED_VERSIONS.contains(&requested) {
        requested
    } else {
        SUPPORTED_VERSIONS[SUPPORTED_VERSIONS.len() - 1]
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "mezz", "version": env!("CARGO_PKG_VERSION") },
        "instructions": format!(
            "Mezzanine analyzes the codebase at {} into a typed entity/relationship graph \
             with per-entity complexity and coupling metrics. Use `overview` for the \
             domain-level shape from the project's Elevator (.elv) specs (if present) \
             before touching code, `map` to get the structural shape of a folder \
             before reading files, `quality` to find smells and complexity hotspots, \
             and `assess_change` to self-review edits by diffing the working tree \
             against a git ref.",
            server.root.display()
        ),
    })
}

/// One tool's implementation, as stored in the dispatch table.
///
/// Two arms because the move from prose to a value is incremental
/// (ADR 0035). A `Prose` tool builds its answer by pushing formatted lines
/// into a vector and has no value in the middle to serialize; a
/// `Structured` one computes a value and renders it twice. Which is which
/// is a fact about the *code*, so the dispatcher reads it off the table —
/// rather than a consumer discovering it from an empty object.
enum Tool {
    Prose(fn(&McpServer, &Value) -> Result<String>),
    Structured(fn(&McpServer, &Value) -> Result<Answer>),
}

impl Tool {
    /// Run it, whichever kind it is. A prose tool's answer simply has no
    /// data beside it.
    fn call(&self, server: &McpServer, args: &Value) -> Result<Answer> {
        match self {
            Tool::Prose(run) => run(server, args).map(Answer::prose),
            Tool::Structured(run) => run(server, args),
        }
    }
}

/// Tool name → implementation. A table rather than a `match`, so adding a
/// tool costs no branch in the dispatcher. CI-001 is the same problem on
/// the CLI side: the complexity gate fails on *any* metric increase to an
/// existing function, which makes a growing `match` unextendable.
/// `tool_definitions` below declares the same names to clients; the two
/// are kept in step by `every_advertised_tool_is_dispatchable`.
const TOOLS: &[(&str, Tool)] = &[
    ("overview", Tool::Prose(tools::overview)),
    ("map", Tool::Structured(tools::map)),
    ("quality", Tool::Structured(tools::quality)),
    ("impact", Tool::Prose(tools::impact)),
    ("context", Tool::Prose(tools::context)),
    ("hotspots", Tool::Structured(tools::hotspots)),
    ("tests_for", Tool::Prose(tools::tests_for)),
    ("trace", Tool::Prose(tools::trace)),
    ("similar", Tool::Prose(tools::similar)),
    ("dead_code", Tool::Structured(tools::dead_code)),
    ("assess_change", Tool::Prose(tools::assess_change)),
    ("spec_slice", Tool::Prose(slice::spec_slice)),
    ("reshape", Tool::Prose(reshape::reshape)),
    ("layout", Tool::Prose(layout::layout)),
    ("boundaries", Tool::Prose(boundaries::boundaries)),
    ("cost", Tool::Prose(cost::cost)),
];

/// The tool by that name, or the error a caller should see for a typo.
fn lookup(name: &str) -> Result<&'static Tool> {
    TOOLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, tool)| tool)
        .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))
}

/// The tools that can answer as JSON, in the order they are listed.
///
/// Public because the honest way to run a partial migration is to be able
/// to name what is in it (ADR 0035): this list is what `--format json`
/// prints when it is asked for a tool that is not on it.
pub fn structured_tools() -> Vec<&'static str> {
    TOOLS
        .iter()
        .filter(|(_, t)| matches!(t, Tool::Structured(_)))
        .map(|(n, _)| *n)
        .collect()
}

/// Every tool name the CLI can ask for, in the order they are listed.
///
/// The CLI builds its subcommands by hand rather than from this, because
/// each one needs its own typed flags — but `every_tool_has_a_cli_command`
/// compares the two lists, so a tool added to [`TOOLS`] and not to the CLI
/// fails the build's tests rather than silently existing on one front door.
pub fn tool_names() -> Vec<&'static str> {
    TOOLS.iter().map(|(n, _)| *n).collect()
}

/// Run one tool once and return its rendered body, footer included.
///
/// The second front door onto [`TOOLS`] (CLI-002). Same implementations and
/// same footer as the stdio server — only the envelope differs, which is the
/// point: two ways to ask one question that disagree is the defect this is
/// meant to remove, not repeat.
///
/// No invalidation watcher and no warm cache worth the name: a one-shot
/// process exits before either could pay for itself. That makes a CLI call
/// cost a full analysis where the long-lived server would often answer from
/// memory — the price of the second door, paid by the caller who chose it.
pub fn run_tool(
    root: PathBuf,
    include_tests: bool,
    languages: Option<Vec<String>>,
    name: &str,
    args: &Value,
) -> Result<String> {
    let server = one_shot_server(root, include_tests, languages)?;
    let answer = lookup(name)?.call(&server, args)?;
    let footer = answer::footer(&server);
    Ok(answer.into_text() + &footer)
}

/// The same call, rendered for a machine (CLI-003).
///
/// Refuses before it analyses anything. A tool that has not been converted
/// cannot answer this question, and the caller should find that out in
/// milliseconds rather than after a full walk of the tree — and by name,
/// rather than by receiving prose from a flag that claimed to produce JSON.
pub fn run_tool_json(
    root: PathBuf,
    include_tests: bool,
    languages: Option<Vec<String>>,
    name: &str,
    args: &Value,
) -> Result<Value> {
    let tool = lookup(name)?;
    if !matches!(tool, Tool::Structured(_)) {
        anyhow::bail!(
            "`{}` has no JSON rendering yet — its answer is still prose only.\n\
             Converted so far: {}.\n\
             The rest are tracked by CLI-004; call `{}` without `--format json` \
             to read it as text.",
            name,
            structured_tools().join(", "),
            name,
        );
    }

    let server = one_shot_server(root, include_tests, languages)?;
    let answer = tool.call(&server, args)?;
    let data = answer
        .data()
        .cloned()
        // Unreachable: the table says this tool is structured, and a
        // structured tool returns its value. Stated rather than unwrapped
        // so a future arm that forgets cannot panic in a user's CI job.
        .ok_or_else(|| anyhow::anyhow!("`{}` returned no value to render", name))?;
    Ok(answer::envelope(&server, name, data))
}

/// The server a one-shot call runs against: no watcher, and a cache that
/// dies with the process.
fn one_shot_server(
    root: PathBuf,
    include_tests: bool,
    languages: Option<Vec<String>>,
) -> Result<McpServer> {
    let root = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Cannot resolve root path {}: {}", root.display(), e))?;
    Ok(McpServer {
        shape_baselines: baseline::ShapeBaselines::rooted_at(&root),
        rules_spelled_out: std::sync::atomic::AtomicBool::new(false),
        layout_caveat_spelled_out: std::sync::atomic::AtomicBool::new(false),
        root,
        include_tests,
        languages,
        graph_cache: Mutex::new(HashMap::new()),
        base_cache: Mutex::new(HashMap::new()),
        generation: Arc::new(AtomicU64::new(0)),
    })
}

fn handle_tools_call(server: &McpServer, id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let Ok(tool) = lookup(name) else {
        return error_response(id, -32602, &format!("Unknown tool: {}", name));
    };
    let result = tool.call(server, &args);
    // On the failure branch too: "Path not found" is exactly what an
    // exclude pattern added to the settings file produces, and that
    // answer needs to name its scope more than a successful one does.
    let footer = answer::footer(server);

    // Per MCP, tool execution failures are reported inside the result
    // (isError: true) so the model can see and react to them; only
    // protocol-level problems use JSON-RPC errors.
    match result {
        Ok(answer) => ok_response(id, tool_result(server, name, &answer, &footer)),
        Err(e) => ok_response(
            id,
            json!({
                "content": [{ "type": "text", "text": format!("Error: {:#}{}", e, footer) }],
                "isError": true,
            }),
        ),
    }
}

/// A successful call's result: the text every client can read, plus
/// `structuredContent` for the ones that would rather not re-read it
/// (CLI-003).
///
/// The text is sent either way. `structuredContent` is an addition to the
/// protocol that older clients ignore, and an agent that has just been
/// handed the prose should not have to ask twice to get the rows — so it
/// travels beside the prose in the same response, rendered from the same
/// value.
fn tool_result(server: &McpServer, name: &str, answer: &Answer, footer: &str) -> Value {
    let mut result = json!({
        "content": [{ "type": "text", "text": answer.text().to_string() + footer }],
        "isError": false,
    });
    if let Some(data) = answer.data() {
        result["structuredContent"] = answer::envelope(server, name, data.clone());
    }
    result
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "overview",
            "description": "Domain-level overview of the project from its Elevator (.elv) \
                specs: Categories → Features → Functionalities plus cross-cutting Concepts, \
                each with a description and code references (cr: paths) pointing at the \
                implementing files. This is the onboarding ground floor — call it before \
                `map` when the project has an Elevator layer. With `focus`, returns the \
                context bundle for one entity (ancestor chain, target subtree, siblings, \
                relevant Concepts) — read this before working on that part of the system. \
                Unlike the other tools, spec content is human-authored, not derived from \
                code, and may lag recent changes: treat descriptions and cr: paths as \
                strong leads, not ground truth.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "focus": {
                        "type": "string",
                        "description": "Entity to centre the artifact on, e.g. `f.protocol` or `fu.protocol.creation`. Bare names work; a miss returns close matches. Omit for the full domain map."
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory containing the .elv spec, relative to the project root. Omit for the whole project."
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "map",
            "description": "Structural map of a folder: source files and the entities \
                (classes, functions, methods) inside them, annotated with lines of code, \
                complexity (cyclomatic/cognitive), and coupling (fan-in/fan-out). Use this \
                to understand the shape of unfamiliar code and decide where to look before \
                reading files — it is cheaper and more complete than reading files one by one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory or file to map, relative to the project root. Omit for the whole project."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 3,
                        "description": "1 = files only, 2 = files + top-level entities (default), 3 = also members (methods inside classes)."
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "quality",
            "description": "Code-quality assessment of a folder: detected smells (God Class, \
                Dispatcher, Feature Envy, Shotgun Surgery, Data Bag), the top complexity/coupling \
                offenders ranked by refactor pressure, dependency cycles, and folder shape — how \
                readable the dependency graph each folder draws is (cyclic / tangled / \
                hierarchical / fractal). Use this to judge the health of an area, to find \
                refactoring targets, or to find where the code's organisation has drifted.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory or file to assess, relative to the project root. Omit for the whole project."
                    },
                    "top": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "description": "How many top offenders to list (default 10)."
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "impact",
            "description": "Blast radius of a potential change, at either of two grains. \
                Given `entity`, or `path` + `line`: what that entity uses (code it relies \
                on — its contract with the rest of the codebase) and what uses it (direct \
                dependents, plus an outline of every transitive dependent **nested under \
                the one it was reached through**, so a row says through what and not only \
                how far), each with file:line. `direction: out` turns that outline around \
                and draws the call tree under the entity instead. Given `path` alone, the \
                same question asked of the **file**: \
                what outside it depends on it and what it depends on outside itself, both \
                grouped by the file at the other end, with edges internal to the file \
                counted rather than listed — the question to ask before moving, splitting \
                or deleting a file, and one a per-entity walk cannot answer without \
                double-counting the file's own wiring. Both grains also report the \
                **effect surface**: which of `fs`, `net`, `proc` and `env` the target \
                reaches and through which call, so \"does changing this touch the disk or \
                the network\" is answered here rather than by reading the callees. \
                Classified from call target names, so a language mezz has no table for \
                says so rather than reporting none.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Name or qualified name of the entity (e.g. `compute_diff` or `diff::compute_diff`). If ambiguous, the response lists candidates."
                    },
                    "path": {
                        "type": "string",
                        "description": "A file, relative to the project root. With `line`, it targets the entity spanning that line; on its own, the file itself is the subject. A directory is refused — call `map`, `reshape` or `boundaries` for a folder."
                    },
                    "line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line inside the entity. The innermost entity spanning this line is targeted. Omit to ask about the whole file."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 5,
                        "description": "How many dependency hops to follow for the transitive blast radius (default 2). Entity-level only: the file view is one hop in each direction."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["in", "out"],
                        "description": "Which way the radius walks. `in` (default) is what breaks if this entity changes, across every kind of dependency edge. `out` is the call tree under it — what runs when it runs, calls only. Entity-level only."
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "context",
            "description": "Minimal context pack for editing one entity: its full source code, \
                plus the signatures (not bodies) of everything it uses and everything that uses \
                it, each with file:line. Use this before editing a function to avoid opening \
                and reading every neighboring file. Target like `impact`: by `entity` name or \
                `path` + `line`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": { "type": "string", "description": "Name or qualified name of the entity." },
                    "path": { "type": "string", "description": "File containing the entity (with `line`)." },
                    "line": { "type": "integer", "minimum": 1, "description": "1-based line inside the entity." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "hotspots",
            "description": "Risk ranking: git churn × complexity. Files that are both changed \
                often and structurally complex are where bugs concentrate — complex-but-stable \
                code ranks low. Use this to prioritize refactoring or reviews. Requires the \
                project to be a git repository.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory to scope the ranking to. Omit for the whole project." },
                    "days": { "type": "integer", "minimum": 1, "maximum": 3650, "description": "History window in days (default 180)." },
                    "top": { "type": "integer", "minimum": 1, "maximum": 50, "description": "How many files to list (default 10)." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "tests_for",
            "description": "Which tests exercise an entity, directly or through intermediate \
                calls — so a change can be verified by running the relevant tests instead of \
                the whole suite. Target by `entity` name or `path` + `line`. Tests are found \
                even when the session excludes test files from analysis.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": { "type": "string", "description": "Name or qualified name of the entity." },
                    "path": { "type": "string", "description": "File containing the entity (with `line`)." },
                    "line": { "type": "integer", "minimum": 1, "description": "1-based line inside the entity." },
                    "depth": { "type": "integer", "minimum": 1, "maximum": 6, "description": "Dependency hops to search (default 3)." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "trace",
            "description": "Shortest dependency path between two entities: how does A reach B? \
                Returns up to 3 shortest chains with the relationship kind of every hop and \
                file:line per node; if no forward path exists, checks the reverse direction. \
                Use for flow questions during onboarding or debugging.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Name or qualified name of the starting entity." },
                    "to": { "type": "string", "description": "Name or qualified name of the destination entity." },
                    "max_hops": { "type": "integer", "minimum": 1, "maximum": 30, "description": "Search limit (default 10)." }
                },
                "required": ["from", "to"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "similar",
            "description": "Does something like this already exist? Ranks entities by similarity \
                to a free-text query using identifier tokens and signature types. Call BEFORE \
                implementing a new helper or utility to avoid duplicating existing code. Scores \
                are 0.00–1.00: the fraction of the query's meaningful words the entity matches, \
                discounted when the match falls outside its own name. Common English words (`to`, \
                `of`, `for`, …) carry almost no weight and cannot produce a match on their own. \
                Results below 0.40 are dropped, so fewer than `top` results — or none at all — is \
                the normal signal that nothing close exists and implementing fresh is reasonable.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What you are about to implement, e.g. \"resolve git ref to sha\"." },
                    "kind": { "type": "string", "description": "Optional entity-kind filter, e.g. `function`, `struct`." },
                    "top": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Upper bound on results (default 10). Not a target — the 0.40 score floor applies first." }
                },
                "required": ["query"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "dead_code",
            "description": "Entities nothing in the project references — fan-in 0 — grouped by \
                file with a per-file count. Tests, entry points (`main`, Python dunders, \
                trait/interface members reached through the abstraction), members of a class \
                whose base type is outside the analyzed tree (a framework lifecycle hook the \
                host calls) and public API are \
                excluded, because for those \"no dependent\" is not evidence of death; so are \
                names the source mentions anywhere else, which catches calls made inside macro \
                bodies and other references no graph can hold. The header states how many of \
                each were filtered. Use this to find deletable code before a cleanup, or to \
                check whether an entity you are about to change is still used at all. Fan-in is \
                always computed over the whole project, so `path` narrows the report rather \
                than manufacturing false positives. The tool errs toward under-reporting; \
                still verify a candidate before deleting it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory or file to report on, relative to the project root. Omit for the whole project." },
                    "include_public": { "type": "boolean", "description": "Also list unreferenced public API (default false). Turn this on for an application or binary, where `pub` is convenience rather than a published contract." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "assess_change",
            "description": "Self-review a change: compares the current working tree (including \
                uncommitted edits) against a git base ref and reports per-entity metric deltas \
                (complexity, nesting, coupling), added/removed entities, and smells introduced \
                or resolved. Use this after making edits to check their structural impact \
                before presenting them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "base_ref": {
                        "type": "string",
                        "description": "Git ref to compare the working tree against, e.g. HEAD (default), main, HEAD~3, a SHA."
                    }
                },
                "additionalProperties": false
            },
            // Read-only despite the temporary git worktree: it is created
            // in the system temp dir and removed before the call returns,
            // leaving no lasting modification.
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "spec_slice",
            "description": "The project's Elevator (.elv) spec, narrowed to one folder and \
                re-emitted as standalone .elv source: every spec entity whose `cr:` claims a \
                path inside `path`, everything it contains, the Concepts it uses, and its \
                ancestors as pruned context. Use it to capture what the domain layer says \
                about the area a task touches — with `out`, the slice is written to a file \
                (e.g. inside a ticket folder) as a small, diffable snapshot of the branch \
                being worked on; without `out`, the source comes back in the response. When \
                nothing claims a path inside the folder, the narrowest claim above it is used \
                and the response says so. Read `overview` first if you need the whole map.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Folder (or file) whose spec claims to extract, relative to the project root, e.g. `src/mcp`. Required — for the whole spec, call `overview`."
                    },
                    "out": {
                        "type": "string",
                        "description": "Write the slice here instead of returning it, relative to the project root, e.g. `.mezz/spec-slice.elv`. Missing parent directories are created; the path must stay inside the project."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "Allow `out` to replace an existing file (default false)."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            },
            // Not read-only: with `out` it writes a file. The write is
            // fenced to the project root and never overwrites unless
            // asked, so it is additive rather than destructive.
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
        },
        {
            "name": "reshape",
            "description": "How to improve one folder's architecture, as a concrete instruction. \
                Returns the graph the folder draws — its immediate children with their levels, \
                every dependency between them marked as stepping down a level / skipping one / \
                closing a loop, which files outsiders reach in through, and which reach past \
                them — then names the single change that would move it up one tier of \
                cyclic → tangled → hierarchical → fractal, classified into the situation it \
                actually is: a redundant path that one removal clears, a shared data contract \
                to leave alone, or a level too wide to read. Call it again after making the \
                change — it keeps the drawing it showed you and opens with what really moved, \
                so a tier that rose while no dependency changed is reported rather than \
                claimed. Use it after `quality` flags a \
                folder's shape, when asked to restructure or reorganise an area, or when a \
                graph of the code is too tangled to follow. Answers 'what exactly do I change \
                here', where `quality` answers 'where is it worst'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Folder to reshape, relative to the project root, e.g. `src/parser`. Omit for the repository root. A file has no children and so draws no graph."
                    }
                },
                "additionalProperties": false
            },
            // Reads the tree and reports; the restructuring is the caller's
            // to carry out.
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "boundaries",
            "description": "Which of this folder's (or this file's) own imports reach past another \
                folder's door into its interior — the cross-folder tangle, asked of the code that \
                wrote it. \
                Every other tool grades a folder on what arrives; `reshape` says outright that \
                where its outgoing dependencies land 'stays their own folder's business', and \
                since every folder says that, a file deep in one area importing a file deep in \
                another is a defect nobody owns. This takes the other half: where an import lands \
                is the target's business, whether you knocked on the front door is yours. Splits \
                the folder's outgoing dependencies three ways — landed on a door, landed on shared \
                vocabulary several folders reach (leave alone), or reached past a door (the work \
                list) — names each offending import with its line, the door it bypassed, and which \
                of four fixes applies. Point it at a single file to grade only the imports written \
                in that file, against the doors of the folder holding it — the grain at which \
                somebody actually fixes an import. Use it when asked to untangle cross-folder \
                dependencies, to enforce that folders talk through their entry points, when \
                picking up one file and asking which of its imports are a liability, or after \
                `reshape` and `layout` have made one folder's own drawing clean and the mess is \
                between folders. Reads only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "What to grade the outgoing dependencies of, relative to the project root. A folder (`src/parser`) grades every import its files write; a file (`src/parser/rust.rs`) grades only that file's, against the boundary of the folder holding it. Omit for the repository root, which has nothing outside it."
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "layout",
            "description": "Where a folder's files would sit if its own dependency drawing decided, \
                and what that arrangement measures out at — without moving anything. Called with \
                just a folder, it proposes the subfolders the drawing implies: each headed by the \
                one child every path into the group passes through, so every folder it proposes \
                has exactly one way in by construction. Called with `moves`, it scores your \
                arrangement instead. Either way the 'after' numbers are measured, not estimated — \
                the paths are rewritten and the same folder pass is re-run over the result, so the \
                score reported is the score the folder will have once you make the move. Use it \
                when `reshape` says a level is too wide or its branching is short, when asked to \
                reorganise or split a folder, or to settle 'what would moving this buy me' before \
                editing. Answers 'what should this folder look like', where `reshape` answers \
                'what is the one thing wrong with it'. It writes nothing: the output is a list of \
                `git mv` lines.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Folder to lay out, relative to the project root, e.g. `src/parser`. Omit for the repository root."
                    },
                    "moves": {
                        "type": "array",
                        "description": "Score this arrangement instead of proposing one. Omit to have mezz read the layout off the drawing.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "what": {
                                    "type": "string",
                                    "description": "File or folder to move, relative to the project root. A folder carries everything under it."
                                },
                                "into": {
                                    "type": "string",
                                    "description": "Folder it moves into, relative to the project root. Need not exist yet."
                                }
                            },
                            "required": ["what", "into"],
                            "additionalProperties": false
                        }
                    }
                },
                "additionalProperties": false
            },
            // Proposes and scores; every move is the caller's to make.
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "cost",
            "description": "How code **scales**, and on what — the question every other \
                complexity number here answers wrongly. `cyclomatic`, `cognitive` and \
                `max_nesting` measure how hard a body is to *read*, so a flat 40-arm `match` \
                scores far worse than a doubly-nested loop, and the doubly-nested loop is \
                the one that falls over at 10k rows. Given `entity`, or `path` + `line`, \
                returns an estimated worst-case time complexity — `O(1)`, `O(n)`, \
                `O(n log n)`, `O(n^k)` for k nested loops — with one evidence line per \
                contributing construct: each loop with its `file:line` and the header \
                the author wrote, each recognised library operation that walks (`sort`, \
                `indexOf`, `Iterator::position`) and how many loops it sits inside, and the \
                recursion shape. **It composes along the call chain**, which is where real \
                cost lives: a cheap-looking function calling a cheap-looking helper from \
                inside a loop, three frames down, is the O(n³) no other tool will say. Each \
                frame reached to `depth` hops is charged its own cost times the loops the \
                chain passed through to reach it, and the report names the frame the total \
                is charged to with its `file:line`. `from` + `to` prices one named route \
                instead (`trace`'s targeting, with the loops `trace` drops). Ask it before \
                optimising, before accepting a loop over a collection that grows, or when a \
                review asks 'is this a problem on a large input'. The exponent counts loop \
                *levels*, not one shared `n`: two loops over different collections are \
                `n × m`. Structural worst case, not dataflow and not a proof — it \
                over-reports a loop whose bound it cannot see is constant and under-reports \
                a linear call it has no rule for, and the report names both directions. \
                Recursion is classified, never solved; a ring stops the chain and is \
                reported unsolved. A language mezz has no loop table for says so rather than \
                raising `max_nesting` to a power, and an unbindable call makes the total a \
                floor rather than an answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Name or qualified name of the callable (e.g. `compute_diff` or `diff::compute_diff`). If ambiguous, the response lists candidates."
                    },
                    "path": {
                        "type": "string",
                        "description": "A file, relative to the project root. Needs `line`: `cost` answers about one body, so a file on its own is not a subject it has."
                    },
                    "line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line inside the callable. With `path`, targets the innermost entity spanning it."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 5,
                        "description": "Call hops to compose the cost over (default 2, max 5). `0` answers about this body alone. Raising it widens the walk fast; the report says when it stopped early."
                    },
                    "from": {
                        "type": "string",
                        "description": "With `to`: price one named route instead of everything an entry point reaches. Both are entity names, as `trace` takes them."
                    },
                    "to": {
                        "type": "string",
                        "description": "With `from`: the far end of the route. The worst chain reaching it within `depth` hops is the one priced."
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TOOLS` and `tool_definitions` declare the same surface twice: a
    /// tool in one but not the other is either unreachable or invisible.
    /// The `match` that used to make that impossible is gone, so this
    /// test is what keeps them in step.
    #[test]
    fn every_advertised_tool_is_dispatchable() {
        let mut advertised: Vec<String> = tool_definitions()
            .as_array()
            .expect("tool_definitions is an array")
            .iter()
            .map(|t| t["name"].as_str().expect("every tool is named").to_string())
            .collect();
        let mut dispatched: Vec<String> = TOOLS.iter().map(|(n, _)| n.to_string()).collect();
        advertised.sort();
        dispatched.sort();
        assert_eq!(advertised, dispatched);
    }

    /// The digest is of what an analysis *includes*, so a settings change
    /// that narrows the scope moves it. If it did not, the footer would
    /// print the same six characters across a configuration change and be
    /// worse than no footer — a reassurance instead of a signal.
    #[test]
    fn the_scope_digest_moves_with_what_the_analysis_includes() {
        let mut config = Config::for_path(Path::new("."));
        let before = scope_id(&config);
        assert_eq!(before.len(), 6, "a footer digest has to fit on the line");
        config.analysis.exclude_patterns.push("**/*.never".to_string());
        assert_ne!(before, scope_id(&config));
    }

    /// Every tool response names the configuration that produced it —
    /// the failure branch included, since "Path not found" is exactly
    /// what an exclude pattern added to the settings file produces
    /// (CFG-014). What the caveat beside it says is asserted in
    /// [`super::answer`], which now owns both renderings of it.
    #[test]
    fn a_tool_response_names_the_scope_and_version_that_produced_it() {
        let server = McpServer {
            root: std::env::temp_dir().canonicalize().unwrap(),
            include_tests: false,
            languages: None,
            graph_cache: Mutex::new(HashMap::new()),
            base_cache: Mutex::new(HashMap::new()),
            generation: Arc::new(AtomicU64::new(0)),
            shape_baselines: Default::default(),
            rules_spelled_out: Default::default(),
            layout_caveat_spelled_out: Default::default(),
        };
        let call = json!({ "name": "map", "arguments": { "path": "no-such-folder-here" } });
        let response = handle_tools_call(&server, json!(1), &call);
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("a tool response carries text");
        assert_eq!(response["result"]["isError"], json!(true));
        // `contains`, not `ends_with`: the footer carries a third fact when
        // the graph is missing imports, and that one is appended after the
        // version. Both facts this test exists for are still asserted.
        assert!(
            text.contains(&format!(
                "_scope {} · mezz {}",
                server.scope_id(),
                env!("CARGO_PKG_VERSION")
            )) && text.trim_end().ends_with('_'),
            "the answer does not say what produced it:\n{text}"
        );
    }

    /// A fixture that reads as production source: `dead_code` classifies by
    /// path, and a directory named "test" makes every entity in it test code.
    struct TmpDir(std::path::PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            // The counter, not just the clock: tests run in parallel
            // threads of one process, and two of them reaching this line
            // inside the same clock tick got the same directory — so the
            // first to finish deleted the other's tree mid-analysis and
            // the victim reported an empty graph. Found the honest way.
            static NTH: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "mezz-doors-{}-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                NTH.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&path).unwrap();
            TmpDir(path)
        }

        fn write(&self, rel: &str, body: &str) {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, body).unwrap();
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The stdio door, called the way a client calls it — a fresh server per
    /// call, because that is what the CLI door gets and the once-per-server
    /// latches (`rules_spelled_out`, `layout_caveat_spelled_out`) would
    /// otherwise make the second call of a session differ from the first for
    /// a reason that has nothing to do with the two doors.
    fn through_stdio(root: &Path, name: &str, args: &Value) -> String {
        let server = McpServer {
            shape_baselines: baseline::ShapeBaselines::rooted_at(root),
            rules_spelled_out: std::sync::atomic::AtomicBool::new(false),
            layout_caveat_spelled_out: std::sync::atomic::AtomicBool::new(false),
            root: root.to_path_buf(),
            include_tests: false,
            languages: None,
            graph_cache: Mutex::new(HashMap::new()),
            base_cache: Mutex::new(HashMap::new()),
            generation: Arc::new(AtomicU64::new(0)),
        };
        let call = json!({ "name": name, "arguments": args });
        let response = handle_tools_call(&server, json!(1), &call);
        assert_eq!(
            response["result"]["isError"],
            json!(false),
            "{name} failed over stdio: {}",
            response["result"]["content"][0]["text"]
        );
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("a tool response carries text")
            .to_string()
    }

    /// One implementation, two front doors (CLI-002). The risk the ticket was
    /// written about is a second implementation drifting from the first —
    /// which is what `stats` against `quality` and `find` against `similar`
    /// already are. Name parity is checked in `main.rs`; this is the other
    /// half, that the same arguments get the same answer, footer included.
    ///
    /// Four tools rather than sixteen: one per argument shape (a path, a path
    /// plus a count, a flag, a query). They share `run_tool`, so a fork would
    /// have to be in the dispatch both go through, and these four cross it.
    #[test]
    fn both_doors_give_the_same_answer_to_the_same_question() {
        let dir = TmpDir::new("same-body");
        dir.write(
            "src/lib.rs",
            r#"
pub mod shape;

pub fn resolve_git_ref(git_ref: &str) -> String {
    shape::normalise(git_ref)
}
"#,
        );
        dir.write(
            "src/shape.rs",
            r#"
pub fn normalise(name: &str) -> String {
    name.trim().to_string()
}

fn never_called(name: &str) -> String {
    name.to_string()
}
"#,
        );
        let root = dir.0.canonicalize().unwrap();

        let calls: [(&str, Value); 4] = [
            ("map", json!({ "path": "src", "depth": 2 })),
            ("quality", json!({ "path": "src", "top": 5 })),
            ("dead_code", json!({ "include_public": true })),
            ("similar", json!({ "query": "normalise" })),
        ];

        for (name, args) in &calls {
            let cli = run_tool(root.clone(), false, None, name, args)
                .unwrap_or_else(|e| panic!("{name} failed on the CLI door: {e:#}"));
            let mcp = through_stdio(&root, name, args);
            // Equality is only worth asserting over a real answer: two empty
            // strings match, and a tool that silently returned nothing would
            // pass a bare `assert_eq!` while proving nothing at all.
            assert!(
                cli.lines().count() > 3 && cli.contains("_scope "),
                "`{name}` returned nothing worth comparing:\n{cli}"
            );
            assert_eq!(
                cli, mcp,
                "`{name}` answers the two doors differently — one implementation is the point"
            );
        }
    }

    /// A tree with six mutually-recursive pairs, so the cycle list is
    /// longer than the five the prose prints — the smallest fixture that
    /// can tell a complete JSON list from a capped one.
    fn six_cycles(name: &str) -> TmpDir {
        let dir = TmpDir::new(name);
        let mut src = String::from("pub mod widget;\n");
        for i in 0..6 {
            src.push_str(&format!(
                "pub fn ping{i}(n: u32) -> u32 {{ if n == 0 {{ 0 }} else {{ pong{i}(n - 1) }} }}\n\
                 pub fn pong{i}(n: u32) -> u32 {{ if n == 0 {{ 0 }} else {{ ping{i}(n - 1) }} }}\n",
            ));
        }
        dir.write("src/lib.rs", &src);
        dir.write("src/widget.rs", "pub fn draw() -> u32 { 1 }\n");
        // A file no parser reads, so the census has something to hold back
        // and `map`'s two numbers are genuinely two numbers.
        dir.write("src/page.astro", "<h1>hello</h1>\n");
        dir
    }

    /// The number a heading states, e.g. `2` from
    /// `## Dependency cycles (2)`.
    fn heading_count(text: &str, heading: &str) -> usize {
        let line = text
            .lines()
            .find(|l| l.starts_with(heading))
            .unwrap_or_else(|| panic!("no `{heading}` heading in:\n{text}"));
        let digits: String = line
            .split_once('(')
            .expect("the heading states a count")
            .1
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().expect("the count is a number")
    }

    /// The drift CLI-002 was written about, one level down: two renderings
    /// of one call that disagree about how many things there are.
    ///
    /// Counts rather than whole bodies, because that is what a CI job
    /// reads — "how many smells, and did that number go up" is the
    /// question CLI-003 exists to let it ask without grepping a heading.
    #[test]
    fn the_prose_and_the_json_report_the_same_counts() {
        let dir = six_cycles("same-counts");
        let root = dir.0.canonicalize().unwrap();
        let args = json!({ "path": "src", "top": 5 });

        let text = run_tool(root.clone(), false, None, "quality", &args).expect("quality as text");
        let json = run_tool_json(root, false, None, "quality", &args).expect("quality as json");
        let data = &json["data"];

        assert_eq!(
            heading_count(&text, "## Dependency cycles ("),
            data["cycles"]["total"].as_u64().expect("a cycle count") as usize,
        );
        assert_eq!(
            heading_count(&text, "## Smells ("),
            data["smells"]["total"].as_u64().expect("a smell count") as usize,
        );
        assert_eq!(
            heading_count(&text, "## Folder shape ("),
            data["folder_shape"]["tally"]["total"]
                .as_u64()
                .expect("a folder count") as usize,
        );
    }

    /// `map`'s two numbers — what it listed against what the folder holds
    /// — are the ones a reader is most likely to mistake for each other,
    /// which is why the prose spells them "19 of 21 files listed".
    #[test]
    fn the_json_map_says_what_the_listing_could_not_show() {
        let dir = six_cycles("map-census");
        let root = dir.0.canonicalize().unwrap();
        let args = json!({ "path": "src", "depth": 1 });

        let text = run_tool(root.clone(), false, None, "map", &args).expect("map as text");
        let data = run_tool_json(root, false, None, "map", &args).expect("map as json")["data"]
            .clone();

        assert_eq!(data["listed"], json!(2), "two .rs files are listed");
        assert_eq!(data["held"], json!(3), "the .astro file is held and unread");
        assert_eq!(data["unread"]["unsupported"], json!(1));
        assert_eq!(data["unread"]["extensions"], json!([".astro"]));
        assert!(
            text.contains("2 of 3 files listed"),
            "the prose states the same two numbers:\n{text}"
        );
    }

    /// The prose caps its cycle list at five and says so; a script wants
    /// all of them (ADR 0035). A cap the *renderer* chose is a property of
    /// prose, not of the answer.
    #[test]
    fn a_cap_the_prose_chose_does_not_reach_the_json() {
        let dir = six_cycles("prose-cap");
        let root = dir.0.canonicalize().unwrap();
        let args = json!({ "path": "src" });

        let text = run_tool(root.clone(), false, None, "quality", &args).expect("quality as text");
        let data = run_tool_json(root, false, None, "quality", &args).expect("quality as json")
            ["data"]
            .clone();

        let total = data["cycles"]["total"].as_u64().expect("a cycle count");
        assert!(
            total > SHOWN_CYCLES_IN_PROSE,
            "the fixture has to out-run the cap to prove anything: {total}"
        );
        assert_eq!(
            data["cycles"]["cycles"].as_array().map(Vec::len),
            Some(total as usize),
            "the JSON carries every cycle it counted"
        );
        assert_eq!(
            text.lines().filter(|l| l.starts_with("- ") && l.contains(" → ")).count(),
            SHOWN_CYCLES_IN_PROSE as usize,
            "the prose still prints five:\n{text}"
        );
        assert!(
            text.contains(&format!(
                "… and {} more cycles.",
                total - SHOWN_CYCLES_IN_PROSE
            )),
            "and still says how many it left out:\n{text}"
        );
    }

    /// What `quality`'s prose prints before it summarises — stated here so
    /// the test above fails loudly if the budget moves, rather than
    /// silently asserting the new one.
    const SHOWN_CYCLES_IN_PROSE: u64 = 5;

    /// A bound the *caller* passed is part of the question, so both
    /// renderings honour it — and both say how many there were.
    #[test]
    fn a_bound_the_caller_set_is_honoured_by_both_renderings() {
        let dir = six_cycles("caller-bound");
        let root = dir.0.canonicalize().unwrap();
        let args = json!({ "path": "src", "top": 2 });

        let data = run_tool_json(root, false, None, "quality", &args).expect("quality as json")
            ["data"]
            .clone();

        let pressure = &data["pressure"];
        assert_eq!(pressure["entities"].as_array().map(Vec::len), Some(2));
        assert_eq!(pressure["shown"], json!(2));
        assert!(
            pressure["total"].as_u64().expect("a total") > 2,
            "the fixture has more entities than `top` asked for"
        );
    }

    /// Twelve tools still answer in prose, and a caller asking one of them
    /// for JSON has to be told which — not handed prose from a flag that
    /// promised JSON, and not an empty object (ADR 0035).
    #[test]
    fn a_tool_with_no_json_rendering_refuses_by_name() {
        let dir = six_cycles("no-json");
        let root = dir.0.canonicalize().unwrap();
        let error = run_tool_json(root, false, None, "reshape", &json!({ "path": "src" }))
            .expect_err("reshape has no JSON rendering yet")
            .to_string();

        assert!(
            error.contains("`reshape`"),
            "the refusal names the tool:\n{error}"
        );
        for converted in structured_tools() {
            assert!(
                error.contains(converted),
                "the refusal lists `{converted}` as somewhere to go:\n{error}"
            );
        }
    }

    /// The footer's two facts, and the caveat beside them, are fields
    /// rather than a trailing string a consumer has to regex (CLI-003).
    #[test]
    fn the_envelope_names_the_scope_that_produced_it() {
        let dir = six_cycles("envelope");
        let root = dir.0.canonicalize().unwrap();
        let json = run_tool_json(root.clone(), false, None, "map", &json!({ "path": "src" }))
            .expect("map as json");

        assert_eq!(json["schema_version"], json!(answer::SCHEMA_VERSION));
        assert_eq!(json["tool"], json!("map"));
        assert_eq!(json["mezz_version"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(json["scope"]["include_tests"], json!(false));
        assert_eq!(json["scope"]["root"], json!(root.display().to_string()));
        assert_eq!(
            json["scope"]["digest"].as_str().map(str::len),
            Some(6),
            "the digest is the same six characters the footer prints"
        );
        assert!(
            json["caveats"].is_array(),
            "caveats are always an array, empty when the graph is whole"
        );
    }

    /// An agent that has just been handed the prose should not have to ask
    /// twice to get the rows. Converted tools carry both in one response;
    /// the rest carry text alone rather than an empty structure.
    #[test]
    fn the_stdio_door_carries_the_value_beside_the_prose() {
        let dir = six_cycles("stdio-pair");
        let root = dir.0.canonicalize().unwrap();
        let server = McpServer {
            shape_baselines: baseline::ShapeBaselines::rooted_at(&root),
            rules_spelled_out: std::sync::atomic::AtomicBool::new(false),
            layout_caveat_spelled_out: std::sync::atomic::AtomicBool::new(false),
            root,
            include_tests: false,
            languages: None,
            graph_cache: Mutex::new(HashMap::new()),
            base_cache: Mutex::new(HashMap::new()),
            generation: Arc::new(AtomicU64::new(0)),
        };

        let converted = handle_tools_call(
            &server,
            json!(1),
            &json!({ "name": "map", "arguments": { "path": "src" } }),
        );
        assert_eq!(converted["result"]["structuredContent"]["tool"], json!("map"));
        assert!(
            converted["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("# Map of")),
            "the prose still travels with it"
        );

        let prose_only = handle_tools_call(
            &server,
            json!(2),
            &json!({ "name": "reshape", "arguments": { "path": "src" } }),
        );
        assert!(
            prose_only["result"]["structuredContent"].is_null(),
            "an unconverted tool sends no structure at all, not an empty one"
        );
    }
}
