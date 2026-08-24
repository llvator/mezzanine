//! Integration tests for the Go parser.
//!
//! Everything here goes through the full `parse` entry point rather than
//! calling an extractor directly, because what breaks first is a seam: a
//! receiver reaching call resolution as the wrong text, a type node
//! reaching the `UsesType` pass from a subtree that belongs to something
//! else. Neither misbehaves anywhere but end to end.
//!
//! The unit-level rules — what counts as a stdlib import, what a doc
//! comment strips, what decoration a type name sheds — are tested beside
//! the code that owns them.

use super::GoParser;
use crate::models::{EntityKind, RelationshipKind, Visibility};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    GoParser::new()
        .parse(Path::new("service.go"), src)
        .expect("parse")
}

fn entity<'a>(result: &'a ParseResult, name: &str) -> &'a crate::models::CodeEntity {
    result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("entity `{name}` not found"))
}

fn names_of_kind(result: &ParseResult, kind: EntityKind) -> Vec<String> {
    let mut names: Vec<String> = result
        .entities
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    names
}

/// Sorted, de-duplicated targets of one relationship kind out of `source`.
fn targets(result: &ParseResult, source: &str, kind: RelationshipKind) -> Vec<String> {
    let ids: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.name == source)
        .map(|e| e.id.as_str())
        .collect();
    assert!(!ids.is_empty(), "entity `{source}` not found");
    let mut found: Vec<String> = result
        .relationships
        .iter()
        .filter(|r| r.kind == kind && ids.contains(&r.source_id.as_str()))
        .map(|r| r.target_id.clone())
        .collect();
    found.sort();
    found.dedup();
    found
}

fn calls(result: &ParseResult, source: &str) -> Vec<String> {
    targets(result, source, RelationshipKind::Calls)
}

fn instantiates(result: &ParseResult, source: &str) -> Vec<String> {
    targets(result, source, RelationshipKind::Instantiates)
}

fn uses_types(result: &ParseResult, source: &str) -> Vec<String> {
    targets(result, source, RelationshipKind::UsesType)
}

/// Every relationship out of `source`, with the metadata value at `key`.
fn metadata_of(result: &ParseResult, source: &str, key: &str) -> Vec<(String, String)> {
    let ids: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.name == source)
        .map(|e| e.id.as_str())
        .collect();
    let mut found: Vec<(String, String)> = result
        .relationships
        .iter()
        .filter(|r| ids.contains(&r.source_id.as_str()))
        .filter_map(|r| {
            r.metadata
                .get(key)
                .map(|v| (r.target_id.clone(), v.clone()))
        })
        .collect();
    found.sort();
    found
}

// ---------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------

#[test]
fn a_file_yields_its_functions_types_and_values() {
    let result = parse(
        r#"
package store

const MaxRows = 100

var defaultTimeout = 30

type Row struct {
    ID   int
    Name string
}

type Reader interface {
    Read(id int) (*Row, error)
}

func Load(id int) *Row {
    return nil
}
"#,
    );

    assert_eq!(names_of_kind(&result, EntityKind::Function), ["Load"]);
    assert_eq!(names_of_kind(&result, EntityKind::Struct), ["Row"]);
    assert_eq!(names_of_kind(&result, EntityKind::Interface), ["Reader"]);
    assert_eq!(names_of_kind(&result, EntityKind::Constant), ["MaxRows"]);
    assert_eq!(
        names_of_kind(&result, EntityKind::Variable),
        ["defaultTimeout"]
    );
}

/// Go writes access control into the identifier, so this is the one
/// language where visibility comes from the name and nowhere else.
#[test]
fn an_uppercase_initial_is_what_exports_a_declaration() {
    let result = parse(
        r#"
package store

func Load() {}
func parse() {}

type Row struct {
    ID   int
    name string
}
"#,
    );

    assert_eq!(entity(&result, "Load").visibility, Visibility::Public);
    assert_eq!(entity(&result, "parse").visibility, Visibility::Internal);

    let row = entity(&result, "Row");
    let exported: Vec<&str> = row
        .fields
        .iter()
        .filter(|f| f.visibility == Some(Visibility::Public))
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(exported, ["ID"]);
    assert_eq!(row.metrics.public_field_ratio, Some(0.5));
}

