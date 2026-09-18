//! Translating parsed DDL statements into [`SchemaOp`]s.
//!
//! Nothing here decides what the schema *is* — that is the fold's job. This
//! module only restates each statement in a form the fold can replay.

use super::ops::{
    implicit_fk_name, implicit_pk_name, implicit_unique_name, ForeignKey, SchemaOp, UniqueKey,
};
use crate::models::Parameter;
use sqlparser::ast::{
    AlterTableOperation, ColumnDef, ColumnOption, CreateIndex, CreateTable, CreateView, Expr,
    IndexColumn, ObjectName, RenameTableNameKind, TableConstraint,
};

/// Schema assumed when a name is not qualified. Matches PostgreSQL's default
/// search path, so `users` and `public.users` fold into one table.
pub const DEFAULT_SCHEMA: &str = "public";

/// Split an `ObjectName` into `(schema, name)`, lowercased and unquoted.
///
/// Identifiers are case-insensitive unless quoted, and quoting is applied
/// inconsistently in practice — `public.Users`, `"users"` and `users` all
/// name one table and must normalise together.
pub fn split_name(name: &ObjectName) -> (String, String) {
    let parts: Vec<String> = name
        .0
        .iter()
        .map(|p| p.to_string().trim_matches('"').to_lowercase())
        .collect();
    match parts.as_slice() {
        [] => (DEFAULT_SCHEMA.to_string(), String::new()),
        [only] => (DEFAULT_SCHEMA.to_string(), only.clone()),
        [.., schema, table] => (schema.clone(), table.clone()),
    }
}

fn ident(i: &sqlparser::ast::Ident) -> String {
    i.value.trim_matches('"').to_lowercase()
}

pub fn from_create_table(ct: &CreateTable) -> SchemaOp {
    let (schema, name) = split_name(&ct.name);
    let columns = ct.columns.iter().map(column_field).collect();

    // Inline and table-level forms say the same things, so each kind of
    // constraint is gathered across both rather than once per syntax. The
    // three passes read as three sentences; folding them into one loop with
    // two accumulators saved nothing and hid which form contributes what.
    let mut foreign_keys: Vec<ForeignKey> = ct
        .columns
        .iter()
        .filter_map(|c| column_foreign_key(&name, c))
        .collect();
    foreign_keys.extend(ct.constraints.iter().filter_map(|c| table_foreign_key(&name, c)));

    let mut unique_keys: Vec<UniqueKey> = ct
        .columns
        .iter()
        .filter_map(|c| column_unique_key(&name, c))
        .collect();
    unique_keys.extend(ct.constraints.iter().filter_map(|c| table_unique_key(&name, c)));

    SchemaOp::CreateTable {
        schema,
        name,
        columns,
        foreign_keys,
        unique_keys,
        if_not_exists: ct.if_not_exists,
        is_view: false,
    }
}

/// `CREATE UNIQUE INDEX one_profile ON profiles (user_id)` — uniqueness
/// declared outside the table, which is how a migration adds it to a table
/// that already exists. A non-unique index says nothing about cardinality
/// and produces no operation.
pub fn from_create_index(ci: &CreateIndex) -> Vec<SchemaOp> {
    // A partial index — `… WHERE deleted_at IS NULL` — constrains only the
    // rows matching its predicate. Reading it as unconditional uniqueness
    // would call an edge one-to-one that is one-to-many everywhere outside
    // that subset, which is a worse answer than saying nothing.
    if !ci.unique || ci.predicate.is_some() {
        return Vec::new();
    }
    let (schema, table) = split_name(&ci.table_name);
    let columns = index_columns(&ci.columns);
    if columns.is_empty() {
        // An expression index — `LOWER(email)` — constrains a computed value,
        // not a column set, so no foreign key can be matched against it.
        return Vec::new();
    }
    let name = ci
        .name
        .as_ref()
        .map(|n| split_name(n).1)
        .unwrap_or_else(|| implicit_unique_name(&table, &columns));
    vec![SchemaOp::AddUniqueKey {
        schema,
        table,
        unique_key: UniqueKey {
            name,
            columns,
            is_primary: false,
        },
    }]
}

