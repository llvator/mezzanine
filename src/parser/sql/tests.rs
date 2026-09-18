//! Tests for the SQL schema parser.
//!
//! The corpus these are drawn from is a real 350-file PostgreSQL migration
//! set; the shapes below are the ones that actually occur in it, not
//! synthetic minimal cases.

use super::*;
use crate::analyzer::sql_fold::{self, table_id, FoldedSchema};
use crate::models::{EntityKind, RelationshipKind};
use std::path::PathBuf;

/// Parse one file. The parser emits operations, not tables — see ADR-0007.
fn parse(sql: &str) -> ParseResult {
    parse_named("schema.sql", sql)
}

fn parse_named(name: &str, sql: &str) -> ParseResult {
    SqlParser::new()
        .parse(&PathBuf::from(name), sql)
        .expect("parse must not fail the file")
}

/// Parse one file and fold it, which is what the pipeline does. Most tests
/// want the resulting schema rather than the raw operations.
fn schema_of(sql: &str) -> FoldedSchema {
    fold_files(&[("schema.sql", sql)])
}

/// Parse several named files and fold them in filename order.
fn fold_files(files: &[(&str, &str)]) -> FoldedSchema {
    let parsed: Vec<(PathBuf, ParseResult)> = files
        .iter()
        .map(|(name, sql)| (PathBuf::from(name), parse_named(name, sql)))
        .collect();
    sql_fold::fold(
        parsed
            .iter()
            .map(|(path, r)| sql_fold::FileOps {
                path: path.as_path(),
                ops: &r.schema_ops,
            })
            .collect(),
    )
}

fn table<'a>(s: &'a FoldedSchema, name: &str) -> &'a crate::models::CodeEntity {
    s.entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named {name}"))
}

fn names(s: &FoldedSchema) -> Vec<&str> {
    let mut v: Vec<&str> = s.entities.iter().map(|e| e.name.as_str()).collect();
    v.sort();
    v
}

#[test]
fn create_table_becomes_a_table_entity_with_columns() {
    let s = schema_of(
        "CREATE TABLE users (
            user_id UUID PRIMARY KEY,
            display_name TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now()
        );",
    );

    let users = table(&s, "users");
    assert_eq!(users.kind, EntityKind::Table);
    assert_eq!(users.qualified_name, "public.users");
    assert_eq!(users.fields.len(), 3);
    assert_eq!(users.metrics.field_count, Some(3));

    let created = &users.fields[2];
    assert_eq!(created.name, "created_at");
    assert_eq!(created.type_name.as_deref(), Some("TIMESTAMPTZ"));
    assert_eq!(created.default_value.as_deref(), Some("now()"));
}

#[test]
fn inline_references_emits_a_foreign_key_edge() {
    let s = schema_of(
        "CREATE TABLE paths (
            path_id UUID PRIMARY KEY,
            user_id UUID NOT NULL REFERENCES users(user_id)
        );",
    );

    assert_eq!(s.relationships.len(), 1);
    let fk = &s.relationships[0];
    assert_eq!(fk.kind, RelationshipKind::References);
    assert_eq!(fk.source_id, table_id("public", "paths"));
    assert_eq!(fk.target_id, table_id("public", "users"));
    assert_eq!(fk.metadata.get("fk_columns").unwrap(), "user_id");
    assert_eq!(fk.metadata.get("fk_target_columns").unwrap(), "user_id");
}

#[test]
fn table_level_foreign_key_emits_the_same_edge() {
    let s = schema_of(
        "CREATE TABLE memberships (
            org_id UUID,
            FOREIGN KEY (org_id) REFERENCES orgs(id) ON DELETE CASCADE
        );",
    );

    let fk = &s.relationships[0];
    assert_eq!(fk.target_id, table_id("public", "orgs"));
    assert_eq!(fk.metadata.get("fk_columns").unwrap(), "org_id");
    assert!(fk.metadata.get("on_delete").unwrap().contains("CASCADE"));
}

