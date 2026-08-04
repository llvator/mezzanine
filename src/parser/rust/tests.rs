//! Integration tests for the Rust parser's call extraction, focused on the
//! callee strings that reach the resolver (AN-006).
//!
//! The bug these cover is a recall bug, so the assertions are about *which
//! callee string is emitted* rather than about the final edge: a callee of
//! `graph::detect_smells` matches no entity and lands on a ghost, which is why
//! `impact` reported "Used by (0)" for a function with callers.

use super::RustParser;
use crate::models::RelationshipKind;
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    let parser = RustParser::new();
    parser.parse(Path::new("test.rs"), src).expect("parse")
}

/// The deferred-receiver hints (AN-012) the parser handed to the analyzer:
/// `(recv_path, recv_member)` per call edge that carries one.
fn deferred(result: &ParseResult) -> Vec<(String, String)> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .filter_map(|r| {
            Some((
                r.metadata.get("recv_path")?.clone(),
                r.metadata.get("recv_member")?.clone(),
            ))
        })
        .collect()
}

fn callees(result: &ParseResult) -> Vec<String> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .map(|r| r.target_id.clone())
        .collect()
}

#[test]
fn let_mut_binding_still_infers_its_type() {
    // `let mut` wraps the binding in a `mut_pattern`. Before AN-006 the
    // inference pass filtered to bare `identifier` and dropped it, so the
    // receiver fell back to its source text.
    let src = r#"
fn run() {
    let mut walker = FileWalker::new();
    walker.collect();
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.contains(&"FileWalker::collect".to_string()),
        "expected the receiver to resolve to its type, got {found:?}"
    );
    assert!(
        !found.contains(&"walker::collect".to_string()),
        "receiver text leaked into the callee: {found:?}"
    );
}

#[test]
fn immutable_binding_inference_is_unchanged() {
    let src = r#"
fn run() {
    let walker = FileWalker::new();
    walker.collect();
}
"#;
    assert!(callees(&parse(src)).contains(&"FileWalker::collect".to_string()));
}

#[test]
fn self_constructor_resolves_to_the_impl_type() {
    // `let mut graph = Self::new()` infers `graph: Self`; `Self::detect_smells`
    // matches no entity because the index is keyed by the real type name.
    let src = r#"
impl DependencyGraph {
    fn build() -> Self {
        let mut graph = Self::new();
        graph.detect_smells();
        graph
    }
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.contains(&"DependencyGraph::detect_smells".to_string()),
        "expected Self to resolve to the enclosing impl type, got {found:?}"
    );
}

#[test]
fn mut_pattern_without_an_inferable_value_is_left_alone() {
    // No constructor, no annotation — nothing to infer. The receiver-text
    // fallback still applies; this test pins that AN-006 did not start
    // guessing types it cannot see.
    let src = r#"
fn run(input: Vec<u8>) {
    let mut thing = input;
    thing.consume();
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.iter().any(|c| c.ends_with("consume")),
        "call should still be recorded, got {found:?}"
    );
}

// ---- AN-009: parameter types ----

#[test]
fn parameter_type_qualifies_a_method_call() {
    // The largest remaining miss class: the receiver is a parameter, whose
    // type is written down in the signature.
    let src = r#"
fn run(client: &mut LspClient) {
    client.shutdown();
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.contains(&"LspClient::shutdown".to_string()),
        "expected the parameter's declared type, got {found:?}"
    );
    assert!(!found.contains(&"client::shutdown".to_string()));
}

#[test]
fn parameter_type_survives_lifetimes_generics_and_paths() {
    let src = r#"
fn run(a: &'a mut crate::lsp::LspClient, b: Vec<Thing>, c: &dyn Renderer) {
    a.shutdown();
    b.drain();
    c.render();
}
"#;
    let found = callees(&parse(src));
    for expected in ["LspClient::shutdown", "Vec::drain", "Renderer::render"] {
        assert!(found.contains(&expected.to_string()), "missing {expected} in {found:?}");
    }
}

#[test]
fn a_local_rebinding_shadows_the_parameter() {
    let src = r#"
fn run(thing: OuterType) {
    let thing = InnerType::new();
    thing.act();
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.contains(&"InnerType::act".to_string()),
        "the let binding must win over the parameter, got {found:?}"
    );
    assert!(!found.contains(&"OuterType::act".to_string()));
}

