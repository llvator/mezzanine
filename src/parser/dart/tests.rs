//! Integration tests for the Dart parser.
//!
//! Everything here goes through the full `parse` entry point rather than
//! calling an extractor directly, because what is most likely to break is a
//! seam: a member reaching `callables` in a shape it does not recognise, or
//! a receiver reaching the relationship builder as the wrong text. Neither
//! misbehaves anywhere but end to end.
//!
//! The Dart 3 block is the load-bearing one. It is what justifies the
//! vendored grammar, and it is what would regress silently if anyone swapped
//! that grammar for a published release.
//!
//! The unit-level rules — what counts as a type token, what a doc comment
//! strips — are tested beside the code that owns them.

use super::DartParser;
use crate::models::{EntityKind, RelationshipKind, Visibility};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    DartParser::new()
        .parse(Path::new("test.dart"), src)
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

/// Sorted targets of the given relationship kind originating from `source`.
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

// ---------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------

#[test]
fn top_level_functions_and_variables_are_entities() {
    let result = parse(
        r#"
        const double taxRate = 0.2;
        Receipt checkout(Order order) {
          return Receipt(order);
        }
        "#,
    );
    assert_eq!(names_of_kind(&result, EntityKind::Function), ["checkout"]);
    let rate = entity(&result, "taxRate");
    assert_eq!(rate.kind, EntityKind::Property);
    assert_eq!(rate.return_type.as_deref(), Some("double"));
}

#[test]
fn a_class_records_its_supertypes_mixins_and_interfaces() {
    let result = parse(
        r#"
        class CartRepository extends Repository with Auditable implements Comparable {
        }
        "#,
    );
    let cart = entity(&result, "CartRepository");
    assert_eq!(cart.kind, EntityKind::Class);
    assert_eq!(cart.extends, ["Repository"]);
    // A `with` mixin contributes members the way an interface contributes a
    // contract, and neither is the type's parent.
    assert_eq!(cart.implements, ["Auditable", "Comparable"]);
}

#[test]
fn an_abstract_class_is_its_own_kind() {
    let result = parse("abstract class Repository { Future<Order> find(String id); }");
    let repo = entity(&result, "Repository");
    assert_eq!(repo.kind, EntityKind::AbstractClass);
    assert!(repo.tags.contains("abstract"));
    // The bodyless member still scores, or `mezz quality` cannot see it.
    let find = entity(&result, "find");
    assert_eq!(find.metrics.cyclomatic, Some(1));
}

#[test]
fn members_are_parented_to_their_class() {
    let result = parse(
        r#"
        class Cart {
          final Map<String, Order> _orders = {};
          Cart(this.client);
          Cart.empty();
          factory Cart.of(Client c) => Cart(c);
          String get label => _label;
          set label(String v) => _label = v;
          int operator +(Cart other) => 1;
          void _hidden() {}
        }
        "#,
    );
    let cart_id = &entity(&result, "Cart").id;
    for name in ["_orders", "Cart.empty", "label", "operator+", "_hidden"] {
        assert_eq!(
            entity(&result, name).parent_id.as_deref(),
            Some(cart_id.as_str()),
            "`{name}` must be contained by its class"
        );
    }
    // A named constructor keeps its suffix so two never collide.
    assert!(entity(&result, "Cart.empty").tags.contains("constructor"));
    assert!(entity(&result, "Cart.of").tags.contains("factory"));
    assert!(entity(&result, "label").tags.contains("accessor"));
}

#[test]
fn a_leading_underscore_is_the_visibility_modifier() {
    let result = parse("class Cart { void _hidden() {} void shown() {} }");
    assert_eq!(entity(&result, "_hidden").visibility, Visibility::Private);
    assert_eq!(entity(&result, "shown").visibility, Visibility::Public);
}

#[test]
fn a_mixin_is_an_interface_that_records_its_on_clause() {
    let result = parse("mixin Auditable on Cart implements Loggable { void audit() {} }");
    let mixin = entity(&result, "Auditable");
    assert_eq!(mixin.kind, EntityKind::Interface);
    assert!(mixin.tags.contains("mixin"));
    assert_eq!(mixin.extends, ["Cart"]);
    assert_eq!(mixin.implements, ["Loggable"]);
    assert_eq!(entity(&result, "audit").kind, EntityKind::Method);
}

