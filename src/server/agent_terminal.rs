//! `POST /api/agents/terminal` — open a Claude Code terminal on the host
//! machine for one entity (SRV-017).
//!
//! A web page cannot open a terminal. The engine can, and when the two are on
//! the same machine — the default, since both bind loopback — that is the
//! whole feature: no PTY relay (SRV-015), no terminal emulator in the page
//! (UI-039). The terminal is the user's own, with their shell and their
//! ability to answer the agent's permission prompts.
//!
//! # Why this route authenticates itself
//!
//! Every other route here serves read-only data. This one causes **code
//! execution on the host**, so it does not inherit the access model:
//!
//! * `AccessPolicy::needs_token` exempts loopback origins, because loopback is
//!   not a boundary against a browser and the trade was made for reading a
//!   graph. A page the user happens to have open would inherit that exemption
//!   and could start an agent against their repo.
//! * The token middleware also skips requests carrying **no `Origin` header at
//!   all**, which is every non-browser client.
//!
//! Neither is wrong for the data API; both are wrong here. So this handler
//! authenticates itself, and the route is not registered unless
//! `--allow-agent-spawn` was passed.
//!
//! The bar is **"a page this engine served, or someone holding the token"** —
//! not "anything local". `is_same_origin` draws that line with the `Origin`
//! header, which a browser sets and a page cannot forge.
//!
//! Requiring the token *unconditionally* was the first attempt, and it was
//! wrong in a way worth recording: the bundled UI is served same-origin by
//! this very engine and has no way to learn the token, which is printed once
//! in the startup banner. Every click returned 401.

use std::path::{Path, PathBuf};
use std::process::Command;

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::Json,
};
use serde::{Deserialize, Serialize};

use super::access::{is_webview_origin, TOKEN_QUERY_PARAM};
use super::state::AppState;
use super::types::ScopeRequest;

#[derive(Deserialize)]
pub(crate) struct TerminalRequest {
    pub entity_id: String,
    /// Passed through to the prompt builder (SRV-016).
    #[serde(default)]
    pub prompt_context: Option<String>,
    /// The pairing token. Accepted in the body because `fetch` from the UI
    /// already sends JSON and this avoids a second auth path to keep in step.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct TerminalResponse {
    pub launched: bool,
    /// The terminal program used, so the UI can say what it opened.
    pub terminal: String,
    /// Where the prompt was written, for a user who wants to inspect it.
    pub prompt_file: String,
}

/// Constant-time-ish comparison. The token is short and local, but there is no
/// reason to leak its prefix through timing.
fn tokens_match(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Is this request from a page *this server itself* served?
///
/// The distinction the token was reaching for is not "local" — it is "mine".
/// A browser sets `Origin` on every cross-origin POST and a page cannot forge
/// it, so an exact match against the `Host` this request arrived on separates
/// the bundled UI from any other page the user happens to have open. A page on
/// `https://evil.example` gets its own origin and is refused; a different local
/// server on another port is a different origin too, and also refused.
///
/// This is the check that lets the same-origin UI work at all: it is served by
/// the engine and has no way to learn the pairing token, which is printed once
/// in the startup banner.
fn is_same_origin(headers: &HeaderMap) -> bool {
    let get = |k: header::HeaderName| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let (Some(origin), Some(host)) = (get(header::ORIGIN), get(header::HOST)) else {
        // No Origin means a non-browser client. Those must bring the token —
        // this allowance exists for the served page, not for curl.
        return false;
    };
    // Compare authorities: strip the scheme from Origin, leaving `host:port`.
    let authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&origin);
    authority.eq_ignore_ascii_case(&host)
}

/// Resolve the terminal program to launch.
///
/// `NAO_TERMINAL` overrides everything, because terminal choice is personal
/// and any built-in list will be wrong for someone. Otherwise probe the
/// platform's usual suspects rather than assuming one is installed.
fn resolve_terminal() -> Option<String> {
    if let Ok(explicit) = std::env::var("NAO_TERMINAL") {
        let explicit = explicit.trim().to_string();
        if !explicit.is_empty() {
            return Some(explicit);
        }
    }
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        // `open` is always present on macOS and dispatches to the user's
        // configured handler for the script.
        &["open"]
    } else if cfg!(target_os = "windows") {
        &["wt.exe", "cmd.exe"]
    } else {
        &[
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "alacritty",
            "xterm",
        ]
    };
    candidates
        .iter()
        .find(|c| which(c))
        .map(|c| (*c).to_string())
}

