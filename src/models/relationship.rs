//! Relationship types between code entities.

use crate::models::Span;
use serde::{Deserialize, Serialize};

/// Represents a relationship between two code entities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Relationship {
    /// Unique identifier for this relationship
    pub id: String,

    /// Source entity ID
    pub source_id: String,

    /// Target entity ID
    pub target_id: String,

    /// Type of relationship
    pub kind: RelationshipKind,

    /// Optional label for the relationship
    pub label: Option<String>,

    /// Weight/strength (useful for visualization)
    pub weight: u32,

    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,

    /// Where the edge was *written* — the source-side statement that
    /// created it (AN-024). `None` on every edge nobody has taught to
    /// record its site yet.
    ///
    /// Deliberately not either endpoint's declaration span. A renderer
    /// reaching for `target.span` prints where the depended-on thing *is*,
    /// which is a different fact that coincides with this one often enough
    /// to read as attribution — an import at the foot of one file, of a
    /// symbol declared at the head of another, cites two unrelated lines.
    ///
    /// A field rather than a `metadata` key for the reason `precision` is
    /// one: a location a consumer has to re-parse out of a string map is a
    /// location most consumers will not print. `#[serde(default)]` plus
    /// `skip_serializing_if` so pre-AN-024 caches load and a spanless edge
    /// serialises byte-identically to what it did before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,

    /// Resolution precision of this edge (AN-004). `Some(Exact)` when a
    /// language server (rust-analyzer) resolved the call site to this
    /// definition; `Some(Heuristic)` when mezz's name-based resolver picked
    /// it; `None` where precision doesn't apply (structural edges, imports,
    /// …). `#[serde(default)]` so pre-AN-004 JSON loads cleanly as `None`.
    #[serde(default)]
    pub precision: Option<Precision>,
}

/// Where one file's dependency on another was written (AN-024).
///
/// The file-granularity companion to [`Relationship::span`], and the form
/// a folder drawing can use: that drawing collapses every entity edge
/// between two files into one arrow, so the arrow has no single site, but
/// the import statements behind it each have one.
///
/// Kept beside the graph rather than on it. An import specifier resolves
/// to a *file*, and a file is a node only when it is a Groovy script —
/// which is why the resolver's `Imports` edges cover that one case and why
/// nothing downstream could cite an import before this existed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportSite {
    /// The file the statement was written in.
    pub from: std::path::PathBuf,
    /// The file it names.
    pub to: std::path::PathBuf,
    /// 0-based line of the statement, as source files are indexed
    /// everywhere else in the model. Renderers add the 1.
    pub line: usize,
    /// The statement was a re-export (`export { X } from './y'`), so the
    /// file passes the name on without using it.
    pub is_reexport: bool,
    /// The build erases the statement — TypeScript's `import type`, Python's
    /// `if TYPE_CHECKING:` — so no bundler ever resolves the specifier
    /// (AN-022).
    ///
    /// Here as well as on the `Imports` edge's metadata because this is the
    /// granularity every language gets: a folder drawing reads sites, and
    /// the edge only exists where the importing file is a node.
    #[serde(default)]
    pub is_type_only: bool,
}

/// How a (call) edge's target was resolved. The load-bearing trust signal
/// AN-004 exists to deliver: agents can act on `Exact` or `Heuristic`, but
/// not on an unlabeled maybe — so every renderer that shows call edges shows
/// this. Kept a first-class field (not a stringly metadata key) precisely so
/// "label every call edge everywhere" is enforceable rather than a
/// convention each renderer must remember.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    /// A language server resolved the call site to this exact definition.
    Exact,
    /// mezz's name-based heuristic picked this target.
    Heuristic,
}

impl Precision {
    /// Short marker for tool output (`exact` / `heuristic`).
    pub fn marker(self) -> &'static str {
        match self {
            Precision::Exact => "exact",
            Precision::Heuristic => "heuristic",
        }
    }
}

