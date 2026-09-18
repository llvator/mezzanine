//! Fold ordered SQL migrations into the effective schema (SQL-002).
//!
//! A migration repo never writes its schema down. The schema is the *result*
//! of applying every migration in order — in the corpus this was built
//! against, `ALTER TABLE` outnumbers `CREATE TABLE` more than two to one, so
//! reading the files as an unordered set describes a database that never
//! existed: dropped tables still present, renamed tables under every name
//! they ever had, and columns attached to nothing.
//!
//! This is the one place in Mezzanine where file order carries meaning. ADR-0007
//! records why it is confined here rather than pushed into the parser: the
//! parser's per-file output stays content-hash cacheable, so editing one
//! migration re-runs this replay and nothing else.
//!
//! Order is filename order — the convention sqlx, Flyway and Alembic all
//! define (`20260228000001_initial_schema.sql`).

use super::sql_cardinality::{self, Cardinality, Junction, TableKeys};
use crate::models::{CodeEntity, EntityKind, Parameter, Relationship, RelationshipKind, Span};
use crate::parser::sql::ops::{ForeignKey, SchemaOp, UniqueKey};
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
    /// `PRIMARY KEY` and `UNIQUE`, keyed the same way and for the same
    /// reason. Kept because uniqueness on the referencing side is what
    /// decides a foreign key's cardinality — see [`super::sql_cardinality`].
    unique_keys: BTreeMap<String, UniqueKey>,
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

    /// Where a `CREATE TABLE` lands: the guard that decides *whether* it
    /// does. What the resulting table looks like is [`new_table`]'s
    /// business — holding both here put every column of the statement in
    /// view beside the map it is going into, which is one name too many.
    fn create(&mut self, op: &SchemaOp, path: &Path, span: Span) {
        let SchemaOp::CreateTable {
            schema,
            name,
            if_not_exists,
            ..
        } = op
        else {
            return;
        };
        let key = (schema.clone(), name.clone());
        if *if_not_exists && self.tables.contains_key(&key) {
            return;
        }
        self.dropped.retain(|d| d != &table_id(schema, name));
        // `extend` over the option rather than an `if let`: the `None` arm is
        // unreachable — the destructure above already proved this is a
        // `CreateTable` — and writing it as a branch would put a second
        // decision in a function whose only real one is the guard.
        self.tables
            .extend(new_table(op, path, span).map(|table| (key, table)));
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
        self.tables
            .insert((schema.to_string(), to.to_string()), table);

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
                    unique_keys: BTreeMap::new(),
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
        // Cardinality is a property of the *finished* schema, so the key
        // shapes are taken once here rather than maintained during the
        // replay: a `UNIQUE` added by the last migration is as binding as
        // one written in the first.
        let keys: Vec<TableKeys> = live.iter().map(|t| key_shape(t)).collect();

        FoldedSchema {
            entities: live.iter().map(|t| build_entity(t)).collect(),
            relationships: build_edges(&live, &keys),
            dropped: self.dropped,
            alters_on_unknown_tables: self.alters_on_unknown_tables,
        }
    }
}

/// Every edge the finished schema implies: one per foreign key, plus one per
/// join table for the many-to-many its two keys spell out together.
fn build_edges(live: &[&Table], keys: &[TableKeys]) -> Vec<Relationship> {
    let by_id: BTreeMap<&str, &TableKeys> = keys.iter().map(|k| (k.id.as_str(), k)).collect();
    let mut edges: Vec<Relationship> = Vec::new();
    for (table, source) in live.iter().zip(keys) {
        for fk in table.foreign_keys.values() {
            let target = by_id
                .get(table_id(&fk.target_schema, &fk.target_table).as_str())
                .copied();
            edges.push(build_edge(source, fk, target));
        }
    }
    edges.extend(sql_cardinality::junctions(keys).iter().map(build_junction_edge));
    edges
}

/// The constraints of one table, in the form the cardinality rules read.
fn key_shape(table: &Table) -> TableKeys {
    TableKeys {
        id: table_id(&table.schema, &table.name),
        name: table.name.clone(),
        is_view: table.is_view,
        foreign_keys: table.foreign_keys.values().cloned().collect(),
        unique_keys: table.unique_keys.values().cloned().collect(),
    }
}

/// The table a `CREATE TABLE` declares, as the fold will hold it.
///
/// Answers `None` for any other operation, which is the same guard
/// [`FoldState::create`] already applies — restated rather than threaded
/// through seven arguments, because seven is past the bar this repo draws
/// for a parameter list.
fn new_table(op: &SchemaOp, path: &Path, span: Span) -> Option<Table> {
    let SchemaOp::CreateTable {
        schema,
        name,
        columns,
        foreign_keys,
        unique_keys,
        is_view,
        ..
    } = op
    else {
        return None;
    };
    Some(Table {
        schema: schema.clone(),
        name: name.clone(),
        columns: columns.clone(),
        foreign_keys: keyed_foreign_keys(foreign_keys),
        unique_keys: keyed_unique_keys(unique_keys),
        is_view: *is_view,
        origin: path.to_path_buf(),
        span,
        former_names: Vec::new(),
    })
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
        // Constraints next door: they name a thing rather than reshape the
        // column list, and both kinds live in one namespace.
        other => apply_constraint(table, other),
    }
}

