//! Java parser — the entry point, and the phases one file runs through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a callable body yields once its declaration is placed
//!
//! What both of them read from a grammar node, and write their findings into,
//! sits here beside them:
//! - [`ctx`] — the context threaded through the walk
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics for a body
//! - [`helpers`] — visibility, modifiers, parameters, generics, type lists
//! - [`javadoc`] — `/** ... */` extraction
//! - [`types`] — `UsesType` edges from signature/field types (post-pass)

pub(crate) mod bodies;
mod complexity;
mod ctx;
mod declarations;
mod helpers;
mod javadoc;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Parser, Tree};

pub struct JavaParser {
    parser: Parser,
}

impl JavaParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::language())
            .expect("Failed to set Java language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Java code"))
    }
}

impl Default for JavaParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for JavaParser {
    fn language(&self) -> Language {
        Language::Java
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        declarations::extract_file(tree.root_node(), path, content, &mut result);

        // Emit UsesType edges from signature/field types (JV-001).
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