/// Types of relationships between code entities
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    // Structural relationships
    /// Parent contains child (e.g., class contains method)
    Contains,
    /// ansible-deploy: a Playbook includes a Role (`roles:` /
    /// `include_role`). Structural but not ownership — a Role is shared
    /// across playbooks.
    Includes,
    /// Inheritance relationship
    Inherits,
    /// Interface/trait implementation
    Implements,

    // Dependency relationships
    /// Imports/uses another entity
    Imports,
    /// Direct function call
    Calls,
    /// Creates an instance of
    Instantiates,
    /// Uses as a type (parameter, return type, field)
    UsesType,
    /// Names a function without calling it at that site — a function
    /// passed as a value, most often a handler handed to a dispatcher
    /// (`add_leaf(node, ctx, enums::parse_enum)`).
    ///
    /// A dependency of the same weight as a call: the naming file breaks
    /// when the named function's signature changes. It is kept apart from
    /// `Calls` because no call happens here, so call ordering and call-site
    /// metadata would be fiction, and apart from `References` because that
    /// kind is deliberately excluded from the coupling tallies and this is
    /// coupling. Without it a dispatcher that reaches every sibling through
    /// a function pointer draws as depending on none of them (ADR 0021).
    UsesFn,
    /// Reads a name declared in another file without calling it — a
    /// constant, a shared lookup table, or any imported binding used as a
    /// value.
    ///
    /// A dependency of the same weight as `UsesFn`, and for the same
    /// reason ADR 0021 gives: change the constant's value or type and the
    /// file that reads it changes with it. It is neither `Calls` (no call
    /// happens, so call ordering and call-site metadata would be fiction)
    /// nor `References` (that kind is deliberately outside the coupling
    /// tallies, and this is coupling). Without it a module of shared
    /// constants has no edges at all and draws as a stray nothing depends
    /// on (ADR 0027).
    UsesValue,
    /// References without calling
    References,
    /// ansible-deploy: a DeploymentEntry renders a TemplateFile
    /// (`{path}/{template ?? name}.yml.j2`). The load-bearing edge that
    /// ties "what is declared as deployed" to "which file defines it".
    RendersFrom,
    /// A template interpolates a name it did not declare.
    ///
    /// ansible-deploy: a TemplateFile interpolates a `{{ variable }}`,
    /// linking it to the variables it depends on (the templating
    /// dimension), which resolve to their `group_vars`/`host_vars`
    /// definitions post-merge.
    ///
    /// Svelte: a Component interpolates an imported name inside `{…}`
    /// (SV-002). Same relation, same reason it counts as a dependency —
    /// the markup breaks if the target changes.
    Interpolates,

    // Data flow relationships
    /// Reads from
    ReadsFrom,
    /// Writes to
    WritesTo,
    /// Returns data of type
    Returns,
    /// Takes a parameter of type (type -> function)
    TakesParam,

    // High-level relationships
    /// Depends on (generic dependency)
    DependsOn,
    /// Communicates with (service-level)
    CommunicatesWith,
    /// Provides functionality
    Provides,
    /// Requires functionality
    Requires,

    // Association
    /// Generic association
    AssociatedWith,
    /// Composition (strong ownership)
    ComposedOf,
    /// Aggregation (weak ownership)
    Aggregates,
}

impl RelationshipKind {
    /// Returns true if this relationship implies a dependency
    pub fn is_dependency(&self) -> bool {
        matches!(
            self,
            RelationshipKind::Imports
                | RelationshipKind::Calls
                | RelationshipKind::Instantiates
                | RelationshipKind::UsesType
                | RelationshipKind::UsesFn
                | RelationshipKind::UsesValue
                | RelationshipKind::DependsOn
                | RelationshipKind::Requires
                | RelationshipKind::RendersFrom
                | RelationshipKind::Interpolates
        )
    }

