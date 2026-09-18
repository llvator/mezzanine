//! Reading cardinality off a folded schema (SQL-006).
//!
//! A foreign key says two tables are joined. It does not say how many rows
//! sit at either end — and that is the half a reader actually wants, because
//! "an order has many items" and "a user has one profile" are different
//! shapes drawn with the same arrow.
//!
//! The missing fact is **uniqueness on the referencing side**. `profiles
//! .user_id REFERENCES users(id)` is one-to-one exactly when `profiles
//! .user_id` is itself unique; without that constraint the same user may
//! appear on many rows and the edge is many-to-one. So this module needs
//! nothing but the key constraints of each table, which is what
//! [`TableKeys`] carries.
//!
//! Its own module rather than more of [`super::sql_fold`]: the fold answers
//! *what the schema is* by replaying migrations in order, and that is already
//! the one place in Mezzanine where file order carries meaning. What the
//! finished schema *implies* is a separate question, decided from the result
//! and testable without replaying anything.

use crate::parser::sql::ops::{ForeignKey, UniqueKey};
use std::collections::{BTreeMap, BTreeSet};

/// How many rows may sit at each end of a foreign key.
///
/// Written from the referencing table's point of view, which is the direction
/// the edge is drawn in: `orders.user_id → users` is [`ManyToOne`] because
/// many orders point at one user.
///
/// [`ManyToOne`]: Cardinality::ManyToOne
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// The referencing columns are themselves unique, so at most one row
    /// here can point at any given row there.
    OneToOne,
    /// The default, and the shape of most foreign keys: nothing stops two
    /// rows from naming the same target.
    ManyToOne,
    /// Neither side constrains the other — the join lives in a third table.
    /// Never read off a single key; see [`junctions`].
    ManyToMany,
}

impl Cardinality {
    /// The compact spelling that fits on an edge label.
    pub fn short(self) -> &'static str {
        match self {
            Cardinality::OneToOne => "1:1",
            Cardinality::ManyToOne => "N:1",
            Cardinality::ManyToMany => "N:M",
        }
    }

    /// The spelling for a tooltip, where there is room to be unambiguous —
    /// `N:1` is easy to read in the wrong direction at a glance.
    pub fn describe(self) -> &'static str {
        match self {
            Cardinality::OneToOne => "one-to-one",
            Cardinality::ManyToOne => "many-to-one",
            Cardinality::ManyToMany => "many-to-many",
        }
    }

    /// The lowercase form used as relationship metadata, so a consumer
    /// filters on a stable token rather than on display text.
    pub fn tag(self) -> &'static str {
        match self {
            Cardinality::OneToOne => "1:1",
            Cardinality::ManyToOne => "n:1",
            Cardinality::ManyToMany => "n:m",
        }
    }
}

/// One table, reduced to the constraints that decide cardinality.
///
/// A clone rather than a borrow of the fold's own `Table`: a schema has tens
/// of tables where a codebase has thousands of functions, and the lifetime
/// knot a borrow would tie through two maps costs more to read than the
/// copies cost to make.
#[derive(Debug, Clone)]
pub struct TableKeys {
    pub id: String,
    pub name: String,
    pub is_view: bool,
    pub foreign_keys: Vec<ForeignKey>,
    pub unique_keys: Vec<UniqueKey>,
}

impl TableKeys {
    /// The primary key's columns, or empty when the table declares none.
    /// This is what the bare `REFERENCES users` form means by "the key".
    pub fn primary_key(&self) -> &[String] {
        self.unique_keys
            .iter()
            .find(|k| k.is_primary)
            .map_or(&[], |k| k.columns.as_slice())
    }

    /// Whether some `UNIQUE` or `PRIMARY KEY` covers exactly `columns`.
    ///
    /// Exactly, not merely includes: `UNIQUE (user_id, slug)` permits many
    /// rows per `user_id`, so it does not make a key on `user_id` alone
    /// one-to-one. Compared as sets because column order within a
    /// constraint carries no meaning for this question.
    fn is_unique_over(&self, columns: &[String]) -> bool {
        let wanted: BTreeSet<&String> = columns.iter().collect();
        self.unique_keys
            .iter()
            .any(|k| k.columns.iter().collect::<BTreeSet<_>>() == wanted)
    }
}

