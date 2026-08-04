//! Python parser — entry point and entity-extraction dispatcher.
//!
//! Submodules group the parsing logic by concern:
//! - [`classes`] — class definitions and field collection
//! - [`functions`] — functions, methods, parameters, fluent-self detection
//! - [`assignments`] — module-level constants and variables
//! - [`calls`] — call-site relationship extraction with branch threading
//!   plus self/local write tracking
//! - [`complexity`] — cyclomatic / cognitive metrics
//! - [`flow`] — synthetic Branch / Loop / try-arm entity emission
//! - [`generics`] — PEP 695 type parameters and `TypeVar` factory calls
//! - [`imports`] — import statements
//! - [`docstrings`] — docstring extraction and dedenting
//! - [`decorators`] — decorator collection
//! - [`stdlib`] — built-in name table for call filtering
//! - [`types`] — `UsesType` edges from annotations (post-pass)

mod assignments;
mod calls;
mod classes;
mod complexity;
mod decorators;
mod docstrings;
mod flow;
mod functions;
mod generics;
mod imports;
mod stdlib;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{ImportCondition, LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

pub struct PythonParser {
    parser: Parser,
}

/// Shared context threaded through the entity-extraction walk.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub result: &'a mut ParseResult,
    /// The conditional wrapper the walk is currently inside, if any. Set by
    /// `descend_conditional` and read only when recording imports (PY-024).
    pub import_condition: Option<ImportCondition>,
}

impl PythonParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::language())
            .expect("Failed to set Python language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Python code"))
    }
}

/// Walk a node's children and dispatch to the appropriate extractor for each
/// kind. Recurses into containers that may hold nested definitions.
fn extract_entities(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                functions::handle_function(&child, parent_id, ctx, extract_entities);
            }
            "class_definition" => {
                classes::handle_class(&child, parent_id, ctx, extract_entities);
            }
            "import_statement" | "import_from_statement" => handle_imports(&child, ctx),
            // PY-024: an import inside an `if` or a `try` is a weaker claim
            // than one at the top of the file. Recording *which* wrapper it
            // sits in is what separates `if TYPE_CHECKING:` from the
            // `try: import ujson / except ImportError: import json` shape.
            "if_statement" | "try_statement" => descend_conditional(child, parent_id, ctx),
            "decorated_definition" => {
                // Recurse — the inner function/class will be matched next.
                extract_entities(child, parent_id, ctx);
            }
            "expression_statement" => {
                // Only extract module-level assignments as entities.
                // Class-level annotations are captured in class.fields instead.
                if parent_id.is_none() {
                    assignments::handle_module_assignment(&child, ctx);
                }
            }
            "type_alias_statement" => handle_type_alias(&child, parent_id, ctx),
            _ => {
                extract_entities(child, parent_id, ctx);
            }
        }
    }
}

/// Record a file's imports, stamping whichever conditional wrapper the walk
/// is currently inside onto each one.
fn handle_imports(node: &Node, ctx: &mut ExtractCtx<'_>) {
    for mut import in imports::parse_import(node, ctx.source) {
        if let Some(condition) = ctx.import_condition {
            import = import.conditional(condition);
        }
        ctx.result.add_import(import);
    }
}

/// Walk into an `if` / `try` with `condition` in force, then put back
/// whatever was in force before — a `try` nested in an `if` must not leave
/// the walk thinking the rest of the `if` is a fallback.
fn descend_conditional(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    // A `try` says "one of these arms is an acceptable dependency"; an `if`
    // says "this one, when the gate opens".
    let condition = match node.kind() {
        "try_statement" => ImportCondition::Fallback,
        _ => ImportCondition::Guarded,
    };
    let outer = ctx.import_condition.replace(condition);
    extract_entities(node, parent_id, ctx);
    ctx.import_condition = outer;
}

/// PEP 695 `type X = …`.
fn handle_type_alias(node: &Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    if let Some(entity) =
        assignments::parse_type_alias_statement(node, ctx.source, ctx.path, parent_id)
    {
        ctx.result.add_entity(entity);
    }
}

impl Default for PythonParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for PythonParser {
    fn language(&self) -> Language {
        Language::Python
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        let mut ctx = ExtractCtx {
            source: content,
            path,
            result: &mut result,
            import_condition: None,
        };
        extract_entities(tree.root_node(), None, &mut ctx);

        // Emit UsesType edges from annotations (PY-025).
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
