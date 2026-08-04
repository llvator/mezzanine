//! Standalone CLI for the Elevator (`.elv`) spec language.
//!
//! Reads every `.elv` file under a directory, resolves imports, and
//! emits a low-token text artifact suitable for LLM consumption (or
//! for piping into a docs page, prompt, etc.). Same engine as
//! `nao analyze -l elevator -f elevator-text` — this binary just
//! exposes the relevant slice with a focused command line.
//!
//! ```text
//! elevator ./my-spec                   # full spec to stdout
//! elevator ./my-spec --root library    # subtree rooted at `library`
//! elevator ./my-spec -o spec.txt       # write to a file
//! ```

use clap::Parser;
use nao::{
    analyzer::Analyzer,
    config::Config,
    graph::DependencyGraph,
    models::file_info::Language,
    output::{
        self, elevator_check, elevator_code_map, elevator_extract, elevator_list, OutputFormat,
    },
};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "elevator",
    version,
    about = "Render Elevator (.elv) specs to a low-token text artifact.",
    long_about = "Walks a directory for `.elv` files, resolves their imports, and \
emits a compact text rendering of the spec — categories, features, functionalities, \
concepts, and the cross-references between them. Designed as a project map for \
human onboarding and LLM consumption.",
    after_help = "TYPICAL WORKFLOW (sketch first, deepen where you work):\n  \
elevator . --stats            what exists; new areas start as one-line sketches\n  \
elevator . --focus <entity>   context bundle before working on that area\n  \
<after code changes>          deepen the touched branch: fu verbs + cr: pointers\n  \
elevator . --check            errors = fix now; hints = a to-deepen list\n  \
elevator . --code-map         duplicates and per-path coverage\n  \
elevator . --drift            staleness radar: cr paths + identifier anchors vs the code\n  \
elevator . --extract <entity> -o work/slice.elv   snapshot the branch you touched\n\n\
Full language guide: elevator --docs"
)]
struct Cli {
    /// Path to walk for `.elv` files (defaults to the current directory).
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Restrict rendering to the given entity and its descendants.
    /// Accepts a bare name (`library`), a kind-prefixed qualname
    /// (`c.library`, `f.protocol`), or a full ID
    /// (`elevator::c.library`).
    #[arg(long)]
    root: Option<String>,

    /// Focus on a specific entity: emit its ancestor chain, siblings,
    /// own subtree, and any cross-cutting concepts touching the path.
    /// The right artifact when feeding an LLM context to work *on*
    /// this node (e.g. implementing a Functionality). Takes
    /// precedence over `--root`.
    #[arg(long)]
    focus: Option<String>,

    /// Write to this file instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Suppress the analyzer's progress lines on stderr.
    #[arg(short, long)]
    quiet: bool,

    /// Skip the format-key preamble. Default keeps it — it's the bit
    /// that lets a fresh-context LLM interpret the markers (`c`,
    /// `f`, `fu`, `@`, `[where:]`, etc.). Use this when piping into
    /// a tool that already knows the format.
    #[arg(long)]
    no_legend: bool,

    /// Run the spec checker instead of rendering an artifact.
    /// Reports parse / lex errors, unresolved references, orphan
    /// Features, empty Categories, and unused Concepts. Exits 1 if
    /// any errors were found, 0 otherwise (hints don't fail).
    #[arg(long)]
    check: bool,

    /// Print an inverted index of code references: each unique path
    /// referenced via `cr:` / `cr.<tag>:`, listed with the entities
    /// that point at it. Paths referenced by 2+ entities with the
    /// same `cr` kind are marked `★ DUPLICATE` — the strong signal
    /// that two differently-named spec entities point at the same
    /// physical code, which usually means the spec has a duplicate.
    #[arg(long)]
    code_map: bool,

    /// Check the spec against the code it claims. Verifies that every
    /// `cr:` path resolves under the code root (missing = error, exit
    /// 1) and that identifier-shaped names mentioned in `d:`
    /// descriptions (classes, methods, config keys, job codes) still
    /// appear in the claimed files (unanchored = hint, exit 0 — the
    /// rename/delete drift signal). Use `--code-root` when the spec
    /// lives outside the code repository.
    #[arg(long)]
    drift: bool,

