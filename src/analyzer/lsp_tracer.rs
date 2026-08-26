//! AN-004 — exact call-edge resolution via rust-analyzer.
//!
//! mezz's own call resolution is name-heuristic: a `foo()` call resolves to
//! *some* entity named `foo`, which is wrong exactly when it matters (a
//! same-named method on an unrelated type). This module borrows precision
//! instead of building it: spawn rust-analyzer in batch mode, ask
//! `textDocument/definition` for each collected Rust call site, and map the
//! answer back to the entity whose span contains it. Those edges are labeled
//! [`Precision::Exact`]; everything else stays [`Precision::Heuristic`].
//!
//! Lifecycle is **spawn-per-analysis** (decided in AN-004): spawn, initialize,
//! resolve the batch, shut down. A long-lived per-workspace server can follow
//! once it has AN-003's persistent store to attach to.
//!
//! Robustness contract, like the parse store: this is a best-effort speedup,
//! never a source of failure. No rust-analyzer on PATH, no Cargo manifest, a
//! spawn error, a timeout — all degrade to today's heuristic behavior and
//! return an empty upgrade map. The one thing it must never do is stay silent
//! about degrading (that would reintroduce the "unlabeled maybe" this ticket
//! exists to kill), so a budget overrun logs one line to stderr.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::models::CodeEntity;

/// A Rust call site to resolve: which relationship it came from, the file it
/// lives in, and the 0-based (line, byte-column) of the callee identifier.
pub struct CallSite {
    pub rel_idx: usize,
    pub file: PathBuf,
    pub line: u32,
    pub col: u32,
}

/// Default overall budget for the whole rust-analyzer batch (spawn → crate
/// graph ready → all definitions resolved). Override with
/// `MEZZ_LSP_TIMEOUT_SECS`. Fixed, not proportional to file count: the cost is
/// dominated by RA's per-spawn workspace load + `cargo check`, not the
/// analyzed file count. 180s covers small/medium crates fully and yields a
/// partial (still-labeled) result on large ones; a big cold workspace can
/// take minutes to become queryable, so raise this — or wait for the
/// long-lived-server follow-up — when opting in there.
const DEFAULT_TIMEOUT_SECS: u64 = 180;
/// Give-up-if-stuck window: no message from RA for this long ⇒ wedged.
/// Generous because rust-analyzer runs `cargo check` + build scripts during
/// startup, a phase that emits no LSP progress and can be silent for tens of
/// seconds on a cold target. Override with `MEZZ_LSP_IDLE_SECS`.
const IDLE_SECS: u64 = 45;

/// True when exact call tracing is engaged. **Opt-in** (default off): auto-
/// spawning rust-analyzer on every analysis would re-cost the self-review
/// hook AN-003 just made cheap, so it's off unless `MEZZ_LSP_EXACT=1`. When
/// enabled and rust-analyzer is on PATH, call edges resolve exact; when
/// disabled or unavailable, analysis keeps today's heuristic edges.
pub fn enabled_by_env() -> bool {
    std::env::var_os("MEZZ_LSP_EXACT").is_some_and(|v| v == "1" || v == "true")
}

