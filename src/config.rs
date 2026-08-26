//! Configuration for the Mezzanine.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::models::file_info::Language;
use crate::models::{EntityKind, RelationshipKind};
use crate::output::OutputFormat;

/// Main configuration for the Mezzanine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Root directory to analyze
    pub root_path: PathBuf,

    /// Output format
    pub output_format: OutputFormat,

    /// Output file path (None for stdout)
    pub output_path: Option<PathBuf>,

    /// Analysis settings
    pub analysis: AnalysisConfig,

    /// Filter settings
    pub filters: FilterConfig,

    /// Display settings
    pub display: DisplayConfig,
}

/// Analysis-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    /// Maximum depth for dependency traversal (0 = unlimited)
    pub max_depth: usize,

    /// Languages to analyze (empty = all supported)
    pub languages: HashSet<Language>,

    /// Whether to follow external dependencies
    pub include_external: bool,

    /// Whether to analyze test files
    pub include_tests: bool,

    /// Whether to analyze documentation (Markdown) alongside the code.
    ///
    /// The widening switch for opt-in languages ([`Language::is_opt_in`]).
    /// `languages` cannot express this: it is a *restricting* filter, so
    /// `-l markdown` yields a docs-only graph rather than adding docs to the
    /// normal one, and asking for both would mean naming every other language
    /// by hand. Same shape and same reason as `include_tests`, which widens
    /// the file set rather than narrowing it.
    pub include_docs: bool,

    /// Whether local and module-level assignments become entities.
    ///
    /// Off by default, because on any real repo they are most of the graph
    /// and no consumer lists them. Measured on tinygrad (449 files): 162,550
    /// of 225,606 real entities are `Variable` or `Constant` — 72% — and they
    /// carry 104k of the 228k relationships. Meanwhile `mcp::tools::is_listed`
    /// hides `Variable` from every agent-facing answer, and the canvas draws
    /// at most `DRAW_CEILING` (2,000) nodes. The default analysis was paying
    /// to build, serialize and ship a population nothing displays (CFG-005).
    ///
    /// The same widening-switch shape as `include_docs`, and for the same
    /// reason: `filters.entity_kinds` is an *allow-list*, so "everything
    /// except variables" would mean naming all twenty kinds by hand.
    ///
    /// What "off" does not touch is in [`crate::analyzer::Analyzer`]'s
    /// filter: class fields survive, and so does any assignment doing
    /// structural work. See `apply_filters`.
    ///
    /// `serde(default)` so a config serialized before this field existed
    /// still loads — and loads with the new default, which is the point.
    #[serde(default)]
    pub include_locals: bool,

    /// Whether to include standard library references
    pub include_stdlib: bool,

    /// File patterns to exclude (glob patterns)
    pub exclude_patterns: Vec<String>,

    /// File patterns to include (glob patterns)
    pub include_patterns: Vec<String>,

    /// Whether analysis passes that *execute code from the analyzed tree*
    /// may run. Today that's only AN-004's rust-analyzer tracer, which
    /// drives `cargo check` and therefore runs `build.rs` and proc-macros
    /// from the target repo.
    ///
    /// `true` everywhere the operator chose the path they're analyzing
    /// (CLI, `mezz watch`, `mezz mcp`). `mezz serve` sets it to `false`,
    /// because there the tree came from a URL a stranger pasted — see
    /// [`crate::server::serve`]. Gating here rather than on the env var
    /// alone means a `MEZZ_LSP_EXACT=1` inherited from the environment
    /// cannot re-enable the pass under serve.
    #[serde(default = "default_allow_unsafe_passes")]
    pub allow_unsafe_passes: bool,

    /// Where the Elevator (`.elv`) spec lives, when it isn't simply
    /// "wherever it happens to be under the root".
    ///
    /// `None` — the default and the behaviour mezz has always had — means
    /// every `.elv` file the walk finds is part of the spec. That is right
    /// for a repo whose only `.elv` files *are* its spec, and wrong for two
    /// layouts that turn up often enough to need saying:
    ///
    /// - The specs live outside the analyzed tree — a docs repo beside the
    ///   code, or a monorepo where you watch one service and the domain
    ///   model sits at the top. Those files are never walked, so the spec
    ///   layer is simply empty.
    /// - The specs live inside the tree, but so do other `.elv` files —
    ///   fixtures, examples, a tutorial. Those are entities in the spec
    ///   graph that no one meant to publish.
    ///
    /// Set, it answers both at once with one rule: **a `.elv` file is part
    /// of the spec if and only if it lives here.** Files elsewhere under
    /// the root stop counting, and a directory outside the root gets walked
    /// for `.elv` files that would otherwise never be seen.
    ///
    /// Relative paths resolve against [`Config::root_path`], so a repo file
    /// can say `"spec_dir": "docs/domain"` without knowing where it was
    /// cloned. See [`Config::spec_root`].
    #[serde(default)]
    pub spec_dir: Option<PathBuf>,
}