#[test]
fn an_extension_depends_on_the_type_it_extends_without_inheriting_it() {
    let result = parse("extension OrderX on Order { double get total => 0.0; }");
    let ext = entity(&result, "OrderX");
    assert!(ext.tags.contains("extension"));
    assert!(ext.extends.is_empty(), "an extension inherits nothing");
    assert!(ext.attributes.iter().any(|a| a == "on:Order"));
    assert_eq!(
        targets(&result, "OrderX", RelationshipKind::UsesType),
        ["Order"]
    );
}

#[test]
fn enum_constants_land_in_fields() {
    let result = parse("enum Status { pending, shipped, delivered }");
    let status = entity(&result, "Status");
    assert_eq!(status.kind, EntityKind::Enum);
    let names: Vec<&str> = status.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["pending", "shipped", "delivered"]);
}

#[test]
fn documentation_survives_an_annotation_written_between() {
    let result = parse(
        r#"
        class Cart {
          /// Finds an order.
          @override
          Future<Order> find(String id) async { return load(id); }
        }
        "#,
    );
    let find = entity(&result, "find");
    assert_eq!(find.documentation.as_deref(), Some("Finds an order."));
    assert!(find.attributes.iter().any(|a| a == "@override"));
}

// ---------------------------------------------------------------------
// Dart 3
//
// The vendored grammar exists for these. Every one of them was an `ERROR`
// node under the published ABI-14 grammar, and an `ERROR` at declaration
// level costs the whole declaration and damages recovery for the next one —
// so these assertions are what the vendoring buys, and what would regress if
// anyone swapped the grammar back. See `vendor/tree-sitter-dart/README.md`.
// ---------------------------------------------------------------------

#[test]
fn every_class_modifier_keeps_its_declaration_and_becomes_a_tag() {
    let result = parse(
        r#"
        sealed class Shape {}
        base class Square extends Shape { void draw() {} }
        final class Locked {}
        interface class Contract {}
        abstract interface class Both {}
        mixin class Blended {}
        base mixin Audited {}
        "#,
    );
    assert_eq!(
        names_of_kind(&result, EntityKind::Class),
        ["Blended", "Contract", "Locked", "Shape", "Square"]
    );
    // `abstract` still decides the kind, whatever else is written with it.
    assert_eq!(names_of_kind(&result, EntityKind::AbstractClass), ["Both"]);
    for (name, modifier) in [
        ("Shape", "sealed"),
        ("Square", "base"),
        ("Locked", "final"),
        ("Contract", "interface"),
        ("Blended", "mixin"),
        ("Audited", "base"),
    ] {
        assert!(
            entity(&result, name).tags.contains(modifier),
            "`{modifier} class {name}` must carry the `{modifier}` tag"
        );
    }
    assert_eq!(entity(&result, "Square").extends, ["Shape"]);
    // The body of a modified class is still walked.
    assert_eq!(entity(&result, "draw").kind, EntityKind::Method);
    // `base mixin` is still a mixin, not a class.
    assert_eq!(entity(&result, "Audited").kind, EntityKind::Interface);
}

#[test]
fn an_extension_type_is_a_container_over_its_representation() {
    let result = parse("extension type Meters(int value) { Meters twice() => this; }");
    let meters = entity(&result, "Meters");
    assert!(meters.tags.contains("extension_type"));
    assert_eq!(entity(&result, "twice").kind, EntityKind::Method);
    assert_eq!(
        entity(&result, "twice").parent_id.as_deref(),
        Some(meters.id.as_str())
    );
}

#[test]
fn record_types_and_pattern_switches_do_not_cost_their_callable() {
    let result = parse(
        r#"
        class Router {
          (int, String) split(String raw) => (1, raw);

          String describe(Object o) => switch (o) {
            int i when i > 0 => positive(),
            (int a, int b) => pair(),
            _ => other(),
          };
        }
        "#,
    );
    // Both members survive, which is the part the old grammar could not do.
    assert_eq!(entity(&result, "split").kind, EntityKind::Method);
    assert_eq!(entity(&result, "describe").kind, EntityKind::Method);
    // And the calls written inside the pattern arms are still edges.
    assert_eq!(
        calls(&result, "describe"),
        ["Router.other", "Router.pair", "Router.positive"]
    );
}

