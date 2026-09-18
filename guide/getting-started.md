# Getting started

Mezzanine is two CLI binaries plus a VS Code extension. They share one Rust analysis engine. This page is the install-and-run map; deeper docs are linked at the bottom.

## What you'll have after install

| | What it does | When to use it |
|---|---|---|
| **`mezz`** binary | Parses Rust / Python / JS / TS / Java / Kotlin / Dart / Groovy / Impex / Elevator. Emits JSON / DOT / Mermaid / ASCII output. Powers a watch server with SSE for live reload. | Code analysis from the command line; the watch server the VS Code extension talks to. |
| **`elevator`** binary | Standalone CLI focused on `.elv` spec files. Same engine as `mezz analyze -l elevator`, focused command line. | Working on Elevator (.elv) specs without the rest of Mezzanine. LLM context bundles. |
| **VS Code extension** | Activity-bar view, force-directed graph, side panels, bidirectional editor sync. | Most users — the primary way to use Mezzanine. |

## Install (one-time, ~2 min)

Requires: `git`, [`rustup`](https://rustup.rs), Node 18+, the VS Code `code` CLI on your `$PATH` (in VS Code: *Command Palette → Shell Command: Install 'code' command in PATH*).

```bash
git clone https://github.com/llvator/mezzanine.git
cd mezz
./scripts/install.sh
```

That builds and installs both binaries into `~/.cargo/bin/`, then packages the
VS Code extension and installs it into your default profile. Pass profile names
to target [VS Code profiles](https://code.visualstudio.com/docs/editor/profiles)
instead — `./scripts/install.sh Work Personal`. Set `MEZZ_CODE_CLI=cursor` for a
different editor CLI, or `MEZZ_SKIP_EXTENSION=1` for the binaries alone.

Prefer to run the steps yourself:

```bash
cargo install --path . --force     # both binaries
cd vscode-extension && npm install && npm run install:local
```

**Binaries only, no clone** — installs straight from the default branch:

```bash
cargo install --git https://github.com/llvator/mezzanine.git --force
```

Reload VS Code. The Mezzanine icon appears in the activity bar. To update later,
`git pull` and re-run `./scripts/install.sh`.

## Using each piece

### VS Code extension (recommended for most users)

1. Open a folder.
2. Click the Mezzanine icon in the activity bar.
3. Run **Command Palette → Mezzanine: Open Code Visualizer**.
4. In the **Visual Scopes** panel, check the folders/files you want to analyse.

The graph renders entity nodes (functions, classes, Categories, Features…) with edges for containment and dependencies. Saves trigger live reanalysis.

Full panel reference: [vscode-extension/README.md](../vscode-extension/README.md).

### `mezz` CLI

```bash
# One-shot analysis to JSON
mezz analyze ./my-project -f json -o analysis.json

# Watch mode — re-analyses on file change, serves data + SSE on a port
mezz watch ./my-project --port 3200

# Filter by language, limit depth, scope to a path
mezz analyze ./my-project -l rust -l python -d 5
```

`mezz watch` is what the VS Code extension uses internally. You only run it directly if you want to inspect the JSON output, drive the UI from a browser, or wire Mezzanine into an external tool.

### `mezz monitor` — quality, over time

```bash
mezz monitor .
```

A terminal dashboard, and the one surface with a time axis. It re-measures the
tree whenever it settles and shows each figure — smells, complexity, cycles,
folder shape, declared-rule breaches — with how far it has moved since the
baseline, a sparkline, and a list of which files and folders moved.

It is built for the case where you are not the one editing: a pane beside a
swarm of agents, answering "is this getting better or worse, and since when"
at a glance. No browser, no port, no build step.

**The baseline is the last commit.** Everything on screen is a delta against
`HEAD`, measured out of a checkout of it, so the work already sitting
uncommitted in the tree is inside the numbers from the first frame rather than
being the zero they are measured from. `--baseline <ref>` names a different
one (`main`, `HEAD~5`, a SHA); `--baseline working` measures from the tree as
found when the session starts.

The ref is pinned for the session — a swarm that commits four times an hour
would otherwise reset your numbers four times. When HEAD moves past the pin
the header says so, and `B` re-measures the baseline there.

```
q  quit          b  make now the new baseline
p  pause         B  re-baseline at whatever HEAD is now
↑↓ scroll the movers list
```

**Two commits, without watching anything.** `--against <ref>` pins the head
side to a commit as well, and the dashboard becomes a still of the distance
between two states:

```bash
mezz monitor . --baseline v0.4.0 --against HEAD     # what the release cost
mezz monitor . --baseline main --against my-branch  # what the branch did
```

Both sides are measured out of a checkout, so nothing uncommitted is in either
figure and either end can be a state you are nowhere near. There is no tree
being watched and no time axis, so the sparklines are empty and the three keys
that move a zero — `b`, `B`, `p` — are not offered; `q` and the movers list
are. Everything else reads the same, the movers list included: it is the list
of files and folders that differ between the two commits.

Useful flags: `--debounce-ms` (how long the tree must be quiet before it is
measured, default 1000), `--min-interval-ms` (never measure more often than
this, default 2000), and `--log session.jsonl` to keep one JSON line per
reading for afterwards — including the baseline, marked `"baseline": true`,
so a session's deltas can be recomputed from the file.

### Browser UI, served yourself

The visualizer is an ordinary static site and does not have to come from the
engine. `npm run build` in `ui/` writes `ui/dist`; serve that from anywhere and
point it at a running engine:

```bash
cd ui && npm run build
npx vite preview --port 4173                                  # or any static host

mezz watch . --port 3200 --allow-origin http://localhost:4173  # in another terminal
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
mezz watch . --port 3200 --allow-origin http://localhost:4173
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
more gate that is neither mezz's nor yours: the browser's Local Network Access
permission. That is why the standalone build is documented and a hosted
live-connect page is not — measured, with versions, in
ADR 0006.
The loopback-to-loopback recipe above is unaffected.

### VS Code tasks for the browser UI

`mezz init --vscode` writes three tasks into `.vscode/tasks.json`, so the UI is
a **Run Task** away rather than a terminal you have to keep:

| Task | Does |
|---|---|
| `Mezzanine: Start web UI` | `mezz watch .`, in the background |
| `Mezzanine: Open web UI in browser` | opens the port, starting the engine first |
| `Mezzanine: Stop web UI` | kills the engine holding that port — and only if it *is* mezz |

All three carry a port number, baked in when the file is written. It is read
from `.mezz/settings.json`, which `mezz init` pins in the same run — so the
three tasks and the engine they drive cannot drift apart, and moving the port
is one edit in one file followed by `mezz init --vscode --force`. Repos you
open side by side each need their own: the tasks bind, they do not negotiate.

A fourth is written only when asked for, because it is the one that runs code
rather than serving data:

```sh
mezz init --vscode --allow-agent-spawn
```

| Task | Does |
|---|---|
| `Mezzanine: Start web UI (agent spawn)` | `mezz watch . --allow-agent-spawn`, so the quality tables offer a **Refactor** button that opens a Claude Code terminal here |

It is a second task rather than a flag on the first, so the engine a reader
starts by default still cannot spawn anything and picking the other one is a
deliberate act with the reason on the label. It binds the same port, so stop
the plain engine before starting it. `--all` does **not** include it: that
means every optional *file*, and this is not a file. See
[canvas-to-agent.md](workflows/web-ui/canvas-to-agent.md).

The stop task exists because `mezz watch` has no idle shutdown: closing the
browser tab leaves the engine running and the port taken. All three agree on
one port — whatever `.mezz/settings.json` pins, else 3000.

### VS Code tasks for the graph tools

The tasks above are about the browser UI. `--editor-tools` writes a different
kind: eleven of the graph tools, each already pointed at whatever file the
editor has open, so asking one costs a **Run Task** instead of a path you have
to type.

```sh
mezz init --vscode --editor-tools
```

| Task | Runs |
|---|---|
| `Mezzanine: Map this file` | `mezz map <file>` |
| `Mezzanine: Quality of this file` | `mezz quality <file>` |
| `Mezzanine: Dead code in this file` | `mezz dead-code <file>` |
| `Mezzanine: What depends on this file` | `mezz impact --path <file>` |
| `Mezzanine: Impact of the entity at the cursor` | `mezz impact --path <file> --line <cursor>` |
| `Mezzanine: Context for the entity at the cursor` | `mezz context --path <file> --line <cursor>` |
| `Mezzanine: Tests covering the entity at the cursor` | `mezz tests-for --path <file> --line <cursor>` |
| `Mezzanine: Reshape this file's folder` | `mezz reshape <folder>` |
| `Mezzanine: Layout of this file's folder` | `mezz layout <folder>` |
| `Mezzanine: Boundaries of this file's folder` | `mezz boundaries <folder>` |
| `Mezzanine: Hotspots in this file's folder` | `mezz hotspots <folder>` |

The substitution is VS Code's, not mezz's: `${relativeFile}`,
`${relativeFileDirname}` and `${lineNumber}` are resolved before the process
starts. With no editor open there is nothing to resolve, and VS Code says so
instead of running the task against the wrong thing.

Two tasks ask about `impact` because it answers two questions. Without
`--line` the file is the subject — who breaks if this file changes, and what
it owes the rest of the tree. With `--line` the subject is the entity spanning
the cursor, and the answer is that entity's blast radius.

`Quality of this file` is the one task that also fills the **Problems** panel:
its smells arrive as warnings on the lines that carry them, so they are
clickable rather than terminal text you re-read. Its refactor-pressure ranking
deliberately does not — that list opens by calling itself "a ranking, not a
verdict", and the Problems panel is where verdicts go.

Like `--allow-agent-spawn`, this is outside `--all`, for a plainer reason:
eleven entries land in a list you share with your own builds and tests. That is
worth choosing rather than inheriting from a flag that means "everything".

Four of the fifteen tools have no task, because the editor has no answer to
give them: `overview`, `similar` and `trace` want a name, a query or a pair of
entities; `assess_change` wants a git ref and is about the working tree rather
than any one file. `spec-slice` is left out too — it writes a file, which is a
command to run deliberately.

Each task re-analyzes from the repository root, because a question about one
file's coupling is a question about everything that could reach it. The graph
cache makes the runs after the first one cheap.

An existing `tasks.json` is merged into by label, leaving your own tasks
alone; `--force` replaces Mezzanine tasks whose bodies have since changed. One
refusal is deliberate: VS Code accepts comments in `tasks.json` and JSON does
not, so a file mezz cannot parse is left **untouched** with the tasks printed
for you to paste — rewriting it would delete the comments.

### MCP registration for agents

`mezz init --mcp` writes `.mcp.json`, so an agent opened in the repo has the
code graph — `map`, `quality`, `assess_change` and the rest — without anyone
wiring it up:

```json
{ "mcpServers": { "mezz": { "command": "mezz", "args": ["mcp"] } } }
```

The binary by name rather than the path your machine has it at, because the
file is committed and an absolute path out of your home directory registers a
server none of your colleagues can start. It has to be on `PATH`, and mezz
says so when it is not. No path argument either: `mezz mcp` defaults its root
to the directory the agent runs in.

Other servers in that file are merged around, never replaced, and an entry
already named `mezz` is left alone — somebody may have pointed it at a local
build on purpose — until `--force`. A file mezz cannot parse as
`{"mcpServers": {…}}` is left **untouched**, with the entry printed for you
to paste, for the same reason as `tasks.json`: what is registered there is
worth more than what we came to add.

### Push-mode hooks for agents

`mezz init --hooks` wires the two push-mode legs into `.claude/settings.json`
as `Stop` hooks. `--mcp` above gives an agent tools it has to *remember* to
call; this is the half that arrives on its own:

```json
{ "type": "command",
  "command": "o=$(mezz hook self-review --block 2>/dev/null); r=$?; [ -n \"$o\" ] && echo \"$o\" >&2; [ $r -eq 2 ] && exit 2; exit 0" }
```

An agent that ends a turn having introduced a new smell, dependency cycle,
complexity jump, folder-shape fall or `.mezz/rules.json` breach is stopped
once, handed the finding, and can fix it before the turn ends. A clean tree
says nothing at all.

The wrapper is not decoration. `mezz` writes findings to stdout and its
progress to stderr precisely so a caller can drop one and keep the other; a
`Stop` hook that exits 0 has its stdout sent to a debug log rather than the
agent, so `--block` exits **2** instead — the one channel `Stop` has — and the
findings are re-emitted on stderr where a blocked stop reads them. Only exit 2
propagates, so a `mezz` that is missing or half-built ends the hook quietly
instead of wedging every stop in the repo.

Nothing blocks twice: a finding is recorded as it is emitted, so the next run
no longer counts it as new. Each regression costs at most one extra turn, and
a finding that gets fixed reports `✓ resolved` without blocking at all.

Your own `Stop` hooks are merged around, never replaced, and `--force`
replaces only the legs mezz itself wrote. A file mezz cannot parse is left
**untouched** — your permission allowlist is worth more than our hooks.

Full detail: [Push review](workflows/cli/push-review.md).

One file `mezz init` deliberately does **not** write is
`.vscode/settings.json`. The extension's `mezz.language` and
`mezz.includeTests` reach the engine as CLI flags, and a flag beats
`.mezz/settings.json` — so scaffolding them would hand the repo a second,
uncommitted copy of its own configuration that silently wins. `mezz.language`
is worse still: one string against the pinned list, so writing it into a
Rust-and-TypeScript repo would drop TypeScript from every graph the extension
draws. Leave those unset and let the committed file rule.

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
but the browser UI that `mezz watch` serves comes from `ui/dist/`. After a
`ui/**` change, run `./scripts/build.sh --ui` if you use the browser UI.

`mezz watch` finds that build in the first place it exists: `--ui-dir <path>`,
then `MEZZ_UI_DIR`, then `ui_dir` in your settings file (below), then a
`ui/dist` beside the `mezz` binary, then `./ui/dist`. Running from the repo
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
| **User** | Properties of *this installation* | `$XDG_CONFIG_HOME/mezz/settings.json`, else `~/.config/mezz/settings.json` |
| **Repo** | Properties of *the analyzed repo* | `<repo>/.mezz/settings.json` — commit it |

Both are optional, both are plain JSON, and the keys are the CLI flag
names. **JSON means no comments and no trailing commas** — mezz warns and
discards the whole file on a parse error, so one stray `//` costs you every
setting in it.

`mezz init` writes the repo file for you. It walks the tree — honoring
`.gitignore`, so vendored code never votes — and pins the languages the repo
is *actually* written in, plus `spec_dir` when every `.elv` file sits in one
directory:

```sh
mezz init                 # in the repo you want set up
mezz init --vscode        # also add the browser-UI tasks (below)
mezz init --mcp           # also register the MCP server in .mcp.json (below)
mezz init --hooks         # also wire the push-mode Stop hooks (below)
mezz init --all           # every optional file above
mezz init --allow-agent-spawn  # also the VS Code task that starts the engine
                              # with --allow-agent-spawn (implies --vscode)
mezz init --editor-tools  # also a VS Code task per graph tool, each scoped
                          # to the open file (implies --vscode, below)
mezz init --force         # replace what is already there
```

It writes only what it inferred. Keys with working defaults — `port`,
`output_dir` — are left out on purpose: a scaffold that spelled out today's
defaults would freeze them into every repo that ran it. A stray `.py` script
does not make yours a Python repo either; a language has to hold a twentieth
of the tree, with Elevator exempt because a spec is outnumbered by design.

For the fuller starting point, there is a ready-made template in
[`.mezz.example/`](../.mezz.example/README.md) — copy it to `.mezz/`, edit, and
commit:

```sh
cp -R .mezz.example .mezz
```

`~/.config/mezz/settings.json` — yours, not the repo's. Most people need
nothing here: `scripts/install.sh` puts the browser UI beside the installed
binary, and `mezz watch` finds it there without being told. Set `ui_dir` only
to override that:

```json
{ "ui_dir": "/path/to/a/built/ui/dist" }
```

`<repo>/.mezz/settings.json` — the repo's, shared with everyone who clones it:

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
CLI flag  →  env var  →  your repos entry  →  repo settings  →  user settings  →  built-in defaults
```

### Your own settings for one repo

`repos` is the one link that breaks the "repo beats user" rule, and it sits in
the user file. It holds repo-scope settings keyed by a checkout's path:

```json
{
  "repos": {
    "~/src/app": { "spec_dir": "/abs/path/sibling-docs" },
    "~/work/monorepo/services/api": { "language": ["go"], "max_depth": 2 }
  }
}
```

An entry may set anything a repo file may set, it wins against that repo's own
`.mezz/settings.json`, and a flag still beats it. Keys are matched by resolved
directory, so `~`, a symlinked path and a trailing slash all find the same
entry — and an entry for a repo you are not analyzing is simply dormant.

Reach for it when a setting is true of one checkout *on your machine* and does
not belong in a file everyone clones — most often a `spec_dir` outside the
tree, which the repo file may not name at all (below). Anything true of the
repo for everyone belongs in the repo file, where it is version-controlled
beside the code.

`repos` is user-scope only, and it is **rejected** in a repo file rather than
merely ignored: a block a stranger shipped would choose what mezz reads for
every checkout on the machine, not just its own.

### What goes where

| Key | User | Repo |
|---|:---:|:---:|
| `ui_dir`, `content_fallback` | ✓ | — |
| `repos` | ✓ | — |
| `output_dir`, `spec_dir` | — | ✓ |
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
of them on by existing. They are flag-and-env only. `mezz serve` goes further
and ignores a submitted repo's settings file entirely.

`spec_dir` is the odd one out: a path mezz *reads from*, rather than a filter
over what it already found. A repo-scope one must therefore stay inside the
repo — relative, no `..` — for the same reason as the last row. Specs that
live somewhere else entirely (a sibling docs repo, or the top of a monorepo
whose services you watch one at a time) are a real layout, but naming that
directory takes an operator. Three ways to be one, from least durable to most:

- `mezz watch . --spec-dir ../docs/domain` — this invocation only.
- The **Spec folder** field in the browser UI's Analysis scope panel — the
  running session only, without touching any file. Saving it as the repo's
  default is refused, for the reason above.
- A `repos` entry in your user file (above) — every command, every session,
  including `mezz watch`, `mezz mcp` and the VS Code extension, none of which
  is launched with a flag you could add.

Three behaviours worth knowing:

- **`spec_dir` narrows as well as widens.** Once set, an `.elv` file counts as
  spec only if it lives there — which is the fix for a tree whose `examples/`
  or fixtures contain `.elv` files nobody meant to publish. Unset, every
  `.elv` under the root is the spec, as it has always been. A `spec_dir`
  pointing at a directory that isn't there says so and falls back to that
  default, rather than reporting an empty spec.
- **`exclude_patterns` and `include_patterns` extend the defaults**, they don't
  replace them. Adding `**/generated/**` does not re-enable scanning
  `node_modules`. `language` and `kind` work the other way — they're a choice,
  so the narrower scope wins outright.
- **A pattern is matched against the path relative to the repo root** — the
  same root the settings file is read from. So `src/contracts.d.ts` excludes
  that file whether you run `mezz analyze .`, `mezz analyze src`, or
  `mezz analyze /path/to/repo/src`; one spelling works from anywhere. Analyze a
  directory that isn't in a checkout and the analyzed root stands in for the
  repo root. Note that `*` crosses `/`, so `*.d.ts` matches `src/a.d.ts` and
  the leading `**/` above is decorative — surprising, but every pattern
  written so far relies on it.
- **A bad settings file is never fatal.** Malformed JSON, an unknown key, or a
  key in the wrong scope prints a warning naming the key and the file, then the
  command runs as if that line weren't there.

`MEZZ_CONFIG_DIR` overrides the user-scope location, mostly for tests and
wrapper scripts.

## Installing the extension into multiple VS Code profiles

If you keep separate [VS Code profiles](https://code.visualstudio.com/docs/editor/profiles)
(e.g. Playground vs Work) and want Mezzanine in both, pass their names:

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
- **Extension installed but the Mezzanine icon doesn't appear** — reload the window (`Cmd+Shift+P → Developer: Reload Window`).
- **Graph shows file nodes instead of entity nodes** — the auto-level picker has escalated to "file" because there are too many entities for the viewport budget. In View Options, pin the level to "entity", or scope to a smaller folder via the Visual Scopes panel.
- **`.elv` file edits don't refresh the graph** — make sure your installed `mezz` binary is up to date (`cargo install --path . --force`); pre-2026 versions had a watcher gap on `.elv`.