#[test]
fn declarations_are_qualified_by_their_package() {
    let result = parse(
        r#"
package store

type Row struct{}

func Load() {}

func (r *Row) Save() {}
"#,
    );

    assert_eq!(entity(&result, "Load").qualified_name, "store.Load");
    assert_eq!(entity(&result, "Row").qualified_name, "store.Row");
    assert_eq!(entity(&result, "Save").qualified_name, "store.Row.Save");
}

/// A method is a top-level declaration in Go. What binds it to its type is
/// the receiver, and that binding is the parent edge.
#[test]
fn a_method_is_parented_by_its_receiver_type() {
    let result = parse(
        r#"
package store

type Row struct{}

func (r *Row) Save() error { return nil }
func (r Row) String() string { return "" }
"#,
    );

    let row = entity(&result, "Row");
    let save = entity(&result, "Save");
    assert_eq!(save.kind, EntityKind::Method);
    assert_eq!(save.parent_id.as_deref(), Some(row.id.as_str()));
    assert!(save.tags.contains("pointer_receiver"));
    assert!(entity(&result, "String").tags.contains("value_receiver"));
}

/// The type a method hangs off is routinely declared in another file of
/// the same package. The bare type name is then the parent, which is the
/// shape `graph.rs` already resolves for Rust's impl blocks.
#[test]
fn a_method_on_a_type_from_elsewhere_parents_by_name() {
    let result = parse(
        r#"
package store

func (c *Cache) Get(k string) string { return "" }
"#,
    );

    assert_eq!(entity(&result, "Get").parent_id.as_deref(), Some("Cache"));
}

#[test]
fn interface_methods_are_entities_under_their_interface() {
    let result = parse(
        r#"
package store

type Reader interface {
    Read(id int) (*Row, error)
    Close() error
}
"#,
    );

    let reader = entity(&result, "Reader");
    let read = entity(&result, "Read");
    assert_eq!(read.kind, EntityKind::Method);
    assert_eq!(read.parent_id.as_deref(), Some(reader.id.as_str()));
    assert!(read.tags.contains("interface_method"));
    // A signature with no body is one straight-through path.
    assert_eq!(read.metrics.cyclomatic, Some(1));
    assert_eq!(
        names_of_kind(&result, EntityKind::Method),
        ["Close", "Read"]
    );
}

/// Embedding promotes the embedded type's methods onto the embedder, which
/// is what an inheritance edge means to a reader of the graph.
#[test]
fn embedding_reads_as_inheritance() {
    let result = parse(
        r#"
package store

type Server struct {
    *Logger
    name string
}

type ReadWriter interface {
    Reader
    Writer
}
"#,
    );

    let server = entity(&result, "Server");
    assert_eq!(server.extends, ["Logger"]);
    assert!(server.tags.contains("embeds"));
    assert_eq!(entity(&result, "ReadWriter").extends, ["Reader", "Writer"]);
}

/// `type X = Y` and `type X Y` are different declarations, and Go's own
/// documentation is careful about which is which.
#[test]
fn a_defined_type_is_not_an_alias() {
    let result = parse(
        r#"
package store

type Celsius float64
type Alias = Celsius
"#,
    );

    let defined = entity(&result, "Celsius");
    assert_eq!(defined.kind, EntityKind::TypeAlias);
    assert!(defined.tags.contains("defined_type"));
    assert_eq!(defined.return_type.as_deref(), Some("float64"));

    let alias = entity(&result, "Alias");
    assert!(alias.tags.contains("alias"));
    assert_eq!(alias.return_type.as_deref(), Some("Celsius"));
}

