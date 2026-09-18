# Mezzanine — Code Visualizer

An interactive map of your codebase, one floor up. LSP is precise but
pointwise — one symbol at a time. Grep is flexible but structureless.
Documentation is high-level but stale. Mezzanine sits in the unoccupied
middle: a structural view of entities and the typed relationships between
them, always derived from the code as it is.

![Mezzanine's canvas showing one subsystem's files, the dependency edges between them, and the selected file's metrics and source](https://raw.githubusercontent.com/llvator/mezzanine/main/assets/screenshots/graph-and-details.png)

Pick any node and the panels answer for it: where it lives, what it depends
on, what depends on it, and every metric behind the badges.

![Mezzanine's Quality tab showing a repo score, per-metric pass/warn/fail counts, and entities ranked by how far past the bar they are](https://raw.githubusercontent.com/llvator/mezzanine/main/assets/screenshots/quality.png)

The Quality tab measures the whole analysis scope rather than the canvas
selection, so its ranking answers "what in this repo most needs attention"
rather than "what am I looking at".

Rust, TypeScript/JavaScript, Python, Java, Kotlin, Go, Dart and Groovy, all
parsed with tree-sitter.

## Install

Install from the Marketplace and open a folder — that is the whole setup.
Packages for macOS (Apple Silicon), Linux x64 and Windows x64 carry the
analysis engine inside them, so there is no toolchain to install first.

On any other platform (Intel Mac, Linux arm64, Alpine) you get the universal
package, which looks for a `mezz` binary on your `PATH`. Prebuilt binaries
are on the [releases page](https://github.com/llvator/mezzanine/releases),
or point `mezz.binaryPath` at one you built yourself.

## Getting started

1. Open a folder.
2. Click the **Mezzanine** icon in the activity bar.
3. Run **Mezzanine: Open Code Visualizer** from the command palette.

The canvas draws the folder you are in. Check folders in **Visual Scopes**
to change what is drawn, and click a node to fill the panels.

## Panels

Each lives in the Mezzanine activity bar.

- **Visual Scopes** — which files and folders the graph draws.
- **Analysis Scope** — which files and folders are *analyzed*, decoupled
  from display, so you can analyze the whole project and render a corner
  of it. Can follow the active editor.
- **Controls** — aggregation level (entity / file / folder), tree depth and
  density, label toggles, zoom, and bidirectional editor ↔ graph sync.
- **Diff** — what changed between two commits, computed structurally rather
  than textually.
- **Inspector** — the selected entity's kind, location and metrics, with
  **Go to Source**.
- **Description** — what the selected region is, from the project's
  Elevator spec where one exists.
- **Quality** — smells and complexity hotspots across the analysis scope,
  ranked by how far past the bar they are.
- **Educator** — good-practice lessons and rules at the cursor, for Java.

## Commands

| Command | Does |
|---|---|
| Mezzanine: Open Code Visualizer | The full canvas |
| Mezzanine: Visualize Current File | A compact view around the cursor |
| Mezzanine: Select Git Repository… | Choose which checkout to analyze |
| Mezzanine: Restart Server / Stop Server | Manage the analysis engine |

## Settings

| Setting | Default | Does |
|---|---|---|
| `mezz.binaryPath` | *(bundled)* | Path to the engine. Empty uses the one shipped with this extension, falling back to `mezz` on `PATH`. |
| `mezz.serverPort` | `3200` | Port for the internal watch server. |
| `mezz.autoVisualize` | `true` | Re-draw when you switch files. |
| `mezz.includeTests` | `false` | Include test files in the analysis. |
| `mezz.includeDocs` | `false` | Analyze Markdown alongside code — one node per file and the links between them. Off by default because a repo's READMEs would otherwise add hundreds of nodes. |
| `mezz.language` | *(all)* | Restrict analysis to one language, e.g. `ansible` to see only a repo's deploy topology. |
| `mezz.analysisScopeFollowMode` | `folder` | Whether "follow active editor" narrows to the file or its folder. |
| `mezz.educator.hoverEnabled` | `true` | Java good-practice content in the editor hover. |
| `mezz.claudeBinary` | *(PATH)* | Claude Code executable for the Quality view's Refactor action. |

## Beyond the editor

The same engine is a CLI and an MCP server. `mezz analyze`, `mezz quality`
and `mezz check` run in a terminal or in CI, and `mezz mcp` exposes the code
graph to AI agents as 15 tools — the mid-altitude map is precisely what does
not fit in an agent's context window. See the
[repository](https://github.com/llvator/mezzanine) for both.

## Building from source

Clone [llvator/mezzanine](https://github.com/llvator/mezzanine) and run
`./scripts/install.sh`, which builds the engine, the browser UI and this
extension and installs all three. Requires a Rust toolchain and Node 18+.
Per-change rebuild commands are in the repository's
[CONTRIBUTING.md](https://github.com/llvator/mezzanine/blob/main/CONTRIBUTING.md).

## Licence

AGPL-3.0-only, or a commercial licence for organisations that cannot take
the AGPL — see
[COMMERCIAL.md](https://github.com/llvator/mezzanine/blob/main/COMMERCIAL.md).
