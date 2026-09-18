//! Python parser — the entry point, and the phases one file runs through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a callable body yields once its declaration is placed
//!
//! What both of them read from a grammar node, and write their findings
//! into, sits here beside them:
//! - [`ctx`] — the context threaded through the walk
//! - [`decorators`] — decorator text and the edges decorators imply
//! - [`docstrings`] — docstring extraction and dedenting
//! - [`generics`] — PEP 695 type parameters and `TypeVar` factory calls
//! - [`types`] — `UsesType` edges from annotations (post-pass)
//! - [`values`] — `UsesValue` edges from imported names read as values

mod bodies;
mod ctx;
mod declarations;
mod decorators;
mod docstrings;
mod generics;
mod types;
mod values;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Parser, Tree};

pub struct PythonParser {
    parser: Parser,
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
        declarations::extract_file(tree.root_node(), path, content, &mut result);

        // Emit UsesType edges from annotations (PY-025).
        types::emit_uses_type_edges(&mut result);

        // Emit UsesValue edges for imported names read as values (PY-030).
        // After the declaration walk because it sources each edge from the
        // entity whose span encloses the read.
        values::emit_uses_value_edges(&tree.root_node(), content, &mut result);

        Ok(result)
    }
}