pub fn from_create_view(cv: &CreateView) -> SchemaOp {
    let (schema, name) = split_name(&cv.name);
    SchemaOp::CreateTable {
        schema,
        name,
        columns: cv
            .columns
            .iter()
            .map(|c| Parameter {
                name: ident(&c.name),
                ..Default::default()
            })
            .collect(),
        foreign_keys: Vec::new(),
        unique_keys: Vec::new(),
        if_not_exists: false,
        is_view: true,
    }
}

/// `DROP TABLE a, b` — one op per name.
pub fn from_drop_tables(names: &[ObjectName]) -> Vec<SchemaOp> {
    names
        .iter()
        .map(|n| {
            let (schema, name) = split_name(n);
            SchemaOp::DropTable { schema, name }
        })
        .collect()
}

/// Translate one `ALTER TABLE` into zero or more ops.
///
/// Operations this does not model (`ALTER COLUMN … SET NOT NULL`, row-level
/// security, ownership) change no table topology and are dropped.
pub fn from_alter_table(name: &ObjectName, operations: &[AlterTableOperation]) -> Vec<SchemaOp> {
    let (schema, table) = split_name(name);
    operations
        .iter()
        .flat_map(|op| alter_op(&schema, &table, op))
        .collect()
}

fn alter_op(schema: &str, table: &str, op: &AlterTableOperation) -> Vec<SchemaOp> {
    match op {
        AlterTableOperation::AddColumn { column_def, .. } => vec![SchemaOp::AddColumn {
            schema: schema.into(),
            table: table.into(),
            column: column_field(column_def),
            foreign_key: column_foreign_key(table, column_def),
        }],
        AlterTableOperation::DropColumn { column_names, .. } => column_names
            .iter()
            .map(|c| SchemaOp::DropColumn {
                schema: schema.into(),
                table: table.into(),
                column: ident(c),
            })
            .collect(),
        AlterTableOperation::RenameColumn {
            old_column_name,
            new_column_name,
        } => vec![SchemaOp::RenameColumn {
            schema: schema.into(),
            table: table.into(),
            from: ident(old_column_name),
            to: ident(new_column_name),
        }],
        AlterTableOperation::RenameTable { table_name } => {
            let renamed = match table_name {
                RenameTableNameKind::To(o) | RenameTableNameKind::As(o) => o,
            };
            vec![SchemaOp::RenameTable {
                schema: schema.into(),
                from: table.into(),
                to: split_name(renamed).1,
            }]
        }
        AlterTableOperation::AddConstraint { constraint, .. } => {
            added_constraint(schema, table, constraint)
        }
        AlterTableOperation::DropConstraint { name, .. } => vec![SchemaOp::DropConstraint {
            schema: schema.into(),
            table: table.into(),
            name: ident(name),
        }],
        _ => Vec::new(),
    }
}

/// A column, as a `Parameter` — the same shape struct fields use, so
/// `field_count` and the detail panel need no special-casing.
fn column_field(column: &ColumnDef) -> Parameter {
    Parameter {
        name: ident(&column.name),
        type_name: Some(column.data_type.to_string()),
        default_value: column.options.iter().find_map(|o| match &o.option {
            ColumnOption::Default(expr) => Some(expr.to_string()),
            _ => None,
        }),
        visibility: None,
    }
}

/// `col UUID REFERENCES users(user_id)` — the inline form, and the majority
/// of foreign keys in practice.
fn column_foreign_key(table: &str, column: &ColumnDef) -> Option<ForeignKey> {
    let fk = column.options.iter().find_map(|o| match &o.option {
        ColumnOption::ForeignKey(fk) => Some(fk),
        _ => None,
    })?;
    let columns = vec![ident(&column.name)];
    let (target_schema, target_table) = split_name(&fk.foreign_table);
    Some(ForeignKey {
        name: fk
            .name
            .as_ref()
            .map(ident)
            .unwrap_or_else(|| implicit_fk_name(table, &columns)),
        columns,
        target_schema,
        target_table,
        target_columns: fk.referred_columns.iter().map(ident).collect(),
        on_delete: fk.on_delete.map(|a| a.to_string()),
    })
}

