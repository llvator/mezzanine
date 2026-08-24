//! Language parser trait and common functionality.

use crate::models::file_info::Language;
use crate::models::{CodeEntity, FileInfo, Position, Relationship, Span};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Result of parsing a source file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParseResult {
    /// File information
    pub file_info: Option<FileInfo>,

    /// Entities found in the file
    pub entities: Vec<CodeEntity>,

    /// Relationships found in the file
    pub relationships: Vec<Relationship>,

    /// Imports/dependencies detected
    pub imports: Vec<ImportInfo>,

    /// Any warnings during parsing
    pub warnings: Vec<String>,

    /// Documentation for the file as a whole — Rust's `//!` header, and the
    /// equivalent in any other language that has one.
    ///
    /// Separate from entity documentation because it belongs to no entity:
    /// the module a `.rs` file defines is declared in a *different* file, so
    /// there is nothing in this parse to hang it on. `parse_file_standalone`
    /// lifts it onto `FileInfo::documentation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_documentation: Option<String>,

    /// Ordered SQL schema operations (SQL-002). Empty for every other
    /// language — language-specific in the same way `CodeEntity::impl_blocks`
    /// is Rust-specific.
    ///
    /// SQL files state *changes* to a schema, not the schema, so the parser
    /// cannot decide what a table finally looks like without seeing every
    /// other file. It emits operations here instead and `analyzer::sql_fold`
    /// replays them in order. See ADR-0007 for why the split falls there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_ops: Vec<crate::parser::sql::ops::PositionedOp>,
}

/// Information about an import/dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportInfo {
    /// The import path/module name
    pub path: String,

    /// Specific items imported (empty if importing entire module)
    pub items: Vec<String>,

    /// Is this a relative import?
    pub is_relative: bool,

    /// The alias if renamed
    pub alias: Option<String>,

    /// Location in source
    pub span: Span,

    /// `from x import *` — the module's entire surface rather than a listed
    /// set of names. Without this, an empty `items` was ambiguous between
    /// "the parser captured nothing" and "the user asked for everything"
    /// (PY-018). Resolvers should read it as a dependency on the module and
    /// not try to enumerate symbols.
    #[serde(default)]
    pub is_wildcard: bool,

    /// Set when the statement is a re-export — `export { X } from './y'`
    /// — rather than an import (AN-024). The file names another module's
    /// symbol in order to hand it on; it does not use it itself.
    ///
    /// Recorded because the distinction is the one an agent most often has
    /// to reconstruct by hand: an edge that looks like a direct dependency
    /// on a file the reader never named, arriving through a shim. Reshape's
    /// own rules call a re-export shim not-a-door, so the edge it produces
    /// has to say which kind it is.
    #[serde(default)]
    pub is_reexport: bool,

    /// Set when the import sits inside an `if` or a `try` (PY-024). A
    /// conditional import is a weaker claim than an unconditional one: the
    /// dependency may be optional, version-gated, or one of several
    /// interchangeable alternatives.
    #[serde(default)]
    pub condition: Option<ImportCondition>,

    /// Set when the build erases the statement: TypeScript's
    /// `import type { X } from './y'`, and Python's `if TYPE_CHECKING:`
    /// (AN-022). The specifier is never resolved at runtime and the module
    /// is not in the emitted bundle.
    ///
    /// Named for the category rather than for a keyword, because the
    /// category is wider than TypeScript: a Rust `use` that only names a
    /// trait for a bound and an annotation-only Java import are the same
    /// fact, and each language should answer this question in the same
    /// field.
    ///
    /// A statement counts only when *every* name it binds is erased.
    /// `import { type X, y }` still resolves the specifier for `y`, so it
    /// is an ordinary dependency wearing a `type` keyword on one of its
    /// names.
    #[serde(default)]
    pub is_type_only: bool,
}

