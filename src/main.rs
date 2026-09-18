//! Mezzanine CLI
//!
//! A tool for visualizing code relationships and dependencies.

mod usage;

use anyhow::Result;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use serde_json::Value;
use mezz::{
    analyzer::Analyzer,
    config::{Config, LayoutDirection},
    graph::DependencyGraph,
    models::{file_info::Language, EntityKind},
    output::{self, JsonRenderer, OutputFormat},
};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "mezz")]
#[command(author = "llvator")]
#[command(version)]
#[command(about = "Visualize code relationships, dependencies, and quality metrics", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set a repo up for mezz: write `.mezz/settings.json` with the languages
    /// the tree is actually written in, and optionally add the VS Code tasks
    /// that start, open and stop the browser UI, and the `.mcp.json` entry
    /// that gives an agent working here the code graph.
    Init {
        /// Repo to set up (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Also add the Mezzanine tasks to `.vscode/tasks.json`, creating it or
        /// merging into what is already there.
        #[arg(long)]
        vscode: bool,

        /// Also register `mezz mcp` in `.mcp.json`, creating it or adding to
        /// the servers already there.
        #[arg(long)]
        mcp: bool,

        /// Also wire the push-mode `Stop` hooks into `.claude/settings.json`,
        /// creating it or adding to the hooks already there. An agent that
        /// ends a turn having introduced a smell, cycle, complexity jump,
        /// folder-shape fall or rule breach is blocked once, handed the
        /// finding, and can fix it before stopping. Silent otherwise.
        #[arg(long)]
        hooks: bool,

        /// Every optional file above — the same as `--vscode --mcp --hooks`.
        #[arg(long)]
        all: bool,

        /// Add a fourth VS Code task that starts the engine with
        /// `--allow-agent-spawn`, so the browser UI's quality tables offer a
        /// Refactor button that opens a Claude Code terminal. Implies
        /// `--vscode`, and is deliberately outside `--all`: it writes the one
        /// task that runs code on this machine rather than serving data.
        #[arg(long)]
        allow_agent_spawn: bool,

        /// Also add one VS Code task per graph tool, each scoped to whatever
        /// file the editor has open — `quality` on this file, `impact` on the
        /// entity under the cursor, `reshape` on this file's folder. Implies
        /// `--vscode`, and is outside `--all`: it is eleven entries in a task
        /// list shared with the repo's own builds and tests, which is a cost
        /// a reader should choose rather than inherit.
        #[arg(long)]
        editor_tools: bool,

        /// Replace what is already there. Without it, an existing settings
        /// file is left alone, existing tasks keep their current bodies, and
        /// an existing `mezz` MCP entry is not rewritten.
        #[arg(long)]
        force: bool,
    },

    /// Grade the tree against the rules the repo declared in
    /// `.mezz/rules.json`: exit 0 when every one holds, 1 when one does not,
    /// 2 when no verdict could be computed. Mezzanine declares no rules of its
    /// own, so a repo without that file passes and says so.
    Check {
        /// Path to check (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Output format: `human` (default) or `json` for CI consumers.
        #[arg(long, default_value = "human")]
        format: CheckFormatArg,
    },

    /// What the abbreviations in an entity row mean — `cx`, `cog`, `ws`,
    /// `in`, `out`, `cycle` and the smells behind `⚠`. Reads nothing and
    /// analyses nothing; the key is the same whichever repo you are in.
    Explain {
        /// Token or smell to explain, e.g. `ws` or "Overfull Head". Omit for
        /// the whole key.
        token: Option<String>,
    },

    /// Analyze a codebase and generate dependency visualization
    Analyze {
        /// Path to analyze (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Output format (dot, mermaid, json, ascii)
        #[arg(short, long, default_value = "ascii")]
        format: OutputFormatArg,

        /// Output file (defaults to stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Maximum dependency depth to traverse. Unset falls back to
        /// `max_depth` in the settings file, then to 3.
        #[arg(short, long)]
        depth: Option<usize>,

        /// Filter by language (rust, python, javascript, typescript, java, go)
        #[arg(short, long)]
        language: Option<Vec<String>>,

        /// Filter by entity kind (class, struct, function, interface, module)
        #[arg(short, long)]
        kind: Option<Vec<EntityKindArg>>,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Analyze Markdown documents alongside the code. Widens the
        /// analysis; `-l markdown` narrows it to docs only.
        #[arg(long)]
        include_docs: bool,

        /// Keep local and module-level assignments as entities. Off by
        /// default because they are most of a graph and nothing lists them;
        /// worth turning on for a focused reading of a few files. The durable
        /// answer for a repo is `include_locals` in `.mezz/settings.json`.
        #[arg(long)]
        include_locals: bool,

        /// Include external dependencies
        #[arg(long)]
        include_external: bool,

        /// Graph layout direction
        #[arg(long, default_value = "tb")]
        layout: LayoutDirectionArg,

        /// Group entities by file
        #[arg(long)]
        group_by_file: bool,

        /// Show line numbers
        #[arg(long)]
        line_numbers: bool,

        /// Show full qualified names
        #[arg(long)]
        qualified_names: bool,

        /// Restrict rendering to the given entity and its descendants.
        /// Currently honored by the `elevator-text` format. Accepts a
        /// bare name (`library`), a kind-prefixed qualname
        /// (`c.library`, `f.protocol`), or a full ID
        /// (`elevator::c.library`).
        #[arg(long)]
        root: Option<String>,

        /// Focus on a specific entity: emit its ancestor chain,
        /// siblings, own subtree, and the cross-cutting concepts
        /// touching the path. Currently honored by the
        /// `elevator-text` format. Takes precedence over `--root`.
        #[arg(long)]
        focus: Option<String>,
    },

    /// [deprecated: use `mezz impact`] Show what a file depends on, and what
    /// depends on it. Analyses the repository and filters to the target, so
    /// `--reverse` names the files that would break if this one went.
    /// `mezz impact --path <file>` answers both directions at once, with the
    /// edge kinds and the landing declarations this summary leaves out.
    Deps {
        /// Target file or entity to analyze
        target: PathBuf,

        /// Maximum depth to traverse. Unset falls back to `max_depth` in the
        /// settings file, then to 2.
        #[arg(short, long)]
        depth: Option<usize>,

        /// Show reverse dependencies (what depends on this)
        #[arg(short, long)]
        reverse: bool,

        /// Accepted and ignored: this report is text in one shape, and has
        /// been since it became a file-level listing rather than a graph
        /// render. Kept so a script passing `-f` to every command does not
        /// fail on this one; a `deps` that really answered in JSON would be
        /// a feature, not a flag that is already here.
        #[arg(short, long, default_value = "ascii")]
        format: OutputFormatArg,
    },

    /// [deprecated: use `mezz similar`] Find a specific entity by name
    /// substring. `similar` ranks by relevance and matches signature types,
    /// where this matches any entity whose name contains the pattern.
    Find {
        /// Name pattern to search for
        pattern: String,

        /// Path to search in
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Filter by entity kind
        #[arg(short, long)]
        kind: Option<EntityKindArg>,
    },

    /// Detect circular dependencies
    Cycles {
        /// Path to analyze
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Output format
        #[arg(short, long, default_value = "ascii")]
        format: OutputFormatArg,
    },

    /// [deprecated: use `mezz quality`] Show raw tallies about the codebase.
    /// Its "most connected entities" ranks unfiltered graph nodes, so it is
    /// headed by `String` and `Vec`; `quality` reports smells, refactor
    /// pressure and folder shape over the same graph.
    Stats {
        /// Path to analyze
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Compare two git commits and visualize structural changes
    Diff {
        /// Path to the git repository
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Base commit (from)
        #[arg(long)]
        from: String,

        /// Head commit (to, defaults to working tree)
        #[arg(long, default_value = "HEAD")]
        to: String,

        /// Output file for the diff JSON
        #[arg(short, long, default_value = "ui/public/diff.json")]
        output: PathBuf,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Filter by language
        #[arg(short, long)]
        language: Option<Vec<String>>,
    },

    /// Scan a Java file and print every Educator rule that fires (linter-style report)
    Educate {
        /// File to scan
        file: PathBuf,

        /// Output JSON instead of the human-readable linter format
        #[arg(long)]
        json: bool,

        /// Content root for rule corpus (defaults to MEZZ_EDUCATOR_CONTENT or `<cwd>/content`)
        #[arg(long)]
        content: Option<PathBuf>,
    },

    /// Generate the construct-kind catalog markdown for a language (Educator authoring aid)
    ConstructKinds {
        /// Language name (e.g. `java`)
        language: String,

        /// Where to write the catalog. Defaults to `content/<language>/construct-kinds.md`.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Fail (exit 1) if the existing file at the output path differs from the freshly
        /// generated content. Used in CI to keep the checked-in catalog up to date.
        #[arg(long)]
        check: bool,
    },

    /// Generate the per-language Educator content index (rules + lessons grouped by construct-kind)
    EducatorIndex {
        /// Language name (e.g. `java`)
        language: String,

        /// Where to write the index. Defaults to `content/<language>/INDEX.md`.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Content root holding `<language>/{rules,lessons}/`. Defaults to
        /// `MEZZ_EDUCATOR_CONTENT` or `<cwd>/content`.
        #[arg(long)]
        content: Option<PathBuf>,

        /// Fail (exit 1) if the existing file at the output path differs from the freshly
        /// generated content. Used in CI to keep the checked-in index up to date.
        #[arg(long)]
        check: bool,
    },

    /// Watch a codebase for changes and serve a live-updating visualization
    Watch {
        /// Path to analyze and watch
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Output directory for JSON files (served to the UI).
        /// Unset falls back to the settings file, then to `ui/public`.
        #[arg(short, long)]
        output_dir: Option<PathBuf>,

        /// Port for the HTTP server.
        /// Unset falls back to the settings file, then to 3000.
        #[arg(short, long)]
        port: Option<u16>,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Analyze Markdown documents alongside the code. Widens the
        /// analysis; `-l markdown` narrows it to docs only.
        #[arg(long)]
        include_docs: bool,

        /// Filter by language
        #[arg(short, long)]
        language: Option<Vec<String>>,

        /// Directory holding this repo's Elevator (`.elv`) spec. Unset —
        /// the usual case — means every `.elv` under the analyzed root is
        /// the spec. Set it when the spec lives somewhere the walk misses
        /// (a sibling docs repo, or above the service you're watching), or
        /// when the tree also contains `.elv` files that aren't spec.
        /// Unlike the settings-file key it may point outside the root.
        #[arg(long)]
        spec_dir: Option<PathBuf>,

        /// Debounce interval in milliseconds.
        /// Unset falls back to the settings file, then to 300.
        #[arg(long)]
        debounce_ms: Option<u64>,

        /// Fallback Educator content root, used when neither
        /// `MEZZ_EDUCATOR_CONTENT` nor `<workspace>/content/` resolves. The
        /// VS Code extension passes its bundled `content/` directory here
        /// so rules and lessons work in workspaces that don't ship their
        /// own corpus.
        #[arg(long)]
        content_fallback: Option<PathBuf>,

        /// Let a browser page on this origin read this server's responses.
        /// Repeatable. Same-origin and the VS Code webview are always
        /// allowed; everything else — including a UI you serve yourself on
        /// another port — needs naming here, because loopback is not a
        /// boundary against a browser on the same machine.
        #[arg(long, value_name = "URL")]
        allow_origin: Vec<String>,

        /// Don't require a pairing token from non-loopback origins.
        /// By default a token is minted per run and printed in the banner;
        /// an origin that isn't loopback or the VS Code webview has to send
        /// it, so allowlisting a public host isn't by itself enough to read
        /// your source. Pass this to skip that step.
        #[arg(long)]
        no_token: bool,

        /// Directory holding the built browser UI. Falls back to
        /// `MEZZ_UI_DIR`, then a `ui/dist` beside the `mezz` binary, then
        /// `./ui/dist`. Without any of them the server still runs and the
        /// root page explains how to connect a UI hosted elsewhere.
        #[arg(long, value_name = "PATH")]
        ui_dir: Option<PathBuf>,

        /// Let the browser UI open a Claude Code terminal on this machine to
        /// refactor an entity. Off by default: it is the one route that runs
        /// code rather than serving data, so it does not exist unless you ask
        /// for it, and it always requires the pairing token — including from
        /// loopback, where reading the graph does not. Set `MEZZ_TERMINAL` to
        /// choose the terminal and `MEZZ_CLAUDE_BIN` for a non-PATH install.
        #[arg(long)]
        allow_agent_spawn: bool,

        /// Keep a loaded diff exactly where it was computed. By default a
        /// `working tree` diff follows the watcher: every re-analysis
        /// recomputes it against the same base ref, so the overlay describes
        /// the tree you are editing rather than the one you had when you
        /// pressed the button. Pass this to pin it to that moment instead.
        /// Diffs between two commits are fixed comparisons and never follow.
        #[arg(long)]
        pin_diff: bool,
    },

    /// Watch a codebase's quality and draw it as a live terminal dashboard.
    ///
    /// Where `watch` serves a browser UI the whole graph, this holds a
    /// handful of figures and the one thing neither `quality` nor the UI
    /// has: a time axis. Built for a tmux pane beside a swarm of agents —
    /// smells, complexity, cycles, folder shape and rule breaches, each with
    /// a delta since you started watching and a list of what moved.
    Monitor(MonitorArgs),

    /// Host several analyzed repos at once and serve the browser UI against
    /// a chosen one. Unlike `watch`, there is no path argument, no file
    /// watcher, and no live-reload: each repo is analyzed once and answered
    /// from memory under `/api/repos/<slug>/*`.
    Serve {
        /// Port for the HTTP server.
        /// Unset falls back to the user settings file, then to 3000.
        #[arg(short, long)]
        port: Option<u16>,

        /// Pre-load an already-cloned local path as `<slug>=<path>`.
        /// Repeatable. Slugs are `<owner>__<repo>`, e.g.
        /// `--seed tinygrad__tinygrad=/tmp/tinygrad`.
        #[arg(long, value_name = "SLUG=PATH")]
        seed: Vec<String>,

        /// Where submitted repos are cloned and their analyses cached.
        /// Repos found here are restored at startup instead of re-analyzed.
        /// Defaults to `$XDG_CACHE_HOME/mezz/serve` or `~/.cache/mezz/serve`.
        #[arg(long, value_name = "PATH")]
        cache_dir: Option<PathBuf>,

        /// How many submissions may clone/analyze at once. Analysis is
        /// already parallel internally, so more mostly means more contention.
        #[arg(long, default_value = "1")]
        jobs: usize,

        /// Give up on a clone after this many seconds
        #[arg(long, default_value = "300")]
        clone_timeout_secs: u64,

        /// Reject a cloned repo larger than this, in megabytes
        #[arg(long, default_value = "500")]
        max_repo_mb: u64,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Filter by language
        #[arg(short, long)]
        language: Option<Vec<String>>,

        /// Let a browser page on this origin read this server's responses.
        /// Repeatable. See `mezz watch --help` for why it is needed.
        #[arg(long, value_name = "URL")]
        allow_origin: Vec<String>,

        /// Don't require a pairing token from non-loopback origins.
        /// See `mezz watch --help`.
        #[arg(long)]
        no_token: bool,

        /// Directory holding the built browser UI. Same resolution order as
        /// `mezz watch --ui-dir`.
        #[arg(long, value_name = "PATH")]
        ui_dir: Option<PathBuf>,
    },

    /// Serve Mezzanine as an MCP (Model Context Protocol) server over stdio,
    /// exposing `map`, `quality`, and `assess_change` tools to AI agents
    Mcp {
        /// Root directory the server analyzes (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Filter by language
        #[arg(short, long)]
        language: Option<Vec<String>>,
    },

    /// Push-mode hooks: emit mezz's structural signal without being asked.
    /// Designed to run from a Claude Code Stop/PostToolUse hook.
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },

    /// Render the `assess_change` report as a PR comment body (MCP-008).
    /// Full report when the change is structural, else a one-liner.
    /// Never fails the build — see the reference GitHub Actions workflow.
    PrReport {
        /// Root directory to analyze (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Git ref to compare the working tree against (e.g. the PR
        /// merge-base). Defaults to HEAD.
        #[arg(long, default_value = "HEAD")]
        base_ref: String,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Filter by language
        #[arg(short, long)]
        language: Option<Vec<String>>,
    },

    /// The graph tools, also served over MCP. Flattened, so they read as
    /// top-level commands.
    #[command(flatten)]
    Tool(ToolCommand),
}