/// The whole point of a name-derived id: a foreign key in one file must be
/// able to name a table declared in another. Parse them separately and check
/// the ids line up, which is what the graph assembler joins on.
#[test]
fn foreign_key_targets_resolve_across_files() {
    let s = fold_files(&[
        (
            "001_users.sql",
            "CREATE TABLE users (user_id UUID PRIMARY KEY);",
        ),
        (
            "002_sessions.sql",
            "CREATE TABLE sessions (
                id UUID PRIMARY KEY,
                user_id UUID REFERENCES users(user_id)
            );",
        ),
    ]);

    let fk = s
        .relationships
        .iter()
        .find(|r| r.source_id == table_id("public", "sessions"))
        .expect("sessions has a foreign key");
    assert_eq!(
        fk.target_id,
        table(&s, "users").id,
        "the FK must target the id the other file's CREATE TABLE produced"
    );
}

#[test]
fn schema_qualification_and_quoting_normalise_to_one_id() {
    let plain = schema_of("CREATE TABLE users (id UUID);");
    let qualified = schema_of("CREATE TABLE public.\"Users\" (id UUID);");

    assert_eq!(table(&plain, "users").id, table(&qualified, "users").id);
}

#[test]
fn a_non_default_schema_is_a_distinct_table() {
    let s = schema_of("CREATE TABLE auth.users (id UUID);");
    assert_eq!(table(&s, "users").id, table_id("auth", "users"));
    assert_ne!(table(&s, "users").id, table_id("public", "users"));
}

#[test]
fn create_view_is_a_view_not_a_table() {
    let s = schema_of("CREATE VIEW active_users AS SELECT * FROM users WHERE archived_at IS NULL;");
    assert_eq!(table(&s, "active_users").kind, EntityKind::View);
}

/// One unparseable statement must cost that statement and nothing else.
/// Without this the 53 `DO $$` blocks in the motivating corpus would take
/// every table declared beside them.
#[test]
fn an_unparseable_statement_does_not_cost_the_file() {
    let r = parse(
        "CREATE TABLE before_it (id UUID);

         DO $$
         BEGIN
             IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector') THEN
                 RAISE NOTICE 'hello; world';
             END IF;
         END $$;

         CREATE TABLE after_it (id UUID);",
    );

    assert_eq!(r.warnings.len(), 1, "the skipped statement is reported");
    assert_eq!(
        r.schema_ops.len(),
        2,
        "both CREATE TABLEs survive the bad statement"
    );
}

/// Semicolons inside dollar-quoted bodies, string literals and comments must
/// not split a statement. `RAISE NOTICE 'hello; world'` above already covers
/// the nested case; this pins the simpler ones.
#[test]
fn semicolons_inside_literals_and_comments_do_not_split() {
    let r = parse(
        "CREATE TABLE t (
            note TEXT DEFAULT 'a; b',  -- a comment; with a semicolon
            /* block; comment */
            other TEXT
        );",
    );

    let s = schema_of(
        "CREATE TABLE t (
            note TEXT DEFAULT 'a; b',  -- a comment; with a semicolon
            /* block; comment */
            other TEXT
        );",
    );
    assert_eq!(s.entities.len(), 1);
    assert_eq!(table(&s, "t").fields.len(), 2);
    let _ = r;
}

/// PL/pgSQL bodies contain prose, and prose contains apostrophes escaped as
/// `''`. A quote scanner that mishandles that desynchronises and silently
/// swallows everything after it — this is a regression test for exactly that
/// failure, which cost three tables when the prototype had the bug.
#[test]
fn a_doubled_apostrophe_in_a_comment_does_not_desynchronise() {
    let r = parse(
        "CREATE TABLE first (id UUID);
         -- validated to be inside the user''s allowlist
         CREATE TABLE second (id UUID);",
    );

    assert_eq!(r.schema_ops.len(), 2);
}

#[test]
fn dml_and_indexes_parse_but_contribute_nothing() {
    let r = parse(
        "CREATE TABLE t (id UUID);
         CREATE INDEX idx_t ON t (id);
         INSERT INTO t (id) VALUES (gen_random_uuid());
         UPDATE t SET id = id;",
    );

    assert_eq!(r.schema_ops.len(), 1, "only the CREATE TABLE is an op");
    assert!(r.warnings.is_empty(), "none of these should warn");
}

/// A table holds columns as `fields`, not as child entities — nothing ever
/// has a table as its `parent_id`. Keeping it out of `is_container` is what
/// keeps it out of the container smell rules, where many columns and no
/// methods would read as a `DataBag`. That is what a table *is*, and
/// "group related fields into nested sub-structs" is not actionable for a
/// schema.
#[test]
fn tables_are_not_containers_so_smells_do_not_apply() {
    assert!(!EntityKind::Table.is_container());
    assert!(!EntityKind::View.is_container());
    assert!(EntityKind::Struct.is_container(), "control");
}