/// Why an import is conditional. The two cases mean different things to a
/// reader, so they stay distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportCondition {
    /// Inside an `if` — gated on a version, a platform, or `TYPE_CHECKING`.
    /// The dependency is real when the gate opens.
    Guarded,
    /// Inside a `try` — the `try: import ujson / except ImportError: import
    /// json` shape. Every arm is *an* acceptable dependency; which one is
    /// used is a runtime fact.
    Fallback,
}

/// Trait for language-specific parsers.
pub trait LanguageParser: Send + Sync {
    /// Get the language this parser handles
    fn language(&self) -> Language;

    /// Parse source code and extract entities and relationships
    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult>;

    /// Check if this parser can handle the given file
    fn can_parse(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                self.language()
                    .extensions()
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(ext))
            })
            .unwrap_or(false)
    }
}

impl ParseResult {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an entity to the result
    pub fn add_entity(&mut self, entity: CodeEntity) {
        self.entities.push(entity);
    }

    /// Add a relationship to the result
    pub fn add_relationship(&mut self, rel: Relationship) {
        self.relationships.push(rel);
    }

    /// Add an import
    pub fn add_import(&mut self, import: ImportInfo) {
        self.imports.push(import);
    }

    /// Add a warning
    pub fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// Merge another parse result into this one
    pub fn merge(&mut self, other: ParseResult) {
        self.entities.extend(other.entities);
        self.relationships.extend(other.relationships);
        self.imports.extend(other.imports);
        self.warnings.extend(other.warnings);
        self.schema_ops.extend(other.schema_ops);
    }
}

impl ImportInfo {
    pub fn new(path: impl Into<String>, span: Span) -> Self {
        Self {
            path: path.into(),
            items: Vec::new(),
            is_relative: false,
            alias: None,
            span,
            is_wildcard: false,
            is_reexport: false,
            condition: None,
            is_type_only: false,
        }
    }

    /// `from x import *`.
    pub fn wildcard(mut self) -> Self {
        self.is_wildcard = true;
        self
    }

    /// `export { X } from './y'` — a name passed on rather than used.
    pub fn reexport(mut self) -> Self {
        self.is_reexport = true;
        self
    }

    /// Record that this import sits inside an `if` or a `try`.
    pub fn conditional(mut self, condition: ImportCondition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// `import type { X } from './y'` — a statement the build erases.
    pub fn type_only(mut self) -> Self {
        self.is_type_only = true;
        self
    }

    pub fn with_items(mut self, items: Vec<String>) -> Self {
        self.items = items;
        self
    }

    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }

    pub fn relative(mut self) -> Self {
        self.is_relative = true;
        self
    }
}

/// Helper to convert tree-sitter Point to our Position
pub fn point_to_position(point: tree_sitter::Point, offset: usize) -> Position {
    Position::new(point.row, point.column, offset)
}

/// Helper to convert tree-sitter Node range to our Span
pub fn node_to_span(node: &tree_sitter::Node) -> Span {
    Span::new(
        point_to_position(node.start_position(), node.start_byte()),
        point_to_position(node.end_position(), node.end_byte()),
    )
}

/// Extract text from a node in the source
pub fn node_text<'a>(node: &tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

/// Find the first direct child of `node` with the given AST `kind`.
/// Common utility used by every tree-sitter based parser.
pub fn find_child_by_kind<'a>(
    node: &tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

/// Parse a non-container entity and add it to the result. Eliminates the
/// repeated `if let Some(entity) = parse(...) { result.add_entity(entity) }`
/// boilerplate that every parser's `extract_entities` uses for leaf nodes.
pub fn handle_leaf<P>(
    parser: &P,
    node: &tree_sitter::Node,
    source: &str,
    path: &std::path::Path,
    parent_id: Option<&str>,
    result: &mut ParseResult,
    parse: fn(&P, &tree_sitter::Node, &str, &std::path::Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse(parser, node, source, path, parent_id) {
        result.add_entity(entity);
    }
}