    /// Root the `cr:` paths resolve against for `--drift`. Defaults
    /// to the spec path itself (specs co-located with code). Point it
    /// at the code repository when specs live in a separate docs repo.
    #[arg(long, value_name = "DIR")]
    code_root: Option<PathBuf>,

    /// Extract the named entities (and their descendants) into a
    /// standalone `.elv` file — a snapshot of one slice of the spec.
    ///
    /// Accepts the same selectors as `--root` / `--focus`, comma- or
    /// space-separated, repeatable. The slice carries each selection's
    /// descendants, the Concepts it uses, and its ancestors with
    /// child lists pruned to the slice. Reference targets outside the
    /// slice appear as name-only definitions so the edges still
    /// resolve.
    ///
    /// The intended use is agent observability: after deepening the
    /// shared spec, extract the branch you touched into your own
    /// folder and keep it as history.
    ///
    /// Examples:
    ///   `elevator . --extract f.spec_health -o work/slice.elv`
    ///   `elevator . --extract fu.f.spec_health.drift,f.text_artifacts`
    #[arg(long, value_name = "ENTITY", num_args = 1.., value_delimiter = ',')]
    extract: Vec<String>,

    /// Enumerate every Elevator entity grouped by kind. Sorted by
    /// qualified name so consecutive runs produce stable diffs.
    ///
    /// With an optional kind filter, restricts the output to a
    /// single entity kind. Accepted values: `all` (default),
    /// `c` / `category` / `categories`, `f` / `feature` / `features`,
    /// `fu` / `functionality` / `functionalities`, `concept` /
    /// `concepts`, `ui` / `ui-page` / `pages`.
    ///
    /// Examples:
    ///   `elevator . --list`             — every entity
    ///   `elevator . --list features`    — Features only
    ///   `elevator . --list fu`          — Functionalities only
    #[arg(
        long,
        value_name = "KIND",
        num_args = 0..=1,
        default_missing_value = "all"
    )]
    list: Option<String>,

    /// When combined with `--list`, group entities under their full
    /// parent chain instead of a flat alphabetic list. Features
    /// appear under their Category; Functionalities under
    /// `Category / Feature`. Entities with no parent (Categories
    /// themselves, Concepts, UI Pages, orphan Features) render
    /// unchanged. Useful for answering "what's *under* each
    /// Category?" without piping through `grep`/`awk`.
    #[arg(long)]
    grouped: bool,

    /// Compact count table per entity kind, with totals and
    /// unresolved/relationship signals. One-glance project size +
    /// health summary; suitable for CI status checks.
    #[arg(long)]
    stats: bool,

    /// Print the Elevator language reference (syntax, semantics,
    /// examples) and exit. Useful as a quick lookup for humans, and
    /// as context to feed an LLM that's going to write `.elv` files
    /// for an existing codebase. The text is embedded in the binary
    /// at build time so it always matches your installed version.
    #[arg(long)]
    docs: bool,
}

