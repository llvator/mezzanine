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
    /// Every column that participates, in declaration order — `(tenant_id,
    /// user_id)` is one key, not two. Stored as a list rather than the first
    /// column because a reader shown `tenant_id →` for a composite key is
    /// being told something false about how the two tables join (SQL-006).
    pub columns: Vec<String>,
    pub target_schema: String,
    pub target_table: String,
    /// The referenced columns, positionally matched to `columns`. Empty for
    /// the bare `REFERENCES users` form, where SQL means "the target's
    /// primary key" — a fact only the fold can resolve, since the target is
    /// usually declared in another file.
    pub target_columns: Vec<String>,
    pub on_delete: Option<String>,
}

/// A uniqueness constraint — `PRIMARY KEY`, `UNIQUE`, or a `CREATE UNIQUE
/// INDEX` — recorded because it is what separates a one-to-one from a
/// one-to-many (SQL-006).
///
/// A foreign key says two tables are joined. Only uniqueness on the
/// *referencing* side says how many rows may sit at that end, and without it
/// every edge in the schema looks the same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniqueKey {
    /// Constraint name, so `DROP CONSTRAINT` can remove it. Synthesised the
    /// way Postgres would for the inline forms — see [`implicit_unique_name`]
    /// and [`implicit_pk_name`].
    pub name: String,
    pub columns: Vec<String>,
    /// Whether this is the table's primary key. Kept apart from an ordinary
    /// unique constraint because the bare `REFERENCES users` form resolves
    /// against the primary key specifically.
    pub is_primary: bool,
}

/// One schema change, in the order it appeared in its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaOp {
    CreateTable {
        schema: String,
        name: String,
        columns: Vec<Parameter>,
        foreign_keys: Vec<ForeignKey>,
        unique_keys: Vec<UniqueKey>,
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
    /// `ADD CONSTRAINT … PRIMARY KEY/UNIQUE`, and `CREATE UNIQUE INDEX` —
    /// which is the same statement about the schema written outside the
    /// table, and the form migrations reach for once a table exists.
    AddUniqueKey {
        schema: String,
        table: String,
        unique_key: UniqueKey,
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
            | SchemaOp::AddUniqueKey { schema, table, .. }
            | SchemaOp::DropConstraint { schema, table, .. } => (schema, table),
        }
    }
}

/// The name PostgreSQL gives an unnamed foreign key, so an inline
/// `REFERENCES` and an explicit `ADD CONSTRAINT` share one namespace and a
/// later `DROP CONSTRAINT` can remove either.
pub fn implicit_fk_name(table: &str, columns: &[String]) -> String {
    format!("{table}_{}_fkey", columns.join("_"))
}

/// The name PostgreSQL gives an unnamed unique constraint, for the same
/// reason [`implicit_fk_name`] exists: `email TEXT UNIQUE` and a later
/// `DROP CONSTRAINT users_email_key` have to name one thing.
pub fn implicit_unique_name(table: &str, columns: &[String]) -> String {
    format!("{table}_{}_key", columns.join("_"))
}

/// The name PostgreSQL gives a primary key. One per table, so the columns
/// play no part — which is also what makes a later `PRIMARY KEY` replace an
/// earlier one in the fold's map rather than sitting beside it.
pub fn implicit_pk_name(table: &str) -> String {
    format!("{table}_pkey")
}