fn default_allow_unsafe_passes() -> bool {
    true
}

/// Filtering configuration for what to include in output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    /// Entity types to include (empty = all)
    pub entity_kinds: HashSet<EntityKind>,

    /// Relationship types to include (empty = all)
    pub relationship_kinds: HashSet<RelationshipKind>,

    /// Minimum relationship weight to include
    pub min_weight: u32,

    /// Only show entities with external dependencies
    pub only_with_dependencies: bool,

    /// Entity name patterns to include (regex)
    pub name_patterns: Vec<String>,

    /// Entity name patterns to exclude (regex)
    pub exclude_name_patterns: Vec<String>,

    /// Optional entity-level subtree root: when set, renderers that
    /// understand it (currently `elevator-text`) emit only the named
    /// entity and its descendants. Accepts a qualified name (e.g.
    /// `library`, `f.protocol`, `c.library`) or a full ID
    /// (`elevator::c.library`). `None` means render everything.
    #[serde(default)]
    pub root_entity: Option<String>,

    /// Optional focus entity: render the target's *context* — its
    /// ancestor chain up to a top-level Category, the siblings at the
    /// target's level, the target's own subtree, and any Concepts
    /// touching that path. The artifact a coding-LLM session needs
    /// when working *on* a specific entity. Takes precedence over
    /// `root_entity`.
    #[serde(default)]
    pub focus_entity: Option<String>,

    /// When true, the `elevator-text` renderer suppresses its
    /// format-key preamble. Default `false` — the legend is helpful
    /// for fresh-context LLM sessions and the cost (~70 tokens) is
    /// small. Flip on for downstream pipelines that already know the
    /// format and want minimum noise.
    #[serde(default)]
    pub suppress_legend: bool,
}

/// Display/visualization configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// Show full qualified names
    pub show_qualified_names: bool,

    /// Show parameter details
    pub show_parameters: bool,

    /// Show return types
    pub show_return_types: bool,

    /// Show documentation
    pub show_documentation: bool,

    /// Show file paths
    pub show_file_paths: bool,

    /// Show line numbers
    pub show_line_numbers: bool,

    /// Group by file/module
    pub group_by_file: bool,

    /// Cluster related entities
    pub cluster_related: bool,

    /// Maximum entities to display (0 = unlimited)
    pub max_entities: usize,

    /// Layout direction for graph (TB, LR, BT, RL)
    pub layout_direction: LayoutDirection,

    /// Color scheme
    pub color_scheme: ColorScheme,
}

/// Graph layout direction
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum LayoutDirection {
    /// Top to bottom
    #[default]
    TopToBottom,
    /// Left to right
    LeftToRight,
    /// Bottom to top
    BottomToTop,
    /// Right to left
    RightToLeft,
}

impl LayoutDirection {
    pub fn as_dot_rankdir(&self) -> &'static str {
        match self {
            LayoutDirection::TopToBottom => "TB",
            LayoutDirection::LeftToRight => "LR",
            LayoutDirection::BottomToTop => "BT",
            LayoutDirection::RightToLeft => "RL",
        }
    }
}

/// Color scheme for visualization
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum ColorScheme {
    #[default]
    Default,
    Dark,
    Light,
    Colorful,
    Monochrome,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root_path: PathBuf::from("."),
            output_format: OutputFormat::default(),
            output_path: None,
            analysis: AnalysisConfig::default(),
            filters: FilterConfig::default(),
            display: DisplayConfig::default(),
        }
    }
}

