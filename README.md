# Nao

Interactive code visualizer and quality-metrics explorer for VS Code,
backed by a Rust analysis engine.

> **New here? Start with [guide/getting-started.md](guide/getting-started.md)** — a 5-minute install-and-run guide covering the `nao` CLI, the `elevator` CLI, and the VS Code extension. The rest of this README is a feature reference.

> **Big picture:** [guide/capabilities-and-roadmap.md](guide/capabilities-and-roadmap.md) — what Nao can do today (including the MCP tools for AI agents), how the pieces fit together, and where it's headed.

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
| Source files (Rust) | 157 |
| Functions analyzed | 1704 |
| Functions above ceiling (grandfathered) | 109 |
| Cyclomatic complexity (p50 / p90 / max) | 3 / 10 / 40 |
| Cognitive complexity (p50 / p90 / max) | 2 / 14 / 129 |
| Max nesting depth (p50 / p90 / max) | 1 / 3 / 12 |
<!-- repo-health:end -->

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