    /// Returns true if this is a structural/containment relationship
    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            RelationshipKind::Contains
                | RelationshipKind::Inherits
                | RelationshipKind::Implements
                | RelationshipKind::ComposedOf
                | RelationshipKind::Aggregates
        )
    }

    /// Returns the generic display label for this relationship type.
    /// Prefer `display_label_for` when a language context is available.
    ///
    /// Split in two along the enum's own grouping — what one piece of code
    /// does to another, against the deployment and architecture edges below
    /// — because a single flat match over every kind cannot gain an arm
    /// without the complexity gate charging for it, and a kind without a
    /// label is worse than a long function. `every_kind_has_a_label` stands
    /// in for the exhaustiveness the split gives up.
    pub fn display_label(&self) -> &'static str {
        match self {
            RelationshipKind::Contains => "contains",
            RelationshipKind::Includes => "includes",
            RelationshipKind::Inherits => "inherits",
            RelationshipKind::Implements => "implements",
            RelationshipKind::Imports => "imports",
            RelationshipKind::Calls => "calls",
            RelationshipKind::Instantiates => "instantiates",
            RelationshipKind::UsesType => "uses type",
            RelationshipKind::References => "references",
            other => other.naming_label(),
        }
    }

    /// The kinds that name something without invoking it: a function handed
    /// to a dispatcher (ADR 0021), a constant read from another file
    /// (ADR 0027).
    ///
    /// A third step in the chain rather than two more arms above, for the
    /// reason [`Self::display_label`] gives: a flat match cannot gain an arm
    /// without the complexity gate charging for it.
    fn naming_label(&self) -> &'static str {
        match self {
            RelationshipKind::UsesFn => "uses fn",
            RelationshipKind::UsesValue => "uses value",
            other => other.wider_label(),
        }
    }

    /// The kinds that describe deployment topology, data flow and
    /// architecture rather than one symbol reaching another.
    fn wider_label(&self) -> &'static str {
        match self {
            RelationshipKind::RendersFrom => "renders from",
            RelationshipKind::Interpolates => "interpolates",
            RelationshipKind::ReadsFrom => "reads from",
            RelationshipKind::WritesTo => "writes to",
            RelationshipKind::Returns => "returns",
            RelationshipKind::TakesParam => "takes param",
            RelationshipKind::DependsOn => "depends on",
            RelationshipKind::CommunicatesWith => "communicates with",
            RelationshipKind::Provides => "provides",
            RelationshipKind::Requires => "requires",
            RelationshipKind::AssociatedWith => "associated with",
            RelationshipKind::ComposedOf => "composed of",
            RelationshipKind::Aggregates => "aggregates",
            // Only the kinds `display_label` already answered reach here.
            _ => "related to",
        }
    }

    /// Language-aware display label. Falls back to the generic label for
    /// kinds or languages that don't have a specific convention.
    pub fn display_label_for(&self, language: super::file_info::Language) -> &'static str {
        use super::file_info::Language;
        match (self, language) {
            // Contains → different languages use different terminology for
            // "a type has this member declared in its body".
            (RelationshipKind::Contains, Language::Rust) => "defines",
            (RelationshipKind::Contains, Language::Java) => "declares",
            (RelationshipKind::Contains, Language::Python) => "defines",
            (RelationshipKind::Contains, Language::Go) => "declares",
            (RelationshipKind::Contains, Language::TypeScript | Language::JavaScript) => "declares",
            (RelationshipKind::Contains, _) => "declares",

            // Inherits → matches keyword convention.
            (RelationshipKind::Inherits, Language::Java | Language::TypeScript) => "extends",
            (RelationshipKind::Inherits, Language::Python) => "inherits from",
            (RelationshipKind::Inherits, Language::Cpp | Language::CSharp) => "extends",
            (RelationshipKind::Inherits, _) => "inherits",

            // Implements → most languages say "implements"; Go says "satisfies".
            (RelationshipKind::Implements, Language::Go) => "satisfies",
            (RelationshipKind::Implements, _) => "implements",

            // Imports → Rust uses `use`, rest use `import`.
            (RelationshipKind::Imports, Language::Rust) => "uses",
            (RelationshipKind::Imports, _) => "imports",

            // Instantiates → Rust doesn't use `new` keyword.
            (RelationshipKind::Instantiates, Language::Rust) => "constructs",
            (RelationshipKind::Instantiates, Language::Go) => "creates",
            (RelationshipKind::Instantiates, _) => "instantiates",

            // Everything else: generic label.
            _ => self.display_label(),
        }
    }

    /// Passive / incoming form of the label. When the focus is on the
    /// **target** of the relationship (e.g., a method looking at its
    /// parent class), the active label ("declares") reads backward.
    /// This returns the inverted form ("declared by").
    pub fn incoming_label_for(&self, language: super::file_info::Language) -> &'static str {
        use super::file_info::Language;
        match (self, language) {
            (RelationshipKind::Contains, Language::Rust) => "defined in",
            (RelationshipKind::Contains, Language::Java) => "declared in",
            (RelationshipKind::Contains, Language::Python) => "defined in",
            (RelationshipKind::Contains, _) => "declared in",

            (RelationshipKind::Inherits, Language::Java | Language::TypeScript) => "extended by",
            (RelationshipKind::Inherits, Language::Python) => "inherited by",
            (RelationshipKind::Inherits, _) => "inherited by",

            (RelationshipKind::Implements, Language::Go) => "satisfied by",
            (RelationshipKind::Implements, _) => "implemented by",

            (RelationshipKind::Imports, Language::Rust) => "used by",
            (RelationshipKind::Imports, _) => "imported by",

            (RelationshipKind::Calls, _) => "called by",
            (RelationshipKind::Instantiates, _) => "instantiated by",
            (RelationshipKind::UsesType, _) => "type used by",
            (RelationshipKind::References, _) => "referenced by",
            (RelationshipKind::RendersFrom, _) => "rendered by",
            (RelationshipKind::Interpolates, _) => "interpolated by",
            (RelationshipKind::Includes, _) => "included by",
            (RelationshipKind::ReadsFrom, _) => "read by",
            (RelationshipKind::WritesTo, _) => "written by",
            (RelationshipKind::Returns, _) => "returned by",
            // Read as "<function> has param <param>" — describes the edge
            // from the function's active perspective so UI arrow-flipping
            // (which swaps source/target visually when the function is the
            // focused node) produces a correct reading.
            (RelationshipKind::TakesParam, _) => "has param",
            (RelationshipKind::DependsOn, _) => "depended on by",
            (RelationshipKind::Provides, _) => "provided by",
            (RelationshipKind::Requires, _) => "required by",

            _ => self.display_label(),
        }
    }

    /// Returns the arrow style for DOT/Graphviz output
    pub fn dot_arrow_style(&self) -> &'static str {
        match self {
            RelationshipKind::Inherits => "empty",
            RelationshipKind::Implements => "empty",
            RelationshipKind::Contains => "diamond",
            RelationshipKind::ComposedOf => "diamond",
            RelationshipKind::Aggregates => "odiamond",
            _ => "normal",
        }
    }

    /// Returns the line style for DOT/Graphviz output
    pub fn dot_line_style(&self) -> &'static str {
        match self {
            RelationshipKind::Implements => "dashed",
            RelationshipKind::References => "dotted",
            RelationshipKind::CommunicatesWith => "bold",
            _ => "solid",
        }
    }
}