#[test]
fn sql_files_route_to_this_parser() {
    use crate::models::file_info::Language;
    assert_eq!(Language::from_extension("sql"), Language::Sql);
    assert_eq!(Language::from_name("postgres"), Some(Language::Sql));
    assert_eq!(
        crate::parser::detect_language(&PathBuf::from("migrations/001_init.sql")),
        Language::Sql
    );
}

/// Migration files open with banner comments. The span must land on the
/// `CREATE TABLE`, not on the banner, and — more importantly — a warning
/// must cite the line of the statement that actually failed.
#[test]
fn a_leading_comment_banner_does_not_drag_the_span_or_warning_line() {
    let r = parse(
        "-- ======================================\n\
         -- Initial schema\n\
         -- ======================================\n\
         CREATE TABLE t (id UUID);\n\
         \n\
         -- a banner for the broken one\n\
         DO $$ BEGIN NULL; END $$;",
    );

    assert_eq!(r.schema_ops[0].span.start.line, 3, "past the banner");
    assert_eq!(r.warnings.len(), 1);
    assert!(
        r.warnings[0].contains(":7:"),
        "warning should cite the DO line, not its banner — got {:?}",
        r.warnings[0]
    );
}

#[test]
fn spans_point_at_the_statement_that_declared_the_table() {
    let r = parse("-- header\n\nCREATE TABLE t (\n  id UUID\n);");
    let span = r.schema_ops[0].span;
    assert_eq!(span.start.line, 2, "0-indexed line of CREATE TABLE");
    assert_eq!(span.end.line, 4);
}

// ── The fold (SQL-002) ───────────────────────────────────────────────────
//
// These are the cases a per-file read gets wrong. Each one describes a
// schema that never existed at any point in time unless the operations are
// replayed in order.

#[test]
fn add_column_in_a_later_migration_lands_on_the_table() {
    let s = fold_files(&[
        ("001_init.sql", "CREATE TABLE orders (id UUID PRIMARY KEY);"),
        (
            "002_total.sql",
            "ALTER TABLE orders ADD COLUMN total NUMERIC;",
        ),
    ]);

    let orders = table(&s, "orders");
    assert_eq!(orders.fields.len(), 2);
    assert_eq!(orders.fields[1].name, "total");
    assert_eq!(orders.metrics.field_count, Some(2));
}

#[test]
fn filename_order_decides_not_discovery_order() {
    // Same two files, handed over in the wrong order.
    let s = fold_files(&[
        (
            "002_total.sql",
            "ALTER TABLE orders ADD COLUMN total NUMERIC;",
        ),
        ("001_init.sql", "CREATE TABLE orders (id UUID PRIMARY KEY);"),
    ]);

    assert_eq!(
        s.alters_on_unknown_tables, 0,
        "the fold must sort by filename, not trust the order it is given"
    );
    assert_eq!(table(&s, "orders").fields.len(), 2);
}

#[test]
fn a_dropped_table_is_not_in_the_schema() {
    let s = fold_files(&[
        (
            "001_init.sql",
            "CREATE TABLE tasks (id UUID); CREATE TABLE kept (id UUID);",
        ),
        ("002_drop.sql", "DROP TABLE tasks;"),
    ]);

    assert_eq!(names(&s), vec!["kept"]);
    assert_eq!(s.dropped, vec![table_id("public", "tasks")]);
}

#[test]
fn dropping_a_table_removes_foreign_keys_pointing_at_it() {
    let s = fold_files(&[
        (
            "001_init.sql",
            "CREATE TABLE tasks (id UUID PRIMARY KEY);
             CREATE TABLE notes (id UUID, task_id UUID REFERENCES tasks(id));",
        ),
        ("002_drop.sql", "DROP TABLE tasks;"),
    ]);

    assert!(
        s.relationships.is_empty(),
        "an edge to a dropped table would dangle: {:?}",
        s.relationships
    );
}