#[test]
fn primitive_and_destructured_parameters_bind_nothing() {
    // Binding `count` to `usize` or guessing a name for a tuple pattern would
    // invent edges. Both must be left alone.
    let src = r#"
fn run(count: usize, (left, right): (Foo, Bar), name: &str) {
    count.to_string();
    left.act();
    name.len();
}
"#;
    let found = callees(&parse(src));
    assert!(!found.iter().any(|c| c.starts_with("usize::")), "{found:?}");
    assert!(!found.iter().any(|c| c.starts_with("Foo::")), "{found:?}");
    assert!(!found.iter().any(|c| c.starts_with("str::")), "{found:?}");
}

#[test]
fn mut_parameter_is_bound_like_a_mut_local() {
    let src = r#"
fn run(mut buf: OutputBuffer) {
    buf.flush();
}
"#;
    assert!(callees(&parse(src)).contains(&"OutputBuffer::flush".to_string()));
}

// ---- AN-010: struct field types ----

#[test]
fn field_type_qualifies_a_method_call_on_self() {
    let src = r#"
struct Analyzer {
    store: ParseStore,
}

impl Analyzer {
    fn run(&self) {
        self.store.get();
    }
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.contains(&"ParseStore::get".to_string()),
        "expected the field's declared type, got {found:?}"
    );
    assert!(!found.contains(&"self.store::get".to_string()));
}

#[test]
fn field_types_resolve_when_the_impl_precedes_the_struct() {
    // Impl blocks routinely appear above the struct they implement, so the
    // field map has to be built before the walk rather than during it.
    let src = r#"
impl Analyzer {
    fn run(&self) {
        self.store.get();
    }
}

struct Analyzer {
    store: ParseStore,
}
"#;
    assert!(callees(&parse(src)).contains(&"ParseStore::get".to_string()));
}

#[test]
fn a_parameter_shadows_a_field_of_the_same_name() {
    let src = r#"
struct Analyzer {
    store: FieldStore,
}

impl Analyzer {
    fn run(&self, store: ParamStore) {
        store.get();
    }
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.contains(&"ParamStore::get".to_string()),
        "the parameter must win over the field, got {found:?}"
    );
    assert!(!found.contains(&"FieldStore::get".to_string()));
}

#[test]
fn fields_of_other_structs_do_not_leak_into_an_impl() {
    // Keyed by struct name precisely so one type's fields cannot type
    // another's `self.<field>`.
    let src = r#"
struct Other {
    store: WrongType,
}

struct Analyzer {
    other_field: RightType,
}

impl Analyzer {
    fn run(&self) {
        self.store.get();
    }
}
"#;
    let found = callees(&parse(src));
    assert!(!found.contains(&"WrongType::get".to_string()), "{found:?}");
}

#[test]
fn primitive_fields_bind_nothing() {
    let src = r#"
struct Counter {
    count: usize,
}

impl Counter {
    fn run(&self) {
        self.count.to_string();
    }
}
"#;
    assert!(!callees(&parse(src)).iter().any(|c| c.starts_with("usize::")));
}

#[test]
fn a_field_of_a_parameter_resolves_through_the_dotted_path() {
    // The real shape in this codebase: `ctx.result.add_entity()`, where `ctx`
    // is a parameter and `result` is one of its fields. Both types are
    // declared, so the whole path is exact.
    let src = r#"
struct ExtractCtx {
    result: ParseResult,
}

fn handle(ctx: &mut ExtractCtx) {
    ctx.result.add_entity();
}
"#;
    let found = callees(&parse(src));
    assert!(
        found.contains(&"ParseResult::add_entity".to_string()),
        "expected the field's type through the parameter, got {found:?}"
    );
}

#[test]
fn a_dotted_path_through_an_unknown_head_stays_unresolved() {
    // No declaration to read, so guessing would invent an edge.
    let src = r#"
fn handle(ctx: Mystery) {
    ctx.whatever.act();
}
"#;
    let found = callees(&parse(src));
    assert!(!found.contains(&"Mystery::act".to_string()), "{found:?}");
}

#[test]
fn a_call_inside_the_receiver_path_is_not_walked() {
    let src = r#"
struct Holder {
    inner: Thing,
}

fn handle(h: Holder) {
    h.make().inner.act();
}
"#;
    let found = callees(&parse(src));
    assert!(!found.contains(&"Thing::act".to_string()), "{found:?}");
}