/// The `iota` block is Go's enum. A constant that states neither type nor
/// value repeats the line above it, type included.
#[test]
fn an_iota_run_keeps_the_type_its_first_line_declared() {
    let result = parse(
        r#"
package store

type State int

const (
    StateIdle State = iota
    StateRunning
    StateDone
)
"#,
    );

    for name in ["StateIdle", "StateRunning", "StateDone"] {
        assert_eq!(
            entity(&result, name).return_type.as_deref(),
            Some("State"),
            "{name} should carry the run's type"
        );
    }
}

#[test]
fn one_spec_can_declare_several_names() {
    let result = parse(
        r#"
package store

var first, second int

func Move(x, y int) {}
"#,
    );

    assert_eq!(
        names_of_kind(&result, EntityKind::Variable),
        ["first", "second"]
    );
    let names: Vec<&str> = entity(&result, "Move")
        .parameters
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["x", "y"]);
}

#[test]
fn generics_are_captured_on_functions_and_types() {
    let result = parse(
        r#"
package store

type Cache[K comparable, V any] struct {
    items map[K]V
}

func Keys[K comparable, V any](m map[K]V) []K { return nil }
"#,
    );

    assert_eq!(entity(&result, "Cache").generics.len(), 2);
    assert_eq!(entity(&result, "Keys").generics.len(), 2);
}

// ---------------------------------------------------------------------
// Documentation
// ---------------------------------------------------------------------

/// Go marks documentation by adjacency and nothing else, so the blank line
/// is the whole rule.
#[test]
fn a_doc_comment_is_the_block_directly_above() {
    let result = parse(
        r#"
package store

// Load reads one row.
// It returns nil when there is none.
func Load() {}

// A note about something else entirely.

func Save() {}
"#,
    );

    assert_eq!(
        entity(&result, "Load").documentation.as_deref(),
        Some("Load reads one row.\nIt returns nil when there is none.")
    );
    assert_eq!(entity(&result, "Save").documentation, None);
}

#[test]
fn an_ungrouped_type_takes_the_comment_above_its_declaration() {
    let result = parse(
        r#"
package store

// Row is one record.
type Row struct{}

type (
    // Cache holds rows.
    Cache struct{}
)
"#,
    );

    assert_eq!(
        entity(&result, "Row").documentation.as_deref(),
        Some("Row is one record.")
    );
    assert_eq!(
        entity(&result, "Cache").documentation.as_deref(),
        Some("Cache holds rows.")
    );
}

#[test]
fn the_package_comment_documents_the_file() {
    let result = parse(
        r#"// Package store persists rows.
package store

func Load() {}
"#,
    );

    assert_eq!(
        result.file_documentation.as_deref(),
        Some("Package store persists rows.")
    );
}

// ---------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------

#[test]
fn imports_carry_their_path_and_alias() {
    let result = parse(
        r#"
package store

import (
    "fmt"
    u "example.com/team/users"
    . "example.com/team/dsl"
    _ "github.com/lib/pq"
)
"#,
    );

    let paths: Vec<&str> = result.imports.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "fmt",
            "example.com/team/users",
            "example.com/team/dsl",
            "github.com/lib/pq"
        ]
    );
    assert_eq!(result.imports[1].alias.as_deref(), Some("u"));
    assert!(result.imports[2].is_wildcard);
    assert_eq!(result.imports[3].alias, None);
}

// ---------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------

#[test]
fn a_bare_call_is_a_call_to_this_package() {
    let result = parse(
        r#"
package store

func Load() { normalize() }
func normalize() {}
"#,
    );

    assert_eq!(calls(&result, "Load"), ["normalize"]);
}

/// The receiver's type is declared right there in the signature, so
/// `s.persist()` is `Server.persist` and not an edge to something named
/// `s`.
#[test]
fn a_receiver_resolves_to_its_type() {
    let result = parse(
        r#"
package store

type Server struct{}

func (s *Server) Handle() { s.persist() }
func (s *Server) persist() {}
"#,
    );

    assert_eq!(calls(&result, "Handle"), ["Server.persist"]);
}