/// The sixteen graph tools, in the two families they divide into.
///
/// A wrapper rather than sixteen variants on [`Commands`] — and rather than
/// two — so the dispatcher in `main` grows by one arm total. Its `match` is
/// the function CI-001 is about: the gate fails on *any* metric increase to
/// an existing function, which makes every new command cost something there.
/// One is the floor; two was avoidable.
#[derive(Subcommand)]
enum ToolCommand {
    #[command(flatten)]
    Area(AreaTool),

    #[command(flatten)]
    Subject(SubjectTool),
}

/// What every tool call is analysed under: the same three arguments
/// `mezz mcp` takes, because the tools resolve their scope from these and a
/// CLI call that scoped differently would answer a different question than
/// the agent asking it.
///
/// `--format` rides along rather than getting a struct of its own: this is
/// the one thing flattened into all sixteen commands, and a second one
/// would have to be added to each of them by hand.
#[derive(Args, Clone)]
struct Scope {
    /// Repository root to analyze (defaults to current directory). `path`
    /// arguments are relative to this.
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Include test files
    #[arg(long)]
    include_tests: bool,

    /// Filter by language
    #[arg(short, long)]
    language: Option<Vec<String>>,

    /// Output format: `text` (default) or `json` for scripts and CI.
    /// Twelve of the sixteen tools answer in prose only so far and say so
    /// by name when asked for JSON — see `mezz <tool> --help`.
    #[arg(long, value_enum, default_value_t = ToolFormatArg::Text)]
    format: ToolFormatArg,
}

/// The two renderings of a tool's answer (CLI-003).
///
/// Its own enum rather than a reuse of [`OutputFormatArg`]: these answers
/// have no DOT or Mermaid rendering, and offering one would promise a
/// drawing that does not exist.
#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum ToolFormatArg {
    Text,
    Json,
}

/// An entity target: a name, or a position in a file.
///
/// The same either/or the MCP schemas define. Deliberately not a clap
/// `group` with `required = true` — the tools already reject an empty
/// target with a message naming both ways to spell one, and having clap
/// reject it first would replace that with a worse one.
#[derive(Args, Clone)]
struct Target {
    /// Name or qualified name of the entity (e.g. `compute_diff`)
    #[arg(long)]
    entity: Option<String>,

    /// File containing the entity, relative to the root (use with --line)
    #[arg(long)]
    path: Option<String>,

    /// 1-based line inside the entity
    #[arg(long)]
    line: Option<usize>,
}

/// The tools you point at an area of the tree (CLI-002).
///
/// Every variant maps to one entry in `mcp::TOOLS` and carries that tool's
/// schema as typed flags, named as the schema names them, so a call can be
/// moved between the two front doors unchanged.
///
/// Split from [`SubjectTool`] along the line the tools actually differ on —
/// what they take as a target — because one enum of all sixteen put the
/// `match` converting them at cyclomatic 16, one over the repo's ceiling.
/// Both flatten into [`Commands`], so the split is invisible on the command
/// line: `mezz map` and `mezz impact` are siblings there.
#[derive(Subcommand)]
enum AreaTool {
    /// Domain-level overview from the project's Elevator (.elv) specs
    Overview {
        #[command(flatten)]
        scope: Scope,

        /// Entity to centre on, e.g. `f.protocol`. Omit for the full map.
        #[arg(long)]
        focus: Option<String>,

        /// Directory containing the .elv spec, relative to the root
        path: Option<String>,
    },

    /// Structural map of a folder: files, entities, metrics, coupling
    Map {
        #[command(flatten)]
        scope: Scope,

        /// Directory or file to map. Omit for the whole project.
        path: Option<String>,

        /// 1 = files, 2 = files + top-level entities, 3 = also members
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=3))]
        depth: Option<u64>,
    },

    /// Smells, complexity offenders, cycles and folder shape
    Quality {
        #[command(flatten)]
        scope: Scope,

        /// Directory or file to assess. Omit for the whole project.
        path: Option<String>,

        /// How many top offenders to list (default 10)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=50))]
        top: Option<u64>,
    },

    /// Risk ranking: git churn × complexity
    Hotspots {
        #[command(flatten)]
        scope: Scope,

        /// Directory to rank. Omit for the whole project.
        path: Option<String>,

        /// History window in days (default 180)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=3650))]
        days: Option<u64>,

        /// How many files to list (default 10)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=50))]
        top: Option<u64>,
    },

    /// Entities nothing references — candidates for deletion
    DeadCode {
        #[command(flatten)]
        scope: Scope,

        /// Directory or file to report on. Omit for the whole project.
        path: Option<String>,

        /// Also list unreferenced public API
        #[arg(long)]
        include_public: bool,
    },

    /// The Elevator spec narrowed to one folder, as standalone .elv source
    SpecSlice {
        #[command(flatten)]
        scope: Scope,

        /// Folder whose spec claims to extract, e.g. `src/mcp`
        path: String,

        /// Write the slice here instead of printing it
        #[arg(long)]
        out: Option<String>,

        /// Allow --out to replace an existing file
        #[arg(long)]
        overwrite: bool,
    },

    /// The one change that would improve a folder's structure
    Reshape {
        #[command(flatten)]
        scope: Scope,

        /// Folder to reshape. Omit for the repository root.
        path: Option<String>,
    },

    /// Where a folder's files would sit if its dependency drawing decided
    Layout {
        #[command(flatten)]
        scope: Scope,

        /// Folder to lay out. Omit for the repository root.
        path: Option<String>,

        /// Score this arrangement instead of proposing one. Repeatable,
        /// spelled `what:into` — e.g. `--move src/a.rs:src/core`.
        #[arg(long = "move", value_name = "WHAT:INTO")]
        moves: Vec<String>,
    },

    /// Which of a folder's imports reach past another folder's door
    Boundaries {
        #[command(flatten)]
        scope: Scope,

        /// Folder to grade. Omit for the repository root.
        path: Option<String>,
    },
}

