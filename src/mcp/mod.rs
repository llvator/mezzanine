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

mod baseline;
mod boundaries;
mod format;
mod layout;
pub mod push;
mod recipes;
mod reshape;
mod slice;
mod tools;

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

/// What produced this answer, appended to every tool response (CFG-014).
///
/// Two facts, because the two ways a long-running server goes stale are
/// not detectable the same way. A scope change the server can see, so the
/// digest moves on its own. A replaced binary it cannot see at all — the
/// process goes on running the code it was started with — so the version
/// is printed for the *reader*, who can compare it against the one they
/// just installed. That asymmetry is why this is a footer rather than a
/// reload.
fn scope_footer(server: &McpServer) -> String {
    format!(
        "\n\n_scope {} · mezz {}{}_",
        server.scope_id(),
        env!("CARGO_PKG_VERSION"),
        import_coverage_note(server),
    )
}

/// What the footer says when some of what the parsers read did not reach the
/// graph — and nothing at all when it all did.
///
/// Every verdict these tools give is computed over the graph, so a hole in it
/// becomes a confident wrong answer: a folder is reported as a funnel because
/// the imports that would have shown otherwise are missing, not because
/// nothing leaves it. Silence is indistinguishable from cleanliness, and this
/// is the line that separates them. Reported next to the scope digest for the
/// same reason that is there (CFG-014): it is a fact about what produced the
/// answer, not part of the answer.
///
/// Quiet when whole, so a sound graph costs no tokens and the line means
/// something when it appears. The analysis is the warm-cached one the tool
/// just used; a failure to obtain it says nothing here, because the tool's
/// own error already has.
fn import_coverage_note(server: &McpServer) -> String {
    let Ok(graph) = tools::analyze(server, &server.root) else {
        return String::new();
    };
    let (landed, seen) = graph.import_coverage();
    if landed >= seen {
        return String::new();
    }
    format!(
        " · {landed} of {seen} imports in the graph — {} missing, so any verdict \
         over the folders they cross is unsound",
        seen - landed,
    )
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
type ToolFn = fn(&McpServer, &Value) -> Result<String>;

/// Tool name → implementation. A table rather than a `match`, so adding a
/// tool costs no branch in the dispatcher. CI-001 is the same problem on
/// the CLI side: the complexity gate fails on *any* metric increase to an
/// existing function, which makes a growing `match` unextendable.
/// `tool_definitions` below declares the same names to clients; the two
/// are kept in step by `every_advertised_tool_is_dispatchable`.
const TOOLS: &[(&str, ToolFn)] = &[
    ("overview", tools::overview),
    ("map", tools::map),
    ("quality", tools::quality),
    ("impact", tools::impact),
    ("context", tools::context),
    ("hotspots", tools::hotspots),
    ("tests_for", tools::tests_for),
    ("trace", tools::trace),
    ("similar", tools::similar),
    ("dead_code", tools::dead_code),
    ("assess_change", tools::assess_change),
    ("spec_slice", slice::spec_slice),
    ("reshape", reshape::reshape),
    ("layout", layout::layout),
    ("boundaries", boundaries::boundaries),
];

fn handle_tools_call(server: &McpServer, id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let Some((_, run)) = TOOLS.iter().find(|(n, _)| *n == name) else {
        return error_response(id, -32602, &format!("Unknown tool: {}", name));
    };
    let result = run(server, &args);
    // On the failure branch too: "Path not found" is exactly what an
    // exclude pattern added to the settings file produces, and that
    // answer needs to name its scope more than a successful one does.
    let footer = scope_footer(server);

    // Per MCP, tool execution failures are reported inside the result
    // (isError: true) so the model can see and react to them; only
    // protocol-level problems use JSON-RPC errors.
    match result {
        Ok(text) => ok_response(
            id,
            json!({
                "content": [{ "type": "text", "text": text + &footer }],
                "isError": false,
            }),
        ),
        Err(e) => ok_response(
            id,
            json!({
                "content": [{ "type": "text", "text": format!("Error: {:#}{}", e, footer) }],
                "isError": true,
            }),
        ),
    }
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
            "description": "Blast radius of a potential change to one entity: what it uses \
                (code it relies on — its contract with the rest of the codebase) and what \
                uses it (direct dependents, plus transitive dependents level by level), \
                each with file:line positions. Use this before refactoring a function, \
                class, or type to know exactly which code must be checked or updated. \
                Target by `entity` name, or by `path` + `line` for a position in a file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "string",
                        "description": "Name or qualified name of the entity (e.g. `compute_diff` or `diff::compute_diff`). If ambiguous, the response lists candidates."
                    },
                    "path": {
                        "type": "string",
                        "description": "File containing the entity, relative to the project root. Use together with `line` as an alternative to `entity`."
                    },
                    "line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line inside the entity. The innermost entity spanning this line is targeted."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 5,
                        "description": "How many dependency hops to follow for the transitive blast radius (default 2)."
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
            "description": "Which of this folder's own imports reach past another folder's door \
                into its interior — the cross-folder tangle, asked of the folder that wrote it. \
                Every other tool grades a folder on what arrives; `reshape` says outright that \
                where its outgoing dependencies land 'stays their own folder's business', and \
                since every folder says that, a file deep in one area importing a file deep in \
                another is a defect nobody owns. This takes the other half: where an import lands \
                is the target's business, whether you knocked on the front door is yours. Splits \
                the folder's outgoing dependencies three ways — landed on a door, landed on shared \
                vocabulary several folders reach (leave alone), or reached past a door (the work \
                list) — names each offending import with its line, the door it bypassed, and which \
                of four fixes applies. Use it when asked to untangle cross-folder dependencies, to \
                enforce that folders talk through their entry points, or after `reshape` and \
                `layout` have made one folder's own drawing clean and the mess is between folders. \
                Reads only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Folder whose outgoing dependencies to grade, relative to the project root, e.g. `src/parser`. Omit for the repository root, which has nothing outside it."
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
    /// (CFG-014).
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
}
