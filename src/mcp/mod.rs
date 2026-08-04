//! MCP (Model Context Protocol) server over stdio.
//!
//! Exposes Nao's graph to AI agents as callable tools — `overview`
//! (domain-level Elevator shape), `map` (structural map), `quality`
//! (smells + complexity offenders), `assess_change` (metric deltas of
//! the working tree vs a git ref), and the rest listed in
//! `tool_definitions`.
//!
//! Tools-only server, so the protocol surface is five JSON-RPC methods.
//! Hand-rolled rather than an SDK dependency: serde_json is already in
//! the tree and the message loop fits in this file. Protocol JSON goes
//! to stdout exclusively; all diagnostics go to stderr.

pub mod push;
mod slice;
mod tools;

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{json, Value};

use crate::graph::DependencyGraph;

/// Protocol revisions this server can speak. `initialize` echoes the
/// client's requested revision when supported, else the newest here.
/// Tools-only servers are unaffected by the differences between these.
const SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// A cached analysis, valid only while `gen` matches the server's
/// current generation (the watcher bumps it on source changes).
pub struct CachedGraph {
    pub graph: Arc<DependencyGraph>,
    pub gen: u64,
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
}

pub fn run(root: PathBuf, include_tests: bool, languages: Option<Vec<String>>) -> Result<()> {
    let root = root.canonicalize().map_err(|e| {
        anyhow::anyhow!("Cannot resolve root path {}: {}", root.display(), e)
    })?;
    eprintln!("nao MCP server on stdio — root: {}", root.display());

    let generation = Arc::new(AtomicU64::new(0));
    spawn_invalidation_watcher(root.clone(), generation.clone());

    let server = McpServer {
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
/// parseable language extension (same by-construction rule as `nao
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
        "serverInfo": { "name": "nao", "version": env!("CARGO_PKG_VERSION") },
        "instructions": format!(
            "Nao analyzes the codebase at {} into a typed entity/relationship graph \
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
];

fn handle_tools_call(server: &McpServer, id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let Some((_, run)) = TOOLS.iter().find(|(n, _)| *n == name) else {
        return error_response(id, -32602, &format!("Unknown tool: {}", name));
    };
    let result = run(server, &args);

    // Per MCP, tool execution failures are reported inside the result
    // (isError: true) so the model can see and react to them; only
    // protocol-level problems use JSON-RPC errors.
    match result {
        Ok(text) => ok_response(id, json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false,
        })),
        Err(e) => ok_response(id, json!({
            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
            "isError": true,
        })),
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
                offenders ranked by refactor pressure, and dependency cycles. Use this to judge \
                the health of an area or to find refactoring targets.",
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
                trait/interface members reached through the abstraction) and public API are \
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
                        "description": "Write the slice here instead of returning it, relative to the project root, e.g. `.nao/spec-slice.elv`. Missing parent directories are created; the path must stay inside the project."
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
}