/// The tools you point at a subject rather than a place (CLI-002).
///
/// An entity (`impact`, `context`, `tests_for`), a pair of them (`trace`), a
/// free-text query (`similar`), or a change (`assess_change`). See
/// [`AreaTool`] for why the sixteen are split in two.
#[derive(Subcommand)]
enum SubjectTool {
    /// Blast radius of a change to one entity, or to a whole file when
    /// `--path` is given without `--line`
    Impact {
        #[command(flatten)]
        scope: Scope,

        #[command(flatten)]
        target: Target,

        /// Dependency hops for the transitive radius (default 2)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=5))]
        depth: Option<u64>,

        /// Which way the radius walks: `in` = what breaks if this changes
        /// (default), `out` = the call tree under it
        #[arg(long, value_parser = ["in", "out"])]
        direction: Option<String>,
    },

    /// How code scales, and on what — worst-case time complexity, composed
    /// along the call chain
    Cost {
        #[command(flatten)]
        scope: Scope,

        #[command(flatten)]
        target: Target,

        /// Call hops to compose the cost over (default 2, 0 for this body alone)
        #[arg(long, value_parser = clap::value_parser!(u64).range(0..=5))]
        depth: Option<u64>,

        /// Price one named route instead: its start. Use with --to.
        #[arg(long)]
        from: Option<String>,

        /// Price one named route instead: its end. Use with --from.
        #[arg(long)]
        to: Option<String>,
    },

    /// Minimal context pack for editing one entity
    Context {
        #[command(flatten)]
        scope: Scope,

        #[command(flatten)]
        target: Target,
    },

    /// Which tests exercise an entity, directly or transitively
    TestsFor {
        #[command(flatten)]
        scope: Scope,

        #[command(flatten)]
        target: Target,

        /// Dependency hops to search (default 3)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=6))]
        depth: Option<u64>,
    },

    /// Shortest dependency path between two entities
    Trace {
        #[command(flatten)]
        scope: Scope,

        /// Starting entity
        #[arg(long)]
        from: String,

        /// Destination entity
        #[arg(long)]
        to: String,

        /// Search limit (default 10)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=30))]
        max_hops: Option<u64>,
    },

    /// Does something like this already exist? Ranked by similarity.
    Similar {
        #[command(flatten)]
        scope: Scope,

        /// What you are about to implement
        query: String,

        /// Entity-kind filter, e.g. `function`
        #[arg(long)]
        kind: Option<String>,

        /// Upper bound on results (default 10)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=50))]
        top: Option<u64>,
    },