#[test]
fn a_renamed_table_appears_once_under_its_final_name() {
    let s = fold_files(&[
        ("001_init.sql", "CREATE TABLE feeds (id UUID PRIMARY KEY);"),
        (
            "002_rename.sql",
            "ALTER TABLE feeds RENAME TO api_connectors;",
        ),
    ]);

    assert_eq!(names(&s), vec!["api_connectors"]);
    let t = table(&s, "api_connectors");
    assert_eq!(t.id, table_id("public", "api_connectors"));
    assert!(t.tags.contains("renamed"));
    assert!(t.documentation.as_deref().unwrap().contains("feeds"));
}

/// The case that breaks a naive fold: an edge written against the old name
/// has to follow the rename, or it dangles.
#[test]
fn foreign_keys_follow_a_renamed_target() {
    let s = fold_files(&[
        (
            "001_init.sql",
            "CREATE TABLE feeds (id UUID PRIMARY KEY);
             CREATE TABLE rows (id UUID, feed_id UUID REFERENCES feeds(id));",
        ),
        (
            "002_rename.sql",
            "ALTER TABLE feeds RENAME TO api_connectors;",
        ),
    ]);

    let live: Vec<&String> = s.entities.iter().map(|e| &e.id).collect();
    assert_eq!(s.relationships.len(), 1);
    let target = &s.relationships[0].target_id;
    assert_eq!(*target, table_id("public", "api_connectors"));
    assert!(live.contains(&target), "the FK target must be a live table");
}

/// Three hops, which the motivating corpus actually contains.
#[test]
fn a_multi_hop_rename_chain_keeps_full_provenance() {
    let s = fold_files(&[
        ("001.sql", "CREATE TABLE feed_row_origins (id UUID);"),
        (
            "002.sql",
            "ALTER TABLE feed_row_origins RENAME TO api_connector_row_origins;",
        ),
        (
            "003.sql",
            "ALTER TABLE api_connector_row_origins RENAME TO connector_row_origins;",
        ),
    ]);

    assert_eq!(names(&s), vec!["connector_row_origins"]);
    let doc = table(&s, "connector_row_origins")
        .documentation
        .clone()
        .unwrap();
    assert!(doc.contains("feed_row_origins"), "{doc}");
    assert!(doc.contains("api_connector_row_origins"), "{doc}");
}

#[test]
fn dropped_columns_and_their_foreign_keys_go_away() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE posts (id UUID, author UUID REFERENCES users(id), body TEXT);",
        ),
        ("002.sql", "ALTER TABLE posts DROP COLUMN author;"),
    ]);

    let posts = table(&s, "posts");
    assert_eq!(posts.fields.len(), 2);
    assert!(posts.fields.iter().all(|c| c.name != "author"));
    assert!(s.relationships.is_empty(), "the FK went with its column");
}

#[test]
fn a_renamed_column_keeps_its_position_and_its_foreign_key() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE posts (id UUID, author UUID REFERENCES users(id), body TEXT);",
        ),
        (
            "002.sql",
            "ALTER TABLE posts RENAME COLUMN author TO author_id;",
        ),
    ]);

    let posts = table(&s, "posts");
    assert_eq!(posts.fields[1].name, "author_id", "position preserved");
    assert_eq!(
        s.relationships[0].metadata.get("fk_columns").unwrap(),
        "author_id"
    );
}

/// `DROP CONSTRAINT` has to be able to remove an inline `REFERENCES`, which
/// was never given an explicit name. Postgres names it `<t>_<c>_fkey`, and
/// the fold synthesises the same name so the drop can find it.
#[test]
fn drop_constraint_removes_an_inline_foreign_key() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE posts (id UUID, author UUID REFERENCES users(id));",
        ),
        (
            "002.sql",
            "ALTER TABLE posts DROP CONSTRAINT posts_author_fkey;",
        ),
    ]);

    assert!(
        s.relationships.is_empty(),
        "the dropped constraint should be gone: {:?}",
        s.relationships
    );
}

#[test]
fn add_constraint_in_a_later_migration_creates_the_edge() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE posts (id UUID, author UUID);",
        ),
        (
            "002.sql",
            "ALTER TABLE posts ADD CONSTRAINT posts_author_fk
                 FOREIGN KEY (author) REFERENCES users(id);",
        ),
    ]);

    assert_eq!(s.relationships.len(), 1);
    assert_eq!(s.relationships[0].target_id, table_id("public", "users"));
}

