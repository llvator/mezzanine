# Getting started

Nao is two CLI binaries plus a VS Code extension. They share one Rust analysis engine. This page is the install-and-run map; deeper docs are linked at the bottom.

## What you'll have after install

| | What it does | When to use it |
|---|---|---|
| **`nao`** binary | Parses Rust / Python / JS / TS / Java / Kotlin / Groovy / Impex / Elevator. Emits JSON / DOT / Mermaid / ASCII output. Powers a watch server with SSE for live reload. | Code analysis from the command line; the watch server the VS Code extension talks to. |
| **`elevator`** binary | Standalone CLI focused on `.elv` spec files. Same engine as `nao analyze -l elevator`, focused command line. | Working on Elevator (.elv) specs without the rest of Nao. LLM context bundles. |
| **VS Code extension** | Activity-bar view, force-directed graph, side panels, bidirectional editor sync. | Most users — the primary way to use Nao. |

## Install (one-time, ~2 min)

Requires: `git`, [`rustup`](https://rustup.rs), Node 18+, the VS Code `code` CLI on your `$PATH` (in VS Code: *Command Palette → Shell Command: Install 'code' command in PATH*).

```bash
git clone https://github.com/llvator/nao.git
cd nao
./scripts/install.sh
```

That builds and installs both binaries into `~/.cargo/bin/`, then packages the
VS Code extension and installs it into your default profile. Pass profile names
to target [VS Code profiles](https://code.visualstudio.com/docs/editor/profiles)
instead — `./scripts/install.sh Work Personal`. Set `NAO_CODE_CLI=cursor` for a
different editor CLI, or `NAO_SKIP_EXTENSION=1` for the binaries alone.

Prefer to run the steps yourself:

```bash
cargo install --path . --force     # both binaries
cd vscode-extension && npm install && npm run install:local
```

**Binaries only, no clone** — installs straight from the default branch:

```bash
cargo install --git https://github.com/llvator/nao.git --force
```

Reload VS Code. The Nao icon appears in the activity bar. To update later,
`git pull` and re-run `./scripts/install.sh`.

## Using each piece

### VS Code extension (recommended for most users)

1. Open a folder.
2. Click the Nao icon in the activity bar.
3. Run **Command Palette → Nao: Open Code Visualizer**.
4. In the **Visual Scopes** panel, check the folders/files you want to analyse.

The graph renders entity nodes (functions, classes, Categories, Features…) with edges for containment and dependencies. Saves trigger live reanalysis.

Full panel reference: [vscode-extension/README.md](../vscode-extension/README.md).

### `nao` CLI

```bash
# One-shot analysis to JSON
nao analyze ./my-project -f json -o analysis.json

# Watch mode — re-analyses on file change, serves data + SSE on a port
nao watch ./my-project --port 3200

# Filter by language, limit depth, scope to a path
nao analyze ./my-project -l rust -l python -d 5
```

`nao watch` is what the VS Code extension uses internally. You only run it directly if you want to inspect the JSON output, drive the UI from a browser, or wire Nao into an external tool.

### Browser UI, served yourself

The visualizer is an ordinary static site and does not have to come from the
engine. `npm run build` in `ui/` writes `ui/dist`; serve that from anywhere and
point it at a running engine:

```bash
cd ui && npm run build
npx vite preview --port 4173                                  # or any static host

nao watch . --port 3200 --allow-origin http://localhost:4173  # in another terminal
open http://localhost:4173
```

The page opens on a connect screen: type `3200`, it checks the engine before
committing, and remembers it for next time. `http://localhost:4173/?api=http://localhost:3200`
skips the screen, which is the form to bookmark.

This is the third way to run the UI, alongside the VS Code extension and letting
the engine serve `ui/dist` itself. Nothing about the engine changes — it has
never had a compile-time dependency on the UI, and after this it no longer needs
the files to be anywhere near it either.

**Why the flag.** The watch server answers a browser page
only when that page came from the server itself or from the VS Code webview.
Binding loopback is not a boundary here — the browser is on the same machine as
the server, so any page you happen to have open could otherwise read
`/api/details`, which contains your source. So any other origin has to be named:

```bash
nao watch . --port 3200 --allow-origin http://localhost:4173
```

The flag is repeatable, and every origin you pass is echoed in the startup
banner so an unexpected one is visible rather than silent. Non-browser clients
(`curl`, an agent) are unaffected either way: this is a browser-enforced policy,
not authentication.

An origin that isn't loopback — a UI on a real host rather than one you started
on your own machine — must also send the **pairing token** the banner prints,
because allowlisting a host trusts everything served from it, forever. Paste it
into the UI's connect field, or send it yourself:

```bash
curl -H 'Origin: https://example.com' -H 'Authorization: Bearer <token>' \
  http://localhost:3200/api/graph
```

The token is new on every run, so restarting the engine invalidates it.
`--no-token` turns the requirement off if you'd rather it were only an
allowlist.

A UI on a *hosted* `https://` page reaching `http://localhost` runs into one
more gate that is neither nao's nor yours: the browser's Local Network Access
permission. That is why the standalone build is documented and a hosted
live-connect page is not — measured, with versions, in
ADR 0006.
The loopback-to-loopback recipe above is unaffected.

### `elevator` CLI (`.elv` spec files)

```bash
elevator ./my-spec                              # full project map
elevator ./my-spec --focus fu.protocol.creation # LLM context for one entity
elevator ./my-spec --check                      # spec checker (CI-friendly)
elevator ./my-spec --code-map                   # path → entities, surfaces duplicates
elevator --docs                                 # full language reference
```

Full reference: [elevator-language.md](elevator-language.md).

## Updating after a code change

```bash
./scripts/install.sh          # rebuilds and reinstalls everything you changed
```

Then in VS Code: `Cmd+Shift+P → Developer: Reload Window`.

The script is safe to re-run for any change — it reinstalls both binaries and
repackages the extension regardless of what you touched. For who-rebuilds-what
if you'd rather run the narrower command, see the table in
[vscode-extension/README.md § Rebuilding](../vscode-extension/README.md#rebuilding-after-changes).

**While iterating**, `scripts/build.sh` compiles without installing:

```bash
./scripts/build.sh --debug    # fastest — binaries to target/debug/
./scripts/build.sh --all      # binaries + both frontend builds
```

One thing the install path does *not* cover: `ui/` has two build targets.
`install.sh` refreshes `vscode-extension/webview-dist/` (the VS Code webview),
but the browser UI that `nao watch` serves comes from `ui/dist/`. After a
`ui/**` change, run `./scripts/build.sh --ui` if you use the browser UI.

`nao watch` finds that build in the first place it exists: `--ui-dir <path>`,
then `NAO_UI_DIR`, then `ui_dir` in your settings file (below), then a
`ui/dist` beside the `nao` binary, then `./ui/dist`. Running from the repo
root hits the last one, which is why it needs no flag. Anywhere else — or
from a `cargo install`ed binary with no checkout — set `ui_dir` once in your
settings file, pass `--ui-dir`, or let the server come up without a UI and
read the page it serves at its root, which lists the options.

## Settings files

Anything you would otherwise retype on every run can be written down once.
There are two files, and which one a setting belongs in depends on what it
describes:

| File | Holds | Found at |
|---|---|---|
| **User** | Properties of *this installation* | `$XDG_CONFIG_HOME/nao/settings.json`, else `~/.config/nao/settings.json` |
| **Repo** | Properties of *the analyzed repo* | `<repo>/.nao/settings.json` — commit it |

Both are optional, both are plain JSON, and the keys are the CLI flag
names. **JSON means no comments and no trailing commas** — nao warns and
discards the whole file on a parse error, so one stray `//` costs you every
setting in it.

There is a ready-made template in
[`.nao.example/`](../.nao.example/README.md) — copy it to `.nao/`, edit, and
commit:

```sh
cp -R .nao.example .nao
```

`~/.config/nao/settings.json` — yours, not the repo's. Most people need
nothing here: `scripts/install.sh` puts the browser UI beside the installed
binary, and `nao watch` finds it there without being told. Set `ui_dir` only
to override that:

```json
{ "ui_dir": "/path/to/a/built/ui/dist" }
```

`<repo>/.nao/settings.json` — the repo's, shared with everyone who clones it:

```json
{
  "language": ["rust", "typescript"],
  "include_tests": true,
  "exclude_patterns": ["**/generated/**"],
  "port": 3300
}
```

A flag always beats the file, and the repo file beats the user file:

```text
CLI flag  →  env var  →  repo settings  →  user settings  →  built-in defaults
```

### What goes where

| Key | User | Repo |
|---|:---:|:---:|
| `ui_dir`, `content_fallback` | ✓ | — |
| `output_dir` | — | ✓ |
| `language`, `kind` | ✓ | ✓ |
| `include_tests`, `include_external` | ✓ | ✓ |
| `exclude_patterns`, `include_patterns` | ✓ | ✓ |
| `max_depth`, `min_weight` | ✓ | ✓ |
| `port`, `debounce_ms` | ✓ | ✓ |
| `allow_agent_spawn`, `no_token`, `allow_origin`, `allow_unsafe_passes` | — | — |

The last row is deliberate. Those four grant capability — opening a terminal
on your machine, dropping the pairing-token requirement, letting another
browser origin read your source, permitting analysis passes that execute build
scripts out of the tree — and a repo you cloned should not be able to turn any
of them on by existing. They are flag-and-env only. `nao serve` goes further
and ignores a submitted repo's settings file entirely.

Two behaviours worth knowing:

- **`exclude_patterns` and `include_patterns` extend the defaults**, they don't
  replace them. Adding `**/generated/**` does not re-enable scanning
  `node_modules`. `language` and `kind` work the other way — they're a choice,
  so the narrower scope wins outright.
- **A bad settings file is never fatal.** Malformed JSON, an unknown key, or a
  key in the wrong scope prints a warning naming the key and the file, then the
  command runs as if that line weren't there.

`NAO_CONFIG_DIR` overrides the user-scope location, mostly for tests and
wrapper scripts.

## Installing the extension into multiple VS Code profiles

If you keep separate [VS Code profiles](https://code.visualstudio.com/docs/editor/profiles)
(e.g. Playground vs Work) and want Nao in both, pass their names:

```bash
./scripts/install.sh Playground Work
```

The extension is packaged once and installed into each profile in turn. Reload
each profile's window after install (`Cmd+Shift+P → Developer: Reload Window`).

## Where to go next

| You want to… | Read |
|---|---|
| Put it to work — the patterns each surface is good at | [workflows/](workflows/) — [terminal](workflows/cli/) and [visualizer](workflows/web-ui/) |
| Learn the Elevator (.elv) spec language | [elevator-language.md](elevator-language.md) — or just run `elevator --docs` |
| Understand the VS Code panels in detail | [vscode-extension/README.md](../vscode-extension/README.md) |
| See the project's high-level features (quality metrics, code smells) | [README.md](../README.md) |
| Understand the domain glossary | [CONTEXT.md](../CONTEXT.md) |

## Troubleshooting

- **`elevator: command not found`** — `cargo install --path .` didn't run, or `~/.cargo/bin` isn't on your `$PATH`. Check `echo $PATH | tr ':' '\n' | grep cargo`.
- **`code: command not found`** — install the VS Code CLI: *Command Palette → Shell Command: Install 'code' command in PATH*.
- **Extension installed but the Nao icon doesn't appear** — reload the window (`Cmd+Shift+P → Developer: Reload Window`).
- **Graph shows file nodes instead of entity nodes** — the auto-level picker has escalated to "file" because there are too many entities for the viewport budget. In View Options, pin the level to "entity", or scope to a smaller folder via the Visual Scopes panel.
- **`.elv` file edits don't refresh the graph** — make sure your installed `nao` binary is up to date (`cargo install --path . --force`); pre-2026 versions had a watcher gap on `.elv`.
