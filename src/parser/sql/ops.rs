//! Schema operations — what one `.sql` file says, before ordering.
//!
//! A migration file is not a statement about the schema; it is a statement
//! about a *change* to the schema. `ALTER TABLE orders ADD COLUMN total` is
//! true of the file regardless of what any other file says, but it says
//! nothing on its own about what `orders` finally looks like.
//!
//! So the parser emits these, and the fold (SQL-002) replays them in order to
//! produce tables. Keeping the parser at this level is what lets its output
//! stay content-hash cacheable per file while the schema still depends on all
//! of them — see ADR-0007.

use crate::models::{Parameter, Span};
use serde::{Deserialize, Serialize};

/// A foreign key, as declared. The target is a name rather than an id
/// because the table it points at is usually declared in another file, and
/// may not have been parsed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    /// Constraint name. Postgres derives `<table>_<column>_fkey` when the
    /// constraint is written inline, and the fold synthesises the same name,
    /// so `DROP CONSTRAINT` can find an inline key by the name the database
    /// would have given it.
    pub name: String,
    pub column: String,
    pub target_schema: String,
    pub target_table: String,
    pub target_column: Option<String>,
    pub on_delete: Option<String>,
}

/// One schema change, in the order it appeared in its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaOp {
    CreateTable {
        schema: String,
        name: String,
        columns: Vec<Parameter>,
        foreign_keys: Vec<ForeignKey>,
        /// `CREATE TABLE IF NOT EXISTS` — must not clobber an existing table.
        if_not_exists: bool,
        is_view: bool,
    },
    AddColumn {
        schema: String,
        table: String,
        column: Parameter,
        foreign_key: Option<ForeignKey>,
    },
    DropColumn {
        schema: String,
        table: String,
        column: String,
    },
    RenameColumn {
        schema: String,
        table: String,
        from: String,
        to: String,
    },
    RenameTable {
        schema: String,
        from: String,
        to: String,
    },
    DropTable {
        schema: String,
        name: String,
    },
    AddForeignKey {
        schema: String,
        table: String,
        foreign_key: ForeignKey,
    },
    DropConstraint {
        schema: String,
        table: String,
        name: String,
    },
}

/// A `SchemaOp` with the position it came from, so the fold can order
/// operations within a file and attribute entities back to their source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionedOp {
    pub op: SchemaOp,
    pub span: Span,
}

impl SchemaOp {
    /// The table this operation acts on, as `(schema, name)`. For a rename
    /// this is the name *before* the rename.
    pub fn subject(&self) -> (&str, &str) {
        match self {
            SchemaOp::CreateTable { schema, name, .. }
            | SchemaOp::DropTable { schema, name }
            | SchemaOp::RenameTable {
                schema, from: name, ..
            } => (schema, name),
            SchemaOp::AddColumn { schema, table, .. }
            | SchemaOp::DropColumn { schema, table, .. }
            | SchemaOp::RenameColumn { schema, table, .. }
            | SchemaOp::AddForeignKey { schema, table, .. }
            | SchemaOp::DropConstraint { schema, table, .. } => (schema, table),
        }
    }
}

/// The name PostgreSQL gives an unnamed foreign key, so an inline
/// `REFERENCES` and an explicit `ADD CONSTRAINT` share one namespace and a
/// later `DROP CONSTRAINT` can remove either.
pub fn implicit_fk_name(table: &str, column: &str) -> String {
    format!("{table}_{column}_fkey")
}