    /// Self-review a change: metric deltas vs a git base ref
    AssessChange {
        #[command(flatten)]
        scope: Scope,

        /// Git ref to compare the working tree against (default HEAD)
        #[arg(long)]
        base_ref: Option<String>,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Report new structural regressions of the working tree vs a git
    /// ref, quiet-when-clean and once-per-session (MCP-007).
    SelfReview {
        /// Root directory to analyze (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Git ref to compare the working tree against. Defaults to HEAD.
        #[arg(long, default_value = "HEAD")]
        base_ref: String,

        /// Session state file (fingerprints already surfaced). Defaults
        /// to a temp-dir file keyed by repo + base SHA, which resets
        /// naturally when a new commit moves the base.
        #[arg(long)]
        state: Option<PathBuf>,

        /// Minimum severity to report: low | medium | high (default low).
        #[arg(long, default_value = "low")]
        min_severity: String,

        /// Hard cap on output lines before pointing at `assess_change`.
        #[arg(long, default_value_t = mezz::mcp::push::DEFAULT_LINE_CAP)]
        cap: usize,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Filter by language
        #[arg(short, long)]
        language: Option<Vec<String>>,

        /// Send findings back to the agent instead of into the void.
        ///
        /// A `Stop` hook that exits 0 has its stdout written to the debug log
        /// alone — not the transcript, and never Claude's context. Only exit 2
        /// reaches the agent, on stderr, and blocks the stop so it can act.
        /// Off by default: the plain command stays advisory, as the docs and
        /// every CI caller assume.
        #[arg(long)]
        block: bool,
    },

    /// Report only the `.mezz/rules.json` violations the working tree
    /// introduced, quiet-when-clean and once-per-session (MCP-019). Not
    /// every violation — that is `mezz check`, which answers a different
    /// question and is the one to run by hand.
    Check {
        /// Root directory to grade (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Git ref the working tree is judged against. Defaults to HEAD.
        #[arg(long, default_value = "HEAD")]
        base_ref: String,

        /// Session state file (violations already surfaced). Defaults to
        /// a temp-dir file keyed by repo + base SHA, which resets
        /// naturally when a new commit moves the base.
        #[arg(long)]
        state: Option<PathBuf>,

        /// Hard cap on output lines before pointing at `mezz check`.
        #[arg(long, default_value_t = mezz::mcp::push::DEFAULT_LINE_CAP)]
        cap: usize,

        /// Send findings back to the agent instead of into the void.
        ///
        /// A `Stop` hook that exits 0 has its stdout written to the debug log
        /// alone — not the transcript, and never Claude's context. Only exit 2
        /// reaches the agent, on stderr, and blocks the stop so it can act.
        /// Off by default: the plain command stays advisory, as the docs and
        /// every CI caller assume.
        #[arg(long)]
        block: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormatArg {
    Dot,
    Mermaid,
    Json,
    Ascii,
    /// Compact text rendering of an Elevator (`.elv`) spec —
    /// designed as a low-token artifact for LLM consumption.
    ElevatorText,
}

impl From<OutputFormatArg> for OutputFormat {
    fn from(arg: OutputFormatArg) -> Self {
        match arg {
            OutputFormatArg::Dot => OutputFormat::Dot,
            OutputFormatArg::Mermaid => OutputFormat::Mermaid,
            OutputFormatArg::Json => OutputFormat::Json,
            OutputFormatArg::Ascii => OutputFormat::Ascii,
            OutputFormatArg::ElevatorText => OutputFormat::ElevatorText,
        }
    }
}

/// `mezz check`'s two renderings. Its own enum rather than a reuse of
/// [`OutputFormatArg`]: a verdict has no DOT or Mermaid rendering, and
/// offering one would promise a drawing that does not exist.
#[derive(Clone, Copy, ValueEnum)]
enum CheckFormatArg {
    Human,
    Json,
}

impl From<CheckFormatArg> for mezz::check::Format {
    fn from(arg: CheckFormatArg) -> Self {
        match arg {
            CheckFormatArg::Human => mezz::check::Format::Human,
            CheckFormatArg::Json => mezz::check::Format::Json,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum EntityKindArg {
    File,
    Module,
    Class,
    AbstractClass,
    Struct,
    Interface,
    Enum,
    Function,
    Method,
}

impl From<EntityKindArg> for EntityKind {
    fn from(arg: EntityKindArg) -> Self {
        match arg {
            EntityKindArg::File => EntityKind::File,
            EntityKindArg::Module => EntityKind::Module,
            EntityKindArg::Class => EntityKind::Class,
            EntityKindArg::AbstractClass => EntityKind::AbstractClass,
            EntityKindArg::Struct => EntityKind::Struct,
            EntityKindArg::Interface => EntityKind::Interface,
            EntityKindArg::Enum => EntityKind::Enum,
            EntityKindArg::Function => EntityKind::Function,
            EntityKindArg::Method => EntityKind::Method,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum LayoutDirectionArg {
    Tb,
    Lr,
    Bt,
    Rl,
}

impl From<LayoutDirectionArg> for LayoutDirection {
    fn from(arg: LayoutDirectionArg) -> Self {
        match arg {
            LayoutDirectionArg::Tb => LayoutDirection::TopToBottom,
            LayoutDirectionArg::Lr => LayoutDirection::LeftToRight,
            LayoutDirectionArg::Bt => LayoutDirection::BottomToTop,
            LayoutDirectionArg::Rl => LayoutDirection::RightToLeft,
        }
    }
}

fn main() -> Result<()> {
    // Parsed through the command clap built for `Cli` rather than through
    // `Cli::parse()`, so `--help` can be answered with the grouped command
    // list in [`usage`] instead of clap's flat one. Parsing itself is
    // unchanged: same command, same arguments, same errors.
    let matches = usage::with_grouped_help(Cli::command()).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit());

    // Dispatch is grouped so that adding a subcommand touches a small
    // function instead of one match over every command in the tool. Each
    // arm returns its group's `Result` directly rather than `?;`-ing it,
    // because mezz counts `?` as a branch and a bare delegation has nothing
    // to recover from. See CI-001.
    //
    // The match is exhaustive over `Commands`, so the compiler — not a
    // catch-all — guarantees every subcommand is routed somewhere.
    match cli.command {
        // Its own arm rather than a group: `init` is the one subcommand that
        // writes the repo's own configuration instead of reading a codebase.
        Commands::Init {
            path,
            vscode,
            mcp,
            hooks,
            all,
            allow_agent_spawn,
            editor_tools,
            force,
        } => mezz::init::run(
            &path,
            mezz::init::Targets::new(vscode, mcp, hooks, all)
                .with_agent_spawn(allow_agent_spawn)
                .with_editor_tools(editor_tools),
            force,
        ),

        // Its own arm rather than a group: `check` is the one subcommand
        // whose exit code is a verdict on the tree, so it ends the process
        // itself instead of returning a `Result` that only says whether mezz
        // ran (ADR 0024).
        Commands::Check { path, format } => {
            std::process::exit(mezz::check::run(&path, format.into()))
        }

        // Its own arm rather than a group: `explain` is the one subcommand
        // that answers without a codebase — no scope, no analysis, no cache.
        Commands::Explain { token } => run_explain(token.as_deref()),

        c @ (Commands::Analyze { .. }
        | Commands::Deps { .. }
        | Commands::Find { .. }
        | Commands::Cycles { .. }
        | Commands::Stats { .. }
        | Commands::Diff { .. }) => dispatch_analysis(c),

        c @ (Commands::Watch { .. } | Commands::Serve { .. } | Commands::Monitor(..)) => {
            dispatch_server(c)
        }

        c @ (Commands::Educate { .. }
        | Commands::ConstructKinds { .. }
        | Commands::EducatorIndex { .. }) => dispatch_educator(c),

        c @ (Commands::Mcp { .. } | Commands::Hook { .. } | Commands::PrReport { .. }) => {
            dispatch_agent(c)
        }

        Commands::Tool(t) => dispatch_tool(t),
    }
}

/// Print the metric key, or the long answer about one token.
///
/// An unknown token is an error rather than a fallback to the whole key: a
/// reader who typed `mezz explain churn` wants to be told mezz does not
/// measure churn per entity, not handed a wall of text to search for a word
/// that is not in it.
fn run_explain(token: Option<&str>) -> Result<()> {
    let Some(query) = token else {
        println!("{}", mezz::explain::key());
        return Ok(());
    };
    match mezz::explain::lookup(query) {
        Some(entry) => {
            println!("{entry}");
            Ok(())
        }
        None => anyhow::bail!(
            "no token or smell called `{query}` — mezz explains {}",
            mezz::explain::known()
        ),
    }
}

/// A tool argument object with every unset argument dropped.
///
/// Absent and `null` are not the same thing to a tool: the schemas read
/// their fields with `.get(..)`, so a serialized `None` would arrive as a
/// present-but-null argument and take whichever branch that happens to
/// hit. Dropping them makes the CLI's call byte-identical to the agent's,
/// which is what `cli_and_mcp_agree_on_one_call` compares.
fn args_of(pairs: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .filter_map(|(k, v)| Some((k.to_string(), v?)))
            .collect(),
    )
}

/// The three ways to spell an entity target, as tool arguments.
fn target_args(t: Target) -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("entity", t.entity.map(Value::from)),
        ("path", t.path.map(Value::from)),
        ("line", t.line.map(|l| Value::from(l as u64))),
    ]
}

/// `--move what:into`, repeated, as `layout`'s `moves` array.
///
/// A colon rather than a second flag because a move is one fact, and two
/// positionally-paired flags are a way to get them out of step. Paths
/// containing a colon are not supported and are rejected rather than
/// silently split at the wrong one — `split_once` takes the first, so
/// `a:b:c` would otherwise move `a` into `b:c`.
fn move_args(moves: Vec<String>) -> Result<Option<Value>> {
    if moves.is_empty() {
        return Ok(None);
    }
    let parsed = moves
        .iter()
        .map(|m| match m.split_once(':') {
            Some((what, into)) if !what.is_empty() && !into.is_empty() && !into.contains(':') => {
                Ok(serde_json::json!({ "what": what, "into": into }))
            }
            _ => Err(anyhow::anyhow!(
                "--move wants `what:into` with no colon in either path, got `{}`",
                m
            )),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(Value::Array(parsed)))
}

impl AreaTool {
    /// This tool as the scope it runs under and the call it makes.
    fn call(self) -> Result<(Scope, &'static str, Value)> {
        let call = match self {
            AreaTool::Overview { scope, focus, path } => (
                scope,
                "overview",
                args_of(vec![
                    ("focus", focus.map(Value::from)),
                    ("path", path.map(Value::from)),
                ]),
            ),
            AreaTool::Map { scope, path, depth } => (
                scope,
                "map",
                args_of(vec![
                    ("path", path.map(Value::from)),
                    ("depth", depth.map(Value::from)),
                ]),
            ),
            AreaTool::Quality { scope, path, top } => (
                scope,
                "quality",
                args_of(vec![
                    ("path", path.map(Value::from)),
                    ("top", top.map(Value::from)),
                ]),
            ),
            AreaTool::Hotspots {
                scope,
                path,
                days,
                top,
            } => (
                scope,
                "hotspots",
                args_of(vec![
                    ("path", path.map(Value::from)),
                    ("days", days.map(Value::from)),
                    ("top", top.map(Value::from)),
                ]),
            ),
            AreaTool::DeadCode {
                scope,
                path,
                include_public,
            } => (
                scope,
                "dead_code",
                args_of(vec![
                    ("path", path.map(Value::from)),
                    ("include_public", flag(include_public)),
                ]),
            ),
            AreaTool::SpecSlice {
                scope,
                path,
                out,
                overwrite,
            } => (
                scope,
                "spec_slice",
                args_of(vec![
                    ("path", Some(Value::from(path))),
                    ("out", out.map(Value::from)),
                    ("overwrite", flag(overwrite)),
                ]),
            ),
            AreaTool::Reshape { scope, path } => (
                scope,
                "reshape",
                args_of(vec![("path", path.map(Value::from))]),
            ),
            AreaTool::Layout { scope, path, moves } => (
                scope,
                "layout",
                args_of(vec![
                    ("path", path.map(Value::from)),
                    ("moves", move_args(moves)?),
                ]),
            ),
            AreaTool::Boundaries { scope, path } => (
                scope,
                "boundaries",
                args_of(vec![("path", path.map(Value::from))]),
            ),
        };
        Ok(call)
    }
}

impl SubjectTool {
    /// This tool as the scope it runs under and the call it makes.
    ///
    /// Infallible, unlike [`AreaTool::call`]: nothing here needs parsing beyond
    /// what clap already did — only `layout`'s `--move` pairs do.
    fn call(self) -> (Scope, &'static str, Value) {
        match self {
            SubjectTool::Impact {
                scope,
                target,
                depth,
                direction,
            } => {
                let mut args = target_args(target);
                args.push(("depth", depth.map(Value::from)));
                args.push(("direction", direction.map(Value::from)));
                (scope, "impact", args_of(args))
            }
            SubjectTool::Cost {
                scope,
                target,
                depth,
                from,
                to,
            } => {
                let mut args = target_args(target);
                args.push(("depth", depth.map(Value::from)));
                args.push(("from", from.map(Value::from)));
                args.push(("to", to.map(Value::from)));
                (scope, "cost", args_of(args))
            }
            SubjectTool::Context { scope, target } => {
                (scope, "context", args_of(target_args(target)))
            }
            SubjectTool::TestsFor {
                scope,
                target,
                depth,
            } => {
                let mut args = target_args(target);
                args.push(("depth", depth.map(Value::from)));
                (scope, "tests_for", args_of(args))
            }
            SubjectTool::Trace {
                scope,
                from,
                to,
                max_hops,
            } => (
                scope,
                "trace",
                args_of(vec![
                    ("from", Some(Value::from(from))),
                    ("to", Some(Value::from(to))),
                    ("max_hops", max_hops.map(Value::from)),
                ]),
            ),
            SubjectTool::Similar {
                scope,
                query,
                kind,
                top,
            } => (
                scope,
                "similar",
                args_of(vec![
                    ("query", Some(Value::from(query))),
                    ("kind", kind.map(Value::from)),
                    ("top", top.map(Value::from)),
                ]),
            ),
            SubjectTool::AssessChange { scope, base_ref } => (
                scope,
                "assess_change",
                args_of(vec![("base_ref", base_ref.map(Value::from))]),
            ),
        }
    }
}

/// A boolean flag, sent only when set.
///
/// `false` is every one of these tools' default, so an unset flag is left
/// out entirely rather than sent as `false` — same call as the agent makes
/// when it does not mention the flag.
fn flag(set: bool) -> Option<Value> {
    set.then(|| Value::from(true))
}

/// The graph tools, run once and printed (CLI-002).
///
/// Errors propagate rather than being printed with the footer the MCP side
/// attaches to a failure: a command that failed should exit non-zero with
/// its message on stderr, which is what the caller of a CLI acts on. That
/// covers `--format json` on a tool that has none: a script piping into
/// `jq` has to fail, not receive prose.
fn dispatch_tool(command: ToolCommand) -> Result<()> {
    let (scope, name, args) = match command {
        ToolCommand::Area(t) => t.call()?,
        ToolCommand::Subject(t) => t.call(),
    };
    let (root, include_tests, language) = (scope.root, scope.include_tests, scope.language);

    if scope.format == ToolFormatArg::Json {
        let value = mezz::mcp::run_tool_json(root, include_tests, language, name, &args)?;
        // Pretty rather than compact: a person reading a `--format json`
        // answer in a terminal is the common case, and `jq` does not care.
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let body = mezz::mcp::run_tool(root, include_tests, language, name, &args)?;
    // Styled here rather than in the tool: the same prose goes to an agent
    // over MCP, where an escape sequence is noise in a transcript. The CLI
    // is the only caller with a terminal to dress for.
    println!("{}", mezz::output::tty_prose::style_headings(&body));
    Ok(())
}

/// Every subcommand that reads a codebase and prints or writes a view of it.
fn dispatch_analysis(command: Commands) -> Result<()> {
    match command {
        Commands::Analyze {
            path,
            format,
            output,
            depth,
            language,
            kind,
            include_tests,
            include_docs,
            include_locals,
            include_external,
            layout,
            group_by_file,
            line_numbers,
            qualified_names,
            root,
            focus,
        } => run_analyze(
            path,
            format.into(),
            output,
            depth,
            language,
            kind,
            include_tests,
            include_docs,
            include_locals,
            include_external,
            layout.into(),
            group_by_file,
            line_numbers,
            qualified_names,
            root,
            focus,
        ),

        Commands::Deps {
            target,
            depth,
            reverse,
            // Parsed and dropped — see the flag's own doc comment.
            format: _,
        } => run_deps(target, depth, reverse),

        Commands::Find {
            pattern,
            path,
            kind,
        } => run_find(&pattern, path, kind),

        Commands::Cycles { path, format } => run_cycles(path, format.into()),

        Commands::Stats { path, json } => run_stats(path, json),

        Commands::Diff {
            path,
            from,
            to,
            output,
            include_tests,
            language,
        } => run_diff(path, &from, &to, output, include_tests, language),

        // `main` routes only the variants above here.
        _ => unreachable!("dispatch_analysis received a non-analysis command"),
    }
}

/// The long-running HTTP servers: single-repo `watch`, multi-repo `serve`.
fn dispatch_server(command: Commands) -> Result<()> {
    match command {
        Commands::Watch {
            path,
            output_dir,
            port,
            include_tests,
            include_docs,
            language,
            spec_dir,
            debounce_ms,
            content_fallback,
            allow_origin,
            no_token,
            allow_agent_spawn,
            pin_diff,
            ui_dir,
        } => {
            // Watch analyzes a path the operator chose, so both scopes apply.
            // Kept unmerged: `/api/settings` reports which file each value
            // came from, and merging is where that is lost.
            let loaded = mezz::settings::load_scoped(&path);
            let settings = loaded.merged();
            // What the command line named, so the report can say a flag won
            // rather than guessing from a value it cannot distinguish.
            let flags_named = mezz::settings::report::named(&[
                ("output_dir", output_dir.is_some()),
                ("port", port.is_some()),
                ("include_tests", include_tests),
                ("include_docs", include_docs),
                ("language", language.is_some()),
                ("spec_dir", spec_dir.is_some()),
                ("debounce_ms", debounce_ms.is_some()),
                ("content_fallback", content_fallback.is_some()),
                ("ui_dir", ui_dir.is_some()),
            ]);
            mezz::server::run(mezz::server::WatchOptions {
                output_dir: output_dir
                    .or_else(|| settings.output_dir.clone())
                    .unwrap_or_else(|| PathBuf::from("ui/public")),
                port: port
                    .or(settings.port)
                    .unwrap_or(mezz::settings::DEFAULT_PORT),
                include_tests: include_tests || settings.include_tests.unwrap_or(false),
                include_docs: include_docs || settings.include_docs.unwrap_or(false),
                languages: language.or_else(|| settings.language.clone()),
                // The flag first, then the repo's own answer — and the repo's
                // is guaranteed to stay inside the repo, which the flag's
                // deliberately isn't. See `settings::Settings::spec_dir`.
                spec_dir: spec_dir.or(settings.spec_dir.clone()),
                debounce_ms: debounce_ms.or(settings.debounce_ms).unwrap_or(300),
                content_fallback: content_fallback.or_else(|| settings.content_fallback.clone()),
                access: mezz::server::AccessOptions {
                    allow_origin,
                    no_token,
                },
                ui_dir,
                settings_ui_dir: settings.ui_dir.clone(),
                allow_agent_spawn,
                pin_diff,
                loaded,
                flags_named,
                path,
            })
        }

        Commands::Monitor(args) => run_monitor(args),

        Commands::Serve {
            port,
            seed,
            cache_dir,
            jobs,
            clone_timeout_secs,
            max_repo_mb,
            include_tests,
            language,
            allow_origin,
            no_token,
            ui_dir,
        } => {
            // User scope only: a repo submitted to `serve` arrived from a URL
            // a stranger pasted, and does not get to configure the server
            // analyzing it.
            let settings = mezz::settings::user();
            run_serve(ServeArgs {
                port: port
                    .or(settings.port)
                    .unwrap_or(mezz::settings::DEFAULT_PORT),
                seed,
                cache_dir,
                jobs,
                clone_timeout_secs,
                max_repo_mb,
                include_tests: include_tests || settings.include_tests.unwrap_or(false),
                languages: language.or_else(|| settings.language.clone()),
                access: mezz::server::AccessOptions {
                    allow_origin,
                    no_token,
                },
                ui_dir,
                settings_ui_dir: settings.ui_dir,
            })
        }

        _ => unreachable!("dispatch_server received a non-server command"),
    }
}

/// `mezz monitor`'s flags exactly as clap parsed them, in the shape
/// [`ServeArgs`] takes for the same reason.
///
/// A struct rather than a dozen fields on the variant, because a handler
/// that destructures every flag holds every flag's name in view at once:
/// the twelfth one put `run_monitor` over the `OverfullHead` bar this repo
/// grades other people's code by. Derived rather than assembled in the
/// dispatcher, so the flags and their help text are clap's, unchanged.
///
/// Eleven fields and no methods is a `DataBag`, and it is meant to be one:
/// that is what Introduce Parameter Object produces, it is the shape
/// [`ServeArgs`] and `MonitorOptions` already have for the same role, and a
/// record whose whole job is to carry what clap parsed has no behaviour to
/// group the fields around.
#[derive(clap::Args)]
struct MonitorArgs {
    /// Path to analyze and watch
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Include test files
    #[arg(long)]
    include_tests: bool,

    /// Analyze Markdown documents alongside the code
    #[arg(long)]
    include_docs: bool,

    /// Filter by language
    #[arg(short, long)]
    language: Option<Vec<String>>,

    /// Directory holding this repo's Elevator (`.elv`) spec. See
    /// `mezz watch --help`.
    #[arg(long)]
    spec_dir: Option<PathBuf>,

    /// How long the tree must be quiet before it is measured.
    /// Unset falls back to the settings file, then to 1000 — four times
    /// `watch`'s, because a swarm never stops typing and a reading taken
    /// mid-edit measures a half-written function.
    #[arg(long)]
    debounce_ms: Option<u64>,

    /// Never measure more often than this, however fast the edits
    /// arrive. The floor that keeps the dashboard from spending a
    /// machine the agents need.
    #[arg(long, default_value = "2000")]
    min_interval_ms: u64,

    /// Readings kept for the sparklines.
    #[arg(long, default_value = "500")]
    history: usize,

    /// What the deltas are measured from: a git ref (`HEAD`, a SHA,
    /// `main`, `HEAD~5`), or `working` for the tree as found when the
    /// session starts.
    ///
    /// Defaults to `HEAD`, so the figures read "since the last commit"
    /// and the work already sitting in the tree is inside them from the
    /// first frame. The ref is resolved once and pinned: `B` re-measures
    /// it at whatever HEAD has become. A tree that is not a git
    /// repository measures from its first reading instead.
    #[arg(long, value_name = "REF")]
    baseline: Option<String>,

    /// Pin the *head* side to a commit as well, which turns the session
    /// into a still comparison of two states instead of a watch:
    /// `--baseline v0.4.0 --against HEAD` reads every tile as "what
    /// changed between those two commits".
    ///
    /// Both sides are measured out of a checkout, so nothing uncommitted
    /// is in either figure. There is no tree to watch and no time axis, so
    /// the sparklines are empty and `b`, `B` and `p` are not offered.
    /// Requires `--baseline` to name a commit too.
    #[arg(long, value_name = "REF")]
    against: Option<String>,

    /// Append one JSON line per reading here, so a session can be read
    /// back after it ends. Off by default — the dashboard is live, and
    /// a file nobody asked for is a file nobody cleans up.
    #[arg(long, value_name = "PATH")]
    log: Option<PathBuf>,
}

/// `mezz monitor`, resolved out of `dispatch_server`.
///
/// Its own function for the reason [`mezz::check::rules`]' `SPECS` table
/// gives: in a flat dispatch, cyclomatic complexity *is* the number of arms
/// plus whatever each one does inline, so an arm that resolves five settings
/// with `||` and `or_else` charges all five to the dispatcher. This arm put
/// `dispatch_server` over the repo's cognitive ceiling (19 → 24, bar 22) and
/// would have made the next command unlandable. Every neighbouring arm that
/// does real work — `run_analyze`, `run_deps`, `run_serve` — is already
/// shaped this way.
///
/// Taking [`MonitorArgs`] rather than the `Commands` it arrived in also
/// retires the `unreachable!` this used to open with: a handler that can only
/// be called with what it can handle has no wrong case to word.
fn run_monitor(args: MonitorArgs) -> Result<()> {
    // The same scoped resolution `watch` uses, and for the same reason: the
    // operator chose this path, so the repo's own settings apply on top of
    // theirs.
    let settings = mezz::settings::load_scoped(&args.path).merged();
    mezz::monitor::run(mezz::monitor::MonitorOptions {
        include_tests: args.include_tests || settings.include_tests.unwrap_or(false),
        include_docs: args.include_docs || settings.include_docs.unwrap_or(false),
        languages: args.language.or_else(|| settings.language.clone()),
        spec_dir: args.spec_dir.or(settings.spec_dir.clone()),
        debounce_ms: args.debounce_ms.or(settings.debounce_ms).unwrap_or(1000),
        min_interval_ms: args.min_interval_ms,
        history: args.history,
        baseline: args.baseline,
        against: args.against,
        log: args.log,
        settings,
        path: args.path,
    })
}

/// Educator: linting a file, and the two content-generation aids.
fn dispatch_educator(command: Commands) -> Result<()> {
    match command {
        Commands::Educate {
            file,
            json,
            content,
        } => run_educate(file, json, content),

        Commands::ConstructKinds {
            language,
            output,
            check,
        } => run_construct_kinds(&language, output, check),

        Commands::EducatorIndex {
            language,
            output,
            content,
            check,
        } => run_educator_index(&language, output, content, check),

        _ => unreachable!("dispatch_educator received a non-educator command"),
    }
}

/// Surfaces built for agents rather than people: the MCP server, the
/// push-mode hooks, and the PR comment renderer.
fn dispatch_agent(command: Commands) -> Result<()> {
    match command {
        Commands::Mcp {
            path,
            include_tests,
            language,
        } => mezz::mcp::run(path, include_tests, language),

        Commands::Hook { action } => dispatch_hook(action),

        Commands::PrReport {
            path,
            base_ref,
            include_tests,
            language,
        } => run_pr_report(path, base_ref, include_tests, language),

        _ => unreachable!("dispatch_agent received a non-agent command"),
    }
}

/// MCP-007: emit only the newly-surfaced structural regressions to
/// stdout (silent when clean), leaving analysis diagnostics on stderr.
#[allow(clippy::too_many_arguments)]
fn run_hook_self_review(
    path: PathBuf,
    base_ref: String,
    state: Option<PathBuf>,
    min_severity: String,
    cap: usize,
    include_tests: bool,
    language: Option<Vec<String>>,
    block: bool,
) -> Result<()> {
    use mezz::mcp::push::{self, Severity};

    let root = path.canonicalize().unwrap_or(path);
    let min = Severity::parse(&min_severity).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid --min-severity '{}': use low|medium|high",
            min_severity
        )
    })?;

    let output = push::self_review(&root, &base_ref, include_tests, &language, state, min, cap)?;
    emit_hook_output(&output, block, "assess_change", &base_label(&root, &base_ref))
}

/// Where a hook's rendered delta goes.
///
/// Findings always go to stdout, whether or not the stop is blocked: the
/// analyzer writes its progress to stderr, and a caller that has to discard
/// that noise must not be discarding the findings with it.
///
/// `--block` adds the one channel a `Stop` hook has into the agent's context.
/// Exit 0 from a `Stop` hook sends stdout to a debug log — not the transcript,
/// and never Claude — so an advisory run is a run nothing reads. Exit 2 blocks
/// the stop instead and hands the agent the hook's stderr, which is why the
/// wired command re-emits this stdout there.
///
/// Resolved-only output never blocks. That is the loop closing, and halting an
/// agent to tell it the tree improved costs a turn and teaches it the channel
/// is noise.
///
/// Nothing here guards against blocking forever, because nothing needs to:
/// `push`'s session state records a finding as it is emitted, so the run after
/// a block no longer counts it as new. Each finding costs at most one extra
/// turn, and an agent that fixes them converges.
fn emit_hook_output(output: &str, block: bool, tool: &str, base: &str) -> Result<()> {
    if output.is_empty() {
        return Ok(());
    }
    if !(block && mezz::mcp::push::has_new_findings(output)) {
        println!("{output}");
        return Ok(());
    }
    println!("{output}\n\n{}", blocking_note(base, tool));
    std::process::exit(2);
}

/// What the hook says over a finding it cannot attribute.
///
/// Not "this session introduced". The hook compares the working tree against
/// a ref; it has no notion of which files a session touched, so that sentence
/// was asserted about every uncommitted change in the checkout regardless of
/// origin. With one session per tree the two sets coincide and it was true by
/// construction — concurrent sessions in one directory break that, and
/// nothing here noticed.
///
/// Three field reports on one day, and a fourth occurrence while fixing them:
/// each names a file the session never opened, and each records the same
/// near-miss. The message is imperative and the hook exits 2, so the cheapest
/// way out of a blocked turn is to edit whatever was named — in a shared
/// checkout, that is another agent's half-written file. On a cold context
/// there is nothing in the output to check the claim against.
///
/// So it states what was measured and leaves attribution to the reader, who
/// can do it. The `git status` pointer is there because that is the check
/// every reporter ran by hand to disprove the claim.
fn blocking_note(base: &str, tool: &str) -> String {
    format!(
        "The working tree differs from {base} by the structural regressions \
         above. If they are not yours, they belong to uncommitted work already \
         in the checkout — `git status` says whose. Fix them, run `{tool}` for \
         the full picture, or say why they stand."
    )
}

/// The base as the reader spelled it, plus what it resolved to: `HEAD
/// (a8cbe16)`.
///
/// Both halves, because a ref moves. `HEAD` alone cannot be compared against
/// the next run's, and a bare SHA is not the thing anybody typed.
fn base_label(root: &std::path::Path, base_ref: &str) -> String {
    match mezz::diff::resolve_git_ref(root, base_ref) {
        Ok(sha) if !sha.is_empty() => format!("{base_ref} ({})", &sha[..sha.len().min(7)]),
        _ => base_ref.to_string(),
    }
}

/// The push-mode legs, dispatched apart from everything else so adding one
/// does not widen `dispatch_agent` — the complexity gate fails on any metric
/// increase to a function that already exists (CI-001).
fn dispatch_hook(action: HookAction) -> Result<()> {
    match action {
        HookAction::SelfReview {
            path,
            base_ref,
            state,
            min_severity,
            cap,
            include_tests,
            language,
            block,
        } => run_hook_self_review(
            path,
            base_ref,
            state,
            min_severity,
            cap,
            include_tests,
            language,
            block,
        ),
        HookAction::Check {
            path,
            base_ref,
            state,
            cap,
            block,
        } => run_hook_check(path, base_ref, state, cap, block),
    }
}

/// MCP-019: emit only the rule violations this edit introduced, and
/// nothing at all when it introduced none.
fn run_hook_check(
    path: PathBuf,
    base_ref: String,
    state: Option<PathBuf>,
    cap: usize,
    block: bool,
) -> Result<()> {
    let root = path.canonicalize().unwrap_or(path);
    let output = mezz::mcp::push::check_new(&root, &base_ref, state, cap)?;
    emit_hook_output(&output, block, "mezz check", &base_label(&root, &base_ref))
}

/// MCP-008: render the PR comment body to stdout. Always exits 0 so a CI
/// job wiring this in stays non-blocking (signals, not gates).
fn run_pr_report(
    path: PathBuf,
    base_ref: String,
    include_tests: bool,
    language: Option<Vec<String>>,
) -> Result<()> {
    use mezz::mcp::push;

    let root = path.canonicalize().unwrap_or(path);
    match push::pr_report(&root, &base_ref, include_tests, &language) {
        Ok(body) => println!("{body}"),
        Err(e) => {
            // Non-blocking: surface the reason but do not fail the build.
            eprintln!("mezz pr-report: {e:#}");
            println!(
                "{}\n**mezz:** report unavailable ({e}).",
                push::PR_COMMENT_MARKER
            );
        }
    }
    Ok(())
}

fn run_educate(file: PathBuf, json: bool, content: Option<PathBuf>) -> Result<()> {
    use colored::Colorize;
    use mezz::educator::Educator;

    let cwd = std::env::current_dir()?;
    let content_root = content
        .or_else(|| Educator::resolve_content_root(&cwd))
        .ok_or_else(|| anyhow::anyhow!(
            "no Educator content found — pass --content <dir>, set MEZZ_EDUCATOR_CONTENT, or run from a workspace with a content/ directory"
        ))?;
    let educator = Educator::load(&content_root)?;
    let response = educator.scan_file(&file)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    if response.hits.is_empty() {
        eprintln!("{} no rules fired for {}", "✓".green(), file.display());
        return Ok(());
    }

    // Linter-style: <file>:<line>:<col>  <severity>  <rule-id>  <title>
    // Lines and columns are 1-based on output (editor convention) even though
    // tree-sitter / VS Code use 0-based internally.
    let path_label = file.display().to_string();
    let mut by_severity: std::collections::BTreeMap<&str, usize> = Default::default();
    for hit in &response.hits {
        let severity_label = match hit.severity.as_str() {
            "error" => "error".red().bold(),
            "warning" => "warning".yellow().bold(),
            _ => "info".blue().bold(),
        };
        *by_severity
            .entry(severity_label_key(&hit.severity))
            .or_insert(0) += 1;
        println!(
            "{}:{}:{}  {}  {}  {}",
            path_label,
            hit.line + 1,
            hit.col + 1,
            severity_label,
            hit.rule_id.cyan(),
            hit.title.dimmed(),
        );
    }
    eprintln!();
    let total = response.hits.len();
    let summary = by_severity
        .iter()
        .map(|(sev, n)| format!("{} {}", n, sev))
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!("{} {} hit(s) — {}", "→".bright_white(), total, summary);
    Ok(())
}

fn severity_label_key(sev: &str) -> &'static str {
    match sev {
        "error" => "error",
        "warning" => "warning",
        _ => "info",
    }
}

fn run_educator_index(
    language: &str,
    output: Option<PathBuf>,
    content: Option<PathBuf>,
    check: bool,
) -> Result<()> {
    use mezz::educator::Educator;

    let cwd = std::env::current_dir()?;
    let content_root = content
        .or_else(|| Educator::resolve_content_root(&cwd))
        .ok_or_else(|| anyhow::anyhow!(
            "no Educator content found — pass --content <dir>, set MEZZ_EDUCATOR_CONTENT, or run from a workspace with a content/ directory"
        ))?;
    let educator = Educator::load(&content_root)?;
    let rendered = mezz::educator::index::render_index(&educator, language);
    let target = output.unwrap_or_else(|| content_root.join(format!("{}/INDEX.md", language)));

    if check {
        let current = std::fs::read_to_string(&target).unwrap_or_default();
        if current != rendered {
            anyhow::bail!(
                "{} is out of date — run `mezz educator-index {}` to regenerate",
                target.display(),
                language
            );
        }
        eprintln!("✓ {} is up to date", target.display());
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, &rendered)?;
    eprintln!("✓ wrote {}", target.display());
    Ok(())
}

/// `mezz serve`'s flags exactly as clap parsed them.
///
/// A struct rather than eleven positional arguments, and separate from
/// `ServeOptions` because two of these still need work — the seeds are raw
/// `slug=path` strings and the cache directory may be unset — and that work
/// can fail, which is what `run_serve` is for.
struct ServeArgs {
    port: u16,
    seed: Vec<String>,
    cache_dir: Option<PathBuf>,
    jobs: usize,
    clone_timeout_secs: u64,
    max_repo_mb: u64,
    include_tests: bool,
    languages: Option<Vec<String>>,
    access: mezz::server::AccessOptions,
    ui_dir: Option<PathBuf>,
    settings_ui_dir: Option<PathBuf>,
}

/// `mezz serve` — parse the `--seed` pairs, resolve the cache directory, and
/// hand off to the server. Kept out of `main`'s match so the dispatch stays a
/// thin delegation, like every other subcommand.
fn run_serve(args: ServeArgs) -> Result<()> {
    let seeds = args
        .seed
        .iter()
        .map(|s| mezz::server::parse_seed(s))
        .collect::<Result<Vec<_>>>()?;
    mezz::server::serve(mezz::server::ServeOptions {
        port: args.port,
        seeds,
        cache_dir: args
            .cache_dir
            .unwrap_or_else(mezz::server::default_cache_dir),
        jobs: args.jobs,
        clone_timeout_secs: args.clone_timeout_secs,
        max_repo_mb: args.max_repo_mb,
        include_tests: args.include_tests,
        languages: args.languages,
        access: args.access,
        ui_dir: args.ui_dir,
        settings_ui_dir: args.settings_ui_dir,
    })
}

fn run_construct_kinds(language: &str, output: Option<PathBuf>, check: bool) -> Result<()> {
    use mezz::educator::catalog;

    let specs = catalog::for_language(language).ok_or_else(|| {
        anyhow::anyhow!(
            "no Educator catalog registered for language `{}` (known: java)",
            language
        )
    })?;
    let rendered = catalog::render_catalog(language, specs);
    let target =
        output.unwrap_or_else(|| PathBuf::from(format!("content/{}/construct-kinds.md", language)));

    if check {
        let current = std::fs::read_to_string(&target).unwrap_or_default();
        if current != rendered {
            anyhow::bail!(
                "{} is out of date — run `mezz construct-kinds {}` to regenerate",
                target.display(),
                language
            );
        }
        eprintln!("✓ {} is up to date", target.display());
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, &rendered)?;
    eprintln!("✓ wrote {}", target.display());
    Ok(())
}

/// Fold the two selection flags into a config.
///
/// Split out of `run_analyze` because the loops are the only nesting in an
/// otherwise flat function, and the complexity ceiling in CONTRIBUTING.md is
/// measured per function.
fn apply_filters(
    config: &mut Config,
    languages: Option<Vec<String>>,
    kinds: Option<Vec<EntityKindArg>>,
) {
    for lang in languages.into_iter().flatten() {
        match Language::from_name(&lang) {
            Some(language) => {
                config.analysis.languages.insert(language);
            }
            None => eprintln!("   \u{26a0} unknown language `{lang}` \u{2014} ignoring it."),
        }
    }
    for kind in kinds.into_iter().flatten() {
        config.filters.entity_kinds.insert(kind.into());
    }
}

fn run_analyze(
    path: PathBuf,
    format: OutputFormat,
    output_path: Option<PathBuf>,
    depth: Option<usize>,
    languages: Option<Vec<String>>,
    kinds: Option<Vec<EntityKindArg>>,
    include_tests: bool,
    include_docs: bool,
    include_locals: bool,
    include_external: bool,
    layout: LayoutDirection,
    group_by_file: bool,
    line_numbers: bool,
    qualified_names: bool,
    root: Option<String>,
    focus: Option<String>,
) -> Result<()> {
    let mut config = Config::for_path(&path)
        .with_output_format(format)
        .with_layout_direction(layout);

    config.analysis.include_tests = include_tests;
    config.analysis.include_docs = include_docs;
    config.analysis.include_locals = include_locals;
    config.analysis.include_external = include_external;
    config.display.group_by_file = group_by_file;
    config.display.show_line_numbers = line_numbers;
    config.display.show_qualified_names = qualified_names;
    config.filters.root_entity = root;
    config.filters.focus_entity = focus;

    apply_filters(&mut config, languages, kinds);

    // Last: the settings file fills only what the flags above left alone.
    // `depth` travels separately because a defaulted `max_depth` and a typed
    // one are the same number by the time the config gets here.
    let flags = mezz::settings::Flags {
        max_depth: depth,
        ..Default::default()
    };
    mezz::settings::load(&path).apply_with(&mut config, flags);

    // Run analysis
    eprintln!("Analyzing {}...", path.display());
    let mut analyzer = Analyzer::new(config.clone());
    let result = analyzer.analyze()?;

    eprintln!(
        "Found {} entities and {} relationships",
        result.entities.len(),
        result.relationships.len()
    );
    if !result.warnings.is_empty() {
        eprintln!("\n{} warning(s):", result.warnings.len());
        for w in &result.warnings {
            eprintln!("  - {}", w);
        }
    }

    // Build graph and render
    let graph = DependencyGraph::from_analysis(&result);
    let output_str = output::render(&graph, &config)?;

    // Write output
    if let Some(out_path) = output_path {
        std::fs::write(&out_path, &output_str)?;
        eprintln!("Output written to {}", out_path.display());

        // For JSON output, also write sidecar detail and index files
        if matches!(config.output_format, OutputFormat::Json) {
            let details_path = out_path.with_extension("details.json");
            let details_str = JsonRenderer::render_details(&graph, &config)?;
            std::fs::write(&details_path, &details_str)?;
            eprintln!("Details written to {}", details_path.display());

            let index_path = out_path.with_extension("index.json");
            let index_str = JsonRenderer::render_index(&graph, &config)?;
            std::fs::write(&index_path, &index_str)?;
            eprintln!("Index written to {}", index_path.display());
        }
    } else {
        println!("{}", output_str);
    }

    Ok(())
}

/// `deps` traverses shallower than the rest of mezz — two hops, not three —
/// because it answers "what does this file touch" rather than "what shape is
/// this repo". That 2 is the bottom of the chain, under both the flag and the
/// settings file.
const DEPS_DEFAULT_DEPTH: usize = 2;

fn run_deps(target: PathBuf, depth: Option<usize>, reverse: bool) -> Result<()> {
    print!("{}", deps_report(&target, depth, reverse)?);
    Ok(())
}

/// The report itself, split from the printing so a test can hold it.
///
/// The split is the point of CLI-001 rather than tidiness: the defect was
/// never in the rendering, it was in *what graph* was handed to it, and a
/// test that builds the graph the way the renderer's own tests do would have
/// passed against the broken command. This is the seam a regression test can
/// stand on — a repo on disk in, the finished report out.
fn deps_report(target: &Path, depth: Option<usize>, reverse: bool) -> Result<String> {
    // The repo, not the file's own directory (CLI-001). Every dependent of a
    // file lives outside it, so a graph built from the file alone answers
    // "nothing depends on this" by construction — no parser improvement would
    // ever have moved it. It also puts settings back on the root every other
    // command resolves, instead of wherever the file happens to sit.
    let root = mezz::settings::repo_root(target.parent().unwrap_or(target));
    let mut config = Config::for_path(&root).with_max_depth(DEPS_DEFAULT_DEPTH);
    let flags = mezz::settings::Flags {
        max_depth: depth,
        ..Default::default()
    };
    mezz::settings::load(&root).apply_with(&mut config, flags);
    // The settled depth, whichever link of the chain supplied it. The
    // report below walks that many levels out of the file.
    let depth = config.analysis.max_depth;

    // Warm on repeat calls: the parse store holds the tree from whichever
    // command analysed it last, so the wider analysis is paid once.
    let mut analyzer = Analyzer::new(config.clone());
    let result = analyzer.analyze()?;

    let graph = DependencyGraph::from_analysis(&result);

    Ok(mezz::output::deps_report::render(
        &graph, &root, target, depth, reverse,
    ))
}

fn run_find(pattern: &str, path: PathBuf, kind: Option<EntityKindArg>) -> Result<()> {
    let mut config = Config::for_path(&path);
    mezz::settings::load(&path).apply_to_config(&mut config);

    let mut analyzer = Analyzer::new(config);
    let result = analyzer.analyze()?;

    let pattern_lower = pattern.to_lowercase();

    println!("Searching for '{}' in {}...", pattern, path.display());
    println!();

    let mut found = 0;
    for entity in &result.entities {
        // Filter by kind if specified
        if let Some(k) = kind {
            let filter_kind: EntityKind = k.into();
            if entity.kind != filter_kind {
                continue;
            }
        }

        // Match by name
        if entity.name.to_lowercase().contains(&pattern_lower)
            || entity
                .qualified_name
                .to_lowercase()
                .contains(&pattern_lower)
        {
            println!(
                "{} {} in {} [L{}]",
                entity.kind.display_name(),
                entity.name,
                entity.file_path.display(),
                entity.span.start.line + 1
            );
            found += 1;
        }
    }

    println!("\nFound {} matches.", found);

    Ok(())
}

fn run_cycles(path: PathBuf, format: OutputFormat) -> Result<()> {
    let mut config = Config::for_path(&path).with_output_format(format);
    mezz::settings::load(&path).apply_to_config(&mut config);

    let mut analyzer = Analyzer::new(config);
    let result = analyzer.analyze()?;

    let graph = DependencyGraph::from_analysis(&result);
    let cycles = graph.find_cycles();

    if cycles.is_empty() {
        println!("✓ No circular dependencies found!");
    } else {
        println!("⚠ Found {} circular dependencies:\n", cycles.len());

        for (i, cycle) in cycles.iter().enumerate() {
            println!("Cycle {}:", i + 1);
            for (j, entity_id) in cycle.iter().enumerate() {
                let name = graph
                    .get_entity(entity_id)
                    .map(|e| e.name.as_str())
                    .unwrap_or(entity_id);

                if j == cycle.len() - 1 {
                    println!("  └─→ {} (back to start)", name);
                } else {
                    println!("  {} → ", name);
                }
            }
            println!();
        }
    }

    Ok(())
}

fn run_stats(path: PathBuf, json: bool) -> Result<()> {
    let mut config = Config::for_path(&path);
    mezz::settings::load(&path).apply_to_config(&mut config);

    let mut analyzer = Analyzer::new(config);
    let result = analyzer.analyze()?;

    let graph = DependencyGraph::from_analysis(&result);
    let metrics = graph.metrics();

    // Count by kind
    let mut kind_counts = std::collections::HashMap::new();
    for entity in &result.entities {
        *kind_counts.entry(entity.kind).or_insert(0usize) += 1;
    }

    // Count by relationship kind
    let mut rel_counts = std::collections::HashMap::new();
    for rel in &result.relationships {
        *rel_counts.entry(rel.kind).or_insert(0usize) += 1;
    }

    if json {
        let stats = serde_json::json!({
            "path": path.display().to_string(),
            "entities": {
                "total": metrics.node_count,
                "by_kind": kind_counts.iter().map(|(k, v)| (k.display_name(), v)).collect::<std::collections::HashMap<_, _>>()
            },
            "relationships": {
                "total": metrics.edge_count,
                "by_kind": rel_counts.iter().map(|(k, v)| (k.display_label(), v)).collect::<std::collections::HashMap<_, _>>()
            },
            "metrics": {
                "average_degree": metrics.average_degree,
                "cycle_count": metrics.cycle_count,
                "most_connected": metrics.most_connected.iter().take(5).collect::<Vec<_>>()
            },
            "files": result.files.len()
        });
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Code Statistics for: {}", path.display());
        println!("═══════════════════════════════════════════════════════");
        println!();
        println!("Files analyzed: {}", result.files.len());
        println!("Total entities: {}", metrics.node_count);
        println!("Total relationships: {}", metrics.edge_count);
        println!();

        println!("Entities by kind:");
        for (kind, count) in &kind_counts {
            println!("  {}: {}", kind.display_name(), count);
        }
        println!();

        println!("Relationships by type:");
        for (kind, count) in &rel_counts {
            println!("  {}: {}", kind.display_label(), count);
        }
        println!();

        println!("Graph metrics:");
        println!(
            "  Average connections per entity: {:.2}",
            metrics.average_degree
        );
        println!("  Circular dependencies: {}", metrics.cycle_count);
        println!();

        if !metrics.most_connected.is_empty() {
            println!("Most connected entities:");
            for (i, (id, count)) in metrics.most_connected.iter().take(5).enumerate() {
                let name = graph.get_entity(id).map(|e| e.name.as_str()).unwrap_or(id);
                println!("  {}. {} ({} connections)", i + 1, name, count);
            }
        }
    }

    Ok(())
}

fn run_diff(
    path: PathBuf,
    from_ref: &str,
    to_ref: &str,
    output_path: PathBuf,
    include_tests: bool,
    languages: Option<Vec<String>>,
) -> Result<()> {
    use mezz::diff::{
        analyze_with, build_analysis_config, checkout_root, compute_diff, create_worktree,
        remove_worktree, render_base_details, resolve_git_ref, rooted_at, verify_git_repo,
        write_diff_outputs,
    };

    let repo_root = path.canonicalize()?;
    verify_git_repo(&repo_root)?;

    let from_sha = resolve_git_ref(&repo_root, from_ref)?;
    let to_sha = resolve_git_ref(&repo_root, to_ref)?;
    eprintln!("🔀 Diffing {} → {}", from_sha, to_sha);

    // Create + analyze base worktree.
    let tmp = std::env::temp_dir();
    let base_dir = tmp.join(format!("mezz-diff-base-{}", from_sha));
    let head_dir = tmp.join(format!("mezz-diff-head-{}", to_sha));

    // One scope for both sides, settled from the working tree. Each
    // worktree carries the `.mezz/settings.json` committed at its own ref,
    // so building a config per checkout would let a settings change between
    // the two refs read as every file it excludes being added or removed.
    let scope = build_analysis_config(&repo_root, include_tests, &languages);

    create_worktree(&repo_root, &base_dir, from_ref)?;
    // Both sides are checkouts, so both need the subtree that corresponds to
    // the analyzed root rather than the checkout's top (SRV-021). They move
    // together: the head has the same defect and is invisible today only
    // because both sides are wrong identically — fixing one would make it
    // visible and worse.
    let base_root = checkout_root(&repo_root, &base_dir);
    let (base_graph, base_config) = analyze_with(
        rooted_at(&scope, &base_root),
        &format!("base ({})", from_sha),
    )?;

    // Create + analyze head worktree.
    if let Err(e) = create_worktree(&repo_root, &head_dir, to_ref) {
        remove_worktree(&repo_root, &base_dir);
        return Err(e);
    }
    let head_root = checkout_root(&repo_root, &head_dir);
    let (head_graph, head_config) =
        analyze_with(rooted_at(&scope, &head_root), &format!("head ({})", to_sha))?;

    // Compute diff.
    eprintln!("  Computing structural diff...");
    let diff = compute_diff(
        &base_graph,
        &head_graph,
        &base_root,
        &head_root,
        &from_sha,
        &to_sha,
    );
    eprintln!(
        "  Result: {} added, {} removed, {} modified, {} unchanged",
        diff.summary.added, diff.summary.removed, diff.summary.modified, diff.summary.unchanged,
    );

    // Write output files. The base details are rendered before the worktrees
    // go, since the file half of them is read off disk.
    let data_dir = output_path.parent().unwrap_or(std::path::Path::new("."));
    let base_details = render_base_details(&base_graph, &base_config)?;
    write_diff_outputs(data_dir, &head_graph, &head_config, &base_details, &diff)?;
    eprintln!("  Output written to {}", data_dir.display());

    // Clean up worktrees.
    remove_worktree(&repo_root, &base_dir);
    remove_worktree(&repo_root, &head_dir);

    eprintln!("✅ Done. Open the UI to see the diff overlay.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway checkout, removed when the test drops it.
    struct TmpRepo(PathBuf);

    impl TmpRepo {
        /// A directory holding a `.git` marker — what
        /// [`mezz::settings::repo_root`] walks up to — and the given files,
        /// written at paths relative to it.
        fn with(tag: &str, files: &[(&str, &str)]) -> Self {
            let root = std::env::temp_dir().join(format!("mezz-deps-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join(".git")).expect("temp repo");
            for (path, source) in files {
                let file = root.join(path);
                std::fs::create_dir_all(file.parent().expect("a parent")).expect("temp dir");
                std::fs::write(&file, source).expect("temp file");
            }
            TmpRepo(root)
        }

        fn path(&self, rest: &str) -> PathBuf {
            self.0.join(rest)
        }
    }

    impl Drop for TmpRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// CLI-001. `deps --reverse` names the files that depend on the target.
    ///
    /// The caller is put in a *sibling folder* of the target deliberately.
    /// The command used to analyse the target file alone, so the graph it
    /// asked held one file and no edge could have arrived from outside it —
    /// every file in every repo answered "Nothing depends on this file". A
    /// test written against a hand-built graph, or against two files in one
    /// directory, passes on that bug; only a caller the analysis has to walk
    /// up to the repo root to find fails on it.
    #[test]
    fn reverse_deps_find_a_caller_in_another_folder() {
        let repo = TmpRepo::with(
            "reverse",
            &[
                ("src/target.rs", "pub fn target_fn() -> usize {\n    7\n}\n"),
                (
                    "lib/caller.rs",
                    "pub fn caller_fn() -> usize {\n    target_fn() + 1\n}\n",
                ),
            ],
        );

        let out = deps_report(&repo.path("src/target.rs"), Some(1), true)
            .expect("the report is built");

        // Named relative to the repo root, not by the absolute path the
        // command was handed: the answer must not depend on which directory
        // the reader was standing in (CFG-013).
        assert!(out.contains("← lib/caller.rs: caller_fn (calls)"), "{out}");
        assert!(out.starts_with("Dependencies for: src/target.rs\n"), "{out}");
        assert!(!out.contains("Nothing depends on this file"), "{out}");
    }

    /// The forward half of the same fix: the dependency is a declaration in
    /// another file of the repo, and the `+ 1` the caller also does is not a
    /// dependency of anything.
    #[test]
    fn forward_deps_name_a_callee_in_another_folder() {
        let repo = TmpRepo::with(
            "forward",
            &[
                ("src/target.rs", "pub fn target_fn() -> usize {\n    7\n}\n"),
                (
                    "lib/caller.rs",
                    "pub fn caller_fn() -> usize {\n    target_fn() + 1\n}\n",
                ),
            ],
        );

        let out = deps_report(&repo.path("lib/caller.rs"), Some(1), false)
            .expect("the report is built");

        assert!(out.contains("target.rs: target_fn"), "{out}");
    }

    /// Every tool the MCP server dispatches is reachable from the CLI.
    ///
    /// The two front doors declare their surfaces separately — `TOOLS` as a
    /// table, the CLI as typed subcommands — because each needs arguments the
    /// other cannot express. This is what keeps them in step: a tool added to
    /// one and not the other fails here rather than quietly existing on half
    /// the product, which is the state CLI-002 was written about.
    #[test]
    fn every_tool_has_a_cli_command() {
        use clap::CommandFactory;

        let commands: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .collect();

        let missing: Vec<&str> = mezz::mcp::tool_names()
            .into_iter()
            .filter(|tool| !commands.contains(&tool.replace('_', "-")))
            .collect();

        assert!(
            missing.is_empty(),
            "tools served over MCP with no CLI command: {missing:?}"
        );
    }

    /// An unset argument is left out, not sent as null.
    ///
    /// The tools read their arguments with `.get(..)`, so a serialized `None`
    /// arrives as present-and-null and takes whichever branch that happens to
    /// hit — `depth` defaulting to 2 is not the same as `depth: null`. The
    /// call the CLI builds has to be the call an agent builds.
    #[test]
    fn an_unset_argument_is_absent_rather_than_null() {
        let scope = Scope {
            root: PathBuf::from("."),
            include_tests: false,
            language: None,
            format: ToolFormatArg::Text,
        };
        let (_, name, args) = AreaTool::Map {
            scope,
            path: Some("src/mcp".into()),
            depth: None,
        }
        .call()
        .expect("map builds a call");

        assert_eq!(name, "map");
        assert_eq!(args, serde_json::json!({ "path": "src/mcp" }));
        assert!(args.get("depth").is_none(), "unset depth must not be sent");
    }

    /// A flag left off is absent too, for the same reason.
    #[test]
    fn an_unset_flag_is_absent_rather_than_false() {
        let scope = Scope {
            root: PathBuf::from("."),
            include_tests: false,
            language: None,
            format: ToolFormatArg::Text,
        };
        let (_, _, args) = AreaTool::DeadCode {
            scope,
            path: None,
            include_public: false,
        }
        .call()
        .expect("dead_code builds a call");

        assert_eq!(args, serde_json::json!({}));
    }

    /// A subject tool names its target the way the schema spells it.
    ///
    /// `impact` reached by `--path`/`--line` must send both, and must not
    /// invent an `entity` key — the tool picks its targeting mode by which
    /// arguments are present.
    #[test]
    fn a_positional_target_sends_path_and_line() {
        let scope = Scope {
            root: PathBuf::from("."),
            include_tests: false,
            language: None,
            format: ToolFormatArg::Text,
        };
        let (_, name, args) = SubjectTool::Impact {
            scope,
            target: Target {
                entity: None,
                path: Some("src/mcp/format.rs".into()),
                line: Some(26),
            },
            depth: Some(3),
            direction: None,
        }
        .call();

        assert_eq!(name, "impact");
        assert_eq!(
            args,
            serde_json::json!({ "path": "src/mcp/format.rs", "line": 26, "depth": 3 })
        );
    }

    /// `--move what:into` becomes `layout`'s `moves` array.
    #[test]
    fn a_move_pair_becomes_one_moves_entry() {
        let moves = move_args(vec!["src/a.rs:src/core".into(), "src/b.rs:src/io".into()])
            .expect("well-formed pairs parse")
            .expect("two moves are Some");

        assert_eq!(
            moves,
            serde_json::json!([
                { "what": "src/a.rs", "into": "src/core" },
                { "what": "src/b.rs", "into": "src/io" },
            ])
        );
    }

    /// A move mezz cannot read is refused rather than guessed at.
    ///
    /// `split_once` takes the *first* colon, so `a:b:c` would silently move
    /// `a` into `b:c`. Rejecting is the only safe reading: the tool emits
    /// `git mv` lines, and a wrong one moves a real file.
    #[test]
    fn a_move_without_a_clean_pair_is_refused() {
        for bad in ["src/a.rs", "src/a.rs:", ":src/core", "a:b:c"] {
            assert!(
                move_args(vec![bad.into()]).is_err(),
                "`{bad}` should not parse as a move"
            );
        }
    }

    /// No `--move` at all means the argument is absent, so `layout`
    /// proposes an arrangement instead of scoring an empty one.
    #[test]
    fn no_moves_means_no_moves_argument() {
        assert_eq!(move_args(vec![]).expect("empty is fine"), None);
    }

    /// Field reports, 2026-08-31, filed three times in one day and hit a
    /// fourth time while they were being fixed: the hook said "This session
    /// introduced the structural regressions above. Fix them" over findings
    /// in files the session never opened. It compares the working tree
    /// against a ref and has no way to know who wrote what, so in a shared
    /// checkout the sentence was simply false — and the reader it lands on
    /// is, by construction, an agent that has just been blocked and told to
    /// fix something.
    ///
    /// The claim is what must not come back. The rest of the wording can
    /// move.
    #[test]
    fn a_blocking_hook_does_not_claim_to_know_who_wrote_the_findings() {
        let note = blocking_note("HEAD (a8cbe16)", "assess_change");
        assert!(
            !note.contains("This session introduced") && !note.contains("your changes"),
            "the hook is asserting authorship it cannot observe:\n{note}"
        );
        // What it may say instead: what was compared, and where to look.
        assert!(
            note.contains("HEAD (a8cbe16)") && note.contains("git status"),
            "the reader cannot attribute the finding either:\n{note}"
        );
        // The finding still has to read as actionable — the hook exits 2 and
        // an agent that reads this as advisory learns to ignore the channel.
        assert!(
            note.contains("Fix them") && note.contains("`assess_change`"),
            "the blocking instruction went missing with the false claim:\n{note}"
        );
    }

    /// Both halves of the base, because a ref moves: `HEAD` alone cannot be
    /// compared against the next run's, and a bare SHA is not what anyone
    /// typed. Outside a checkout there is no SHA to add.
    #[test]
    fn the_base_label_carries_the_ref_and_what_it_resolved_to() {
        let loose = std::env::temp_dir().join(format!("mezz-label-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&loose);
        std::fs::create_dir_all(&loose).unwrap();
        assert_eq!(base_label(&loose, "HEAD"), "HEAD");
        let _ = std::fs::remove_dir_all(&loose);
    }
}