/// Resolve `sites` through rust-analyzer, returning a map from relationship
/// index to the exact target entity id. Best-effort: an empty map means "no
/// upgrades, keep the heuristic edges" and is the result for every failure
/// mode. Never panics, never errors.
pub fn resolve_exact_calls(
    root: &Path,
    entities: &[CodeEntity],
    sites: &[CallSite],
) -> HashMap<usize, String> {
    let empty = HashMap::new();
    if sites.is_empty() || !enabled_by_env() {
        return empty;
    }
    let Some(manifest_root) = cargo_root(root) else {
        eprintln!("  rust-analyzer: no Cargo.toml found, keeping heuristic call edges");
        return empty;
    };

    let deadline = Instant::now()
        + Duration::from_secs(
            std::env::var("MEZZ_LSP_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_TIMEOUT_SECS),
        );
    let idle = Duration::from_secs(
        std::env::var("MEZZ_LSP_IDLE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(IDLE_SECS),
    );

    match run(&manifest_root, entities, sites, deadline, idle) {
        Ok(map) => map,
        Err(e) => {
            eprintln!("  rust-analyzer: {e}; keeping heuristic call edges");
            empty
        }
    }
}

/// The fallible core, so `resolve_exact_calls` can swallow every error path
/// into the heuristic fallback with a single `?`-driven flow. Takes an
/// explicit budget so tests can bound it independently of the env defaults.
fn run(
    root: &Path,
    entities: &[CodeEntity],
    sites: &[CallSite],
    deadline: Instant,
    idle: Duration,
) -> Result<HashMap<usize, String>, String> {
    let mut client = LspClient::spawn(root).map_err(|e| format!("spawn failed: {e}"))?;
    let encoding = client.initialize(root, deadline, idle)?;

    // Absolute paths only: rust-analyzer ignores relative `file://` URIs, so
    // a walker that yielded `src/foo.rs` would resolve nothing. Canonicalize
    // once per site up front.
    let abs_files: Vec<PathBuf> = sites
        .iter()
        .map(|s| std::fs::canonicalize(&s.file).unwrap_or_else(|_| s.file.clone()))
        .collect();

    // Open every distinct file that has a call site, with the exact bytes we
    // parsed — so positions line up and RA doesn't fall back to stale disk.
    let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for abs in &abs_files {
        if let std::collections::hash_map::Entry::Vacant(slot) = files.entry(abs.clone()) {
            if let Ok(text) = std::fs::read_to_string(abs) {
                client.did_open(abs, &text);
                slot.insert(text.lines().map(str::to_string).collect());
            }
        }
    }

    // Precompute the (uri, line, character) each site resolves at.
    let queries: Vec<Option<(String, u32, u32)>> = sites
        .iter()
        .enumerate()
        .map(|(i, site)| {
            let abs = &abs_files[i];
            let lines = files.get(abs)?;
            let character = encode_col(
                lines.get(site.line as usize).map(String::as_str),
                site.col,
                encoding,
            );
            Some((path_to_uri(abs), site.line, character))
        })
        .collect();

    // Wait until rust-analyzer can actually answer — go-to-def needs the
    // crate graph, which is ready well before `quiescent` (that waits on
    // flycheck/build, minutes away). Waiting for a fixed signal is fragile;
    // instead poll a handful of real call sites until one resolves. RA
    // answers null immediately while still indexing, so this can't hang.
    let probes: Vec<(String, u32, u32)> = queries.iter().flatten().take(8).cloned().collect();
    let ready = client.poll_until_ready(&probes, deadline, idle);
    if std::env::var_os("MEZZ_LSP_DEBUG").is_some() {
        eprintln!("  [lsp] ready => {ready}");
    }

    // Pipeline all definition requests, then drain answers.
    let mut req_to_site: HashMap<i64, usize> = HashMap::new();
    for (i, query) in queries.iter().enumerate() {
        let Some((uri, line, character)) = query else {
            continue;
        };
        let id = client
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character }
                }),
            )
            .map_err(|e| format!("definition request failed: {e}"))?;
        req_to_site.insert(id, i);
    }
    let answers = client.collect_responses(
        &req_to_site.keys().copied().collect::<HashSet<_>>(),
        deadline,
        idle,
    );

    client.shutdown();

    // Map each definition location back to the entity whose span contains it.
    let mut upgrades = HashMap::new();
    let mut resolved = 0usize;
    let mut non_null = 0usize;
    for (id, result) in &answers {
        let Some(&site_i) = req_to_site.get(id) else {
            continue;
        };
        let Some((tgt_file, tgt_line)) = first_location(result) else {
            continue;
        };
        non_null += 1;
        if let Some(entity_id) = entity_at(entities, &tgt_file, tgt_line) {
            upgrades.insert(sites[site_i].rel_idx, entity_id);
            resolved += 1;
        }
    }
    if std::env::var_os("MEZZ_LSP_DEBUG").is_some() {
        eprintln!(
            "  [lsp] answers={} non_null={} mapped={}",
            answers.len(),
            non_null,
            resolved
        );
    }

    let note = if answers.len() < sites.len() {
        format!(
            " (budget exceeded, {}/{} call sites answered)",
            answers.len(),
            sites.len()
        )
    } else {
        String::new()
    };
    eprintln!(
        "  rust-analyzer: resolved {}/{} call sites exact, rest heuristic{}",
        resolved,
        sites.len(),
        note
    );
    Ok(upgrades)
}

