//! Elevator (`.elv`) — domain-level spec language for describing a
//! project's user-facing capabilities at the level above code.
//!
//! Designed for onboarding: a reader opens the graph, sees Categories
//! at the top (the "ground floor"), and rides the hierarchy down
//! through Features to Functionalities. Cross-cutting logic shared by
//! several Features is expressed as Concepts.
//!
//! Submodules follow the same split as the tree-sitter parsers:
//! - [`lexer`] — tokenizer; recovers from stray characters. Private to
//!   [`grammar`], which is the only reader of a token.
//! - [`ast`] — [`ast::DefStmt`] and friends, plus id/name helpers
//! - [`grammar`] — phase 1: source → definitions, with diagnostics
//! - [`emit`] — phase 2: definitions → entities + edges
//!
//! ## Syntax
//!
//! Definitions are **flat** — every entity is defined at the top level
//! and the hierarchy is built from cross-references inside `{}` bodies.
//! No nested definitions: a `{}` body lists metadata fields and child
//! references by name, never another full definition.
//!
//! ```text
//! import STRING                  // must precede every definition
//!
//! e <name> { body }              // Extension — optional widest grouping
//! c <name> { body }              // Category — flat top-level grouping
//! f <name> { body }              // Feature — flat top-level capability
//! fu <feat>.<verb> { body }      // Functionality — qualified by parent feature
//! concept <name> { body }        // Concept — flat cross-cut namespace
//! ui <qualified> { body }        // UI Page — qualified path
//!
//! body := desc | edge | code_ref | child_ref
//! desc        := "d" ":" STRING
//! edge        := ("where" | "references" | "used_by") ":" reflist
//! code_ref    := "cr" ["." IDENT] ":" STRING ("," STRING)*
//! child_ref   := ("e" | "c" | "f" | "fu" | "concept" | "ui") (qualname | IDENT)
//! ```
//!
//! Whitespace is insignificant; commas between siblings are optional.
//! `#` starts a line comment. Bodies are optional — `f protocol` on
//! its own is a complete definition.
//!
//! ## Entity model
//!
//! - **Extension** (`e`)        — flat. ID = `elevator::e.<name>`.
//! - **Category** (`c`)         — flat. ID = `elevator::c.<name>`.
//! - **Feature** (`f`)          — flat. ID = `elevator::f.<name>`.
//! - **Functionality** (`fu`)   — qualified. ID = `elevator::fu.<feat>.<verb>`.
//! - **Concept** (`concept`)    — flat. ID = `elevator::concept.<name>`.
//! - **UiPage** (`ui`)          — qualified. ID = `elevator::ui.<path>`.
//!
//! ## Reference resolution
//!
//! Inside a parent's body, a child reference `<kw> <ref>` resolves as
//! follows. Any leading kind prefix on `<ref>` (e.g. `f.`, `ui.`) is
//! stripped before lookup so authors can write `f protocol` or
//! `f f.protocol` interchangeably.
//!
//! - `c <name>`, `f <name>`, `concept <name>`         → flat ID.
//! - `fu <bare_verb>` inside `f <feat> { ... }`       → `fu.<feat>.<verb>`.
//! - `fu <qualified>` (contains a dot)                → `fu.<qualified>`.
//! - `ui <qualified>` or `ui <name>`                  → `ui.<as-is>`.
//!
//! Edge fields (`where:`, `references:`, `used_by:`) default the target
//! kind from the field name (`where`→ui, `references`→f, `used_by`→f).
//! An explicit kind prefix on a value overrides that default, so
//! `references: concept.tax` reaches the Concept rather than resolving
//! to a Feature that was never defined.
//!
//! ## Edge model
//!
//! - `Contains` — parent → child, emitted from each child reference.
//!   The child's `parent_id` is set by the analyzer, not here.
//! - `References` (metadata `link:where`) — entity → UI page.
//! - `References` (metadata `link:references`) — entity → Feature.
//! - `References` (metadata `link:used_by`) — Feature → Concept (a
//!   Concept's `used_by:` lists its consumers; we emit the edge from
//!   each consumer to the Concept).
//!
//! Targets that aren't defined in the parsed file are still emitted as
//! relationships with their predicted ID — when a sibling `.elv` file
//! defines them, the analyzer's resolver hooks them up.
//!
//! ## Diagnostics
//!
//! Neither lexing nor parsing can fail the file. A stray character, an
//! unterminated string or an unrecognised keyword costs one token or
//! one definition and the rest of the file still yields entities;
//! every warning carries `line L column C`. The old fail-fast lexer
//! meant one typo silently zeroed a whole spec — the exact
//! silent-miss failure mode the graph is supposed to make impossible.

mod ast;
mod emit;
mod grammar;
mod lexer;

#[cfg(test)]
mod tests;

use super::language_parser::{ImportInfo, LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::Span;
use anyhow::Result;
use std::path::Path;

use self::grammar::parse_source;

pub struct ElevatorParser;

impl ElevatorParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ElevatorParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for ElevatorParser {
    fn language(&self) -> Language {
        Language::Elevator
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut result = ParseResult::new();

        // Phase 1 — a flat list of imports + definitions. No resolution
        // happens here; bodies are captured verbatim. Tokenizing is the
        // grammar's own business, and lexical complaints come back in
        // the same `warnings` list as the parser's.
        let parsed = parse_source(content);
        for w in parsed.warnings {
            result.add_warning(w);
        }
        // Imports become `ImportInfo` entries; the analyzer's
        // `validate_elevator_imports` pass consumes them post-merge to
        // compute per-file scopes.
        for imp in parsed.imports {
            let span = Span::new(imp.start.position(), imp.end.position());
            result.add_import(ImportInfo::new(imp.path, span).relative());
        }

        // Phase 2 — synthesise entities and resolve cross-references
        // into Contains / References edges.
        emit::emit(path, content, &parsed.defs, &mut result);
        Ok(result)
    }
}