impl Relationship {
    /// Create a new relationship
    pub fn new(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        kind: RelationshipKind,
    ) -> Self {
        let source = source_id.into();
        let target = target_id.into();
        let id = format!("{}->{}:{:?}", &source, &target, kind);

        Self {
            id,
            source_id: source,
            target_id: target,
            kind,
            label: None,
            weight: 1,
            metadata: std::collections::HashMap::new(),
            span: None,
            precision: None,
        }
    }

    /// Builder: set label
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Builder: set where the edge was written (AN-024).
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Builder: set resolution precision (AN-004).
    pub fn with_precision(mut self, precision: Precision) -> Self {
        self.precision = Some(precision);
        self
    }

    /// Builder: set weight
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    /// Builder: add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::RelationshipKind::{self, *};

    /// `display_label` used to be one exhaustive match, so a new kind could
    /// not be added without the compiler demanding a label. Splitting it to
    /// keep the complexity gate happy gave that up; this takes it back.
    /// A kind that reaches the fallback has been added and not named.
    #[test]
    fn every_kind_has_a_label() {
        const ALL: &[RelationshipKind] = &[
            Contains,
            Includes,
            Inherits,
            Implements,
            Imports,
            Calls,
            Instantiates,
            UsesType,
            UsesFn,
            UsesValue,
            References,
            RendersFrom,
            Interpolates,
            ReadsFrom,
            WritesTo,
            Returns,
            TakesParam,
            DependsOn,
            CommunicatesWith,
            Provides,
            Requires,
            AssociatedWith,
            ComposedOf,
            Aggregates,
        ];
        for kind in ALL {
            assert_ne!(
                kind.display_label(),
                "related to",
                "{kind:?} reached the fallback — give it a label"
            );
        }
    }

    /// The new kind is coupling, and the folder drawings are built from
    /// whatever `is_dependency` admits. Recording it and leaving it out of
    /// that set would have changed nothing a reader can see.
    #[test]
    fn naming_a_function_counts_as_a_dependency() {
        assert!(UsesFn.is_dependency());
        assert!(!References.is_dependency());
    }

    /// Reading a constant another file declares is the same coupling as
    /// naming a function it declares (AN-028), so it joins the same set.
    #[test]
    fn reading_an_imported_value_counts_as_a_dependency() {
        assert!(UsesValue.is_dependency());
    }
}