#[test]
fn a_declaration_reports_the_source_a_reader_wrote() {
    let src = "/// A shape.\nsealed class Shape {}\n";
    let result = parse(src);
    let shape = entity(&result, "Shape");
    // The span reaches back over the documentation, so `source_code` is the
    // declaration as written rather than as the grammar's node bounds it.
    assert_eq!(shape.span.start.offset, 0);
    assert_eq!(
        shape.source_code.as_deref(),
        Some("/// A shape.\nsealed class Shape {}")
    );
    assert_eq!(shape.documentation.as_deref(), Some("A shape."));
}

// ---------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------

#[test]
fn a_bare_call_qualifies_with_its_enclosing_class() {
    let result = parse("class Cart { void checkout() { settle(); } void settle() {} }");
    assert_eq!(calls(&result, "checkout"), ["Cart.settle"]);
}

#[test]
fn a_receiver_qualifies_the_callee() {
    let result = parse(
        r#"
        class Cart {
          void load(String id) {
            client.fetch(id);
            this.reset();
          }
        }
        "#,
    );
    assert_eq!(calls(&result, "load"), ["Cart.reset", "client.fetch"]);
}

#[test]
fn a_chain_attributes_each_call_to_what_precedes_it() {
    let result = parse("void run() { repo.open().session.commit(); }");
    assert_eq!(
        calls(&result, "run"),
        ["repo.open", "repo.open().session.commit"]
    );
}

#[test]
fn an_uppercase_bare_call_is_a_constructor() {
    let result = parse("void run() { final o = Order(1); }");
    assert_eq!(
        targets(&result, "run", RelationshipKind::Instantiates),
        ["Order"]
    );
}

#[test]
fn the_explicit_new_and_const_forms_instantiate_too() {
    let result = parse("void run() { new Widget(); const Padding(); new http.Client(); }");
    assert_eq!(
        targets(&result, "run", RelationshipKind::Instantiates),
        ["Client", "Padding", "Widget"]
    );
}

#[test]
fn a_bound_call_records_what_it_binds_to() {
    let result = parse("class Cart { void run() { final Order o = repo.find('1'); } }");
    let rel = result
        .relationships
        .iter()
        .find(|r| r.target_id == "repo.find")
        .expect("the call must be recorded");
    assert_eq!(rel.metadata.get("binds_to").map(String::as_str), Some("o"));
    assert_eq!(
        rel.metadata.get("binds_type").map(String::as_str),
        Some("Order")
    );
}

#[test]
fn awaiting_a_call_binds_the_call_and_not_the_await() {
    let result = parse("class Cart { void run() async { final r = await repo.find('1'); } }");
    let rel = result
        .relationships
        .iter()
        .find(|r| r.target_id == "repo.find")
        .expect("the awaited call must be recorded");
    assert_eq!(rel.metadata.get("binds_to").map(String::as_str), Some("r"));
}

#[test]
fn a_cascade_call_keeps_its_receiver() {
    let result = parse("void run() { builder..open()..seal(); }");
    assert_eq!(calls(&result, "run"), ["builder.open", "builder.seal"]);
}

#[test]
fn core_library_noise_is_filtered_but_a_bound_result_is_not() {
    let result = parse(
        r#"
        void run() {
          items.forEach(visit);
          rows.toList();
          final kept = rows.toList();
        }
        "#,
    );
    // `forEach` is core noise and unbound, so it never becomes an edge;
    // the bound `toList` does, because the binding says the value matters.
    assert_eq!(calls(&result, "run"), ["rows.toList"]);
}

#[test]
fn calls_inside_a_nested_class_stay_with_the_nested_member() {
    let result = parse(
        r#"
        class Outer {
          void run() { helper(); }
        }
        class Inner {
          void run2() { other(); }
        }
        "#,
    );
    assert_eq!(calls(&result, "run"), ["Outer.helper"]);
    assert_eq!(calls(&result, "run2"), ["Inner.other"]);
}

