//! TypeScript / TSX parser — the entry point, and the phases one file runs
//! through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a callable body yields once its declaration is placed
//!
//! What both of them read from a grammar node, and write their findings
//! into, sits here beside them:
//! - [`ctx`] — the context threaded through the walk
//! - [`helpers`] — accessibility, parameters, generics, type-text trimming
//! - [`tsdoc`] — `/** ... */` extraction
//! - [`decorators`] — child + sibling decorator collection
//! - [`types`] — `UsesType` edges from signature/member types (post-pass)
//! - [`values`] — `UsesValue` edges from imported names read as values

pub(crate) mod bodies;
mod ctx;
mod declarations;
mod decorators;
mod helpers;
mod tsdoc;
mod types;
mod values;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Parser, Tree};

pub struct TypeScriptParser {
    parser: Parser,
}

impl TypeScriptParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::language_typescript())
            .expect("Failed to set TypeScript language");
        Self { parser }
    }

    /// Pick the TSX or TS grammar based on file extension.
    fn new_for_path(path: &Path) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&grammar_for_path(path))
            .expect("Failed to set TypeScript language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse TypeScript code"))
    }
}

/// Which of the two grammars reads this file.
///
/// TSX is not only for `.tsx`. Every JavaScript extension takes it too
/// (JS-001), because JSX in a plain `.js` file is ordinary React and the TS
/// grammar cannot read it: on a component whose JSX attributes call three
/// functions, the TS grammar recovers the declarations through error repair
/// but loses **all three** call edges, while the TSX grammar keeps them.
/// Nothing is given up in exchange — the one construct TSX cannot parse,
/// the `<T>expr` cast, is TypeScript syntax that no JavaScript file holds.
fn grammar_for_path(path: &Path) -> tree_sitter::Language {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    match ext {
        "tsx" | "jsx" | "js" | "mjs" | "cjs" => tree_sitter_typescript::language_tsx(),
        _ => tree_sitter_typescript::language_typescript(),
    }
}

impl Default for TypeScriptParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for TypeScriptParser {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new_for_path(path);
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        declarations::extract_file(&tree.root_node(), path, content, &mut result);

        // Emit UsesType edges from signature/member types (TS-001).
        types::emit_uses_type_edges(&mut result);

        // Emit UsesValue edges for imported names read as values (AN-028).
        // After the declaration walk because it sources each edge from the
        // entity whose span encloses the read.
        values::emit_uses_value_edges(&tree.root_node(), content, &mut result);

        Ok(result)
    }
}
