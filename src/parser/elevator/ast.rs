//! Syntax tree the Elevator parser produces in phase 1 and consumes in
//! phase 2. Deliberately dumb: it records what the author wrote plus
//! where they wrote it, and resolves nothing.

use crate::models::{EntityKind, Position, Span};

/// The six definable entity kinds, keyed by their source keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum DefKind {
    Extension,
    Category,
    Feature,
    Functionality,
    Concept,
    UiPage,
}

/// Every keyword that opens a definition, longest-prefix first so
/// `fu`/`concept` are matched before `f`/`c` when used as an id prefix.
pub(super) const ALL_KINDS: [DefKind; 6] = [
    DefKind::Functionality,
    DefKind::Concept,
    DefKind::Extension,
    DefKind::Feature,
    DefKind::Category,
    DefKind::UiPage,
];

impl DefKind {
    pub(super) fn keyword(self) -> &'static str {
        match self {
            DefKind::Extension => "e",
            DefKind::Category => "c",
            DefKind::Feature => "f",
            DefKind::Functionality => "fu",
            DefKind::Concept => "concept",
            DefKind::UiPage => "ui",
        }
    }

    /// The segment used in entity ids (`elevator::<segment>.<qualname>`).
    /// Identical to the keyword today; kept separate so the id format
    /// can outlive a keyword rename.
    pub(super) fn id_segment(self) -> &'static str {
        self.keyword()
    }

    pub(super) fn entity_kind(self) -> EntityKind {
        match self {
            DefKind::Extension => EntityKind::Extension,
            DefKind::Category => EntityKind::Category,
            DefKind::Feature => EntityKind::Feature,
            DefKind::Functionality => EntityKind::Functionality,
            DefKind::Concept => EntityKind::Concept,
            DefKind::UiPage => EntityKind::UiPage,
        }
    }

    pub(super) fn from_keyword(kw: &str) -> Option<Self> {
        ALL_KINDS.into_iter().find(|k| k.keyword() == kw)
    }

    /// True for kinds whose names must be flat (no dots). Only
    /// Functionalities are qualified; UI pages accept any path.
    pub(super) fn requires_bare_name(self) -> bool {
        matches!(
            self,
            DefKind::Extension | DefKind::Category | DefKind::Feature | DefKind::Concept
        )
    }
}

/// A source location: everything needed to build a `Position` and to
/// slice the original text.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Loc {
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

impl Loc {
    pub(super) fn position(self) -> Position {
        Position::new(self.line, self.column, self.offset)
    }

    /// Human-facing `line L column C` suffix, 1-based like every other
    /// tool the author reads output from.
    pub(super) fn describe(self) -> String {
        format!("line {} column {}", self.line + 1, self.column + 1)
    }
}

/// One top-level definition, with the byte range of its whole
/// statement — keyword through closing brace.
#[derive(Debug, Clone)]
pub(super) struct DefStmt {
    pub kind: DefKind,
    /// Qualified path stored without the kind prefix
    /// (e.g. `protocol.creation` for a Functionality).
    pub qualname: String,
    pub start: Loc,
    pub end: Loc,
    pub description: Option<String>,
    /// Edge fields verbatim — resolved against the definition map in
    /// phase 2 so forward references work.
    pub where_targets: Vec<EdgeRef>,
    pub references: Vec<EdgeRef>,
    pub used_by: Vec<EdgeRef>,
    /// Code references — opaque path strings pointing at the
    /// implementation. Each entry is `(tag, path)` where `tag` is
    /// the part after the optional `cr.<tag>:` prefix (empty string
    /// for plain `cr:`). The tag is free-form — common conventions
    /// are `fe` (frontend), `be` (backend), `test`, `infra`, but
    /// the parser doesn't enforce a vocabulary.
    pub code_refs: Vec<(String, String)>,
    pub children: Vec<ChildRef>,
}

impl DefStmt {
    pub(super) fn span(&self) -> Span {
        Span::new(self.start.position(), self.end.position())
    }

    /// Byte range of the whole statement in the original source.
    pub(super) fn source_range(&self) -> std::ops::Range<usize> {
        self.start.offset..self.end.offset
    }
}

/// A child reference inside a body: a *use*, never a definition.
///
/// Carries no location because everything a child reference can be
/// wrong about is diagnosed while parsing it; phase 2 only resolves.
#[derive(Debug, Clone)]
pub(super) struct ChildRef {
    pub kind: DefKind,
    /// As written by the author, with any kind prefix already stripped.
    /// May be bare (no dots) or qualified.
    pub raw: String,
}

/// A target in a `where:` / `references:` / `used_by:` list.
///
/// `kind` is `Some` only when the author wrote an explicit prefix
/// (`concept.tax`). Without one the field's own default applies —
/// which is why the prefix has to survive parsing rather than being
/// stripped and forgotten: stripping it made `references: concept.tax`
/// silently resolve to a nonexistent Feature `tax`.
#[derive(Debug, Clone)]
pub(super) struct EdgeRef {
    pub kind: Option<DefKind>,
    pub name: String,
    pub at: Loc,
}

impl EdgeRef {
    /// The kind this reference targets: the explicit prefix if the
    /// author wrote one, else the field's default.
    pub(super) fn resolve_kind(&self, default: DefKind) -> DefKind {
        self.kind.unwrap_or(default)
    }
}

/// Split a leading kind prefix (`fu.`, `concept.`, `e.`, `f.`, `c.`,
/// `ui.`) off a name, returning the kind it named and the remainder.
/// Lets authors write `f protocol` and `f f.protocol` interchangeably.
pub(super) fn split_kind_prefix(qualname: &str) -> (Option<DefKind>, String) {
    for kind in ALL_KINDS {
        let prefix = format!("{}.", kind.keyword());
        if let Some(rest) = qualname.strip_prefix(&prefix) {
            return (Some(kind), rest.to_string());
        }
    }
    (None, qualname.to_string())
}

/// Drop a leading kind prefix, keeping only the name.
pub(super) fn strip_kind_prefix(qualname: &str) -> String {
    split_kind_prefix(qualname).1
}

/// The last dot-separated segment — the entity's display name.
pub(super) fn leaf_segment(qualname: &str) -> &str {
    qualname.rsplit('.').next().unwrap_or(qualname)
}

/// Full entity id for a kind + qualified name.
pub(super) fn entity_id(kind: DefKind, qualname: &str) -> String {
    format!("elevator::{}.{}", kind.id_segment(), qualname)
}

/// Resolve a child reference's qualified name in its parent's context.
/// Bare Functionality names are parent-scoped; everything else is taken
/// as written.
pub(super) fn qualify_child(parent: &DefStmt, child_kind: DefKind, raw: &str) -> String {
    if child_kind == DefKind::Functionality && !raw.contains('.') {
        // Only meaningful when the parent is a Feature; the grammar
        // warns when it isn't.
        return format!("{}.{}", parent.qualname, raw);
    }
    raw.to_string()
}
