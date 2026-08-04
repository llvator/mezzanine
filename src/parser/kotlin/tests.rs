//! Integration tests for the Kotlin parser. Currently focused on the
//! `UsesType` post-pass (KT-001) — wires `types::emit_uses_type_edges`
//! through the full parser flow so the assertions cover both the token
//! extraction rule and where the parser lands each type string
//! (function signatures, primary-constructor fields, body properties).

use super::KotlinParser;
use crate::models::{EntityKind, RelationshipKind};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    let parser = KotlinParser::new();
    parser.parse(Path::new("Test.kt"), src).expect("parse")
}

/// Sorted `UsesType` target names originating from the named entity.
fn uses_type_targets(result: &ParseResult, source_name: &str) -> Vec<String> {
    let source_ids: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.name == source_name)
        .map(|e| e.id.as_str())
        .collect();
    assert!(
        !source_ids.is_empty(),
        "entity `{source_name}` not found"
    );
    let mut targets: Vec<String> = result
        .relationships
        .iter()
        .filter(|r| {
            r.kind == RelationshipKind::UsesType
                && source_ids.contains(&r.source_id.as_str())
        })
        .map(|r| r.target_id.clone())
        .collect();
    targets.sort();
    targets
}

#[test]
fn function_signature_types_yield_uses_type_edges() {
    let src = r#"
        fun process(o: Order, items: List<LineItem>): Receipt {
            return Receipt()
        }
    "#;
    let result = parse(src);
    assert_eq!(
        uses_type_targets(&result, "process"),
        vec!["LineItem", "Order", "Receipt"]
    );
}

#[test]
fn nullable_type_resolves_to_bare_name() {
    let src = r#"
        fun find(id: Int): Order? {
            return null
        }
    "#;
    let result = parse(src);
    assert_eq!(uses_type_targets(&result, "find"), vec!["Order"]);
}

#[test]
fn function_type_parameter_yields_both_sides() {
    let src = r#"
        fun handle(transform: (Order) -> Receipt) {
        }
    "#;
    let result = parse(src);
    assert_eq!(
        uses_type_targets(&result, "handle"),
        vec!["Order", "Receipt"]
    );
}

#[test]
fn data_class_primary_constructor_properties_yield_edges() {
    let src = r#"
        data class Invoice(
            val order: Order,
            val lines: List<LineItem>,
            val total: Double,
        )
    "#;
    let result = parse(src);
    // Verify the parser lands primary-constructor properties in `fields`.
    let invoice = result
        .entities
        .iter()
        .find(|e| e.name == "Invoice" && e.kind == EntityKind::Class)
        .expect("Invoice class");
    assert_eq!(invoice.fields.len(), 3);
    assert_eq!(
        uses_type_targets(&result, "Invoice"),
        vec!["LineItem", "Order"]
    );
}

#[test]
fn class_body_property_types_yield_edges() {
    let src = r#"
        class Cart {
            val pending: Order? = null
            val count: Int = 0
        }
    "#;
    let result = parse(src);
    assert_eq!(uses_type_targets(&result, "pending"), vec!["Order"]);
    assert_eq!(
        uses_type_targets(&result, "count"),
        Vec::<String>::new()
    );
}

#[test]
fn recursive_self_mention_is_skipped() {
    let src = r#"
        data class Node(val next: Node?, val payload: Payload)
    "#;
    let result = parse(src);
    assert_eq!(uses_type_targets(&result, "Node"), vec!["Payload"]);
}

#[test]
fn duplicate_types_across_parameters_dedup_to_one_edge() {
    let src = r#"
        fun merge(a: Order, b: Order): Order {
            return a
        }
    "#;
    let result = parse(src);
    assert_eq!(uses_type_targets(&result, "merge"), vec!["Order"]);
}