#[test]
fn create_if_not_exists_does_not_clobber_an_existing_table() {
    let s = fold_files(&[
        ("001.sql", "CREATE TABLE t (id UUID, extra TEXT);"),
        ("002.sql", "CREATE TABLE IF NOT EXISTS t (id UUID);"),
    ]);

    assert_eq!(table(&s, "t").fields.len(), 2, "the richer definition wins");
}

#[test]
fn a_table_dropped_then_recreated_is_live_again() {
    let s = fold_files(&[
        ("001.sql", "CREATE TABLE t (id UUID);"),
        ("002.sql", "DROP TABLE t;"),
        ("003.sql", "CREATE TABLE t (id UUID, added TEXT);"),
    ]);

    assert_eq!(names(&s), vec!["t"]);
    assert_eq!(table(&s, "t").fields.len(), 2);
    assert!(s.dropped.is_empty(), "it came back");
}

/// A table an extension or hosted platform owns. The operation must not be
/// dropped silently — the columns it adds are still real — but it must be
/// counted, because the same signal catches genuine ordering bugs.
#[test]
fn altering_an_uncreated_table_is_tolerated_and_counted() {
    let s = fold_files(&[(
        "001.sql",
        "ALTER TABLE auth.users ADD COLUMN nickname TEXT;",
    )]);

    assert_eq!(s.alters_on_unknown_tables, 1);
    assert_eq!(table(&s, "users").id, table_id("auth", "users"));
    assert_eq!(table(&s, "users").fields.len(), 1);
}

#[test]
fn tables_are_attributed_to_the_migration_that_created_them() {
    let s = fold_files(&[
        ("001_init.sql", "CREATE TABLE t (id UUID);"),
        ("002_alter.sql", "ALTER TABLE t ADD COLUMN extra TEXT;"),
    ]);

    assert_eq!(
        table(&s, "t").file_path,
        PathBuf::from("001_init.sql"),
        "not the file that last touched it"
    );
}

// ---------------------------------------------------------------------------
// Cardinality and the key that carries it (SQL-006)
// ---------------------------------------------------------------------------

/// The label is the whole feature: an edge that says only "these two tables
/// are joined" is what SQL-006 was filed against.
fn label(s: &FoldedSchema, target: &str) -> String {
    s.relationships
        .iter()
        .find(|r| r.target_id == table_id("public", target))
        .unwrap_or_else(|| panic!("no edge to {target}"))
        .label
        .clone()
        .expect("every SQL edge is labelled")
}

fn card(s: &FoldedSchema, target: &str) -> String {
    s.relationships
        .iter()
        .find(|r| r.target_id == table_id("public", target))
        .unwrap_or_else(|| panic!("no edge to {target}"))
        .metadata
        .get("cardinality")
        .expect("every SQL edge states its cardinality")
        .clone()
}

/// The ordinary case, and the default: nothing stops two orders from naming
/// one user.
#[test]
fn an_unconstrained_foreign_key_is_many_to_one() {
    let s = schema_of(
        "CREATE TABLE users (id UUID PRIMARY KEY);
         CREATE TABLE orders (id UUID PRIMARY KEY, user_id UUID REFERENCES users(id));",
    );
    assert_eq!(card(&s, "users"), "n:1");
    assert_eq!(label(&s, "users"), "user_id → id (N:1)");
}

/// Uniqueness on the *referencing* side is the only thing that makes a
/// foreign key one-to-one, and it is exactly what the parser used to drop.
#[test]
fn a_unique_foreign_key_column_is_one_to_one() {
    let s = schema_of(
        "CREATE TABLE users (id UUID PRIMARY KEY);
         CREATE TABLE profiles (user_id UUID UNIQUE REFERENCES users(id));",
    );
    assert_eq!(card(&s, "users"), "1:1");
    assert_eq!(label(&s, "users"), "user_id → id (1:1)");
}

/// The same claim written as the table-level constraint, and as a primary
/// key — a foreign key that is also the whole primary key is one-to-one.
#[test]
fn a_primary_key_foreign_key_is_one_to_one() {
    let s = schema_of(
        "CREATE TABLE users (id UUID PRIMARY KEY);
         CREATE TABLE profiles (
             user_id UUID REFERENCES users(id),
             PRIMARY KEY (user_id)
         );",
    );
    assert_eq!(card(&s, "users"), "1:1");
}

