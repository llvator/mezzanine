//! Kotlin parser — the entry point, and the phases one file runs through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a function body yields once its declaration is placed:
//!   its calls, the Branch / Loop entities their arms become, and the
//!   complexity metrics the body scores
//!
//! What both of them read from a grammar node, and write their findings into,
//! sits here beside them:
//! - [`ctx`] — the context threaded through the walk
//! - [`helpers`] — parameters, generics, identifier/type extraction
//! - [`kdoc`] — `/** ... */` documentation extraction
//! - [`modifiers`] — visibility + Kotlin's many modifier buckets
//! - [`types`] — `UsesType` edges from signature/property types (post-pass)

mod bodies;
mod ctx;
mod declarations;
mod helpers;
mod kdoc;
mod modifiers;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Parser, Tree};

pub struct KotlinParser {
    parser: Parser,
}

impl KotlinParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_kotlin::language())
            .expect("Failed to set Kotlin language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Kotlin code"))
    }
}

impl Default for KotlinParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for KotlinParser {
    fn language(&self) -> Language {
        Language::Kotlin
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        declarations::extract_file(tree.root_node(), path, content, &mut result);

        // Emit UsesType edges from signature/property types (KT-001).
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