/// The full language guide, baked into the binary so `elevator
/// --docs` is always self-contained — no separate docs install, no
/// network fetch, no version skew.
const LANGUAGE_GUIDE: &str = include_str!("../../guide/elevator-language.md");

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.docs {
        print!("{}", LANGUAGE_GUIDE);
        return ExitCode::SUCCESS;
    }

    let mut config = Config::for_path(&cli.path).with_output_format(OutputFormat::ElevatorText);
    // Restrict the analyzer to Elevator files. Walking with no
    // language filter would pick up every source file in the
    // directory and waste time parsing them with no useful effect.
    config.analysis.languages.insert(Language::Elevator);
    config.filters.root_entity = cli.root.clone();
    config.filters.focus_entity = cli.focus.clone();
    config.filters.suppress_legend = cli.no_legend;

    let mut analyzer = Analyzer::new(config.clone());
    let result = match analyzer.analyze() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: analysis failed: {}", e);
            return ExitCode::from(2);
        }
    };

    if cli.check {
        return run_check(&result);
    }

    if cli.code_map {
        print!("{}", elevator_code_map::render(&result));
        return ExitCode::SUCCESS;
    }

    if cli.drift {
        let code_root = cli.code_root.clone().unwrap_or_else(|| cli.path.clone());
        let report = output::elevator_drift::run(&result, &code_root);
        print!("{}", report.text);
        return if report.has_errors {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        };
    }

    if !cli.extract.is_empty() {
        return run_extract(&result, &cli);
    }

    if let Some(kind_filter) = cli.list.as_deref() {
        match elevator_list::render_list(&result, kind_filter, cli.grouped) {
            Ok(s) => {
                print!("{}", s);
                return ExitCode::SUCCESS;
            }
            Err(msg) => {
                eprintln!("error: {}", msg);
                return ExitCode::from(2);
            }
        }
    }

    if cli.stats {
        print!("{}", elevator_list::render_stats(&result));
        return ExitCode::SUCCESS;
    }

    let graph = DependencyGraph::from_analysis(&result);
    let rendered = match output::render(&graph, &config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: render failed: {}", e);
            return ExitCode::from(2);
        }
    };

    match cli.output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &rendered) {
                eprintln!("error: write failed: {}", e);
                return ExitCode::from(2);
            }
            if !cli.quiet {
                eprintln!("Wrote {} ({} bytes)", path.display(), rendered.len());
            }
        }
        None => {
            print!("{}", rendered);
        }
    }

    if !result.warnings.is_empty() && !cli.quiet {
        eprintln!("\n{} warning(s):", result.warnings.len());
        for w in &result.warnings {
            eprintln!("  - {}", w);
        }
    }

    ExitCode::SUCCESS
}

/// Render the requested slice and write it out, reporting its shape
/// on stderr so a caller piping the slice to stdout still sees what
/// it got.
fn run_extract(result: &nao::analyzer::AnalysisResult, cli: &Cli) -> ExitCode {
    let source = cli.path.display().to_string();
    let slice = match elevator_extract::extract(result, &cli.extract, &source) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {}", msg);
            return ExitCode::from(2);
        }
    };
    if let Err(e) = write_out(&slice.text, cli.output.as_deref()) {
        eprintln!("error: {}", e);
        return ExitCode::from(2);
    }
    if cli.quiet {
        return ExitCode::SUCCESS;
    }

    let mut summary = format!(
        "Extracted {} entity(ies), {} ancestor(s), {} name-only",
        slice.members, slice.ancestors, slice.name_only
    );
    if slice.dropped_unresolved > 0 {
        let _ = write!(
            summary,
            ", {} unresolved edge(s) dropped",
            slice.dropped_unresolved
        );
    }
    match cli.output.as_deref() {
        Some(p) => eprintln!("{} → {}", summary, p.display()),
        None => eprintln!("{}", summary),
    }
    ExitCode::SUCCESS
}

/// Write `text` to `path`, or to stdout when there's no path.
///
/// Creates missing parent directories: `--extract` exists to drop a
/// slice into a folder that is usually per-task and therefore new,
/// and failing on "no such directory" would make every caller run
/// `mkdir -p` first.
fn write_out(text: &str, path: Option<&Path>) -> Result<(), String> {
    let Some(path) = path else {
        print!("{}", text);
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {}", parent.display(), e))?;
        }
    }
    std::fs::write(path, text).map_err(|e| format!("write failed: {}", e))
}

fn run_check(result: &nao::analyzer::AnalysisResult) -> ExitCode {
    let findings = elevator_check::check(result);
    if findings.is_empty() {
        println!("✓ No issues.");
        return ExitCode::SUCCESS;
    }
    let mut errors = 0usize;
    let mut hints = 0usize;
    for f in &findings {
        let label = match f.severity {
            elevator_check::Severity::Error => {
                errors += 1;
                "error"
            }
            elevator_check::Severity::Hint => {
                hints += 1;
                "hint"
            }
        };
        println!("{}: {}", label, f.message);
    }
    println!(
        "\n{} error(s), {} hint(s).",
        errors, hints
    );
    if errors > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