/// `CREATE UNIQUE INDEX` is how a migration adds uniqueness to a table that
/// already exists, and it is a statement the parser previously skipped
/// wholesale along with every other index.
#[test]
fn a_unique_index_in_a_later_migration_makes_the_edge_one_to_one() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE profiles (user_id UUID REFERENCES users(id));",
        ),
        (
            "002.sql",
            "CREATE UNIQUE INDEX one_profile_per_user ON profiles (user_id);",
        ),
    ]);
    assert_eq!(card(&s, "users"), "1:1");
}

/// A non-unique index still says nothing about cardinality, and must not be
/// mistaken for one that does.
#[test]
fn a_plain_index_does_not_make_an_edge_one_to_one() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE orders (user_id UUID REFERENCES users(id));",
        ),
        ("002.sql", "CREATE INDEX orders_by_user ON orders (user_id);"),
    ]);
    assert_eq!(card(&s, "users"), "n:1");
}

/// `UNIQUE (user_id, slug)` permits many rows per `user_id`, so it does not
/// make a key on `user_id` alone one-to-one. Uniqueness has to cover the
/// foreign key's columns *exactly*.
#[test]
fn a_wider_unique_constraint_does_not_make_the_edge_one_to_one() {
    let s = schema_of(
        "CREATE TABLE users (id UUID PRIMARY KEY);
         CREATE TABLE pages (
             user_id UUID REFERENCES users(id),
             slug TEXT,
             UNIQUE (user_id, slug)
         );",
    );
    assert_eq!(card(&s, "users"), "n:1");
}

/// A composite key is one key. Naming only its first column tells the reader
/// something false about how the two tables join.
#[test]
fn a_composite_foreign_key_names_every_column() {
    let s = schema_of(
        "CREATE TABLE users (tenant_id UUID, id UUID, PRIMARY KEY (tenant_id, id));
         CREATE TABLE orders (
             tenant_id UUID,
             user_id UUID,
             FOREIGN KEY (tenant_id, user_id) REFERENCES users(tenant_id, id)
         );",
    );
    let edge = &s.relationships[0];
    assert_eq!(edge.metadata.get("fk_columns").unwrap(), "tenant_id, user_id");
    assert_eq!(
        edge.metadata.get("fk_target_columns").unwrap(),
        "tenant_id, id"
    );
    assert_eq!(
        label(&s, "users"),
        "(tenant_id, user_id) → (tenant_id, id) (N:1)"
    );
}

/// The bare form means "the target's primary key" — a fact only the fold
/// knows, because the target is usually declared in another file.
#[test]
fn a_bare_references_resolves_against_the_targets_primary_key() {
    let s = fold_files(&[
        ("001.sql", "CREATE TABLE users (id UUID PRIMARY KEY);"),
        (
            "002.sql",
            "CREATE TABLE orders (user_id UUID REFERENCES users);",
        ),
    ]);
    assert_eq!(label(&s, "users"), "user_id → id (N:1)");
}

/// A target outside the migration set has no primary key to resolve against.
/// The label says what it knows and stops, rather than guessing `id`.
#[test]
fn an_unresolvable_target_leaves_the_right_hand_side_off() {
    let s = schema_of("CREATE TABLE orders (user_id UUID REFERENCES auth.users);");
    let edge = &s.relationships[0];
    assert_eq!(edge.label.as_deref().unwrap(), "user_id → (N:1)");
    assert!(!edge.metadata.contains_key("fk_target_columns"));
}

/// The junction case: the join table keeps its node and its two edges, and
/// the many-to-many is drawn beside them.
#[test]
fn a_join_table_yields_a_many_to_many_edge_beside_its_own() {
    let s = schema_of(
        "CREATE TABLE orders (id UUID PRIMARY KEY);
         CREATE TABLE products (id UUID PRIMARY KEY);
         CREATE TABLE order_items (
             order_id UUID REFERENCES orders(id),
             product_id UUID REFERENCES products(id),
             PRIMARY KEY (order_id, product_id)
         );",
    );

    // The join table is still a table, with its two ordinary edges.
    assert!(names(&s).contains(&"order_items"));
    assert_eq!(s.relationships.len(), 3, "two foreign keys, one inferred");

    let nm = s
        .relationships
        .iter()
        .find(|r| r.metadata.get("cardinality").map(String::as_str) == Some("n:m"))
        .expect("the many-to-many edge");
    assert_eq!(nm.source_id, table_id("public", "orders"));
    assert_eq!(nm.target_id, table_id("public", "products"));
    assert_eq!(nm.label.as_deref().unwrap(), "N:M via order_items");
    assert_eq!(nm.metadata.get("junction").unwrap(), &table_id("public", "order_items"));
    assert_eq!(nm.metadata.get("inferred").unwrap(), "true");
}