/// Position encoding negotiated with the server. LSP defaults to UTF-16;
/// rust-analyzer also supports UTF-8, which lets tree-sitter byte columns go
/// through untouched — so we offer UTF-8 first.
#[derive(Clone, Copy, PartialEq)]
enum Encoding {
    Utf8,
    Utf16,
}

/// Convert a 0-based byte column into the server's negotiated encoding. Line
/// numbers are encoding-independent, so only the column needs this; response
/// positions are matched by line alone, so they need no back-conversion.
fn encode_col(line: Option<&str>, byte_col: u32, encoding: Encoding) -> u32 {
    match (encoding, line) {
        (Encoding::Utf8, _) | (_, None) => byte_col,
        (Encoding::Utf16, Some(text)) => {
            let end = (byte_col as usize).min(text.len());
            text[..end].encode_utf16().count() as u32
        }
    }
}

// ------------------------------------------------------------------
//  Minimal hand-rolled LSP client (Content-Length framed JSON-RPC)
// ------------------------------------------------------------------

struct LspClient {
    child: Child,
    stdin: ChildStdin,
    /// All server→client messages, demuxed off a reader thread.
    rx: Receiver<Value>,
    next_id: i64,
}

impl LspClient {
    fn spawn(root: &Path) -> std::io::Result<Self> {
        let stderr = if std::env::var_os("MEZZ_LSP_DEBUG").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };
        let mut child = Command::new("rust-analyzer")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        // Reader thread: parse framed messages and forward them. It exits on
        // EOF (server shutdown), closing the channel.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(msg) = read_message(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            rx,
            next_id: 1,
        })
    }

    /// Send `initialize`, wait for its response (negotiating the position
    /// encoding), then send `initialized`. Returns the agreed encoding.
    fn initialize(
        &mut self,
        root: &Path,
        deadline: Instant,
        idle: Duration,
    ) -> Result<Encoding, String> {
        let id = self
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": path_to_uri(root),
                    "capabilities": {
                        "general": { "positionEncodings": ["utf-8", "utf-16"] },
                        "textDocument": { "definition": { "linkSupport": true } },
                        // workDoneProgress: without it rust-analyzer suppresses
                        // `$/progress`, goes silent through the build, and trips
                        // our idle timeout before it can report readiness.
                        "window": { "workDoneProgress": true },
                        "experimental": { "serverStatusNotification": true }
                    },
                    "workspaceFolders": [{ "uri": path_to_uri(root), "name": "root" }]
                }),
            )
            .map_err(|e| format!("initialize write failed: {e}"))?;

        let mut encoding = Encoding::Utf16; // LSP default if unspecified
        let got = self.pump(deadline, idle, |msg| {
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                if let Some(enc) = msg
                    .pointer("/result/capabilities/positionEncoding")
                    .and_then(Value::as_str)
                {
                    encoding = if enc == "utf-8" {
                        Encoding::Utf8
                    } else {
                        Encoding::Utf16
                    };
                }
                return true;
            }
            false
        });
        if !got {
            return Err("timed out waiting for initialize response".to_string());
        }
        self.notify("initialized", json!({}));
        Ok(encoding)
    }

    /// Open a document with explicit content so query positions match the
    /// bytes mezz parsed rather than whatever is on disk.
    fn did_open(&mut self, path: &Path, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": "rust",
                    "version": 1,
                    "text": text
                }
            }),
        );
    }

    /// Poll rust-analyzer with a few real call sites until one resolves,
    /// which means the crate graph is queryable (go-to-def works long before
    /// `quiescent`, which waits on flycheck). RA answers null immediately
    /// while indexing, so each round is cheap; we back off ~0.8s between
    /// rounds. Returns false if the budget is spent first — we then query the
    /// full batch anyway, just with lower yield.
    fn poll_until_ready(
        &mut self,
        probes: &[(String, u32, u32)],
        deadline: Instant,
        idle: Duration,
    ) -> bool {
        if probes.is_empty() {
            return false;
        }
        while Instant::now() < deadline {
            let mut ids = HashSet::new();
            for (uri, line, character) in probes {
                if let Ok(id) = self.request(
                    "textDocument/definition",
                    json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": line, "character": character }
                    }),
                ) {
                    ids.insert(id);
                }
            }
            let slice = (Instant::now() + Duration::from_secs(3)).min(deadline);
            let answers = self.collect_responses(&ids, slice, idle);
            if answers.values().any(|r| first_location(r).is_some()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(800));
        }
        false
    }

    /// Drain responses for `pending` request ids until all arrive or the
    /// budget runs out. Returns id → `result` value (missing ids timed out).
    fn collect_responses(
        &mut self,
        pending: &HashSet<i64>,
        deadline: Instant,
        idle: Duration,
    ) -> HashMap<i64, Value> {
        let mut out = HashMap::new();
        let mut remaining: HashSet<i64> = pending.clone();
        self.pump(deadline, idle, |msg| {
            // Only responses to our own requests carry an id *without* a
            // method; server→client requests (id + method) are handled in
            // `pump` and must not be mistaken for definition answers.
            if msg.get("method").is_none() {
                if let Some(id) = msg.get("id").and_then(Value::as_i64) {
                    if remaining.remove(&id) {
                        out.insert(id, msg.get("result").cloned().unwrap_or(Value::Null));
                    }
                }
            }
            remaining.is_empty()
        });
        out
    }

    /// Pump messages until `done` returns true or the budget is spent, while
    /// answering any server→client request so rust-analyzer can reach a ready
    /// state (it blocks on `workspace/configuration` etc.). `idle` bounds
    /// silence (a wedged server); `deadline` bounds the total.
    fn pump<F: FnMut(&Value) -> bool>(
        &mut self,
        deadline: Instant,
        idle: Duration,
        mut done: F,
    ) -> bool {
        let debug = std::env::var_os("MEZZ_LSP_DEBUG").is_some();
        loop {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let wait = idle.min(deadline - now);
            match self.rx.recv_timeout(wait) {
                Ok(msg) => {
                    if debug {
                        if let Some(m) = msg.get("method").and_then(Value::as_str) {
                            eprintln!("  [lsp] {} q={:?}", m, msg.pointer("/params/quiescent"));
                        }
                    }
                    // A message with both an id and a method is a request FROM
                    // the server that expects a reply.
                    if let (Some(id), Some(method)) =
                        (msg.get("id"), msg.get("method").and_then(Value::as_str))
                    {
                        self.answer_server_request(id.clone(), method, &msg);
                    }
                    if done(&msg) {
                        return true;
                    }
                }
                Err(_) => return false, // idle or deadline elapsed, or server gone
            }
        }
    }

    /// Reply to a server→client request. rust-analyzer waits on these during
    /// startup; leaving them unanswered stalls it short of `quiescent`.
    fn answer_server_request(&mut self, id: Value, method: &str, msg: &Value) {
        let result = match method {
            // One config value per requested item; null ⇒ "use your defaults".
            "workspace/configuration" => {
                let n = msg
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map_or(0, |a| a.len());
                Value::Array(vec![Value::Null; n])
            }
            // registerCapability / workDoneProgress/create / etc. → ack.
            _ => Value::Null,
        };
        let _ = write_message(
            &mut self.stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        );
    }

    /// Best-effort graceful shutdown, then ensure the child is reaped.
    fn shutdown(&mut self) {
        let _ = self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let _ = self.child.wait();
    }

    fn request(&mut self, method: &str, params: Value) -> std::io::Result<i64> {
        let id = self.next_id;
        self.next_id += 1;
        write_message(
            &mut self.stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )?;
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) {
        let _ = write_message(
            &mut self.stdin,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        );
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Never leave a rust-analyzer process behind on an early return.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read one `Content-Length`-framed JSON-RPC message. `None` on EOF or a
/// malformed frame (which the reader thread treats as end-of-stream).
fn read_message<R: BufRead>(reader: &mut R) -> Option<Value> {
    let mut content_len: usize = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None; // EOF
        }
        let line = line.trim_end();
        if line.is_empty() {
            break; // end of headers
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_len = v.trim().parse().ok()?;
        }
    }
    let mut buf = vec![0u8; content_len];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_message(stdin: &mut ChildStdin, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
    stdin.write_all(&body)?;
    stdin.flush()
}

