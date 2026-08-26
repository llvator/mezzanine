//! JavaScript parser (`.js`, `.mjs`, `.cjs`, `.jsx`).
//!
//! There is no second grammar and no second extractor. `tree-sitter-typescript`
//! *is* `tree-sitter-javascript` with the type rules added on top, so every
//! node kind a JavaScript file produces is one the TypeScript walk in
//! [`crate::parser::typescript`] already reads. Pointing that walk at a `.js`
//! file recovers classes, private fields, getters, default and rest
//! parameters, async arrows and the call graph between them — measured on one
//! 27-line service, 25 entities and 23 relationships against the 4 entities
//! and 0 relationships the generic fallback used to return for the same file.
//!
//! So this parser is a router, in the same spirit as [`crate::parser::svelte`],
//! which masks its non-script bytes and delegates the rest here-abouts too.
//! Its own job is to answer [`LanguageParser::language`] with
//! [`Language::JavaScript`], so a `.js` file is never reported as TypeScript,
//! and to be the place the JavaScript-only work lands when it does.
//!
//! Grammar selection is not this module's decision: `TypeScriptParser::parse`
//! reads the extension and `typescript::grammar_for_path` hands every
//! JavaScript extension the TSX grammar, which is what keeps call edges inside
//! JSX attributes.
//!
//! **Not covered yet.** CommonJS is invisible to the import ledger:
//! `require('./x')` yields no `ImportInfo`, and `module.exports = { ... }` is
//! not read as an export surface. The call graph survives it — name
//! resolution still binds `new Repo()` to the class in the required file —
//! but a module-level dependency stated only through `require` is missing
//! from `imports`. `UsesType` edges are absent by nature rather than by
//! omission: an untyped language states no types to draw them from.

use super::language_parser::{LanguageParser, ParseResult};
use super::TypeScriptParser;
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;

#[cfg(test)]
mod tests;

pub struct JavaScriptParser;

impl JavaScriptParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for JavaScriptParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for JavaScriptParser {
    fn language(&self) -> Language {
        Language::JavaScript
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        TypeScriptParser::new().parse(path, content)
    }
}
