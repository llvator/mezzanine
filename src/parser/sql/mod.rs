//! SQL schema parser (SQL-001).
//!
//! Recovers the *shape of a database* from `.sql` files: tables, their
//! columns, and the foreign keys between them. Not a general SQL parser —
//! queries, procedural blocks and DML parse and are then ignored, because the
//! question this answers is "how do the tables relate", not "what does this
//! statement do".
//!
//! Complexity metrics stay empty by design. A schema has no control flow, so
//! cyclomatic and nesting numbers would be zeroes pretending to be
//! measurements — the same framing ADR-0003 applies to declarative infra.
//!
//! **This parser is per-file and deliberately naive about history.** In a
//! migration repo an `ALTER TABLE` in one file mutates a table created in
//! another, and reading the files as an unordered set describes a schema that
//! never existed. Folding them in order is an analyzer pass (SQL-002), not
//! this parser's job; see ADR-0007 for why the split falls here.

mod lexer;
pub mod ops;
mod schema;
#[cfg(test)]
mod tests;

use crate::models::file_info::Language;
use crate::models::{FileInfo, Position, Span};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use anyhow::Result;
use sqlparser::ast::{ObjectType, Statement};
use sqlparser::dialect::{Dialect, GenericDialect, PostgreSqlDialect};
use sqlparser::parser::Parser as SqlAstParser;
use std::path::Path;

pub struct SqlParser {
    /// `Send + Sync` is spelled out because `LanguageParser` requires it and
    /// `dyn Dialect` alone does not imply it, though every concrete dialect
    /// in the crate is a unit struct that satisfies both.
    dialect: Box<dyn Dialect + Send + Sync>,
}

impl SqlParser {
    /// Default to PostgreSQL. It is the dialect the `.sql`-migration
    /// convention is most associated with, and it is a superset of generic
    /// SQL for the DDL this parser reads. `GenericDialect` is available via
    /// [`SqlParser::generic`] for corpora that trip on Postgres-isms.
    pub fn new() -> Self {
        Self {
            dialect: Box::new(PostgreSqlDialect {}),
        }
    }

    pub fn generic() -> Self {
        Self {
            dialect: Box::new(GenericDialect {}),
        }
    }
}

impl Default for SqlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for SqlParser {
    fn language(&self) -> Language {
        Language::Sql
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut result = ParseResult::new();
        result.file_info = Some(FileInfo {
            path: path.to_path_buf(),
            language: Language::Sql,
            size: content.len() as u64,
            line_count: content.lines().count(),
            content_hash: None,
        });

        for chunk in lexer::split_statements(content) {
            self.parse_chunk(chunk, content, path, &mut result);
        }
        Ok(result)
    }
}

impl SqlParser {
    /// Parse one statement and fold whatever it contributes into `result`.
    ///
    /// A chunk that does not parse produces a warning and nothing else.
    /// Failing the file would mean one `DO $$ … $$` block costing every table
    /// declared beside it.
    fn parse_chunk(&self, chunk: &str, content: &str, path: &Path, result: &mut ParseResult) {
        let span = span_of(content, chunk);
        match SqlAstParser::parse_sql(self.dialect.as_ref(), chunk) {
            Ok(statements) => {
                for statement in statements {
                    self.emit(&statement, span, result);
                }
            }
            Err(e) => result.add_warning(format!(
                "{}:{}: could not parse statement ({e})",
                path.display(),
                span.start.line + 1
            )),
        }
    }

    /// Restate a statement as schema operations. Emits no entities: what a
    /// table finally looks like is not knowable from one file, so the fold
    /// builds them (ADR-0007).
    fn emit(&self, statement: &Statement, span: Span, result: &mut ParseResult) {
        let ops = match statement {
            Statement::CreateTable(ct) => vec![schema::from_create_table(ct)],
            Statement::CreateView(cv) => vec![schema::from_create_view(cv)],
            Statement::AlterTable(at) => schema::from_alter_table(&at.name, &at.operations),
            Statement::Drop {
                object_type: ObjectType::Table,
                names,
                ..
            } => schema::from_drop_tables(names),
            // Everything else — DML, indexes, grants, procedural blocks — is
            // valid SQL that changes no table topology.
            _ => Vec::new(),
        };
        result
            .schema_ops
            .extend(ops.into_iter().map(|op| ops::PositionedOp { op, span }));
    }
}

/// Locate `chunk` within `content` and build a span for it.
///
/// `split_statements` returns borrowed slices of `content`, so the offset is
/// exact pointer arithmetic rather than a search.
fn span_of(content: &str, chunk: &str) -> Span {
    let offset = chunk.as_ptr() as usize - content.as_ptr() as usize;
    let before = &content[..offset];
    // Count newlines rather than `lines()`: `lines()` does not yield a final
    // empty line after a trailing `\n`, so "a\n\n" counts as 2 and reports the
    // wrong line for anything that follows a blank line.
    let start_line = before.bytes().filter(|b| *b == b'\n').count();
    let start_col = before.len() - before.rfind('\n').map_or(0, |i| i + 1);
    let end_line = start_line + chunk.bytes().filter(|b| *b == b'\n').count();
    let end_col = chunk.len() - chunk.rfind('\n').map_or(0, |i| i + 1);
    Span::new(
        Position::new(start_line, start_col, offset),
        Position::new(end_line, end_col, offset + chunk.len()),
    )
}
