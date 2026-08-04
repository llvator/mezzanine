//! Fold ordered SQL migrations into the effective schema (SQL-002).
//!
//! A migration repo never writes its schema down. The schema is the *result*
//! of applying every migration in order — in the corpus this was built
//! against, `ALTER TABLE` outnumbers `CREATE TABLE` more than two to one, so
//! reading the files as an unordered set describes a database that never
//! existed: dropped tables still present, renamed tables under every name
//! they ever had, and columns attached to nothing.
//!
//! This is the one place in Nao where file order carries meaning. ADR-0007
//! records why it is confined here rather than pushed into the parser: the
//! parser's per-file output stays content-hash cacheable, so editing one
//! migration re-runs this replay and nothing else.
//!
//! Order is filename order — the convention sqlx, Flyway and Alembic all
//! define (`20260228000001_initial_schema.sql`).

use crate::models::{CodeEntity, EntityKind, Parameter, Relationship, RelationshipKind, Span};
use crate::parser::sql::ops::{ForeignKey, SchemaOp};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Stable, file-independent id for a table or view.
pub fn table_id(schema: &str, name: &str) -> String {
    format!("sql::table.{schema}.{name}")
}

/// One table as the replay currently understands it.
#[derive(Debug, Clone)]
struct Table {
    schema: String,
    name: String,
    /// Insertion-ordered: columns should read in declaration order, and a
    /// renamed column should keep its position rather than jumping.
    columns: Vec<Parameter>,
    /// Keyed by constraint name so `DROP CONSTRAINT` can remove precisely.
    foreign_keys: BTreeMap<String, ForeignKey>,
    is_view: bool,
    /// The migration that created it, and where — the entity's home.
    origin: PathBuf,
    span: Span,
    /// Names this table previously had, oldest first.
    former_names: Vec<String>,
}

/// The result of replaying every operation.
pub struct FoldedSchema {
    /// Entities keyed by the file that created them, so each table is
    /// attributed to the migration it came from.
    pub entities: Vec<CodeEntity>,
    pub relationships: Vec<Relationship>,
    /// Tables that were created and later dropped. Not emitted as entities —
    /// they do not exist — but counted so callers can report them.
    pub dropped: Vec<String>,
    /// `ALTER`s naming a table no migration creates. Non-zero means either an
    /// ordering bug or a table owned by something outside the migration set.
    pub alters_on_unknown_tables: usize,
}

/// One file's operations, already in statement order.
pub struct FileOps<'a> {
    pub path: &'a Path,
    pub ops: &'a [crate::parser::sql::ops::PositionedOp],
}

/// Replay `files` in filename order and return the resulting schema.
pub fn fold(mut files: Vec<FileOps<'_>>) -> FoldedSchema {
    files.sort_by(|a, b| a.path.cmp(b.path));

    let mut state = FoldState::default();
    for file in &files {
        for positioned in file.ops {
            state.apply(&positioned.op, file.path, positioned.span);
        }
    }
    state.finish()
}

#[derive(Default)]
struct FoldState {
    /// Keyed by `(schema, name)` as it stands *now*, so a rename moves the
    /// entry rather than duplicating it.
    tables: BTreeMap<(String, String), Table>,
    dropped: Vec<String>,
    alters_on_unknown_tables: usize,
}

impl FoldState {
    fn apply(&mut self, op: &SchemaOp, path: &Path, span: Span) {
        match op {
            SchemaOp::CreateTable { .. } => self.create(op, path, span),
            SchemaOp::DropTable { schema, name } => self.drop_table(schema, name),
            SchemaOp::RenameTable { schema, from, to } => self.rename_table(schema, from, to),
            _ => self.mutate(op, path, span),
        }
    }

    fn create(&mut self, op: &SchemaOp, path: &Path, span: Span) {
        let SchemaOp::CreateTable {
            schema,
            name,
            columns,
            foreign_keys,
            if_not_exists,
            is_view,
        } = op
        else {
            return;
        };
        let key = (schema.clone(), name.clone());
        if *if_not_exists && self.tables.contains_key(&key) {
            return;
        }
        self.dropped.retain(|d| d != &table_id(schema, name));
        self.tables.insert(
            key,
            Table {
                schema: schema.clone(),
                name: name.clone(),
                columns: columns.clone(),
                foreign_keys: foreign_keys
                    .iter()
                    .map(|fk| (fk.name.clone(), fk.clone()))
                    .collect(),
                is_view: *is_view,
                origin: path.to_path_buf(),
                span,
                former_names: Vec::new(),
            },
        );
    }

    fn drop_table(&mut self, schema: &str, name: &str) {
        let key = (schema.to_string(), name.to_string());
        if self.tables.remove(&key).is_none() {
            return;
        }
        self.dropped.push(table_id(schema, name));
        // Every foreign key pointing at it goes too, or the graph keeps edges
        // to a table that no longer exists.
        for table in self.tables.values_mut() {
            table
                .foreign_keys
                .retain(|_, fk| !(fk.target_schema == schema && fk.target_table == name));
        }
    }