/// The three hops that make Go call resolution work: receiver to type,
/// type to field, field to type.
#[test]
fn a_field_chain_resolves_through_declared_types() {
    let result = parse(
        r#"
package store

type Repository struct{}

type Server struct {
    repo *Repository
}

func (s *Server) Handle(id int) { s.repo.Find(id) }
"#,
    );

    assert_eq!(calls(&result, "Handle"), ["Repository.Find"]);
}

#[test]
fn a_parameter_type_resolves_a_call_through_it() {
    let result = parse(
        r#"
package store

type Repository struct{}

func Run(repo *Repository) { repo.Find(1) }
"#,
    );

    assert_eq!(calls(&result, "Run"), ["Repository.Find"]);
}

/// The alternative — falling back to the operand's source text — is what
/// fills a graph with ghost nodes named after expressions.
#[test]
fn an_unknowable_receiver_drops_the_edge_rather_than_naming_a_ghost() {
    let result = parse(
        r#"
package store

func Run() {
    thing := lookup()
    thing.Whatever()
    helper()
}
"#,
    );

    // Nothing in this file states what `lookup` returns, so `thing` has no
    // type and `thing.Whatever` names nothing. The call to `lookup` itself,
    // and the sibling call, still land.
    assert_eq!(calls(&result, "Run"), ["helper", "lookup"]);
}

/// A type this file does not declare is still a type. The edge is emitted
/// and the resolver decides — it finds the type when another file of the
/// package declares it, and mints one honest ghost when nothing does.
#[test]
fn a_declared_type_qualifies_a_call_even_from_another_file() {
    let result = parse(
        r#"
package store

func Run(c *Cache) { c.Get("k") }
"#,
    );

    assert_eq!(calls(&result, "Run"), ["Cache.Get"]);
}

#[test]
fn a_package_qualified_call_uses_the_package_not_the_alias() {
    let result = parse(
        r#"
package store

import (
    "fmt"
    u "example.com/team/users"
)

func Run() {
    u.Fetch(1)
    fmt.Println("hi")
}
"#,
    );

    // The alias is this file's private business; `users.Fetch` is what the
    // users package's own entities are qualified as. `fmt` is the standard
    // library and is not drawn.
    assert_eq!(calls(&result, "Run"), ["users.Fetch"]);
}

/// `var wg sync.WaitGroup` then `wg.Add(1)` is the same call to the same
/// standard library as `fmt.Println`, reached through a variable instead
/// of directly. The package qualifier on the declared type is the only
/// thing that says so.
#[test]
fn a_stdlib_type_reached_through_a_variable_is_still_the_stdlib() {
    let result = parse(
        r#"
package store

import "sync"

type Pool struct {
    mu    sync.Mutex
    items *Buffer
}

func (p *Pool) Take() {
    p.mu.Lock()
    p.items.Pop()

    var wg sync.WaitGroup
    wg.Wait()
}
"#,
    );

    assert_eq!(calls(&result, "Take"), ["Buffer.Pop"]);
}

/// A project type keeps its edge even when its name collides with a
/// standard-library one — which is the reason the qualifier is checked
/// rather than the bare name.
#[test]
fn a_project_type_named_like_a_stdlib_one_keeps_its_edge() {
    let result = parse(
        r#"
package store

type WaitGroup struct{}

func Run(wg *WaitGroup) { wg.Add(1) }
"#,
    );

    assert_eq!(calls(&result, "Run"), ["WaitGroup.Add"]);
}

#[test]
fn builtins_and_conversions_are_not_calls() {
    let result = parse(
        r#"
package store

func Run(rows []int) {
    n := len(rows)
    _ = string(rune(n))
    rows = append(rows, 1)
    persist(n)
}

func persist(n int) {}
"#,
    );

    assert_eq!(calls(&result, "Run"), ["persist"]);
}

#[test]
fn a_composite_literal_instantiates_its_type() {
    let result = parse(
        r#"
package store

type Row struct{ ID int }

func Run() {
    a := Row{ID: 1}
    b := &Row{}
    c := new(Row)
    d := map[string]int{}
    _, _, _, _ = a, b, c, d
}
"#,
    );

    assert_eq!(instantiates(&result, "Run"), ["Row"]);
    // The map literal names no type the graph holds, so it is not an edge.
    assert_eq!(instantiates(&result, "Run").len(), 1);
}

