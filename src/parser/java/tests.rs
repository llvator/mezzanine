//! Integration tests for the Java parser. Currently focused on the
//! per-callable complexity metrics — wires `compute_complexity` through
//! the full parser flow so the assertions cover both the algorithm and
//! the `declarations::callables::populate_body_metrics` glue.

use super::JavaParser;
use crate::models::{CodeEntity, EntityKind};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    let parser = JavaParser::new();
    parser.parse(Path::new("Test.java"), src).expect("parse")
}

fn method<'a>(result: &'a ParseResult, name: &str) -> &'a CodeEntity {
    result
        .entities
        .iter()
        .find(|e| e.name == name && matches!(e.kind, EntityKind::Method | EntityKind::Function))
        .unwrap_or_else(|| panic!("method `{name}` not found"))
}

#[test]
fn straight_line_method_has_cyclomatic_one() {
    let src = r#"
        class A {
            void m() {
                foo();
                bar();
            }
        }
    "#;
    let result = parse(src);
    let m = method(&result, "m");
    assert_eq!(m.metrics.cyclomatic, Some(1));
    assert_eq!(m.metrics.max_nesting, Some(0));
    assert_eq!(m.metrics.cognitive_complexity, Some(0));
}

#[test]
fn each_branch_increments_cyclomatic() {
    // 1 base + 1 if + 1 while + 1 `&&` + 1 `||` = 5
    let src = r#"
        class A {
            void m(boolean a, boolean b, boolean c) {
                if (a && b) {
                    while (c || a) {
                        foo();
                    }
                }
            }
        }
    "#;
    let result = parse(src);
    let m = method(&result, "m");
    assert_eq!(m.metrics.cyclomatic, Some(5));
    assert_eq!(m.metrics.max_nesting, Some(2));
}

#[test]
fn else_branch_adds_one_but_else_if_does_not_double_count() {
    // if/else = 2 branches → CC contribution of 2. else if chains as a
    // nested if_statement that's counted on its own visit.
    let if_else_src = r#"
        class A {
            void m(int x) {
                if (x > 0) { foo(); } else { bar(); }
            }
        }
    "#;
    let if_else = parse(if_else_src);
    assert_eq!(method(&if_else, "m").metrics.cyclomatic, Some(3));

    let if_elseif_else_src = r#"
        class A {
            void m(int x) {
                if (x > 0) { foo(); }
                else if (x < 0) { bar(); }
                else { baz(); }
            }
        }
    "#;
    let if_elseif_else = parse(if_elseif_else_src);
    // 1 base + 1 outer if + 1 inner if (else-if) + 1 terminal else = 4.
    assert_eq!(method(&if_elseif_else, "m").metrics.cyclomatic, Some(4));
}

#[test]
fn switch_each_case_counts() {
    // 1 base + 3 case labels + 1 default label = 5
    let src = r#"
        class A {
            void m(int x) {
                switch (x) {
                    case 1: foo(); break;
                    case 2: bar(); break;
                    case 3: baz(); break;
                    default: quux();
                }
            }
        }
    "#;
    let result = parse(src);
    let m = method(&result, "m");
    assert_eq!(m.metrics.cyclomatic, Some(5));
}

#[test]
fn cognitive_weights_nested_branches_by_depth() {
    // Flat if: +1 cognitive. Nested if-in-if: +1 (outer) + 2 (inner at depth 1) = 3.
    let flat_src = r#"
        class A {
            void m(int x, int y) {
                if (x > 0) foo();
            }
        }
    "#;
    let flat = parse(flat_src);
    assert_eq!(method(&flat, "m").metrics.cognitive_complexity, Some(1));

    let nested_src = r#"
        class A {
            void m(int x, int y) {
                if (x > 0) {
                    if (y > 0) foo();
                }
            }
        }
    "#;
    let nested = parse(nested_src);
    assert_eq!(method(&nested, "m").metrics.cognitive_complexity, Some(3));
}

#[test]
fn abstract_method_has_default_metrics() {
    let src = r#"
        abstract class A {
            abstract void m();
        }
    "#;
    let result = parse(src);
    let m = method(&result, "m");
    assert_eq!(m.metrics.cyclomatic, Some(1));
    assert_eq!(m.metrics.max_nesting, Some(0));
    assert_eq!(m.metrics.cognitive_complexity, Some(0));
}

#[test]
fn interface_method_has_default_metrics() {
    let src = r#"
        interface I {
            void m();
        }
    "#;
    let result = parse(src);
    let m = method(&result, "m");
    assert_eq!(m.metrics.cyclomatic, Some(1));
}

#[test]
fn loc_param_count_populated() {
    let src = r#"
        class A {
            void m(int a, int b, int c) {
                foo();
            }
        }
    "#;
    let result = parse(src);
    let m = method(&result, "m");
    assert_eq!(m.metrics.param_count, Some(3));
    assert!(m.metrics.loc >= 3);
}

