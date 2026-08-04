//! Translating parsed DDL statements into [`SchemaOp`]s.
//!
//! Nothing here decides what the schema *is* — that is the fold's job. This
//! module only restates each statement in a form the fold can replay.

use super::ops::{implicit_fk_name, ForeignKey, SchemaOp};
use crate::models::Parameter;
use sqlparser::ast::{
    AlterTableOperation, ColumnDef, ColumnOption, CreateTable, CreateView, ObjectName,
    RenameTableNameKind, TableConstraint,
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
    let mut columns = Vec::new();
    let mut foreign_keys = Vec::new();

    for column in &ct.columns {
        columns.push(column_field(column));
        if let Some(fk) = column_foreign_key(&name, column) {
            foreign_keys.push(fk);
        }
    }
    for constraint in &ct.constraints {
        if let Some(fk) = table_foreign_key(&name, constraint) {
            foreign_keys.push(fk);
        }
    }

    SchemaOp::CreateTable {
        schema,
        name,
        columns,
        foreign_keys,
        if_not_exists: ct.if_not_exists,
        is_view: false,
    }
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
            table_foreign_key(table, constraint)
                .map(|fk| SchemaOp::AddForeignKey {
                    schema: schema.into(),
                    table: table.into(),
                    foreign_key: fk,
                })
                .into_iter()
                .collect()
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
    let local = ident(&column.name);
    let (target_schema, target_table) = split_name(&fk.foreign_table);
    Some(ForeignKey {
        name: fk
            .name
            .as_ref()
            .map(ident)
            .unwrap_or_else(|| implicit_fk_name(table, &local)),
        column: local,
        target_schema,
        target_table,
        target_column: fk.referred_columns.first().map(ident),
        on_delete: fk.on_delete.map(|a| a.to_string()),
    })
}

/// `FOREIGN KEY (col) REFERENCES users(user_id)`, including the form
/// `ALTER TABLE … ADD CONSTRAINT` produces.
fn table_foreign_key(table: &str, constraint: &TableConstraint) -> Option<ForeignKey> {
    let TableConstraint::ForeignKey(fk) = constraint else {
        return None;
    };
    let local = ident(fk.columns.first()?);
    let (target_schema, target_table) = split_name(&fk.foreign_table);
    Some(ForeignKey {
        name: fk
            .name
            .as_ref()
            .map(ident)
            .unwrap_or_else(|| implicit_fk_name(table, &local)),
        column: local,
        target_schema,
        target_table,
        target_column: fk.referred_columns.first().map(ident),
        on_delete: fk.on_delete.map(|a| a.to_string()),
    })
}