// ---- AN-012: field types declared in another file ----
//
// The parser's half of the fix. It cannot see the other file, so what it
// owes the analyzer is the point where the walk stalled — the type it did
// resolve, plus the fields left over. Whether the hop then resolves is
// `analyzer::receiver_index`'s half.

#[test]
fn a_field_of_another_files_struct_is_handed_to_the_analyzer() {
    // The reported bug: `CodeEntity` is declared in `src/models/entity.rs`,
    // so this file's struct map cannot type `.kind`.
    let src = r#"
fn handle(entity: &CodeEntity) {
    entity.kind.is_callable();
}
"#;
    let result = parse(src);
    assert_eq!(
        deferred(&result),
        vec![("CodeEntity.kind".to_string(), "is_callable".to_string())],
        "expected the stalled path to be handed over"
    );
    // Until the analyzer finishes the walk the callee is unchanged, so a tree
    // where the type never turns up still lands on a ghost.
    assert!(
        callees(&result).contains(&"entity.kind::is_callable".to_string()),
        "{:?}",
        callees(&result)
    );
}

#[test]
fn a_self_field_of_another_files_struct_is_handed_over_too() {
    // `impl` here, `struct` there — the same stall, with `self` as the head.
    let src = r#"
impl Analyzer {
    fn run(&self) {
        self.store.get();
    }
}
"#;
    assert_eq!(
        deferred(&parse(src)),
        vec![("Analyzer.store".to_string(), "get".to_string())]
    );
}

#[test]
fn the_hand_over_starts_at_the_first_field_this_file_cannot_type() {
    // `Holder.inner` is declared here; `Thing.part` is not. Only the
    // unresolved tail travels, so the analyzer resumes from a real type.
    let src = r#"
struct Holder {
    inner: Thing,
}

fn handle(h: Holder) {
    h.inner.part.act();
}
"#;
    assert_eq!(
        deferred(&parse(src)),
        vec![("Thing.part".to_string(), "act".to_string())]
    );
}

#[test]
fn a_receiver_this_file_can_type_hands_over_nothing() {
    let src = r#"
struct ExtractCtx {
    result: ParseResult,
}

fn handle(ctx: &mut ExtractCtx) {
    ctx.result.add_entity();
}
"#;
    let result = parse(src);
    assert!(callees(&result).contains(&"ParseResult::add_entity".to_string()));
    assert!(deferred(&result).is_empty(), "{:?}", deferred(&result));
}

#[test]
fn an_untyped_head_hands_over_nothing() {
    // No declaration for `ctx` at all: there is no type to resume from, so
    // there is nothing the analyzer could finish and no edge to guess at.
    let src = r#"
fn handle(ctx) {
    ctx.whatever.act();
}
"#;
    assert!(deferred(&parse(src)).is_empty());
}

#[test]
fn a_call_inside_the_path_hands_over_nothing() {
    // `h.make()` has no declared type, so `.inner` cannot be resumed from
    // anywhere — handing over `Holder.make().inner` would invite a guess.
    let src = r#"
struct Holder {
    inner: Thing,
}

fn handle(h: Holder) {
    h.make().inner.act();
}
"#;
    assert!(deferred(&parse(src)).is_empty());
}

#[test]
fn a_receiver_that_wraps_onto_the_next_line_still_walks() {
    // Rustfmt breaks long chains after the receiver, so the segment text
    // carries the newline and indentation with it.
    let src = r#"
fn handle(ctx: &mut ExtractCtx) {
    ctx.result
        .entities
        .push(entity);
}
"#;
    assert_eq!(
        deferred(&parse(src)),
        vec![("ExtractCtx.result.entities".to_string(), "push".to_string())]
    );
}

#[test]
fn a_hand_over_rides_one_edge_per_call_site() {
    // Two calls on the same stalled receiver are two independent edges; the
    // hint is per-edge, so neither can pick up the other's path.
    let src = r#"
fn handle(entity: &CodeEntity, other: &Relationship) {
    entity.kind.is_callable();
    other.kind.is_dependency();
}
"#;
    assert_eq!(
        deferred(&parse(src)),
        vec![
            ("CodeEntity.kind".to_string(), "is_callable".to_string()),
            ("Relationship.kind".to_string(), "is_dependency".to_string()),
        ]
    );
}