/// The cardinality of `fk`, read from the table that declares it.
pub fn of(source: &TableKeys, fk: &ForeignKey) -> Cardinality {
    if source.is_unique_over(&fk.columns) {
        Cardinality::OneToOne
    } else {
        Cardinality::ManyToOne
    }
}

/// The columns `fk` points at, resolving the bare `REFERENCES users` form
/// against the target's primary key.
///
/// Empty when the target is outside the migration set and declares no key
/// here — an honest gap, and better on a label than a guessed `id`.
pub fn target_columns(fk: &ForeignKey, target: Option<&TableKeys>) -> Vec<String> {
    if !fk.target_columns.is_empty() {
        return fk.target_columns.clone();
    }
    target.map(|t| t.primary_key().to_vec()).unwrap_or_default()
}

/// A join table, and the two tables it joins.
pub struct Junction {
    /// The join table itself, which stays in the graph as an ordinary node.
    pub table_id: String,
    pub table_name: String,
    /// The two sides, ordered by table id so the synthetic edge between them
    /// is the same on every run.
    pub sides: [JunctionSide; 2],
}

/// One end of a junction: the table joined, and the columns that reach it.
pub struct JunctionSide {
    pub table_id: String,
    pub columns: Vec<String>,
}

/// Every join table in the schema.
///
/// The rule is deliberately strict, because a false positive puts an edge on
/// the canvas that no statement in the SQL declares. A table qualifies when
/// all of these hold:
///
/// - it is a table, not a view;
/// - it has exactly two foreign keys, reaching two *different* tables;
/// - those keys share no column, so each side is reached independently;
/// - and some `PRIMARY KEY` or `UNIQUE` constraint covers exactly the union
///   of their columns.
///
/// The last is what makes the pairing many-to-many rather than incidental. A
/// table with two foreign keys and a surrogate primary key — `order_id`,
/// `product_id` and an `id` — is *not* a join table unless it also declares
/// `UNIQUE (order_id, product_id)`; without it the same pair may repeat, and
/// what the table records is a list of events, not a relation.
pub fn junctions(tables: &[TableKeys]) -> Vec<Junction> {
    let by_id: BTreeMap<&str, &TableKeys> = tables.iter().map(|t| (t.id.as_str(), t)).collect();
    tables
        .iter()
        .filter_map(|table| junction(table, &by_id))
        .collect()
}

/// One table, tested against the rule in [`junctions`].
fn junction(table: &TableKeys, by_id: &BTreeMap<&str, &TableKeys>) -> Option<Junction> {
    if table.is_view {
        return None;
    }
    let [left_fk, right_fk] = <&[ForeignKey; 2]>::try_from(table.foreign_keys.as_slice()).ok()?;

    let left = side(left_fk, by_id)?;
    let right = side(right_fk, by_id)?;
    if left.table_id == right.table_id {
        // A table joining one table to itself — `follows(follower_id,
        // followee_id)`. Real, but the synthetic edge would be a self-loop
        // between the same pair of nodes, which draws as noise and says
        // nothing the two ordinary edges do not.
        return None;
    }

    let mut columns = left.columns.clone();
    columns.extend(right.columns.iter().cloned());
    let distinct: BTreeSet<&String> = columns.iter().collect();
    if distinct.len() != columns.len() || !table.is_unique_over(&columns) {
        return None;
    }

    let mut sides = [left, right];
    sides.sort_by(|a, b| a.table_id.cmp(&b.table_id));
    Some(Junction {
        table_id: table.id.clone(),
        table_name: table.name.clone(),
        sides,
    })
}

/// One end of a candidate junction, or `None` when the key points outside
/// the schema — a table nobody folded cannot be one side of a relation the
/// graph is about to assert.
fn side(fk: &ForeignKey, by_id: &BTreeMap<&str, &TableKeys>) -> Option<JunctionSide> {
    let id = super::sql_fold::table_id(&fk.target_schema, &fk.target_table);
    by_id.get(id.as_str())?;
    Some(JunctionSide {
        table_id: id,
        columns: fk.columns.clone(),
    })
}
