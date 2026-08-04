//! Code entity representations (classes, functions, interfaces, etc.)

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Represents a code entity such as a class, function, interface, or module.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeEntity {
    /// Unique identifier for this entity
    pub id: String,
    
    /// Human-readable name
    pub name: String,
    
    /// Fully qualified name (e.g., `module::submodule::ClassName`)
    pub qualified_name: String,
    
    /// Type of entity
    pub kind: EntityKind,
    
    /// Visibility/accessibility
    pub visibility: Visibility,
    
    /// File where this entity is defined
    pub file_path: PathBuf,
    
    /// Location within the file
    pub span: super::Span,
    
    /// Parent entity (e.g., class containing a method)
    pub parent_id: Option<String>,
    
    /// Documentation/comments
    pub documentation: Option<String>,
    
    /// Language-specific attributes
    pub attributes: Vec<String>,
    
    /// Generic parameters (for classes/functions)
    pub generics: Vec<String>,
    
    /// For functions: parameter types
    pub parameters: Vec<Parameter>,
    
    /// For functions: return type
    pub return_type: Option<String>,
    
    /// Implemented interfaces/traits
    pub implements: Vec<String>,
    
    /// Extended/inherited types. For most languages this has at most one entry
    /// (single-inheritance: `class X extends Y`). For Java interfaces it can
    /// hold multiple (`interface A extends B, C`).
    pub extends: Vec<String>,
    
    /// Tags for filtering and categorization. Ordered set so serialized
    /// output is byte-stable across runs (AN-002).
    pub tags: BTreeSet<String>,

    /// Source code text of the entity
    pub source_code: Option<String>,

    /// Struct fields, enum variants, or other member declarations
    pub fields: Vec<Parameter>,

    /// Associated impl block source code (Rust-specific)
    pub impl_blocks: Vec<String>,

    /// Code-quality metrics (complexity, coupling, size). Baked in at
    /// analysis time so the UI never recomputes them per render.
    pub metrics: EntityMetrics,
}

/// Per-entity code quality signals. Populated during parsing for the
/// entity-local bits (complexity, LOC, param count) and during graph
/// construction for coupling bits (fan-in, fan-out, cycle membership).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EntityMetrics {
    /// Cyclomatic complexity: branches + 1. Only populated for callables.
    pub cyclomatic: Option<u32>,
    /// Cognitive complexity: like cyclomatic but weights each branch by its
    /// current nesting depth, so deeply nested logic scores higher than flat
    /// guard clauses. Only populated for callables.
    pub cognitive_complexity: Option<u32>,
    /// Maximum control-flow nesting depth. Only populated for callables.
    pub max_nesting: Option<u32>,
    /// Lines of code spanned by the entity (inclusive).
    pub loc: u32,
    /// Parameter count (callables only; excludes `self`).
    pub param_count: Option<u32>,
    /// Number of distinct entities depending on this one (incoming edges).
    pub fan_in: u32,
    /// Number of distinct entities this one depends on (outgoing edges).
    pub fan_out: u32,
    /// True if the entity is part of a dependency cycle (SCC size > 1).
    pub in_cycle: bool,
    /// Number of fields (structs) or variants (enums). None for entities
    /// that don't have fields (functions, modules, etc.).
    pub field_count: Option<u32>,
    /// Number of methods/functions contained in this entity (via Contains
    /// edges). Populated for container kinds (struct/enum/trait/module).
    pub method_count: u32,
    /// Fraction [0, 1] of fields that are publicly exposed. Only populated
    /// for structs where every field's visibility was captured. High values
    /// on a struct that also exposes many methods suggest weak encapsulation.
    pub public_field_ratio: Option<f32>,
    /// Composite "refactor pressure" score in roughly [0, 2+]. Weighted sum
    /// of normalised metrics + cycle bonus. Computed after all per-entity
    /// metrics are finalised (fan-in/out, cycles). The UI consumes this
    /// directly for per-entity ranking and scope-level aggregation.
    #[serde(default)]
    pub composite_score: f32,
    /// Instability index: `fan_out / (fan_in + fan_out)`. In [0, 1]:
    /// 0 = maximally stable (only depended on), 1 = maximally unstable.
    /// `None` when both fan_in and fan_out are 0 (undefined).
    pub instability: Option<f32>,
    /// Number of distinct type components in the return type. Populated
    /// for callables that return tuple types (e.g., returning a 5-tuple
    /// scores 5). `None` when not applicable.
    pub return_complexity: Option<u32>,
    /// Weighted Methods per Class: sum of cyclomatic complexities of the
    /// entity's directly-contained callables. Populated for containers
    /// (struct / class / trait / enum / module). Captures "how much
    /// complexity lives in this one container" better than method_count
    /// alone — 5 simple methods is very different from 5 gnarly ones.
    pub wmc: Option<u32>,
    /// Longest outbound call-chain depth: the maximum number of distinct
    /// hops in any dependency path starting from this entity, capped to
    /// avoid runaway cycles (see graph.rs). A chain like
    ///   a.b().c().d().e().f()
    /// shows up here as ~5 — deep chains are usually Law-of-Demeter
    /// violations or procedural orchestration missing an abstraction.
    pub chain_depth: Option<u32>,
    /// PageRank centrality on the dependency subgraph. Entities that many
    /// *important* things depend on rank higher than those merely with
    /// many direct callers. The number is normalised so it sums to 1.0
    /// across the graph — compare values relatively, not absolutely.
    pub pagerank: Option<f32>,
    /// Detected code smells (anti-pattern signals). Populated by the
    /// `detect_smells` pass after all metrics are finalised.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub smells: Vec<SmellKind>,
}

