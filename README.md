# Nao

Interactive code visualizer and quality-metrics explorer for VS Code,
backed by a Rust analysis engine.

Project website: **[llvator.com](https://llvator.com)**

> **New here? Start with [guide/getting-started.md](guide/getting-started.md)** — a 5-minute install-and-run guide covering the `nao` CLI, the `elevator` CLI, and the VS Code extension. The rest of this README is a feature reference.

> **Big picture:** [guide/capabilities-and-roadmap.md](guide/capabilities-and-roadmap.md) — what Nao can do today (including the MCP tools for AI agents), how the pieces fit together, and where it's headed.

> **Working patterns:** [guide/workflows/](guide/workflows/) — what each surface is actually good at, split into [terminal](guide/workflows/cli/) workflows (the two CLIs and the MCP tools) and [visualizer](guide/workflows/web-ui/) workflows (the canvas).

## VS Code extension

The extension is the primary way to use Nao. It adds a dedicated
activity-bar view with an interactive force-directed graph of your
codebase and bidirectional sync with the editor.

Install and rebuild instructions:
[vscode-extension/README.md](vscode-extension/README.md).

### Side panels

Each panel lives in the **Nao** activity bar.

- **Visual Scopes** — checkbox tree controlling which files/folders
  appear in the graph.
- **Analysis Scope** — checkbox tree controlling which files/folders
  are analyzed (decoupled from display, so you can analyze the whole
  project while rendering a subset).
- **View Options** — aggregation level (file / class / module), tree
  depth and density, label toggles, zoom, and a **Follow selection**
  switch for bidirectional editor ↔ graph sync.
- **Filters** — by language and entity kind.
- **Selected Node Filters** — focused filters applied to the current
  selection.
- **Diff** — highlight what changed between two commits (uses `git`
  worktrees to compute a structural diff, not just a textual one).
- **Selection** — details for the selected entity (kind, location,
  metrics) with a **Go to Source** button.
- **Context** — the surrounding relationships of the selected entity.
- **Quality** — ranked list of entities with metrics and code-smell
  badges. Sortable by any of the metrics below.

### Quality metrics

Per entity (functions, methods, classes/structs, files):

- Cyclomatic complexity
- Cognitive complexity
- Fan-in / fan-out
- Weighted Methods per Class (WMC)
- Dependency-chain depth
- PageRank (how "central" the entity is in the call/use graph)

### Code smells

- **God Class**
- **Dispatcher**
- **Feature Envy**
- **Shotgun Surgery**
- **Data Bag** (informational — flagged separately from the red smells
  since Kotlin `data class` and Python `@dataclass` are idiomatic uses
  of the same pattern).

### Commands

- **Nao: Open Code Visualizer** — opens the full visualizer panel.
- **Nao: Visualize Current File** — compact view focused on the entity
  at your cursor.

### Settings

- `nao.binaryPath` — override the `nao` binary location.
- `nao.serverPort` — port for the internal nao watch server
  (default `3200`).
- `nao.includeTests` — include test files in the analysis.
- `nao.includeDocs` — analyze Markdown documents alongside the code
  (see [Turning on the documentation layer](#turning-on-the-documentation-layer)).
- `nao.autoVisualize` — automatically sync when switching files or
  moving the cursor.

## Supported languages

Ten languages have dedicated parsers. They differ in what they give you, so
the tiers below are by capability rather than by a first-class/fallback split.

**Full: entities, call edges, and exact `UsesType` edges from signatures and
fields** — this is the tier where `impact` is type-accurate.

| Language | Notes |
|---|---|
| Rust | Also the only language with an optional exact call-edge path (`NAO_LSP_EXACT=1`, via rust-analyzer) |
| Python | |
| JavaScript / TypeScript | |
| Java | |
| Kotlin | |
| Groovy | Includes Spring bean and embedded-script handling. Only what the code declares: `def` is the absence of a type, so dynamically-typed members carry no edge |
| Svelte | Component-level; much smaller in scope than the others |

**Entities and call edges, no type edges** — `impact` here approximates type
usage via "used via members".

| Language | Notes |
|---|---|
| Impex (SAP Hybris) | Hand-rolled parser; no tree-sitter grammar exists for the format |
| Ansible / Kubernetes | Topology-oriented: playbooks, roles, vars and templates, not individual tasks |
| Elevator (`.elv`) | The domain-spec language, not source code |
| Markdown (`.md`) | Documents and the links between them, plus the source files they point at. **Opt-in** — see below |

### Turning on the documentation layer

Markdown is the one language nao does not read by default. `.md` is everywhere
in a code repo — READMEs, ADRs, changelogs, issue trackers — and claiming it
unasked would add hundreds of nodes to every graph. Two ways to ask, answering
two different questions:

```sh
nao analyze . --include-docs   # the normal analysis, plus its docs
nao analyze . -l markdown      # the docs alone, as a pure note graph
nao watch . --include-docs     # same, live
```

`--include-docs` **widens**; `-l` **restricts**, as it always does.

Every surface has the same switch:

| Surface | Control |
|---|---|
| CLI | `--include-docs` on `analyze` and `watch` |
| Browser UI / webview | **Include documentation**, in the Parsed Languages panel |
| VS Code | the `nao.includeDocs` setting — then reload the window |

The extension spawns `nao watch` for you, which is why a flag typed in a
terminal never reaches it. The in-panel switch takes effect on **Apply**
without a restart, and reports the server's real state on load rather than
assuming.

Ticking `markdown` in the language list works too, but it is a different
operation: the switch *adds* docs to whatever is selected, the checkbox
*restricts* to a set that contains them. Selecting only `markdown` is how you
get the pure note graph.

To leave it on for every surface at once, put it in the settings file:

```jsonc
// .nao/settings.json  (repo)  or  ~/.config/nao/settings.json  (user)
{ "include_docs": true }
```

A pinned `"language"` list does **not** override this. Asking for docs
explicitly beats a config file that never mentioned them.

Everything else — **including Go** — falls back to a generic parser with
reduced fidelity: entities but no reliable relationships.

Call-edge accuracy is measured, not assumed: **93.7% recall at 98.6%
precision** against a rust-analyzer oracle on this repo. That is Rust-only;
see [Honest limitations](guide/capabilities-and-roadmap.md#honest-limitations)
for what it means in practice and what is *not* measured.

## CLI (secondary)

A `nao` binary is installed as a prerequisite of the extension and can
also be used directly:

```bash
# Set a repo up: pin the languages it is written in, in .nao/settings.json
# (--vscode also adds tasks that start, open and stop the browser UI)
nao init ./my-project --vscode

# Serve an analysis with live reload (used by the extension)
nao watch ./my-project --port 3200

# One-shot JSON analysis
nao analyze ./my-project -f json -o analysis.json

# Filter by language
nao analyze ./my-project -l rust -l python

# Scope to a subtree / limit traversal depth
nao analyze ./my-project -d 5
```

## Repo health

A snapshot of `src/` against the same complexity ceiling CI enforces (cyclomatic ≤ 15, cognitive ≤ 22, max nesting ≤ 4). Regenerated by `scripts/update_repo_health.py` and kept current by the pre-commit hook (install with `bash scripts/install_hooks.sh`).

<!-- generated by scripts/update_repo_health.py — do not edit by hand -->
<!-- repo-health:start -->
| Metric | Value |
|---|---|
| Source files (Rust) | 163 |
| Functions analyzed | 2048 |
| Functions above ceiling (grandfathered) | 110 |
| Cyclomatic complexity (p50 / p90 / max) | 2 / 9 / 41 |
| Cognitive complexity (p50 / p90 / max) | 1 / 12 / 129 |
| Max nesting depth (p50 / p90 / max) | 1 / 3 / 12 |
<!-- repo-health:end -->

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