    fn rename_table(&mut self, schema: &str, from: &str, to: &str) {
        let key = (schema.to_string(), from.to_string());
        let Some(mut table) = self.tables.remove(&key) else {
            self.alters_on_unknown_tables += 1;
            return;
        };
        table.former_names.push(from.to_string());
        table.name = to.to_string();
        self.tables.insert((schema.to_string(), to.to_string()), table);

        // Inbound foreign keys must follow the rename, or they dangle.
        for table in self.tables.values_mut() {
            for fk in table.foreign_keys.values_mut() {
                if fk.target_schema == schema && fk.target_table == from {
                    fk.target_table = to.to_string();
                }
            }
        }
    }

    /// Column- and constraint-level changes, which all need the table to
    /// exist first.
    fn mutate(&mut self, op: &SchemaOp, path: &Path, span: Span) {
        let (schema, name) = op.subject();
        let key = (schema.to_string(), name.to_string());
        if !self.tables.contains_key(&key) {
            // A table created outside the migration set — an extension's, or
            // a hosted platform's. Materialise it rather than dropping the
            // operation, so the columns it gains are still visible.
            self.alters_on_unknown_tables += 1;
            self.tables.insert(
                key.clone(),
                Table {
                    schema: schema.to_string(),
                    name: name.to_string(),
                    columns: Vec::new(),
                    foreign_keys: BTreeMap::new(),
                    is_view: false,
                    origin: path.to_path_buf(),
                    span,
                    former_names: Vec::new(),
                },
            );
        }
        let table = self.tables.get_mut(&key).expect("just inserted");
        apply_to_table(table, op);
    }

    fn finish(self) -> FoldedSchema {
        let live: Vec<&Table> = self.tables.values().collect();
        let mut entities = Vec::with_capacity(live.len());
        let mut relationships = Vec::new();

        for table in live {
            entities.push(build_entity(table));
            let source = table_id(&table.schema, &table.name);
            for fk in table.foreign_keys.values() {
                relationships.push(build_edge(&source, fk));
            }
        }

        FoldedSchema {
            entities,
            relationships,
            dropped: self.dropped,
            alters_on_unknown_tables: self.alters_on_unknown_tables,
        }
    }
}

fn apply_to_table(table: &mut Table, op: &SchemaOp) {
    match op {
        SchemaOp::AddColumn {
            column,
            foreign_key,
            ..
        } => add_column(table, column, foreign_key.as_ref()),
        SchemaOp::DropColumn { column, .. } => drop_column(table, column),
        SchemaOp::RenameColumn { from, to, .. } => rename_column(table, from, to),
        SchemaOp::AddForeignKey { foreign_key, .. } => {
            table
                .foreign_keys
                .insert(foreign_key.name.clone(), foreign_key.clone());
        }
        SchemaOp::DropConstraint { name, .. } => {
            table.foreign_keys.remove(name);
        }
        // Table-level operations are handled by the caller, which needs to
        // move entries between keys rather than edit one in place.
        _ => {}
    }
}

fn add_column(table: &mut Table, column: &Parameter, foreign_key: Option<&ForeignKey>) {
    // `ADD COLUMN IF NOT EXISTS` re-run, or a re-add after a drop — the
    // column must not appear twice.
    match table.columns.iter_mut().find(|c| c.name == column.name) {
        Some(existing) => *existing = column.clone(),
        None => table.columns.push(column.clone()),
    }
    if let Some(fk) = foreign_key {
        table.foreign_keys.insert(fk.name.clone(), fk.clone());
    }
}

fn drop_column(table: &mut Table, column: &str) {
    table.columns.retain(|c| c.name != column);
    // A foreign key cannot outlive the column it constrains.
    table.foreign_keys.retain(|_, fk| fk.column != column);
}

fn rename_column(table: &mut Table, from: &str, to: &str) {
    if let Some(c) = table.columns.iter_mut().find(|c| c.name == from) {
        c.name = to.to_string();
    }
    for fk in table.foreign_keys.values_mut() {
        if fk.column == from {
            fk.column = to.to_string();
        }
    }
}

fn build_entity(table: &Table) -> CodeEntity {
    let kind = if table.is_view {
        EntityKind::View
    } else {
        EntityKind::Table
    };
    let mut entity = CodeEntity::new(table.name.clone(), kind, &table.origin, table.span);
    entity.id = table_id(&table.schema, &table.name);
    entity.qualified_name = format!("{}.{}", table.schema, table.name);
    entity.metrics.field_count = Some(table.columns.len() as u32);
    entity.fields = table.columns.clone();
    if !table.former_names.is_empty() {
        // Provenance a reader cannot get from the final schema: this table
        // is the same one that used to be called something else.
        entity.documentation = Some(format!("Formerly: {}", table.former_names.join(", ")));
        entity.tags.insert("renamed".to_string());
    }
    entity
}

fn build_edge(source_id: &str, fk: &ForeignKey) -> Relationship {
    let mut rel = Relationship::new(
        source_id,
        table_id(&fk.target_schema, &fk.target_table),
        RelationshipKind::References,
    )
    .with_label(format!("{} →", fk.column))
    .with_metadata("fk_column", fk.column.clone());

    if let Some(target) = &fk.target_column {
        rel = rel.with_metadata("fk_target_column", target.clone());
    }
    if let Some(action) = &fk.on_delete {
        rel = rel.with_metadata("on_delete", action.clone());
    }
    rel
}