/// The named constraints — foreign keys and uniqueness — which share one
/// namespace so a `DROP CONSTRAINT` can find either by the name the database
/// would have given it.
///
/// Table-level operations reach here too and do nothing: they are handled by
/// the caller, which needs to move entries between keys rather than edit one
/// in place.
fn apply_constraint(table: &mut Table, op: &SchemaOp) {
    match op {
        SchemaOp::AddForeignKey { foreign_key, .. } => {
            table
                .foreign_keys
                .insert(foreign_key.name.clone(), foreign_key.clone());
        }
        SchemaOp::AddUniqueKey { unique_key, .. } => {
            table
                .unique_keys
                .insert(unique_key.name.clone(), unique_key.clone());
        }
        SchemaOp::DropConstraint { name, .. } => {
            table.foreign_keys.remove(name);
            table.unique_keys.remove(name);
        }
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
    drop_keys_over(table, column);
}

/// Neither kind of key can outlive a column it constrains. A composite key
/// that loses one column stops constraining what it used to, so the whole
/// constraint goes rather than a shortened version of it that would claim a
/// uniqueness the database never had.
fn drop_keys_over(table: &mut Table, column: &str) {
    table.foreign_keys.retain(|_, fk| !covers(&fk.columns, column));
    table.unique_keys.retain(|_, k| !covers(&k.columns, column));
}

fn covers(columns: &[String], column: &str) -> bool {
    columns.iter().any(|c| c == column)
}

fn rename_column(table: &mut Table, from: &str, to: &str) {
    if let Some(c) = table.columns.iter_mut().find(|c| c.name == from) {
        c.name = to.to_string();
    }
    rename_in_keys(table, from, to);
}

/// A rename has to reach every constraint written over the column, or the
/// key still names a column that no longer exists.
fn rename_in_keys(table: &mut Table, from: &str, to: &str) {
    for fk in table.foreign_keys.values_mut() {
        rename_in(&mut fk.columns, from, to);
    }
    for key in table.unique_keys.values_mut() {
        rename_in(&mut key.columns, from, to);
    }
}

fn rename_in(columns: &mut [String], from: &str, to: &str) {
    for c in columns.iter_mut().filter(|c| *c == from) {
        *c = to.to_string();
    }
}

/// Constraints keyed by name, which is the shape `DROP CONSTRAINT` needs and
/// the shape the fold keeps them in.
fn keyed_foreign_keys(keys: &[ForeignKey]) -> BTreeMap<String, ForeignKey> {
    keys.iter().map(|k| (k.name.clone(), k.clone())).collect()
}

fn keyed_unique_keys(keys: &[UniqueKey]) -> BTreeMap<String, UniqueKey> {
    keys.iter().map(|k| (k.name.clone(), k.clone())).collect()
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

/// A column list as it reads on a label: `user_id`, or `(tenant_id, user_id)`
/// once there is more than one and the grouping is the point.
fn key_text(columns: &[String]) -> String {
    match columns {
        [single] => single.clone(),
        many => format!("({})", many.join(", ")),
    }
}

/// What the edge says on the canvas: the join, and its shape.
///
/// Those are the two things a reader is looking at a schema edge to learn
/// (SQL-006). An unresolved target — a table outside the migration set,
/// referenced without naming its columns — leaves the right-hand side off
/// rather than guessing `id`.
fn edge_label(columns: &[String], referenced: &[String], cardinality: Cardinality) -> String {
    match referenced.is_empty() {
        true => format!("{} → ({})", key_text(columns), cardinality.short()),
        false => format!(
            "{} → {} ({})",
            key_text(columns),
            key_text(referenced),
            cardinality.short()
        ),
    }
}

fn build_edge(source: &TableKeys, fk: &ForeignKey, target: Option<&TableKeys>) -> Relationship {
    let cardinality = sql_cardinality::of(source, fk);
    let referenced = sql_cardinality::target_columns(fk, target);

    let mut rel = Relationship::new(
        &source.id,
        table_id(&fk.target_schema, &fk.target_table),
        RelationshipKind::References,
    )
    .with_label(edge_label(&fk.columns, &referenced, cardinality))
    .with_metadata("fk_columns", fk.columns.join(", "))
    .with_metadata("fk_constraint", fk.name.clone())
    .with_metadata("cardinality", cardinality.tag());

    if !referenced.is_empty() {
        rel = rel.with_metadata("fk_target_columns", referenced.join(", "));
    }
    if let Some(action) = &fk.on_delete {
        rel = rel.with_metadata("on_delete", action.clone());
    }
    rel
}

/// The many-to-many a join table stands for, as one edge between the two
/// tables it joins.
///
/// The join table keeps its own node and its own two foreign-key edges — this
/// is drawn *beside* them, not instead of them. It is the one edge in the SQL
/// graph that no statement declares, so it is marked `inferred` and names the
/// table it was read from, and a reader who disagrees can see exactly what
/// the claim rests on.
fn build_junction_edge(junction: &Junction) -> Relationship {
    let [left, right] = &junction.sides;
    let mut rel = Relationship::new(
        &left.table_id,
        &right.table_id,
        RelationshipKind::References,
    );
    // A distinct id: the default is built from the endpoints and the kind
    // alone, which two join tables over one pair of tables — or a join table
    // beside a direct foreign key — would collide on.
    rel.id = format!("{}->{}:ManyToMany:{}", left.table_id, right.table_id, junction.table_id);
    rel.with_label(format!(
        "{} via {}",
        Cardinality::ManyToMany.short(),
        junction.table_name
    ))
    .with_metadata("cardinality", Cardinality::ManyToMany.tag())
    .with_metadata("junction", junction.table_id.clone())
    .with_metadata("junction_table", junction.table_name.clone())
    .with_metadata("junction_source_columns", left.columns.join(", "))
    .with_metadata("junction_target_columns", right.columns.join(", "))
    .with_metadata("inferred", "true")
}