/// Named anti-pattern signal detected by combining multiple metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmellKind {
    /// Container with too many fields, methods, and outgoing dependencies.
    GodClass,
    /// Callable with high branching that mostly routes to other functions
    /// rather than containing domain logic (missing Strategy/Command).
    Dispatcher,
    /// Callable where the majority of outgoing dependencies target a
    /// single foreign type — the method probably belongs on that type.
    FeatureEnvy,
    /// Entity with very high fan-in — any change ripples widely.
    ShotgunSurgery,
    /// Container with many fields but little behavior — a data bag /
    /// configuration record. Not a god class, but the grouping can
    /// usually be made more explicit with nested sub-structs.
    ///
    /// Named "DataBag" rather than "DataClass" to avoid confusion with
    /// Kotlin `data class` / Python `@dataclass`, which are intentional
    /// idioms, not smells.
    DataBag,
}

impl SmellKind {
    /// Human-readable short label for UI display.
    pub fn label(self) -> &'static str {
        match self {
            SmellKind::GodClass => "God Class",
            SmellKind::Dispatcher => "Dispatcher",
            SmellKind::FeatureEnvy => "Feature Envy",
            SmellKind::ShotgunSurgery => "Shotgun Surgery",
            SmellKind::DataBag => "Data Bag",
        }
    }

    /// One-sentence remediation hint.
    pub fn hint(self) -> &'static str {
        match self {
            SmellKind::GodClass =>
                "Split by responsibility — consider Facade, Decorator, or extracting sub-types.",
            SmellKind::Dispatcher =>
                "Replace branching with polymorphism — Strategy, Command, or Chain of Responsibility.",
            SmellKind::FeatureEnvy =>
                "Move this method to the type it mostly interacts with, or extract shared logic.",
            SmellKind::ShotgunSurgery =>
                "Stabilise the interface and consider dependency inversion to reduce ripple risk.",
            SmellKind::DataBag =>
                "Group related fields into nested sub-structs to make the natural hierarchy explicit.",
        }
    }
}

/// Function/method parameter. Also reused to model struct fields and enum
/// variants — in those cases `visibility` is populated from the source
/// modifier (e.g. `pub`), while function parameters leave it `None`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Parameter {
    pub name: String,
    pub type_name: Option<String>,
    pub default_value: Option<String>,
    /// Source-level visibility when this parameter represents a struct
    /// field or enum variant. `None` for function parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
}

