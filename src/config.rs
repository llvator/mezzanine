//! Configuration for the Nao.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::models::{EntityKind, RelationshipKind};
use crate::models::file_info::Language;
use crate::output::OutputFormat;

/// Main configuration for the Nao.
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
    /// (CLI, `nao watch`, `nao mcp`). `nao serve` sets it to `false`,
    /// because there the tree came from a URL a stranger pasted — see
    /// [`crate::server::serve`]. Gating here rather than on the env var
    /// alone means a `NAO_LSP_EXACT=1` inherited from the environment
    /// cannot re-enable the pass under serve.
    #[serde(default = "default_allow_unsafe_passes")]
    pub allow_unsafe_passes: bool,
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

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            languages: HashSet::new(), // All languages
            include_external: false,
            include_tests: false,
            include_stdlib: false,
            exclude_patterns: vec![
                "**/node_modules/**".to_string(),
                "**/target/**".to_string(),
                "**/.git/**".to_string(),
                "**/vendor/**".to_string(),
                "**/__pycache__/**".to_string(),
                "**/dist/**".to_string(),
                "**/build/**".to_string(),
            ],
            include_patterns: Vec::new(), // Include all by default
            allow_unsafe_passes: default_allow_unsafe_passes(),
        }
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            entity_kinds: HashSet::new(), // All kinds
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