impl AnalysisConfig {
    /// Whether files of this language should be analyzed at all.
    ///
    /// **The single answer to that question.** The file walker and the
    /// analyzer's parse loop both ask it, and they used to ask it separately:
    /// the walker discovered a file, counted it, and the parse loop then
    /// dropped it against a filter written one line differently. A file that
    /// is found and then silently discarded is the worst of both — the work
    /// is done and the result is missing — so the rule lives here and both
    /// callers read it. Same reasoning as `parser::detect_language` being the
    /// one place a path becomes a `Language`.
    ///
    /// Three cases:
    ///
    /// - **Opt-in languages** ([`Language::is_opt_in`] — Markdown today) are
    ///   out unless asked for, by `include_docs` or by name in `languages`.
    /// - **`include_docs` widens.** A language it enables is exempt from the
    ///   `languages` filter, so a repo pinning
    ///   `"language": ["rust", …]` in its settings cannot silently overrule an
    ///   explicit `--include-docs`.
    /// - **`languages` restricts**, as it always has. Empty means all.
    pub fn accepts_language(&self, language: Language) -> bool {
        let named = self.languages.contains(&language);

        if language.is_opt_in() {
            return self.include_docs || named;
        }
        self.languages.is_empty() || named
    }
}

/// The ignore globs every analysis starts with, before any settings file
/// adds to them (they extend rather than replace — ADR-0008).
///
/// Named rather than written inline so the walker can tell a shipped default
/// from a pattern somebody wrote. That distinction is the whole of CFG-015's
/// exemption: `**/vendor/**` matches nothing in most repos *by design*, and a
/// zero-match warning about it on every run in every repo would train the
/// reader to skip past the line that matters.
pub const DEFAULT_EXCLUDE_PATTERNS: &[&str] = &[
    "**/node_modules/**",
    "**/target/**",
    "**/.git/**",
    "**/vendor/**",
    "**/__pycache__/**",
    "**/dist/**",
    "**/build/**",
];

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            languages: HashSet::new(), // All languages
            include_external: false,
            include_tests: false,
            include_docs: false,
            include_locals: false,
            include_stdlib: false,
            exclude_patterns: DEFAULT_EXCLUDE_PATTERNS
                .iter()
                .map(|p| (*p).to_string())
                .collect(),
            include_patterns: Vec::new(), // Include all by default
            allow_unsafe_passes: default_allow_unsafe_passes(),
            spec_dir: None, // Every .elv under the root is the spec
        }
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            entity_kinds: HashSet::new(),       // All kinds
            relationship_kinds: HashSet::new(), // All kinds
            min_weight: 0,
            only_with_dependencies: false,
            name_patterns: Vec::new(),
            exclude_name_patterns: Vec::new(),
            root_entity: None,
            focus_entity: None,
            suppress_legend: false,
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            show_qualified_names: false,
            show_parameters: true,
            show_return_types: true,
            show_documentation: false,
            show_file_paths: true,
            show_line_numbers: true,
            group_by_file: true,
            cluster_related: true,
            max_entities: 0,
            layout_direction: LayoutDirection::default(),
            color_scheme: ColorScheme::default(),
        }
    }
}

impl Config {
    /// Create a config for a specific path
    pub fn for_path(path: impl Into<PathBuf>) -> Self {
        Self {
            root_path: path.into(),
            ..Default::default()
        }
    }

    /// [`AnalysisConfig::spec_dir`] resolved into the same path space as the
    /// walk: relative to [`Self::root_path`] unless it is already absolute.
    ///
    /// Deliberately lexical — no `canonicalize`, no existence check. The
    /// walk starts at `root_path` and yields paths that begin with it
    /// verbatim, so `mezz analyze .` produces `./spec/mezz.elv` and this
    /// produces `./spec`. Canonicalizing one side and not the other would
    /// compare `/abs/repo/spec` against `./spec/mezz.elv`, match nothing, and
    /// leave the spec layer silently empty — which is the one failure mode
    /// worth engineering against here, because it looks exactly like a repo
    /// that has no spec.
    pub fn spec_root(&self) -> Option<PathBuf> {
        let dir = self.analysis.spec_dir.as_ref()?;
        Some(if dir.is_absolute() {
            dir.clone()
        } else {
            self.root_path.join(dir)
        })
    }