// ---------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------

#[test]
fn if_else_arms_are_siblings_not_nested() {
    let result = parse(
        r#"
        void run(int a) {
          if (a == 1) { one(); }
          else if (a == 2) { two(); }
          else { rest(); }
        }
        "#,
    );
    let mut arms: Vec<String> = result
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Branch)
        .map(|e| e.name.clone())
        .collect();
    arms.sort();
    assert_eq!(arms, ["c1", "c2", "c3"]);
}

#[test]
fn a_loop_body_becomes_a_loop_entity_that_owns_its_calls() {
    let result = parse("void run() { for (final x in fetch()) { visit(x); } }");
    let loops: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Loop)
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(loops, ["l1"]);
    // The iterable runs on every pass, so it belongs to the loop too.
    for target in ["fetch", "visit"] {
        let rel = result
            .relationships
            .iter()
            .find(|r| r.target_id == target)
            .unwrap_or_else(|| panic!("`{target}` must be recorded"));
        assert_eq!(rel.metadata.get("branch").map(String::as_str), Some("l1"));
    }
}

#[test]
fn a_catch_arm_records_the_type_its_on_clause_names() {
    let result = parse(
        r#"
        void run() {
          try { risky(); }
          on FormatException catch (e) { report(e); }
          finally { cleanup(); }
        }
        "#,
    );
    let arms: Vec<&crate::models::CodeEntity> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("try_arm"))
        .collect();
    assert_eq!(arms.len(), 3, "try body, handler and finally");
    let handler = arms
        .iter()
        .find(|e| e.tags.contains("catch_arm"))
        .expect("a catch arm");
    assert!(handler
        .attributes
        .iter()
        .any(|a| a == "caught:FormatException"));
}

#[test]
fn each_switch_label_becomes_its_own_arm() {
    let result = parse(
        r#"
        void run(int a) {
          switch (a) {
            case 1: one(); break;
            case 2: two(); break;
            default: rest();
          }
        }
        "#,
    );
    let arms: Vec<&crate::models::CodeEntity> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("case_arm"))
        .collect();
    assert_eq!(arms.len(), 3);
    assert!(arms[0].attributes.iter().any(|a| a == "pattern:1"));
}

// ---------------------------------------------------------------------
// Complexity
// ---------------------------------------------------------------------

#[test]
fn branching_raises_cyclomatic_complexity() {
    let result = parse(
        r#"
        void run(int a, int b) {
          if (a > 0 && b > 0) {
            for (var i = 0; i < a; i++) {
              if (i == b) { hit(); }
            }
          } else {
            miss();
          }
        }
        "#,
    );
    let run = entity(&result, "run");
    // if + && + for + if + else = 5 branches over the base path.
    assert_eq!(run.metrics.cyclomatic, Some(6));
    assert!(run.metrics.max_nesting.unwrap() >= 3);
    assert!(run.metrics.cognitive_complexity.unwrap() > 0);
}

#[test]
fn null_coalescing_branches_but_null_aware_access_does_not() {
    let coalesce = parse("void run() { final v = a ?? fallback(); }");
    assert_eq!(entity(&coalesce, "run").metrics.cyclomatic, Some(2));

    let aware = parse("void run() { a?.touch(); }");
    assert_eq!(entity(&aware, "run").metrics.cyclomatic, Some(1));
}

#[test]
fn a_straight_line_body_scores_one() {
    let result = parse("void run() { one(); two(); three(); }");
    let run = entity(&result, "run");
    assert_eq!(run.metrics.cyclomatic, Some(1));
    assert_eq!(run.metrics.max_nesting, Some(0));
    assert_eq!(run.metrics.cognitive_complexity, Some(0));
}

// ---------------------------------------------------------------------
// Types and imports
// ---------------------------------------------------------------------

#[test]
fn signature_types_yield_uses_type_edges() {
    let result = parse("Receipt process(Order o, List<LineItem> items) { return Receipt(); }");
    assert_eq!(
        targets(&result, "process", RelationshipKind::UsesType),
        ["LineItem", "Order", "Receipt"]
    );
}