/// Types of code entities the visualizer can recognize
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// A file/module
    File,
    /// A module or namespace
    Module,
    /// A class
    Class,
    /// A Python `@dataclass`-decorated class (or semantically equivalent
    /// record/data-holder in other languages). Kept distinct from Class
    /// so the UI can visually flag it as a data-oriented type.
    Dataclass,
    /// An abstract class
    AbstractClass,
    /// A struct (specifically for languages that distinguish)
    Struct,
    /// An interface or protocol (generic)
    Interface,
    /// A Rust trait
    Trait,
    /// An enum type
    Enum,
    /// A type alias
    TypeAlias,
    /// A function or method
    Function,
    /// A method (specifically belonging to a class)
    Method,
    /// A constant
    Constant,
    /// A variable or field
    Variable,
    /// A property (getter/setter)
    Property,
    /// A macro
    Macro,
    /// A service (high-level abstraction)
    Service,
    /// A UI component that renders child components ("widgets"). Used for
    /// Svelte single-file components: the `.svelte` file becomes one
    /// Component that contains its script-level members and `Instantiates`
    /// the widgets used in its template markup.
    Component,
    /// A function/method parameter (synthetic entity for visualization)
    Parameter,
    /// A conditional-branch group (synthetic entity: one per arm of an
    /// `if`/`elif`/`else` in a function body). Calls made inside the
    /// branch reattach from the caller to the Branch node so the graph
    /// visually groups them. Treated as ghost-like in metrics: does not
    /// contribute to fan-out/fan-in of its parent callable.
    Branch,
    /// A loop group (synthetic entity: one per `for`/`while` in a
    /// function body). Mirrors `Branch` — calls inside the loop body
    /// reattach from the caller to the Loop node, and the Loop does
    /// not contribute to fan-out/fan-in metrics.
    Loop,
    /// An import statement
    Import,
    /// Elevator: the widest grouping above Category. Optional — a
    /// spec without Extensions just renders Categories at the top.
    /// Used when a project has clearly separable bundles (plugins,
    /// extensions, product variants) each containing their own
    /// Categories and Concepts.
    Extension,
    /// Elevator: a top-level grouping of Features. Behaviourless
    /// container — it has a name, a description, and a list of child
    /// Features. The "ground floor" of the onboarding elevator.
    Category,
    /// Elevator: a user-facing capability of the project. Hierarchical
    /// (Features can contain Features and Functionalities). Categories
    /// hold top-level Features.
    Feature,
    /// Elevator: a verb on a Feature (e.g. "creation", "edition",
    /// "run"). Leaf node — Functionalities never contain children.
    Functionality,
    /// Elevator: cross-cutting domain logic that multiple Features
    /// reference (e.g. "tax calculation", "LLM invocation"). Flat
    /// namespace — Concepts are not hierarchical.
    Concept,
    /// Elevator: a page in the user-facing UI. Targeted by `where:`
    /// edges from Features and Functionalities.
    UiPage,
    /// ansible-deploy: an Ansible playbook (`playbooks/*.yml`). Top of
    /// the deploy graph — includes Roles.
    Playbook,
    /// ansible-deploy: an Ansible role referenced from a playbook
    /// (e.g. `acme_platform.kubernetes.k8s_deployment`). Consumes a
    /// deployment-set variable.
    Role,
    /// ansible-deploy: a host or group variable scope
    /// (`group_vars/<g>` / `host_vars/<h>`). Container for the
    /// DeploymentSets declared in its var files.
    HostGroup,
    /// ansible-deploy: a list variable that enumerates what gets
    /// deployed (`k8s_pre_deployments`, `k8s_post_deployments`,
    /// `helm_deployment_charts`). Contains DeploymentEntries.
    DeploymentSet,
    /// ansible-deploy: one `{name, path, template?}` item in a
    /// DeploymentSet. Renders from exactly one TemplateFile.
    DeploymentEntry,
    /// ansible-deploy: a Jinja-templated manifest under
    /// `files/**/*.yml.j2` (or a raw manifest). Defines one or more
    /// K8sResources.
    TemplateFile,
    /// ansible-deploy: a single Kubernetes object (one `kind:` block)
    /// declared inside a TemplateFile — the actual primitive.
    K8sResource,
    /// ansible-deploy: a Helm chart referenced from
    /// `helm_deployment_charts`.
    HelmChart,
    /// SQL: a database table. Holds its columns as `fields` rather than as
    /// child entities, so it is deliberately **not** a container: nothing
    /// has a table as its `parent_id`. That also keeps it out of the
    /// container smell rules, where a wide table would otherwise read as a
    /// `DataBag` — many columns and no behaviour is what a table *is*.
    ///
    /// Carries
    /// `References` edges to the tables its foreign keys point at. A schema is
    /// topology, not control flow, so complexity metrics stay empty — see
    /// ADR 0003 for the same framing applied to declarative infra.
    Table,
    /// SQL: a view. Kept distinct from Table so the UI can show that its rows
    /// are derived rather than stored.
    View,
    /// Unknown or unrecognized
    Unknown,
}