#[test]
fn a_binding_travels_with_the_edge_it_names() {
    let result = parse(
        r#"
package store

type Row struct{}

func Run() {
    row, err := fetch()
    _, _ = row, err
}

func fetch() (*Row, error) { return nil, nil }
"#,
    );

    assert_eq!(
        metadata_of(&result, "Run", "binds_to"),
        [("fetch".to_string(), "row, err".to_string())]
    );
}

/// How a call is scheduled is not recoverable from its target, so the
/// keyword that dispatched it rides on the edge.
#[test]
fn go_and_defer_tag_the_edges_they_dispatch() {
    let result = parse(
        r#"
package store

func Run() {
    go publish()
    defer cleanup()
    persist()
}

func publish() {}
func cleanup() {}
func persist() {}
"#,
    );

    assert_eq!(
        metadata_of(&result, "Run", "goroutine"),
        [("publish".to_string(), "true".to_string())]
    );
    assert_eq!(
        metadata_of(&result, "Run", "deferred"),
        [("cleanup".to_string(), "true".to_string())]
    );
    assert_eq!(calls(&result, "Run"), ["cleanup", "persist", "publish"]);
}

#[test]
fn calls_inside_a_closure_belong_to_the_function_that_holds_it() {
    let result = parse(
        r#"
package store

func Run() {
    go func() { publish() }()
}

func publish() {}
"#,
    );

    assert_eq!(calls(&result, "Run"), ["publish"]);
}

// ---------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------

#[test]
fn an_else_if_chain_reads_as_a_flat_list_of_arms() {
    let result = parse(
        r#"
package store

func Run(n int) {
    if n == 1 {
        one()
    } else if n == 2 {
        two()
    } else {
        other()
    }
}

func one() {}
func two() {}
func other() {}
"#,
    );

    let arms = names_of_kind(&result, EntityKind::Branch);
    assert_eq!(arms, ["c1", "c2", "c3"]);
    assert_eq!(
        metadata_of(&result, "Run", "branch"),
        [
            ("one".to_string(), "c1".to_string()),
            ("other".to_string(), "c3".to_string()),
            ("two".to_string(), "c2".to_string()),
        ]
    );
}

/// Go has one loop keyword, and the expression a `range` walks belongs to
/// the loop it drives.
#[test]
fn every_for_shape_emits_one_loop() {
    let result = parse(
        r#"
package store

func Run(db *DB) {
    for _, row := range db.Rows() {
        handle(row)
    }
    for i := 0; i < 10; i++ {
        tick()
    }
    for {
        spin()
    }
}

func handle(row int) {}
func tick() {}
func spin() {}
"#,
    );

    assert_eq!(names_of_kind(&result, EntityKind::Loop), ["l1", "l2", "l3"]);
    let branches = metadata_of(&result, "Run", "branch");
    assert!(branches.contains(&("handle".to_string(), "l1".to_string())));
    assert!(branches.contains(&("tick".to_string(), "l2".to_string())));
    assert!(branches.contains(&("spin".to_string(), "l3".to_string())));
}

#[test]
fn switch_and_type_switch_arms_carry_their_labels() {
    let result = parse(
        r#"
package store

func Run(v interface{}) {
    switch x := v.(type) {
    case *Row:
        rowCase()
    default:
        fallbackCase()
    }
    switch 1 {
    case 2:
        two()
    }
}

func rowCase() {}
func fallbackCase() {}
func two() {}
"#,
    );

    let arms: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("case_arm"))
        .filter_map(|e| e.documentation.as_deref())
        .collect();
    assert_eq!(arms, ["pattern: *Row", "pattern: 2"]);
}

