# Changelog

## 1.5.0 — 2026-09-18

**The graph tools answer on the command line too.** Every tool the MCP
server exposes is now a `mezz` subcommand with the same implementation and
the same footer, so a question you asked an agent you can ask yourself, and
a script can read the answer as a value rather than scrape it from prose.

**C++ and C.** Read together, because they share their headers. Field reads
through `this->` are counted, the bare spelling is not, and the parser says
so rather than guessing.

**Dockerfiles and Compose files** are read as one build-and-run topology,
with each entity showing the lines that define it.

### Asking about a change

- The Changes pane became a view rather than a strip: files, names, room to
  read them, grouped into the folders the change happened in, and filterable
  to one kind of change.
- A branch can be reviewed against the one it left, and a distance measured
  between two commits with both ends held still.
- A comparison scopes itself to what it compared — a subtree is compared
  against itself, not against the repo it sits in.
- git is asked what changed before two trees are checked out to find out.

### Asking what code costs

- What a call chain costs, not just one body; how a callable scales, and
  through what.
- What a change runs through, not only how far it reaches.
- `impact` takes a file, and the whole repo can be asked which files depend
  on one.
- What the code does to the world, named apart from what it calls, and the
  libraries an entity calls named apart from what the graph missed.

### The dashboard and the cache

- The quality dashboard is measured from a commit rather than from when you
  started watching, reports what it measured, and says when it measured
  nothing.
- The cache shows what it is holding, and one repository can be cleared out
  of it without emptying the rest.

### Specs

- A spec entity can be pointed at without emptying the graph, the spec is
  re-read without waiting to be told, and your own settings can name a spec
  directory outside the repo.

### Analysis fidelity

- Python gained what the Rust parser has — including the type a receiver
  has that nobody annotated — and what "has" means is now written down.
- A call written in a loop counts as a call; code inside a template literal
  is read, not read past.
- A moved function pairs with itself rather than with its namesake.
- SQL says which key joins two tables, and how many rows sit at each end.

## 1.4.0 — 2026-08-26

**The extension carries its own engine.** Platform packages for macOS
(Apple Silicon), Linux x64 and Windows x64 ship the `mezz` binary
inside the `.vsix`. Installing is a click: no Rust toolchain, no `cargo
install`, no prerequisite at all. Other platforms get a universal package
that looks for `mezz` on your `PATH`, exactly as before.

**Renamed from Nao to Mezzanine.** This is a breaking change and nothing
reads the old spellings:

| was | now |
|---|---|
| the `nao` command | `mezz` |
| `.nao/settings.json` | `.mezz/settings.json` |
| `~/.config/nao/` | `~/.config/mezz/` |
| `NAO_*` variables | `MEZZ_*` |
| `nao.*` settings and commands | `mezz.*` |
| `nao-team.nao-code-visualizer` | `llvator.mezzanine` |

Because the extension identifier changed, this arrives as a new extension
rather than an update. Uninstall the old one, and rename any `.nao/`
directory in your projects to `.mezz/`.

### Structure and boundaries

- A folder can be asked whether its own imports respect other folders'
  doors, and a crossed boundary now comes with the fix, not just the name.
- Entry is scored over every way into a folder rather than only its busiest
  one, so a second front door counts as one.
- A folder's layout is proposed and scored before any file moves.
- `Fractal` is gated on where a folder's exits start, not only where they
  land, and the verdict says when a folder is finished but for an advisory
  gate.

### Analysis fidelity

- TypeScript: imports reached through a module-level constant, and calls
  written inside a branch, now reach the graph.
- JavaScript is read with the grammar that already understands it.
- A closure call no longer resolves to a stranger in another module.
- Reports say how much of an entity's fan-out was actually identified, so a
  hole in the graph is distinguishable from a clean one.

### Agents

- One explanatory document per MCP tool, and a new `boundaries` tool.
- The check hook reports the rules an edit broke rather than the whole
  backlog.
- `mezz init` registers the MCP server and scaffolds the VS Code tasks.

## 1.3.0 — 2026-08-25

- **Go and Dart parsers.** Go resolves calls through declared types; Dart
  reads the whole of Dart 3.
- **Kotlin** gained the metrics a quality report is made of.
- **`mezz check`** grades a tree against rules the project declared in
  `.mezz/rules.json` and fails a build on them — never on opinions of its
  own.
- **Folder shape** says when a folder has lost its shape and which one to
  fix first.
- Every MCP answer names the configuration that produced it, and the server
  is held to the repo's settings file.
- Each drawn dependency cites the line it was written on.

## 1.2.0 — 2026-08-07

Settings you can inspect, a diff that compares like with like, and regions
that tell you whether a folder is a real subsystem.

## 1.1.0 — 2026-08-05

Largely the browser UI, plus a workflows guide and SQL schema support.

## 1.0.0 — 2026-08-04

First public release: an interactive code visualizer and quality-metrics
explorer for VS Code, backed by a Rust analysis engine.
