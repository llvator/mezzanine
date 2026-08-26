# Mezzanine — Code Visualizer

A VS Code extension that visualises code relationships and dependencies
directly in your editor. Built on top of the `mezz` Rust backend which
parses Rust, Python, JavaScript/TypeScript, Java, Go, Kotlin and Dart via
tree-sitter.

## Install on a new machine (from the git repo)

Requirements: `git`, Rust toolchain (`rustup`), Node.js 18+, VS Code CLI
(`code` on your PATH).

```bash
git clone https://github.com/llvator/mezzanine.git
cd mezz
./scripts/install.sh      # backend binaries + extension, in one step
```

Or run the steps yourself:

```bash
cargo install --path .    # the Rust backend (mezz, elevator)
cd vscode-extension
npm install
npm run install:local     # packages the .vsix and installs it into VS Code
```

Reload VS Code. A "Mezzanine" icon appears in the activity bar. Open a folder
and run **Mezzanine: Open Code Visualizer** from the command palette.

If `rustup` isn't installed yet:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

If the `code` CLI isn't on your PATH (macOS): open VS Code → Command
Palette → **Shell Command: Install 'code' command in PATH**.

## Rebuilding after changes

From `vscode-extension/`:

| What changed | Command | Reload needed |
|---|---|---|
| Rust backend (`src/**`, `Cargo.toml`) | `cargo install --path .. --force` | No (server restarts on next panel open) |
| Extension TypeScript (`vscode-extension/src/**`) | `npm run install:local` | Yes (Developer: Reload Window) |
| Svelte UI (`ui/src/**`) | `npm run install:local` | Yes |
| Both TS + UI | `npm run install:local` | Yes |

`npm run install:local` runs `build:all` (webview + extension), produces
a fresh `.vsix`, and installs it — replacing the previous version.

If you'd rather not think about which command matches which change,
`./scripts/install.sh` from the repo root does all of the above every time,
and takes VS Code profile names as arguments (`./scripts/install.sh Work`).

Note that `build:all` builds the Svelte UI into `webview-dist/` for the
webview only. The browser UI served by `mezz watch` is a separate build into
`ui/dist/` — refresh it with `./scripts/build.sh --ui`.

## Prerequisites (for developers editing the extension)

- The `mezz` binary must be on your `PATH`, or set `mezz.binaryPath` in
  VS Code settings to its absolute location.

## Commands

- **Mezzanine: Open Code Visualizer** — opens the full visualiser panel.
- **Mezzanine: Visualize Current File** — opens a compact view focused on the
  file and entity at your cursor.

## Views (Mezzanine activity bar)

- **Scopes** — native tree with checkboxes; check folders/files to scope
  the visualisation.
- **View Options** — aggregation level, tree depth/density, label
  toggles, zoom, and the **Follow selection to editor** switch for
  bidirectional sync.
- **Selection** — entity details (kind, location, metrics, source)
  plus a "Go to Source" button.

## Settings

- `mezz.binaryPath` — override the mezz binary location.
- `mezz.serverPort` — port for the internal mezz watch server
  (default 3200).
- `mezz.includeTests` — include test files in the analysis.
- `mezz.autoVisualize` — automatically sync when switching files / moving
  the cursor.