/// A `select` chooses by channel readiness, not by anything the function
/// wrote — which is why its arms are tagged apart from a switch's.
#[test]
fn select_arms_are_marked_as_their_own_kind() {
    let result = parse(
        r#"
package store

func Run(done chan int, work chan int) {
    select {
    case <-done:
        stop()
    case job := <-work:
        run(job)
    }
}

func stop() {}
func run(job int) {}
"#,
    );

    let select_arms = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("select_arm"))
        .count();
    assert_eq!(select_arms, 2);
}

// ---------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------

#[test]
fn a_multi_value_return_is_what_return_complexity_counts() {
    let result = parse(
        r#"
package store

func Two() (int, error) { return 0, nil }
func One() error { return nil }
"#,
    );

    assert_eq!(entity(&result, "Two").metrics.return_complexity, Some(2));
    assert_eq!(entity(&result, "One").metrics.return_complexity, None);
}

#[test]
fn branching_raises_complexity_and_nesting() {
    let result = parse(
        r#"
package store

func Run(n int, m int) {
    if n > 0 && m > 0 {
        for i := 0; i < n; i++ {
            if i == m {
                hit()
            }
        }
    }
}

func hit() {}
"#,
    );

    let run = entity(&result, "Run");
    // 1 base + if + `&&` + for + inner if.
    assert_eq!(run.metrics.cyclomatic, Some(5));
    assert_eq!(run.metrics.max_nesting, Some(3));
    assert!(run.metrics.cognitive_complexity.unwrap() > 3);
}

#[test]
fn a_bodyless_function_scores_one_straight_path() {
    let result = parse(
        r#"
package store

func Sqrt(x float64) float64
"#,
    );

    let sqrt = entity(&result, "Sqrt");
    assert_eq!(sqrt.metrics.cyclomatic, Some(1));
    assert_eq!(sqrt.metrics.cognitive_complexity, Some(0));
}

// ---------------------------------------------------------------------
// Type usage
// ---------------------------------------------------------------------

#[test]
fn a_signature_names_the_types_it_depends_on() {
    let result = parse(
        r#"
package store

func Save(row *Row, opts []Option) (*Receipt, error) { return nil, nil }
"#,
    );

    assert_eq!(uses_types(&result, "Save"), ["Option", "Receipt", "Row"]);
}

/// The failure a text scan makes first and hides longest: `count` is a
/// parameter name inside a function type, not a type.
#[test]
fn a_parameter_name_is_never_mistaken_for_a_type() {
    let result = parse(
        r#"
package store

func Watch(onTick func(count int) error) {}
"#,
    );

    assert_eq!(uses_types(&result, "Watch"), Vec::<String>::new());
}

/// The rule Java uses — uppercase initials are types — would drop every
/// unexported type in the package, which in Go is most of them.
#[test]
fn an_unexported_type_is_still_a_dependency() {
    let result = parse(
        r#"
package store

type cache struct{}

func Run(c *cache) {}
"#,
    );

    assert_eq!(uses_types(&result, "Run"), ["cache"]);
}

#[test]
fn struct_fields_name_the_types_the_struct_depends_on() {
    let result = parse(
        r#"
package store

import "sync"

type Server struct {
    mu    sync.Mutex
    repo  *Repository
    rows  []Row
    next  *Server
}
"#,
    );

    // `sync.Mutex` is the standard library, and `next *Server` is the
    // struct itself.
    assert_eq!(uses_types(&result, "Server"), ["Repository", "Row"]);
}

#[test]
fn a_qualified_type_keeps_its_package() {
    let result = parse(
        r#"
package api

import u "example.com/team/users"

func Load(id int) *u.Account { return nil }
"#,
    );

    assert_eq!(uses_types(&result, "Load"), ["users.Account"]);
}

#[test]
fn an_interface_methods_types_belong_to_the_method() {
    let result = parse(
        r#"
package store

type Reader interface {
    Read(id RowID) (*Row, error)
}
"#,
    );

    assert_eq!(uses_types(&result, "Read"), ["Row", "RowID"]);
    assert_eq!(uses_types(&result, "Reader"), Vec::<String>::new());
}