/// `FOREIGN KEY (col) REFERENCES users(user_id)`, including the form
/// `ALTER TABLE … ADD CONSTRAINT` produces.
fn table_foreign_key(table: &str, constraint: &TableConstraint) -> Option<ForeignKey> {
    let TableConstraint::ForeignKey(fk) = constraint else {
        return None;
    };
    let columns: Vec<String> = fk.columns.iter().map(ident).collect();
    if columns.is_empty() {
        return None;
    }
    let (target_schema, target_table) = split_name(&fk.foreign_table);
    Some(ForeignKey {
        name: fk
            .name
            .as_ref()
            .map(ident)
            .unwrap_or_else(|| implicit_fk_name(table, &columns)),
        columns,
        target_schema,
        target_table,
        target_columns: fk.referred_columns.iter().map(ident).collect(),
        on_delete: fk.on_delete.map(|a| a.to_string()),
    })
}

/// `ADD CONSTRAINT …` — a foreign key or a uniqueness constraint, whichever
/// it turns out to be.
///
/// Its own function rather than two arms in [`alter_op`]: that match is the
/// list of statements this module models, and a reader scanning it should see
/// one line per statement rather than the body of the one that happens to
/// carry two.
fn added_constraint(schema: &str, table: &str, constraint: &TableConstraint) -> Vec<SchemaOp> {
    if let Some(fk) = table_foreign_key(table, constraint) {
        return vec![SchemaOp::AddForeignKey {
            schema: schema.into(),
            table: table.into(),
            foreign_key: fk,
        }];
    }
    table_unique_key(table, constraint)
        .map(|unique_key| SchemaOp::AddUniqueKey {
            schema: schema.into(),
            table: table.into(),
            unique_key,
        })
        .into_iter()
        .collect()
}

/// The column names an index or key constraint is written over.
///
/// `IndexColumn` wraps an `OrderByExpr`, so `(created_at DESC)` and
/// `(LOWER(email))` arrive in the same shape as a plain column. Only bare
/// identifiers answer; anything computed is dropped by the caller, because a
/// foreign key can never be matched against an expression.
fn index_columns(columns: &[IndexColumn]) -> Vec<String> {
    columns
        .iter()
        .filter_map(|c| match &c.column.expr {
            Expr::Identifier(i) => Some(ident(i)),
            Expr::CompoundIdentifier(parts) => parts.last().map(ident),
            _ => None,
        })
        .collect()
}

/// `id UUID PRIMARY KEY` / `email TEXT UNIQUE` — the inline forms.
fn column_unique_key(table: &str, column: &ColumnDef) -> Option<UniqueKey> {
    let local = vec![ident(&column.name)];
    column.options.iter().find_map(|o| match &o.option {
        ColumnOption::PrimaryKey(pk) => Some(UniqueKey {
            name: pk
                .name
                .as_ref()
                .map(ident)
                .unwrap_or_else(|| implicit_pk_name(table)),
            columns: local.clone(),
            is_primary: true,
        }),
        ColumnOption::Unique(u) => Some(UniqueKey {
            name: u
                .name
                .as_ref()
                .map(ident)
                .unwrap_or_else(|| implicit_unique_name(table, &local)),
            columns: local.clone(),
            is_primary: false,
        }),
        _ => None,
    })
}

/// `PRIMARY KEY (a, b)` / `UNIQUE (a, b)`, including the `ALTER TABLE … ADD
/// CONSTRAINT` form. Composite by nature — which is the whole reason a join
/// table can be recognised at all.
fn table_unique_key(table: &str, constraint: &TableConstraint) -> Option<UniqueKey> {
    match constraint {
        TableConstraint::PrimaryKey(pk) => Some(UniqueKey {
            name: pk
                .name
                .as_ref()
                .map(ident)
                .unwrap_or_else(|| implicit_pk_name(table)),
            columns: index_columns(&pk.columns),
            is_primary: true,
        }),
        TableConstraint::Unique(u) => {
            let columns = index_columns(&u.columns);
            Some(UniqueKey {
                name: u
                    .name
                    .as_ref()
                    .map(ident)
                    .unwrap_or_else(|| implicit_unique_name(table, &columns)),
                columns,
                is_primary: false,
            })
        }
        _ => None,
    }
    .filter(|key| !key.columns.is_empty())
}