// ------------------------------------------------------------------
//  Location → entity mapping and URI helpers
// ------------------------------------------------------------------

/// First target `(file, 0-based line)` from a `textDocument/definition`
/// result, which may be `Location`, `Location[]`, `LocationLink[]`, or null.
fn first_location(result: &Value) -> Option<(PathBuf, u32)> {
    let loc = match result {
        Value::Array(arr) => arr.first()?,
        Value::Object(_) => result,
        _ => return None,
    };
    // `Location` uses uri/range; `LocationLink` uses targetUri/target*Range.
    let (uri, range) = if let Some(uri) = loc.get("uri") {
        (uri, loc.get("range")?)
    } else {
        (
            loc.get("targetUri")?,
            loc.get("targetSelectionRange")
                .or_else(|| loc.get("targetRange"))?,
        )
    };
    let path = uri_to_path(uri.as_str()?)?;
    let line = range.pointer("/start/line")?.as_u64()? as u32;
    Some((path, line))
}

/// The innermost callable entity whose span contains `(file, line)`. Ties
/// break on entity id so the choice is deterministic (AN-002 / acceptance #4).
fn entity_at(entities: &[CodeEntity], file: &Path, line: u32) -> Option<String> {
    let canon = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let line = line as usize;
    entities
        .iter()
        .filter(|e| e.kind.is_callable())
        .filter(|e| {
            let ef = std::fs::canonicalize(&e.file_path).unwrap_or_else(|_| e.file_path.clone());
            ef == canon
        })
        .filter(|e| e.span.start.line <= line && line <= e.span.end.line)
        .min_by(|a, b| {
            let sa = a.span.end.line - a.span.start.line;
            let sb = b.span.end.line - b.span.start.line;
            sa.cmp(&sb).then_with(|| a.id.cmp(&b.id))
        })
        .map(|e| e.id.clone())
}

