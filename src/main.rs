//! Nao CLI
//!
//! A tool for visualizing code relationships and dependencies.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use nao::{
    analyzer::Analyzer,
    config::{Config, LayoutDirection},
    graph::DependencyGraph,
    models::{file_info::Language, EntityKind},
    output::{self, JsonRenderer, OutputFormat},
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nao")]
#[command(author = "Nao Team")]
#[command(version)]
#[command(about = "Visualize code relationships, dependencies, and quality metrics", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set a repo up for nao: write `.nao/settings.json` with the languages
    /// the tree is actually written in, and optionally add the VS Code tasks
    /// that start, open and stop the browser UI.
    Init {
        /// Repo to set up (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Also add the Nao tasks to `.vscode/tasks.json`, creating it or
        /// merging into what is already there.
        #[arg(long)]
        vscode: bool,

        /// Replace what is already there. Without it, an existing settings
        /// file is left alone and existing tasks keep their current bodies.
        #[arg(long)]
        force: bool,
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
        /// answer for a repo is `include_locals` in `.nao/settings.json`.
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

    /// Show dependencies of a specific file or entity
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

        /// Output format
        #[arg(short, long, default_value = "ascii")]
        format: OutputFormatArg,
    },

    /// Find a specific entity (class, function, etc.)
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

    /// Show statistics about the codebase
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

        /// Content root for rule corpus (defaults to NAO_EDUCATOR_CONTENT or `<cwd>/content`)
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
        /// `NAO_EDUCATOR_CONTENT` or `<cwd>/content`.
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
        /// `NAO_EDUCATOR_CONTENT` nor `<workspace>/content/` resolves. The
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
        /// `NAO_UI_DIR`, then a `ui/dist` beside the `nao` binary, then
        /// `./ui/dist`. Without any of them the server still runs and the
        /// root page explains how to connect a UI hosted elsewhere.
        #[arg(long, value_name = "PATH")]
        ui_dir: Option<PathBuf>,

        /// Let the browser UI open a Claude Code terminal on this machine to
        /// refactor an entity. Off by default: it is the one route that runs
        /// code rather than serving data, so it does not exist unless you ask
        /// for it, and it always requires the pairing token — including from
        /// loopback, where reading the graph does not. Set `NAO_TERMINAL` to
        /// choose the terminal and `NAO_CLAUDE_BIN` for a non-PATH install.
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
        /// Defaults to `$XDG_CACHE_HOME/nao/serve` or `~/.cache/nao/serve`.
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
        /// Repeatable. See `nao watch --help` for why it is needed.
        #[arg(long, value_name = "URL")]
        allow_origin: Vec<String>,

        /// Don't require a pairing token from non-loopback origins.
        /// See `nao watch --help`.
        #[arg(long)]
        no_token: bool,

        /// Directory holding the built browser UI. Same resolution order as
        /// `nao watch --ui-dir`.
        #[arg(long, value_name = "PATH")]
        ui_dir: Option<PathBuf>,
    },

    /// Serve Nao as an MCP (Model Context Protocol) server over stdio,
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

    /// Push-mode hooks: emit nao's structural signal without being asked.
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
        #[arg(long, default_value_t = nao::mcp::push::DEFAULT_LINE_CAP)]
        cap: usize,

        /// Include test files
        #[arg(long)]
        include_tests: bool,

        /// Filter by language
        #[arg(short, long)]
        language: Option<Vec<String>>,
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
    let cli = Cli::parse();

    // Dispatch is grouped so that adding a subcommand touches a small
    // function instead of one match over every command in the tool. Each
    // arm returns its group's `Result` directly rather than `?;`-ing it,
    // because nao counts `?` as a branch and a bare delegation has nothing
    // to recover from. See CI-001.
    //
    // The match is exhaustive over `Commands`, so the compiler — not a
    // catch-all — guarantees every subcommand is routed somewhere.
    match cli.command {
        // Its own arm rather than a group: `init` is the one subcommand that
        // writes the repo's own configuration instead of reading a codebase.
        Commands::Init { path, vscode, force } => nao::init::run(&path, vscode, force),

        c @ (Commands::Analyze { .. }
        | Commands::Deps { .. }
        | Commands::Find { .. }
        | Commands::Cycles { .. }
        | Commands::Stats { .. }
        | Commands::Diff { .. }) => dispatch_analysis(c),

        c @ (Commands::Watch { .. } | Commands::Serve { .. }) => dispatch_server(c),

        c @ (Commands::Educate { .. }
        | Commands::ConstructKinds { .. }
        | Commands::EducatorIndex { .. }) => dispatch_educator(c),

        c @ (Commands::Mcp { .. } | Commands::Hook { .. } | Commands::PrReport { .. }) => {
            dispatch_agent(c)
        }
    }
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
            format,
        } => run_deps(target, depth, reverse, format.into()),

        Commands::Find { pattern, path, kind } => run_find(&pattern, path, kind),

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
            let loaded = nao::settings::load_scoped(&path);
            let settings = loaded.merged();
            // What the command line named, so the report can say a flag won
            // rather than guessing from a value it cannot distinguish.
            let flags_named = nao::settings::report::named(&[
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
            nao::server::run(nao::server::WatchOptions {
                output_dir: output_dir
                    .or_else(|| settings.output_dir.clone())
                    .unwrap_or_else(|| PathBuf::from("ui/public")),
                port: port.or(settings.port).unwrap_or(nao::settings::DEFAULT_PORT),
                include_tests: include_tests || settings.include_tests.unwrap_or(false),
                include_docs: include_docs || settings.include_docs.unwrap_or(false),
                languages: language.or_else(|| settings.language.clone()),
                // The flag first, then the repo's own answer — and the repo's
                // is guaranteed to stay inside the repo, which the flag's
                // deliberately isn't. See `settings::Settings::spec_dir`.
                spec_dir: spec_dir.or(settings.spec_dir.clone()),
                debounce_ms: debounce_ms.or(settings.debounce_ms).unwrap_or(300),
                content_fallback: content_fallback.or_else(|| settings.content_fallback.clone()),
                access: nao::server::AccessOptions {
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
            let settings = nao::settings::user();
            run_serve(ServeArgs {
                port: port.or(settings.port).unwrap_or(nao::settings::DEFAULT_PORT),
                seed,
                cache_dir,
                jobs,
                clone_timeout_secs,
                max_repo_mb,
                include_tests: include_tests || settings.include_tests.unwrap_or(false),
                languages: language.or_else(|| settings.language.clone()),
                access: nao::server::AccessOptions {
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

/// Educator: linting a file, and the two content-generation aids.
fn dispatch_educator(command: Commands) -> Result<()> {
    match command {
        Commands::Educate { file, json, content } => run_educate(file, json, content),

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
        } => nao::mcp::run(path, include_tests, language),

        Commands::Hook { action } => match action {
            HookAction::SelfReview {
                path,
                base_ref,
                state,
                min_severity,
                cap,
                include_tests,
                language,
            } => run_hook_self_review(
                path, base_ref, state, min_severity, cap, include_tests, language,
            ),
        },

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
) -> Result<()> {
    use nao::mcp::push::{self, Severity};

    let root = path.canonicalize().unwrap_or(path);
    let min = Severity::parse(&min_severity)
        .ok_or_else(|| anyhow::anyhow!("Invalid --min-severity '{}': use low|medium|high", min_severity))?;

    let output = push::self_review(&root, &base_ref, include_tests, &language, state, min, cap)?;
    // Quiet-when-clean: nothing on stdout, zero tokens into the agent.
    if !output.is_empty() {
        println!("{output}");
    }
    Ok(())
}

/// MCP-008: render the PR comment body to stdout. Always exits 0 so a CI
/// job wiring this in stays non-blocking (signals, not gates).
fn run_pr_report(
    path: PathBuf,
    base_ref: String,
    include_tests: bool,
    language: Option<Vec<String>>,
) -> Result<()> {
    use nao::mcp::push;

    let root = path.canonicalize().unwrap_or(path);
    match push::pr_report(&root, &base_ref, include_tests, &language) {
        Ok(body) => println!("{body}"),
        Err(e) => {
            // Non-blocking: surface the reason but do not fail the build.
            eprintln!("nao pr-report: {e:#}");
            println!("{}\n**nao:** report unavailable ({e}).", push::PR_COMMENT_MARKER);
        }
    }
    Ok(())
}

fn run_educate(file: PathBuf, json: bool, content: Option<PathBuf>) -> Result<()> {
    use colored::Colorize;
    use nao::educator::Educator;

    let cwd = std::env::current_dir()?;
    let content_root = content
        .or_else(|| Educator::resolve_content_root(&cwd))
        .ok_or_else(|| anyhow::anyhow!(
            "no Educator content found — pass --content <dir>, set NAO_EDUCATOR_CONTENT, or run from a workspace with a content/ directory"
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
        *by_severity.entry(severity_label_key(&hit.severity)).or_insert(0) += 1;
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
    use nao::educator::Educator;

    let cwd = std::env::current_dir()?;
    let content_root = content
        .or_else(|| Educator::resolve_content_root(&cwd))
        .ok_or_else(|| anyhow::anyhow!(
            "no Educator content found — pass --content <dir>, set NAO_EDUCATOR_CONTENT, or run from a workspace with a content/ directory"
        ))?;
    let educator = Educator::load(&content_root)?;
    let rendered = nao::educator::index::render_index(&educator, language);
    let target = output.unwrap_or_else(|| {
        content_root.join(format!("{}/INDEX.md", language))
    });

    if check {
        let current = std::fs::read_to_string(&target).unwrap_or_default();
        if current != rendered {
            anyhow::bail!(
                "{} is out of date — run `nao educator-index {}` to regenerate",
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

/// `nao serve`'s flags exactly as clap parsed them.
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
    access: nao::server::AccessOptions,
    ui_dir: Option<PathBuf>,
    settings_ui_dir: Option<PathBuf>,
}

/// `nao serve` — parse the `--seed` pairs, resolve the cache directory, and
/// hand off to the server. Kept out of `main`'s match so the dispatch stays a
/// thin delegation, like every other subcommand.
fn run_serve(args: ServeArgs) -> Result<()> {
    let seeds = args
        .seed
        .iter()
        .map(|s| nao::server::parse_seed(s))
        .collect::<Result<Vec<_>>>()?;
    nao::server::serve(nao::server::ServeOptions {
        port: args.port,
        seeds,
        cache_dir: args.cache_dir.unwrap_or_else(nao::server::default_cache_dir),
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

fn run_construct_kinds(
    language: &str,
    output: Option<PathBuf>,
    check: bool,
) -> Result<()> {
    use nao::educator::catalog;

    let specs = catalog::for_language(language).ok_or_else(|| {
        anyhow::anyhow!(
            "no Educator catalog registered for language `{}` (known: java)",
            language
        )
    })?;
    let rendered = catalog::render_catalog(language, specs);
    let target = output.unwrap_or_else(|| {
        PathBuf::from(format!("content/{}/construct-kinds.md", language))
    });

    if check {
        let current = std::fs::read_to_string(&target).unwrap_or_default();
        if current != rendered {
            anyhow::bail!(
                "{} is out of date — run `nao construct-kinds {}` to regenerate",
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
    let flags = nao::settings::Flags { max_depth: depth, ..Default::default() };
    nao::settings::load(&path).apply_with(&mut config, flags);

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

/// `deps` traverses shallower than the rest of nao — two hops, not three —
/// because it answers "what does this file touch" rather than "what shape is
/// this repo". That 2 is the bottom of the chain, under both the flag and the
/// settings file.
const DEPS_DEFAULT_DEPTH: usize = 2;

fn run_deps(
    target: PathBuf,
    depth: Option<usize>,
    reverse: bool,
    format: OutputFormat,
) -> Result<()> {
    let root = target.parent().unwrap_or(&target);
    let mut config = Config::for_path(root)
        .with_output_format(format)
        .with_max_depth(DEPS_DEFAULT_DEPTH);
    let flags = nao::settings::Flags { max_depth: depth, ..Default::default() };
    nao::settings::load(root).apply_with(&mut config, flags);
    // The settled depth, whichever link of the chain supplied it. The
    // printing below walks the same levels the traversal did.
    let depth = config.analysis.max_depth;

    let mut analyzer = Analyzer::new(config.clone());
    let result = analyzer.analyze_file(&target)?;

    let graph = DependencyGraph::from_analysis(&result);

    println!("Dependencies for: {}", target.display());
    println!();

    // Find the file entity
    for entity in graph.entities() {
        if entity.file_path == target {
            if reverse {
                println!("Dependents (what depends on this):");
                for (dep, rel) in graph.dependents(&entity.id) {
                    println!("  ← {} ({})", dep.name, rel.kind.display_label());
                }
            } else {
                println!("Dependencies:");
                let deps = graph.transitive_dependencies(&entity.id, depth);
                for d in 1..=depth {
                    if let Some(level_deps) = deps.get(&d) {
                        println!("  Level {}:", d);
                        for dep_id in level_deps {
                            if let Some(dep) = graph.get_entity(dep_id) {
                                println!("    → {}", dep.name);
                            } else {
                                println!("    → {} (external)", dep_id);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn run_find(pattern: &str, path: PathBuf, kind: Option<EntityKindArg>) -> Result<()> {
    let mut config = Config::for_path(&path);
    nao::settings::load(&path).apply_to_config(&mut config);

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
            || entity.qualified_name.to_lowercase().contains(&pattern_lower)
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
    nao::settings::load(&path).apply_to_config(&mut config);

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
    nao::settings::load(&path).apply_to_config(&mut config);

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
        println!("  Average connections per entity: {:.2}", metrics.average_degree);
        println!("  Circular dependencies: {}", metrics.cycle_count);
        println!();

        if !metrics.most_connected.is_empty() {
            println!("Most connected entities:");
            for (i, (id, count)) in metrics.most_connected.iter().take(5).enumerate() {
                let name = graph
                    .get_entity(id)
                    .map(|e| e.name.as_str())
                    .unwrap_or(id);
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
    use nao::diff::{
        compute_diff, verify_git_repo, resolve_git_ref,
        create_worktree, remove_worktree, analyze_at, render_base_details, write_diff_outputs,
    };

    let repo_root = path.canonicalize()?;
    verify_git_repo(&repo_root)?;

    let from_sha = resolve_git_ref(&repo_root, from_ref)?;
    let to_sha = resolve_git_ref(&repo_root, to_ref)?;
    eprintln!("🔀 Diffing {} → {}", from_sha, to_sha);

    // Create + analyze base worktree.
    let tmp = std::env::temp_dir();
    let base_dir = tmp.join(format!("nao-diff-base-{}", from_sha));
    let head_dir = tmp.join(format!("nao-diff-head-{}", to_sha));

    create_worktree(&repo_root, &base_dir, from_ref)?;
    let (base_graph, base_config) = analyze_at(&base_dir, include_tests, &languages, &format!("base ({})", from_sha))?;

    // Create + analyze head worktree.
    if let Err(e) = create_worktree(&repo_root, &head_dir, to_ref) {
        remove_worktree(&repo_root, &base_dir);
        return Err(e);
    }
    let (head_graph, head_config) = analyze_at(&head_dir, include_tests, &languages, &format!("head ({})", to_sha))?;

    // Compute diff.
    eprintln!("  Computing structural diff...");
    let diff = compute_diff(&base_graph, &head_graph, &base_dir, &head_dir, &from_sha, &to_sha);
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