impl EntityKind {
    /// Returns true if this entity can contain other entities
    pub fn is_container(&self) -> bool {
        matches!(
            self,
            EntityKind::File
                | EntityKind::Module
                | EntityKind::Class
                | EntityKind::Dataclass
                | EntityKind::AbstractClass
                | EntityKind::Struct
                | EntityKind::Interface
                | EntityKind::Trait
                | EntityKind::Enum
                | EntityKind::Service
                | EntityKind::Component
                | EntityKind::Feature
                | EntityKind::Category
                | EntityKind::Extension
                | EntityKind::Playbook
                | EntityKind::HostGroup
                | EntityKind::DeploymentSet
                | EntityKind::TemplateFile
        )
    }

    /// Returns true if this entity represents a callable
    pub fn is_callable(&self) -> bool {
        matches!(
            self,
            EntityKind::Function | EntityKind::Method | EntityKind::Macro
        )
    }
    
    /// Returns a display name for the entity kind
    pub fn display_name(&self) -> &'static str {
        match self {
            EntityKind::File => "file",
            EntityKind::Module => "module",
            EntityKind::Class => "class",
            EntityKind::Dataclass => "dataclass",
            EntityKind::AbstractClass => "abstract class",
            EntityKind::Struct => "struct",
            EntityKind::Interface => "interface",
            EntityKind::Trait => "trait",
            EntityKind::Enum => "enum",
            EntityKind::TypeAlias => "type",
            EntityKind::Function => "function",
            EntityKind::Method => "method",
            EntityKind::Constant => "constant",
            EntityKind::Variable => "variable",
            EntityKind::Property => "property",
            EntityKind::Macro => "macro",
            EntityKind::Service => "service",
            EntityKind::Component => "component",
            EntityKind::Parameter => "parameter",
            EntityKind::Branch => "branch",
            EntityKind::Loop => "loop",
            EntityKind::Import => "import",
            EntityKind::Extension => "extension",
            EntityKind::Category => "category",
            EntityKind::Feature => "feature",
            EntityKind::Functionality => "functionality",
            EntityKind::Concept => "concept",
            EntityKind::UiPage => "ui page",
            EntityKind::Playbook => "playbook",
            EntityKind::Role => "role",
            EntityKind::HostGroup => "host group",
            EntityKind::DeploymentSet => "deployment set",
            EntityKind::DeploymentEntry => "deployment entry",
            EntityKind::TemplateFile => "template",
            EntityKind::K8sResource => "k8s resource",
            EntityKind::HelmChart => "helm chart",
            EntityKind::Table => "table",
            EntityKind::View => "view",
            EntityKind::Unknown => "unknown",
        }
    }
}

/// Visibility/accessibility levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Public (accessible from anywhere)
    #[default]
    Public,
    /// Private (accessible only within the same scope)
    Private,
    /// Protected (accessible within class and subclasses)
    Protected,
    /// Internal (accessible within the same module/package)
    Internal,
    /// Crate-level (Rust-specific)
    Crate,
}

impl CodeEntity {
    /// Create a new code entity with minimal required fields
    pub fn new(
        name: impl Into<String>,
        kind: EntityKind,
        file_path: impl Into<PathBuf>,
        span: super::Span,
    ) -> Self {
        let name = name.into();
        let file_path_buf: PathBuf = file_path.into();
        let id = format!("{}:{}:{}", file_path_buf.display(), span.start.line, &name);
        
        Self {
            id: id.clone(),
            qualified_name: name.clone(),
            name,
            kind,
            visibility: Visibility::default(),
            file_path: file_path_buf,
            span,
            parent_id: None,
            documentation: None,
            attributes: Vec::new(),
            generics: Vec::new(),
            parameters: Vec::new(),
            return_type: None,
            implements: Vec::new(),
            extends: Vec::new(),
            tags: BTreeSet::new(),
            source_code: None,
            fields: Vec::new(),
            impl_blocks: Vec::new(),
            metrics: EntityMetrics::default(),
        }
    }
    
    /// Builder pattern: set visibility
    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }
    
    /// Builder pattern: set parent ID
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }
    
    /// Builder pattern: set documentation
    pub fn with_documentation(mut self, doc: impl Into<String>) -> Self {
        self.documentation = Some(doc.into());
        self
    }
    
    /// Builder pattern: add a tag
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.insert(tag.into());
        self
    }

    /// Builder pattern: set source code
    pub fn with_source_code(mut self, code: impl Into<String>) -> Self {
        self.source_code = Some(code.into());
        self
    }
}