// ---------------------------------------------------------------------------
// UsesType post-pass (JV-001)
// ---------------------------------------------------------------------------

use crate::models::RelationshipKind;

/// (source entity name, target type name) pairs of all UsesType edges.
fn uses_type_edges(result: &ParseResult) -> Vec<(String, String)> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::UsesType)
        .map(|r| {
            let source_name = result
                .entities
                .iter()
                .find(|e| e.id == r.source_id)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| r.source_id.clone());
            (source_name, r.target_id.clone())
        })
        .collect()
}

#[test]
fn method_signature_types_produce_uses_type_edges() {
    let src = r#"
        class Checkout {
            Receipt process(Order o, List<LineItem> items) {
                return null;
            }
        }
    "#;
    let result = parse(src);
    let mut targets: Vec<_> = uses_type_edges(&result)
        .into_iter()
        .filter(|(s, _)| s == "process")
        .map(|(_, t)| t)
        .collect();
    targets.sort();
    assert_eq!(targets, vec!["LineItem", "Order", "Receipt"]);
}

#[test]
fn field_types_produce_uses_type_edges() {
    let src = r#"
        class Checkout {
            private OrderRepository repository;
            private Map<String, Discount> discounts;
            private int count;
        }
    "#;
    let result = parse(src);
    let edges = uses_type_edges(&result);
    assert!(
        edges.contains(&("repository".into(), "OrderRepository".into())),
        "{edges:?}"
    );
    assert!(
        edges.contains(&("discounts".into(), "Discount".into())),
        "{edges:?}"
    );
    // Primitives and std generics carry no signal.
    assert!(
        !edges.iter().any(|(_, t)| t == "Map" || t == "String"),
        "{edges:?}"
    );
}

#[test]
fn record_component_types_produce_uses_type_edges() {
    let src = r#"
        record Receipt(Order order, List<LineItem> items, BigDecimal total) {}
    "#;
    let result = parse(src);
    let mut targets: Vec<_> = uses_type_edges(&result)
        .into_iter()
        .filter(|(s, _)| s == "Receipt")
        .map(|(_, t)| t)
        .collect();
    targets.sort();
    assert_eq!(targets, vec!["LineItem", "Order"]);
}

#[test]
fn annotations_never_become_type_targets() {
    let src = r#"
        class OrderService {
            @Autowired
            private OrderRepository repository;

            @Transactional
            Receipt handle(@Valid Order order) {
                return null;
            }
        }
    "#;
    let result = parse(src);
    let edges = uses_type_edges(&result);
    for anno in ["Autowired", "Transactional", "Valid"] {
        assert!(!edges.iter().any(|(_, t)| t == anno), "{edges:?}");
    }
    assert!(
        edges.contains(&("handle".into(), "Order".into())),
        "{edges:?}"
    );
    assert!(
        edges.contains(&("handle".into(), "Receipt".into())),
        "{edges:?}"
    );
    assert!(
        edges.contains(&("repository".into(), "OrderRepository".into())),
        "{edges:?}"
    );
}

#[test]
fn quirky_type_shapes_resolve_to_the_inner_name() {
    let src = r#"
        class Batch {
            void arrays(Order[] orders) {}
            void qualified(com.acme.Order order) {}
            void wildcard(List<? extends Order> orders) {}
            void varargs(Order... orders) {}
        }
    "#;
    let result = parse(src);
    let edges = uses_type_edges(&result);
    for m in ["arrays", "qualified", "wildcard", "varargs"] {
        assert!(
            edges.contains(&(m.to_string(), "Order".into())),
            "{m}: {edges:?}"
        );
    }
    assert_eq!(
        edges.iter().filter(|(_, t)| t != "Order").count(),
        0,
        "{edges:?}"
    );
}

#[test]
fn recursive_self_mentions_are_skipped() {
    let src = r#"
        class Node {
            private Node next;
            Node find(Node target) { return null; }
        }
    "#;
    let result = parse(src);
    let edges = uses_type_edges(&result);
    // The field's own container type carries no signal; the method's
    // name differs from `Node`, so its signature edges stay (parity
    // with the Rust post-pass, which only skips name-identical types).
    assert!(
        !edges.contains(&("next".into(), "Node".into())),
        "{edges:?}"
    );
    assert!(edges.contains(&("find".into(), "Node".into())), "{edges:?}");
}

#[test]
fn enum_constant_arguments_do_not_become_type_targets() {
    let src = r#"
        enum Currency {
            USD("US Dollar", 2),
            EUR("Euro", 2);

            private final String label;
        }
    "#;
    let result = parse(src);
    let edges = uses_type_edges(&result);
    assert!(edges.is_empty(), "{edges:?}");
}
