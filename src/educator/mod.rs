//! Educator — surfaces language-specific good-practice guidance at the cursor.
//!
//! See `CONTEXT.md` ("Educator" section) and ADR-0002 for design. Concretely:
//!
//! - Rule files live in `content/<lang>/rules/*.md` — YAML frontmatter + markdown body.
//! - The parser tags every position-relevant span with a **construct-kind** plus an
//!   attribute map (e.g. `synchronized_statement { lock_expr_kind: this_expression }`).
//! - Rules `applies-to` a list of construct-kinds; an optional `match:` predicate
//!   decides whether the rule fires **Specifically** at this instance or stays in
//!   the **General** bucket.
//! - The hover endpoint asks `query_position(file, line, col)` and gets back the
//!   ancestor stack plus the partitioned rule hits.
//!
//! Per ADR-0002: predicate primitives are declarative — `eq`, `in`, `absent`,
//! `present`. There is no Rust-handler escape hatch.
//!
//! This module is the door: [`corpus`] builds and holds the loaded state,
//! [`position`] and [`scan`] are the two ways to interrogate it, and the two
//! methods below are what the server, the CLI and the tests actually call.
//! Everything behind the door depends on `corpus`, never on this file.

pub mod catalog;
pub mod content_files;
pub mod corpus;
pub mod index;
pub mod issues;
pub mod java;
pub mod lessons;
pub mod position;
pub mod predicate;
pub mod rules;
pub mod scan;
pub mod validator;

#[cfg(test)]
mod tests;

use anyhow::Result;
use std::path::Path;

pub use corpus::Educator;
pub use issues::{LoadIssue, LoadIssueSeverity};
pub use lessons::Lesson;
pub use position::{Attachment, ConstructInstance, LessonHit, PositionResponse, RuleHit};
pub use rules::Rule;
pub use scan::{ScanHit, ScanResponse};

impl Educator {
    /// Run a position query and return the ancestor stack + matched rules.
    pub fn query_position(&self, file: &Path, line: u32, col: u32) -> Result<PositionResponse> {
        position::query(self, file, line, col)
    }

    /// Scan a whole file and return every rule that fires, sorted by line/col.
    pub fn scan_file(&self, file: &Path) -> Result<ScanResponse> {
        scan::scan_file(self, file)
    }
}