    /// Whether the spec directory needs a walk of its own — true when it
    /// lies outside the analyzed root, which is the case `spec_dir` mainly
    /// exists for (specs in a sibling docs repo, or above a watched
    /// subdirectory of a monorepo).
    pub fn spec_is_outside_root(&self) -> bool {
        self.spec_root()
            .is_some_and(|spec| !spec.starts_with(&self.root_path))
    }

    /// Builder: set output format
    pub fn with_output_format(mut self, format: OutputFormat) -> Self {
        self.output_format = format;
        self
    }

    /// Builder: set max depth
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.analysis.max_depth = depth;
        self
    }

    /// Builder: add language filter
    pub fn with_language(mut self, lang: Language) -> Self {
        self.analysis.languages.insert(lang);
        self
    }

    /// Builder: add entity kind filter
    pub fn filter_entity_kind(mut self, kind: EntityKind) -> Self {
        self.filters.entity_kinds.insert(kind);
        self
    }

    /// Builder: add relationship kind filter
    pub fn filter_relationship_kind(mut self, kind: RelationshipKind) -> Self {
        self.filters.relationship_kinds.insert(kind);
        self
    }

    /// Builder: set layout direction
    pub fn with_layout_direction(mut self, direction: LayoutDirection) -> Self {
        self.display.layout_direction = direction;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three ways `.md` can be asked for, or not.
    ///
    /// These exist because the rule was written twice — once in the file
    /// walker, once in the analyzer's parse loop — and the copies disagreed
    /// as soon as `include_docs` was added: the walker admitted a Markdown
    /// file and the parse loop dropped it, so `--include-docs` reported the
    /// same entity count as a run without it. Both callers now ask
    /// `accepts_language`, and these pin what it answers.
    fn config_with(languages: &[Language], include_docs: bool) -> AnalysisConfig {
        AnalysisConfig {
            languages: languages.iter().copied().collect(),
            include_docs,
            ..Default::default()
        }
    }

    #[test]
    fn markdown_stays_out_of_a_default_analysis() {
        assert!(!config_with(&[], false).accepts_language(Language::Markdown));
    }

    #[test]
    fn include_docs_lets_markdown_in() {
        assert!(config_with(&[], true).accepts_language(Language::Markdown));
    }

    #[test]
    fn naming_markdown_lets_it_in_without_the_flag() {
        assert!(config_with(&[Language::Markdown], false).accepts_language(Language::Markdown));
    }

    #[test]
    fn naming_markdown_alone_excludes_the_code() {
        let config = config_with(&[Language::Markdown], false);
        assert!(!config.accepts_language(Language::Rust));
    }

    /// The bug this whole flag exists to fix. A repo that pins its languages
    /// in `.mezz/settings.json` — as mezz's own checkout does — must not
    /// silently overrule someone typing `--include-docs`.
    #[test]
    fn a_pinned_language_list_does_not_overrule_include_docs() {
        let config = config_with(&[Language::Rust, Language::TypeScript], true);
        assert!(config.accepts_language(Language::Markdown));
        assert!(config.accepts_language(Language::Rust));
        assert!(!config.accepts_language(Language::Python));
    }

    #[test]
    fn include_docs_does_not_widen_anything_but_the_opt_in_languages() {
        // It is not a general "ignore the filter" switch.
        let config = config_with(&[Language::Rust], true);
        assert!(!config.accepts_language(Language::Python));
    }

    #[test]
    fn an_empty_language_list_still_means_every_normal_language() {
        let config = config_with(&[], false);
        assert!(config.accepts_language(Language::Rust));
        assert!(config.accepts_language(Language::Python));
    }

    #[test]
    fn include_docs_leaves_the_default_all_languages_behaviour_alone() {
        let config = config_with(&[], true);
        assert!(config.accepts_language(Language::Rust));
        assert!(config.accepts_language(Language::Markdown));
    }
}
