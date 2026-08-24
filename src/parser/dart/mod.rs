//! Dart parser — the entry point, and the phases one file runs through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a callable body yields once its declaration is placed
//!
//! What both of them read from a grammar node, and write their findings
//! into, sits here beside them:
//! - [`ctx`] — the context threaded through the walk
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics for a body
//! - [`doc_comments`] — `///` and `/** … */` extraction
//! - [`helpers`] — parameters, generics, visibility, declared types
//! - [`types`] — `UsesType` edges from signature and field types (post-pass)
//!
//! **The grammar is vendored, not a crates.io dependency.** It lives at
//! `vendor/tree-sitter-dart/`, is compiled by `build.rs`, and is upstream's
//! Dart 3 grammar regenerated to target the grammar ABI our `tree-sitter`
//! can load. That directory's README explains why. Nothing else in this
//! folder has to care — the tree it produces is an ordinary tree-sitter
//! tree, and it understands the whole language, including records, patterns,
//! extension types and the Dart 3 class modifiers.

mod bodies;
mod complexity;
mod ctx;
mod declarations;
mod doc_comments;
mod helpers;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Language as TsLanguage, Parser, Tree};

extern "C" {
    /// The vendored grammar's entry point, compiled in by `build.rs`.
    fn tree_sitter_dart() -> TsLanguage;
}

pub struct DartParser {
    parser: Parser,
}

impl DartParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&unsafe { tree_sitter_dart() })
            .expect("Failed to set Dart language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Dart code"))
    }
}

impl Default for DartParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for DartParser {
    fn language(&self) -> Language {
        Language::Dart
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        declarations::extract_file(tree.root_node(), path, content, &mut result);
        result.file_documentation = doc_comments::library_doc(tree.root_node(), content);

        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