/// The surrogate-key form: `id` plus two foreign keys is a log of events,
/// not a relation, unless the pair is also declared unique. Getting this
/// wrong puts an edge on the canvas that the schema does not license.
#[test]
fn two_foreign_keys_without_a_unique_pair_are_not_a_join_table() {
    let s = schema_of(
        "CREATE TABLE orders (id UUID PRIMARY KEY);
         CREATE TABLE products (id UUID PRIMARY KEY);
         CREATE TABLE order_events (
             id UUID PRIMARY KEY,
             order_id UUID REFERENCES orders(id),
             product_id UUID REFERENCES products(id)
         );",
    );
    assert_eq!(s.relationships.len(), 2, "no inferred edge");
}

/// The same table with the pair declared unique *is* a join table — the
/// surrogate primary key does not disqualify it.
#[test]
fn a_surrogate_key_join_table_is_still_a_join_table() {
    let s = schema_of(
        "CREATE TABLE orders (id UUID PRIMARY KEY);
         CREATE TABLE products (id UUID PRIMARY KEY);
         CREATE TABLE order_items (
             id UUID PRIMARY KEY,
             order_id UUID REFERENCES orders(id),
             product_id UUID REFERENCES products(id),
             UNIQUE (order_id, product_id)
         );",
    );
    assert_eq!(s.relationships.len(), 3);
}

/// A table joining one table to itself would draw a self-loop that says
/// nothing its two ordinary edges do not.
#[test]
fn a_self_join_table_infers_no_edge() {
    let s = schema_of(
        "CREATE TABLE users (id UUID PRIMARY KEY);
         CREATE TABLE follows (
             follower_id UUID REFERENCES users(id),
             followee_id UUID REFERENCES users(id),
             PRIMARY KEY (follower_id, followee_id)
         );",
    );
    assert_eq!(s.relationships.len(), 2, "no inferred edge");
}

/// Dropping a column takes the uniqueness that covered it, so an edge that
/// was one-to-one stops claiming to be.
#[test]
fn dropping_a_unique_column_takes_the_one_to_one_with_it() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE profiles (id UUID PRIMARY KEY, user_id UUID UNIQUE REFERENCES users(id));",
        ),
        ("002.sql", "ALTER TABLE profiles DROP COLUMN user_id;"),
    ]);
    assert!(s.relationships.is_empty(), "the key went with the column");
}

/// `ADD CONSTRAINT … UNIQUE` is the other way a later migration tightens an
/// edge, and it shares a namespace with foreign keys for `DROP CONSTRAINT`.
#[test]
fn a_unique_constraint_can_be_added_and_dropped_later() {
    let tightened = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE profiles (user_id UUID REFERENCES users(id));",
        ),
        (
            "002.sql",
            "ALTER TABLE profiles ADD CONSTRAINT one_each UNIQUE (user_id);",
        ),
    ]);
    assert_eq!(card(&tightened, "users"), "1:1");

    let loosened = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE profiles (user_id UUID REFERENCES users(id));",
        ),
        (
            "002.sql",
            "ALTER TABLE profiles ADD CONSTRAINT one_each UNIQUE (user_id);",
        ),
        ("003.sql", "ALTER TABLE profiles DROP CONSTRAINT one_each;"),
    ]);
    assert_eq!(card(&loosened, "users"), "n:1");
}

/// A partial unique index constrains only the rows matching its predicate,
/// so it cannot make the edge one-to-one for the rest of them.
#[test]
fn a_partial_unique_index_does_not_make_an_edge_one_to_one() {
    let s = fold_files(&[
        (
            "001.sql",
            "CREATE TABLE users (id UUID PRIMARY KEY);
             CREATE TABLE memberships (user_id UUID REFERENCES users(id), revoked_at TIMESTAMP);",
        ),
        (
            "002.sql",
            "CREATE UNIQUE INDEX one_active ON memberships (user_id) WHERE revoked_at IS NULL;",
        ),
    ]);
    assert_eq!(card(&s, "users"), "n:1");
}
