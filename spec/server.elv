# Server — the HTTP surface the visualizer and hosted deployments talk to.

import "code_graph.elv"

c server {
    d: "Axum HTTP surface over an analyzed graph, in two deployment shapes that share the handlers: one local root served from disk, and a hosted multi-repo instance that clones on demand."
    cr: "src/server/"
    f local_serve
    f hosted_repos
    f graph_api
    f scope_api
    f access_control
}

f local_serve {
    d: "`nao serve` / `nao watch`: analyze one root, hold it in AppState, serve the UI and stream changes over SSE. spawn_file_watcher re-analyzes on save, gated by is_source_extension and has_source_change so editor noise and non-source writes do not trigger a pass; it takes a second watched root when spec_dir sits outside the first, a spec being something people edit. handle_reanalysis publishes the watched root's config but reads the analysis scope back from the shared one, or the next keystroke would silently undo a narrowing the reader made from the browser. build_router wires the routes; write_json_with_cancel lets an in-flight response abandon work when a newer analysis supersedes it. resolve_startup checks the flags that can fail before any analysis begins, so a typo costs a second rather than a full parse."
    cr: "src/server/mod.rs", "src/server/state.rs"
}

f access_control {
    d: "Who may talk to the server, and where its UI comes from. Both servers bind loopback, which is no boundary against a browser on the same machine, so AccessPolicy replaces permissive CORS with an allowlist: same-origin and vscode-webview:// always, --allow-origin by name, nothing else. needs_token adds a per-process pairing token on top for origins that are neither loopback nor the webview, since allowlisting a host trusts everything served from it; require_token accepts it as a Bearer header or a query parameter, because EventSource cannot set headers. hello_route sits outside both layers and answers everyone, so a refused page can find out it was refused rather than reading a CORS block as a dead port. ui_dir::resolve finds the built UI by flag, env, executable sibling, then working directory, so an installed binary is not stranded without the repo."
    cr: "src/server/access.rs", "src/server/ui_dir.rs"
}

f hosted_repos {
    d: "The multi-tenant shape: a RepoRegistry of RepoSlot entries keyed by slug (is_valid_slug gates the path segment), each with a JobStatus and a StatusEvent stream. parse_github_url accepts the public forms, jobs.rs clones under JobConfig limits — a concurrency JobLimiter, wait_with_timeout, dir_size_bytes as a size ceiling, and sanitize_git_stderr so clone failures cannot echo a token back. Analyzed graphs persist as zstd snapshots (ZSTD_LEVEL) written through write_atomic and rehydrated on restart; unsafe_passes_allowed keeps opt-in passes like trace_exact off in the hosted path."
    cr: "src/server/serve.rs", "src/server/repo.rs", "src/server/jobs.rs"
}

f graph_api {
    d: "The endpoints the canvas reads: graph_handler for nodes and edges, index_handler and details_handler for lazy per-entity detail, commits_handler over git_commits, stashes_handler over git_stashes, and the diff endpoints (diff_handler, set_root_handler, base_details_handler) that drive comparison against a ref. /api/analysis/scope re-runs the analyzer under a new language set, docs switch or spec directory; build_config_with_scope validates before anything expensive, since the walker's fallback lands on a stderr the browser never shows. Its spec_dir is tri-state — absent leaves it, empty clears it, a path sets it — and may name a directory outside the root, which the settings file's key may not: this request came from a page the operator opened, not from a file that arrived with a clone. Educator hits the same router through position_handler, scan_handler and diagnostics_handler. parse_stash_lines is split out of git_stashes and splits each row from BOTH ends — a stash subject embeds a commit subject, which is free text and may hold the separator — and takes the first %P entry as the base, the second being the stashed index and the third the untracked files. It asks for no %gd: with --date=short set for %ad that placeholder renders as stash@{2026-08-12} instead of stash@{0}, so the selector is numbered by row position, which is all it ever was. No serve-mode twin of /api/stashes, unlike commits: serve mode exposes no POST /api/diff, so a stash listing there would be a list of comparisons nothing can run."
    cr: "src/server/handlers.rs", "src/server/diff_handler.rs", "src/server/analysis_handler.rs", "src/server/educator_handler.rs", "src/server/types.rs"
}

f scope_api {
    d: "Assembles a reading scope for an agent or the extension: collect_scope_manual takes an explicit entity set, collect_scope_directed expands from a seed under a traversal profile — refactor_rules follows callers and callees for change-blast questions, understand_rules follows definitions for reading. Returns ScopeEntity bodies with ScopeExports, and read_full_files decides where a whole file beats a stitched set of spans."
    cr: "src/server/scope_handler.rs"
}