/// Is `binary` runnable? Same probe as the VS Code side uses, for the same
/// reason: spawning and hoping produces a window that flashes an error.
fn which(binary: &str) -> bool {
    let probe = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    Command::new(probe)
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Write the prompt and a launcher script beside it.
///
/// The prompt goes in a file rather than a command-line argument for the
/// reason UI-037 measured: a real hotspot's prompt is ~94 KB in `full` mode,
/// which would hit `ARG_MAX` and mangle quoting. The agent reads the file.
fn write_launcher(
    repo_root: &Path,
    prompt: &str,
    claude: &str,
) -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = std::env::temp_dir().join(format!("nao-agent-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let prompt_file = dir.join("refactor.prompt.md");
    std::fs::write(&prompt_file, prompt)?;

    let script = dir.join(if cfg!(target_os = "windows") {
        "launch.cmd"
    } else {
        "launch.command"
    });
    let body = if cfg!(target_os = "windows") {
        format!(
            "@echo off\r\ncd /d \"{}\"\r\n{} \"Read '{}' and carry out the refactoring task it describes.\"\r\n",
            repo_root.display(), claude, prompt_file.display()
        )
    } else {
        format!(
            "#!/bin/sh\ncd '{}'\nexec {} \"Read '{}' and carry out the refactoring task it describes.\"\n",
            repo_root.display(), claude, prompt_file.display()
        )
    };
    std::fs::write(&script, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok((prompt_file, script))
}

fn launch(terminal: &str, script: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(terminal);
    if cfg!(target_os = "macos") {
        // `open` hands the script to the user's terminal application.
        cmd.arg(script);
    } else if cfg!(target_os = "windows") {
        cmd.args(["/c", "start", ""]).arg(script);
    } else {
        // The -e convention is honoured by every candidate in the probe list.
        cmd.arg("-e").arg(script);
    }
    cmd.spawn().map(|_| ())
}

/// POST /api/agents/terminal
pub(crate) async fn terminal_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TerminalRequest>,
) -> Result<Json<TerminalResponse>, (StatusCode, String)> {
    // 1. Authenticate. The bar is "this server's own UI, or someone holding
    //    the token" — deliberately narrower than the data API's "any loopback
    //    origin", which would let any page the user has open start an agent.
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let trusted_ui = is_same_origin(&headers) || is_webview_origin(origin);
    if !trusted_ui {
        if let Some(expected) = state.access_token.as_deref() {
            let given = req.token.as_deref().unwrap_or_default();
            if !tokens_match(given, expected) {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    format!(
                        "This route needs either a page served by this engine or the pairing \
                         token from its startup banner. Send it as `token` in the body or \
                         `?{TOKEN_QUERY_PARAM}=`."
                    ),
                ));
            }
        }
    }

    // 2. Build the prompt through the same path the copy button uses, so the
    //    two can never disagree.
    let scope_req = ScopeRequest {
        entity_id: req.entity_id.clone(),
        mode: "refactor".to_string(),
        depth: 1,
        excluded_files: Vec::new(),
        prompt_context: req.prompt_context.clone(),
    };
    let assembly = {
        let graph = state.graph.read().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Graph lock poisoned: {e}"),
            )
        })?;
        let config = state.config.read().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Config lock poisoned: {e}"),
            )
        })?;
        super::scope_handler::collect_scope(&graph, &config.root_path, &scope_req)?
    };
    let repo_root = state.repo_root.read().await.clone();
    let prompt = super::scope_handler::finish_scope(assembly, &repo_root)
        .exports
        .refactor_prompt;
    if prompt.trim().is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No refactor prompt available for {}", req.entity_id),
        ));
    }

    // 3. Resolve both programs before launching anything: a terminal that
    //    opens and immediately prints "claude: command not found" is worse
    //    than an error the caller can render.
    let claude = std::env::var("NAO_CLAUDE_BIN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "claude".to_string());
    if !which(&claude) {
        return Err((
            StatusCode::PRECONDITION_FAILED,
            format!("'{claude}' is not on PATH — install Claude Code or set NAO_CLAUDE_BIN"),
        ));
    }
    let terminal = resolve_terminal().ok_or((
        StatusCode::PRECONDITION_FAILED,
        "No terminal application found — set NAO_TERMINAL to the one you use".to_string(),
    ))?;

    let (prompt_file, script) = write_launcher(&repo_root, &prompt, &claude).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Could not stage the launcher: {e}"),
        )
    })?;
    launch(&terminal, &script).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Could not launch {terminal}: {e}"),
        )
    })?;

    Ok(Json(TerminalResponse {
        launched: true,
        terminal,
        prompt_file: prompt_file.display().to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(k.clone(), v.parse().unwrap());
        }
        h
    }

    /// The regression that broke every click: the engine's own page is served
    /// same-origin and cannot know the token.
    #[test]
    fn the_engines_own_page_is_recognised() {
        assert!(is_same_origin(&hdrs(&[
            (header::ORIGIN, "http://localhost:3000"),
            (header::HOST, "localhost:3000"),
        ])));
    }

    /// The hole the token was there to close. A page on another origin gets
    /// its own `Origin`, which it cannot rewrite.
    #[test]
    fn other_pages_are_not_same_origin() {
        // A drive-by page.
        assert!(!is_same_origin(&hdrs(&[
            (header::ORIGIN, "https://evil.example"),
            (header::HOST, "localhost:3000"),
        ])));
        // Another local server — a different port is a different origin.
        assert!(!is_same_origin(&hdrs(&[
            (header::ORIGIN, "http://localhost:5199"),
            (header::HOST, "localhost:3000"),
        ])));
        // Same name, different host.
        assert!(!is_same_origin(&hdrs(&[
            (header::ORIGIN, "http://127.0.0.1:3000"),
            (header::HOST, "localhost:3000"),
        ])));
    }

    /// Non-browser clients send no `Origin`. They must bring the token; this
    /// allowance exists for the served page, not for curl.
    #[test]
    fn a_missing_origin_is_never_same_origin() {
        assert!(!is_same_origin(&hdrs(&[(header::HOST, "localhost:3000")])));
        assert!(!is_same_origin(&HeaderMap::new()));
    }

    #[test]
    fn tokens_match_is_exact() {
        assert!(tokens_match("abc123", "abc123"));
        assert!(!tokens_match("abc123", "abc124"));
        assert!(!tokens_match("abc", "abc123"));
        assert!(!tokens_match("", "abc123"));
    }

    /// The prompt must reach the agent through a file: a 94 KB `full` prompt
    /// as a command-line argument would hit `ARG_MAX` and mangle quoting.
    #[test]
    fn launcher_references_the_prompt_file_rather_than_inlining_it() {
        let dir = std::env::temp_dir().join("nao-launcher-test");
        let _ = std::fs::create_dir_all(&dir);
        let big = "x".repeat(200_000);
        let (prompt_file, script) = write_launcher(&dir, &big, "claude").unwrap();

        let body = std::fs::read_to_string(&script).unwrap();
        assert!(
            body.len() < 2_000,
            "prompt was inlined into the script: {} bytes",
            body.len()
        );
        assert!(body.contains(&prompt_file.display().to_string()), "{body}");
        assert_eq!(
            std::fs::read_to_string(&prompt_file).unwrap().len(),
            200_000
        );

        let _ = std::fs::remove_dir_all(prompt_file.parent().unwrap());
    }

    /// An explicit choice always wins — any built-in candidate list is wrong
    /// for somebody.
    #[test]
    fn nao_terminal_overrides_the_probe() {
        std::env::set_var("NAO_TERMINAL", "my-terminal");
        assert_eq!(resolve_terminal().as_deref(), Some("my-terminal"));
        std::env::set_var("NAO_TERMINAL", "   ");
        assert_ne!(resolve_terminal().as_deref(), Some("   "));
        std::env::remove_var("NAO_TERMINAL");
    }
}
