//! Go parser — the entry point, and the phases one file runs through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a callable body yields once its declaration is placed
//!
//! What both of them read from a grammar node, and write their findings into,
//! sits here beside them:
//! - [`ctx`] — the context threaded through the walk
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics for a body
//! - [`doc_comments`] — the `//` block sitting directly above a declaration
//! - [`helpers`] — visibility, parameters, results, generics, type text
//! - [`packages`] — what a file imports, and which of those are the stdlib
//! - [`types`] — `UsesType` edges, emitted from the grammar's type nodes as
//!   the walk places each declaration
//!
//! Two facts about Go shape most of what follows, and neither has an
//! equivalent in the languages parsed before it:
//!
//! * **Visibility is spelled in the name.** An identifier starting with an
//!   uppercase letter is exported from its package; anything else is not.
//!   There is no modifier to read, so [`helpers::visibility_of`] takes a
//!   name rather than a node.
//! * **A method is a top-level declaration.** `func (s *Server) Handle()` is
//!   a sibling of the type it hangs off, not a child of it — and it may sit
//!   in a different file of the same package. The receiver is what binds the
//!   two, so [`declarations::callables`] resolves it to the type's entity
//!   when that type is in this file and falls back to the bare type name
//!   when it is not, which is the shape `graph.rs` already accepts from the
//!   Rust parser's impl blocks.

pub(crate) mod bodies;
mod complexity;
mod ctx;
mod declarations;
mod doc_comments;
mod helpers;
mod packages;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Parser, Tree};

pub struct GoParser {
    parser: Parser,
}

impl GoParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::language())
            .expect("Failed to set Go language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Go code"))
    }
}

impl Default for GoParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for GoParser {
    fn language(&self) -> Language {
        Language::Go
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        declarations::extract_file(tree.root_node(), path, content, &mut result);

        Ok(result)
    }
}