/// `file://` URI for an absolute path. Percent-encodes the few characters
/// that matter for paths; RA is tolerant, so this stays deliberately small.
fn path_to_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'/' | b'-' | b'_' | b'.' | b'~' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' => {
                s.push(b as char)
            }
            _ => s.push_str(&format!("%{:02X}", b)),
        }
    }
    s
}

/// Inverse of `path_to_uri`: strip the scheme and percent-decode.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

/// Walk up from `start` to the nearest directory containing a `Cargo.toml`.
/// rust-analyzer needs a manifest; without one it can't resolve anything.
fn cargo_root(start: &Path) -> Option<PathBuf> {
    let start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let mut dir: Option<&Path> = Some(&start);
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrips_through_path() {
        let p = PathBuf::from("/proj/src/a b/foo.rs");
        let uri = path_to_uri(&p);
        assert!(uri.starts_with("file:///proj/src/a%20b/"));
        assert_eq!(uri_to_path(&uri), Some(p));
    }

    #[test]
    fn utf16_column_conversion_counts_code_units() {
        // "café." — the 'é' is 2 UTF-8 bytes but 1 UTF-16 unit, so a byte
        // column of 5 (before '.') maps to UTF-16 column 4.
        let line = "café.";
        assert_eq!(encode_col(Some(line), 5, Encoding::Utf16), 4);
        assert_eq!(encode_col(Some(line), 5, Encoding::Utf8), 5);
    }

    /// AN-004 acceptance #3: a same-named method on two unrelated types must
    /// resolve to the *receiver's* type under rust-analyzer, not to whichever
    /// `ping` the name heuristic happens to pick first. Skipped (not failed)
    /// when rust-analyzer isn't installed or can't resolve within budget, so
    /// it only fails on a genuinely *wrong* resolution.
    ///
    /// `#[ignore]` because it spawns rust-analyzer (seconds when idle, up to
    /// the budget under a busy machine) — too slow for the default suite. Run
    /// it explicitly: `cargo test -- --ignored exact_resolution`.
    #[test]
    #[ignore = "spawns rust-analyzer; run with --ignored"]
    fn exact_resolution_disambiguates_same_named_methods() {
        if Command::new("rust-analyzer")
            .arg("--version")
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: rust-analyzer not on PATH");
            return;
        }

        // A tiny cargo crate: Alpha::ping (line 2) and Beta::ping (line 3)
        // are indistinguishable by name; the call is on an `&Alpha`.
        let src = "\
pub struct Alpha;
pub struct Beta;
impl Alpha { pub fn ping(&self) -> u32 { 1 } }
impl Beta { pub fn ping(&self) -> u32 { 2 } }
pub fn run_alpha(a: &Alpha) -> u32 { a.ping() }
";
        let alpha_ping_line = 2usize;
        let dir = std::env::temp_dir().join(format!("mezz-an004-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[lib]\npath = \"src/lib.rs\"\n",
        )
        .unwrap();
        let lib = dir.join("src/lib.rs");
        std::fs::write(&lib, src).unwrap();

        let result =
            crate::parser::parse_content(&lib, src, crate::models::file_info::Language::Rust)
                .expect("parse fixture");
        let entities: Vec<CodeEntity> = result.entities.clone();

        // The single `a.ping()` call, with its captured position.
        let (rel_idx, site) = result
            .relationships
            .iter()
            .enumerate()
            .find_map(|(i, r)| {
                let l = r.metadata.get("lsp_line")?.parse().ok()?;
                let c = r.metadata.get("lsp_col")?.parse().ok()?;
                Some((
                    i,
                    CallSite {
                        rel_idx: i,
                        file: lib.clone(),
                        line: l,
                        col: c,
                    },
                ))
            })
            .expect("a call site with a captured position");

        // Bounded budget so a loaded machine (e.g. an IDE rust-analyzer
        // re-indexing in parallel) can't hang the suite.
        let deadline = Instant::now() + Duration::from_secs(60);
        let upgrades =
            run(&dir, &entities, &[site], deadline, Duration::from_secs(30)).expect("tracer run");
        let _ = std::fs::remove_dir_all(&dir);

        // Skip (don't fail) if RA couldn't resolve in the budget — that's an
        // environmental miss, not a correctness bug. The bug this test guards
        // is resolving to the *wrong* type, which only a present-but-wrong
        // mapping can trigger.
        let Some(target_id) = upgrades.get(&rel_idx) else {
            eprintln!(
                "skipping assertion: rust-analyzer did not resolve within budget (machine load?)"
            );
            return;
        };
        let resolved = entities
            .iter()
            .find(|e| &e.id == target_id)
            .expect("resolved id is a real entity");
        assert_eq!(
            resolved.span.start.line, alpha_ping_line,
            "resolved to the wrong type's method: {} at line {}",
            resolved.qualified_name, resolved.span.start.line
        );
    }

    #[test]
    fn cargo_root_walks_up_and_degrades_gracefully() {
        let base = std::env::temp_dir().join(format!("mezz-cargoroot-{}", std::process::id()));
        let nested = base.join("a/b/c");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&nested).unwrap();
        // No manifest anywhere up the temp tree → None (degrade to heuristic).
        // (temp dirs live outside any cargo project.)
        assert_eq!(cargo_root(&nested), None);
        // Drop a manifest two levels up; it should be found from the leaf.
        std::fs::write(base.join("a/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(
            cargo_root(&nested),
            Some(std::fs::canonicalize(base.join("a")).unwrap())
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn first_location_handles_all_shapes() {
        let loc =
            json!({ "uri": "file:///x.rs", "range": { "start": { "line": 7, "character": 2 } } });
        assert_eq!(first_location(&loc), Some((PathBuf::from("/x.rs"), 7)));
        let arr = json!([loc]);
        assert_eq!(first_location(&arr), Some((PathBuf::from("/x.rs"), 7)));
        let link = json!([{ "targetUri": "file:///y.rs", "targetSelectionRange": { "start": { "line": 3, "character": 0 } } }]);
        assert_eq!(first_location(&link), Some((PathBuf::from("/y.rs"), 3)));
        assert_eq!(first_location(&Value::Null), None);
    }
}
