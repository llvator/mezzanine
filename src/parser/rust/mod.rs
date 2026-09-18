//! Rust parser — the entry point, and the phases one file runs through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a callable body yields once its declaration is placed
//!
//! What both of them read from a grammar node, and write their findings
//! into, sits here beside them:
//! - [`ctx`] — the context threaded through the walk
//! - [`helpers`] — visibility, base type names, generics
//! - [`doc_comments`] — `///`, `//!`, `/** */` extraction
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics for a body
//! - [`types`] — `UsesType` edges from signature/field types (post-pass)
//! - [`values`] — `UsesValue` edges from `use`d names read as values
//!
//! Reducing written-out type text to a bare name is pure string work that the
//! analyzer needs as much as the walk does, so it lives outside this folder in
//! [`super::rust_type_names`] and leaves [`RustParser`] as the only way in.

pub(crate) mod bodies;
mod complexity;
mod ctx;
mod declarations;
mod doc_comments;
mod helpers;
mod types;
mod values;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Parser, Tree};

pub struct RustParser {
    parser: Parser,
}

impl RustParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::language())
            .expect("Failed to set Rust language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Rust code"))
    }
}

impl Default for RustParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for RustParser {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        declarations::extract_file(&tree.root_node(), path, content, &mut result);

        // Emit UsesType edges from signature/field types (RS-001).
        types::emit_uses_type_edges(&mut result);

        // Emit UsesValue edges for `use`d names read as values (AN-028).
        // After the declaration walk because it sources each edge from the
        // entity whose span encloses the read.
        values::emit_uses_value_edges(&tree.root_node(), content, &mut result);

        Ok(result)
    }
}