#[test]
fn a_field_type_yields_an_edge_and_a_self_reference_does_not() {
    let result = parse("class Node { Node? next; Payload payload; }");
    assert_eq!(
        targets(&result, "next", RelationshipKind::UsesType),
        Vec::<String>::new()
    );
    assert_eq!(
        targets(&result, "payload", RelationshipKind::UsesType),
        ["Payload"]
    );
}

#[test]
fn a_typedef_depends_on_what_it_aliases() {
    let result = parse("typedef Reducer = Receipt Function(Order);");
    let alias = entity(&result, "Reducer");
    assert_eq!(alias.kind, EntityKind::TypeAlias);
    assert_eq!(
        targets(&result, "Reducer", RelationshipKind::UsesType),
        ["Order", "Receipt"]
    );
}

#[test]
fn imports_record_uri_alias_and_shown_names() {
    let result = parse(
        r#"
        import 'dart:async';
        import 'package:http/http.dart' as http;
        import '../models/order.dart' show Order, LineItem;
        export 'src/cart_impl.dart';
        part 'cart.g.dart';
        "#,
    );
    let by_path = |path: &str| {
        result
            .imports
            .iter()
            .find(|i| i.path == path)
            .unwrap_or_else(|| panic!("import `{path}` not found"))
    };

    assert!(!by_path("dart:async").is_relative);
    assert!(by_path("dart:async").is_wildcard);
    assert_eq!(
        by_path("package:http/http.dart").alias.as_deref(),
        Some("http")
    );
    let relative = by_path("../models/order.dart");
    assert!(relative.is_relative);
    assert_eq!(relative.items, ["Order", "LineItem"]);
    assert_eq!(by_path("src/cart_impl.dart").items, ["export"]);
    assert_eq!(by_path("cart.g.dart").items, ["part"]);
}

#[test]
fn a_library_directive_documents_the_file() {
    let result = parse("/// The shopping cart.\nlibrary shop.cart;\n\nclass Cart {}\n");
    assert_eq!(
        result.file_documentation.as_deref(),
        Some("The shopping cart.")
    );
    assert_eq!(entity(&result, "Cart").qualified_name, "shop.cart.Cart");
}

#[test]
fn a_leading_comment_with_no_library_directive_stays_with_its_declaration() {
    let result = parse("/// Scores a value.\nint score(int x) => x;\n");
    assert_eq!(result.file_documentation, None);
    assert_eq!(
        entity(&result, "score").documentation.as_deref(),
        Some("Scores a value.")
    );
}

#[test]
fn a_pattern_switch_expression_branches_like_a_switch_statement() {
    let expression = parse(
        r#"
        String classify(Object o) => switch (o) {
          int i when i > 0 => positive(),
          String s => text(),
          _ => other(),
        };
        "#,
    );
    let statement = parse(
        r#"
        String classify(Object o) {
          switch (o) {
            case 1: return positive();
            case 2: return text();
            default: return other();
          }
        }
        "#,
    );
    // Three arms either way, so the two spellings must score the same — a
    // Dart 3 codebase that prefers the expression form must not look simpler
    // than one that does not.
    assert_eq!(
        entity(&expression, "classify").metrics.cyclomatic,
        Some(5),
        "three arms plus the `when` guard, over the base path"
    );
    assert_eq!(entity(&statement, "classify").metrics.cyclomatic, Some(4));
}

#[test]
fn a_call_hanging_off_a_multiline_closure_mints_no_ghost() {
    let result = parse(
        r#"
        void rank(List<Spend> buckets) {
          buckets.sort((a, b) =>
            b.cents != a.cents ? b.cents.compareTo(a.cents) : a.label.compareTo(b.label));
        }
        "#,
    );
    // No target may be a block of source. A receiver that spans lines is an
    // expression, not a name, and naming an entity after it is how a graph
    // fills up with unreadable nodes.
    for rel in &result.relationships {
        assert!(
            !rel.target_id.contains('\n'),
            "target `{}` is source text, not a name",
            rel.target_id
        );
    }
    // The calls written inside the closure are still found.
    assert!(calls(&result, "rank")
        .iter()
        .any(|c| c.contains("compareTo")));
}
