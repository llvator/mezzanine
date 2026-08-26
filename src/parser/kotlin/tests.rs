//! Integration tests for the Kotlin parser: the `UsesType` post-pass
//! (KT-001), and the body metrics plus control-flow entities of KT-002.
//!
//! Every case runs through the full parser flow, so the assertions cover
//! both the extraction rule and where the parser lands its result — which
//! matters most for KT-002, where `tree-sitter-kotlin 0.3` names no fields
//! and every part of a construct is found positionally.

use super::KotlinParser;
use crate::models::{EntityKind, RelationshipKind};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    let parser = KotlinParser::new();
    parser.parse(Path::new("Test.kt"), src).expect("parse")
}

/// The `(cyclomatic, max_nesting, cognitive)` triple of the named entity.
fn metrics_of(result: &ParseResult, name: &str) -> (u32, u32, u32) {
    let entity = result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named {name}"));
    (
        entity.metrics.cyclomatic.unwrap(),
        entity.metrics.max_nesting.unwrap(),
        entity.metrics.cognitive_complexity.unwrap(),
    )
}

/// Names of every entity of the given kind, in emission order.
fn names_of_kind(result: &ParseResult, kind: EntityKind) -> Vec<String> {
    result
        .entities
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.name.clone())
        .collect()
}

/// `(target, branch-path)` for every edge that carries a `branch` key.
fn branch_tagged(result: &ParseResult) -> Vec<(String, String)> {
    result
        .relationships
        .iter()
        .filter_map(|r| {
            r.metadata
                .get("branch")
                .map(|b| (r.target_id.clone(), b.clone()))
        })
        .collect()
}

/// Sorted `UsesType` target names originating from the named entity.
fn uses_type_targets(result: &ParseResult, source_name: &str) -> Vec<String> {
    let source_ids: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.name == source_name)
        .map(|e| e.id.as_str())
        .collect();
    assert!(!source_ids.is_empty(), "entity `{source_name}` not found");
    let mut targets: Vec<String> = result
        .relationships
        .iter()
        .filter(|r| {
            r.kind == RelationshipKind::UsesType && source_ids.contains(&r.source_id.as_str())
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
    assert_eq!(uses_type_targets(&result, "count"), Vec::<String>::new());
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

// ---------------------------------------------------------------------------
// KT-002 — body metrics
// ---------------------------------------------------------------------------

/// The audit's gnarly fixture, ported from Java. Java scores 12 / 31 / 6 on
/// it; this pins Kotlin to the same numbers, so a grammar bump that silently
/// changes the scoring fails here rather than in a user's report.
const GNARLY: &str = r#"
class A {
  fun gnarly(a: Int, b: Int, c: Int, d: Int): Int {
    if (a > 0) { for (x in 0 until b) { if (x > 0 && c > 0) { while (d > 0) { try { if (x > 1) {} else if (x < 0) {} else {} } catch (e: RuntimeException) {} } } } }
    else if (b > 0 || c > 0) { return 1 }
    return 2
  }
}
"#;

#[test]
fn gnarly_fixture_scores_the_same_as_java() {
    assert_eq!(metrics_of(&parse(GNARLY), "gnarly"), (12, 6, 31));
}

#[test]
fn a_straight_line_function_is_cyclomatic_one() {
    let res = parse("fun f(): Int {\n    val x = 1\n    return x\n}");
    assert_eq!(metrics_of(&res, "f"), (1, 0, 0));
}

#[test]
fn short_circuit_operators_each_add_one_to_both_scores() {
    // `&&` and `||` are their own node kinds in this grammar rather than
    // operators inside a shared binary node.
    let res = parse("fun f(a: Boolean): Boolean {\n    return a && a || a\n}");
    assert_eq!(metrics_of(&res, "f"), (3, 0, 2));
}

#[test]
fn an_if_used_as_an_expression_still_branches() {
    // `if` is an expression in Kotlin; an expression body is measured like
    // a block body.
    let res = parse("fun classify(a: Boolean): Int = if (a) 1 else 2");
    assert_eq!(metrics_of(&res, "classify"), (3, 1, 2));
}

#[test]
fn a_chained_else_if_is_not_counted_twice() {
    // The chained `else if` scores itself; only the terminal `else` adds the
    // extra path. Flat: 1 + if + else-if + terminal else.
    let res = parse("fun f(a: Boolean) {\n    if (a) {} else if (a) {} else {}\n}");
    let (cyclomatic, _, _) = metrics_of(&res, "f");
    assert_eq!(cyclomatic, 4);
}

#[test]
fn elvis_is_a_branch() {
    // `?:` picks one of two paths, filling the slot Java's ternary occupies.
    let res = parse("fun f(a: String?): String = a ?: \"fallback\"");
    assert_eq!(metrics_of(&res, "f"), (2, 1, 1));
}

#[test]
fn safe_call_is_deliberately_not_a_branch() {
    // Documented in complexity.rs: `?.` is pervasive in Kotlin where Java and
    // Rust write nothing, so counting it would make the scores incomparable
    // across languages. TS-003 took the same decision for `?.`.
    let res = parse("fun f(a: Box?): String? = a?.inner?.name");
    assert_eq!(metrics_of(&res, "f"), (1, 0, 0));
}

#[test]
fn when_arms_add_to_cyclomatic_but_not_to_cognitive() {
    let src = "fun f(a: Int) {\n\
               when (a) {\n\
               1 -> one()\n\
               2 -> two()\n\
               else -> rest()\n\
               }\n\
               }";
    let (cyclomatic, _, cognitive) = metrics_of(&parse(src), "f");
    assert_eq!(cyclomatic, 4, "one per entry plus `else`, plus the base");
    assert_eq!(cognitive, 1, "the `when` itself, not its entries");
}

#[test]
fn every_loop_form_adds_to_both_scores() {
    let src = "fun f(xs: List<Int>) {\n\
               for (x in xs) { visit(x) }\n\
               while (ready()) { step() }\n\
               do { step() } while (ready())\n\
               }";
    assert_eq!(metrics_of(&parse(src), "f"), (4, 1, 3));
}

#[test]
fn a_catch_block_adds_to_both_scores() {
    let src = "fun f() {\n\
               try { risky() } catch (e: IllegalStateException) { report() } finally { cleanup() }\n\
               }";
    assert_eq!(metrics_of(&parse(src), "f"), (2, 1, 1));
}

#[test]
fn a_lambda_deepens_cognitive_nesting_without_branching() {
    let res = parse("fun f(xs: List<Int>) {\n    xs.forEach { x -> if (x > 0) { visit(x) } }\n}");
    let (cyclomatic, _, cognitive) = metrics_of(&res, "f");
    assert_eq!(
        cyclomatic, 2,
        "the inner `if` branches; the lambda does not"
    );
    assert_eq!(cognitive, 3, "lambda@0 = 1, inner if@1 = 2");
}

#[test]
fn a_scope_function_block_nests_like_any_other_lambda() {
    // `let` / `run` / `apply` / `also` / `with` need no special case: their
    // block argument is a `lambda_literal`, which is what is counted.
    let res = parse("fun f(a: Box) {\n    a.let { if (it.ready) { visit(it) } }\n}");
    assert_eq!(metrics_of(&res, "f"), (2, 2, 3));
}

#[test]
fn bodyless_declarations_get_one_rather_than_none() {
    // KT-002: `None` makes `mezz quality` skip the entity entirely, because
    // the hotspot ranking filters on `composite_score > 0.0`.
    let res = parse("interface Store {\n    fun load(id: String): Widget\n}");
    assert_eq!(metrics_of(&res, "load"), (1, 0, 0));
}

#[test]
fn abstract_declarations_get_one_rather_than_none() {
    let res = parse("abstract class Base {\n    abstract fun run(a: String)\n}");
    assert_eq!(metrics_of(&res, "run"), (1, 0, 0));
}

#[test]
fn functions_carry_loc_and_param_count() {
    let res = parse("class S {\n    fun run(a: String, b: String) {\n        return\n    }\n}");
    let entity = res.entities.iter().find(|e| e.name == "run").unwrap();
    assert_eq!(entity.metrics.param_count, Some(2));
    assert_eq!(entity.metrics.loc, 3);
}

// ---------------------------------------------------------------------------
// KT-002 — control-flow entities
// ---------------------------------------------------------------------------

#[test]
fn the_gnarly_fixture_yields_the_same_arm_counts_as_java() {
    // Java: 8 Branch + 2 Loop on this fixture.
    let res = parse(GNARLY);
    assert_eq!(names_of_kind(&res, EntityKind::Branch).len(), 8);
    assert_eq!(names_of_kind(&res, EntityKind::Loop).len(), 2);
}

#[test]
fn an_if_else_chain_flattens_into_sibling_arms() {
    let src = "fun f(a: Int) {\n\
               if (a == 1) { one() } else if (a == 2) { two() } else { rest() }\n\
               }";
    let res = parse(src);
    assert_eq!(
        names_of_kind(&res, EntityKind::Branch),
        vec!["c1", "c2", "c3"]
    );
    let tagged = branch_tagged(&res);
    assert!(tagged.contains(&("one".into(), "c1".into())), "{tagged:?}");
    assert!(tagged.contains(&("two".into(), "c2".into())), "{tagged:?}");
    assert!(tagged.contains(&("rest".into(), "c3".into())), "{tagged:?}");
}

#[test]
fn nested_arms_carry_a_dotted_ancestry_path() {
    let res = parse("fun f(a: Boolean) {\n    if (a) { if (a) { inner() } }\n}");
    assert_eq!(names_of_kind(&res, EntityKind::Branch), vec!["c1", "c1.c2"]);
    assert!(branch_tagged(&res).contains(&("inner".into(), "c1.c2".into())));
}

#[test]
fn loops_use_their_own_path_prefix() {
    let src = "fun f(xs: List<String>) {\n\
               for (x in xs) { visit(x) }\n\
               while (ready()) { step() }\n\
               }";
    let res = parse(src);
    assert_eq!(names_of_kind(&res, EntityKind::Loop), vec!["l1", "l2"]);
    let tagged = branch_tagged(&res);
    assert!(
        tagged.contains(&("visit".into(), "l1".into())),
        "{tagged:?}"
    );
    // The `while` condition runs every iteration, so it groups with the loop.
    assert!(
        tagged.contains(&("ready".into(), "l2".into())),
        "{tagged:?}"
    );
    assert!(tagged.contains(&("step".into(), "l2".into())), "{tagged:?}");
}

#[test]
fn all_three_loop_forms_emit_a_loop_entity() {
    let src = "fun f(xs: List<Int>) {\n\
               for (x in xs) { visit(x) }\n\
               while (ready()) { step() }\n\
               do { step() } while (ready())\n\
               }";
    assert_eq!(names_of_kind(&parse(src), EntityKind::Loop).len(), 3);
}

#[test]
fn when_entries_are_tagged_and_carry_their_pattern() {
    let src = "fun f(a: Int) {\n\
               when (a) {\n\
               1 -> hit()\n\
               else -> miss()\n\
               }\n\
               }";
    let res = parse(src);
    let arms: Vec<_> = res
        .entities
        .iter()
        .filter(|e| e.tags.contains("case_arm"))
        .collect();
    assert_eq!(arms.len(), 2);
    assert!(arms[0].attributes.contains(&"pattern:1".to_string()));
    assert!(
        arms[1]
            .attributes
            .iter()
            .all(|a| !a.starts_with("pattern:")),
        "`else ->` has no pattern"
    );
    let tagged = branch_tagged(&res);
    assert!(tagged.contains(&("hit".into(), "c1".into())), "{tagged:?}");
    assert!(tagged.contains(&("miss".into(), "c2".into())), "{tagged:?}");
}

#[test]
fn a_multi_condition_when_entry_records_every_condition() {
    let src = "fun f(a: Int) {\n\
               when (a) {\n\
               1, 2 -> hit()\n\
               }\n\
               }";
    let res = parse(src);
    let arm = res
        .entities
        .iter()
        .find(|e| e.tags.contains("case_arm"))
        .expect("case arm");
    assert!(
        arm.attributes.contains(&"pattern:1, 2".to_string()),
        "{:?}",
        arm.attributes
    );
}

#[test]
fn try_arms_are_tagged_by_kind_and_carry_the_caught_type() {
    let src = "fun f() {\n\
               try { risky() } catch (e: IllegalStateException) { report() } finally { cleanup() }\n\
               }";
    let res = parse(src);
    let arms: Vec<_> = res
        .entities
        .iter()
        .filter(|e| e.tags.contains("try_arm"))
        .collect();
    assert_eq!(arms.len(), 3);
    assert!(arms[0].tags.contains("try_body_arm"));
    assert!(arms[1].tags.contains("catch_arm"));
    assert!(arms[2].tags.contains("finally_arm"));
    assert!(
        arms[1]
            .attributes
            .contains(&"caught:IllegalStateException".to_string()),
        "{:?}",
        arms[1].attributes
    );
    let tagged = branch_tagged(&res);
    assert!(
        tagged.contains(&("risky".into(), "c1".into())),
        "{tagged:?}"
    );
    assert!(
        tagged.contains(&("report".into(), "c2".into())),
        "{tagged:?}"
    );
    assert!(
        tagged.contains(&("cleanup".into(), "c3".into())),
        "{tagged:?}"
    );
}

#[test]
fn an_empty_catch_still_produces_an_arm() {
    // This grammar inlines a block's braces into its parent, so an empty
    // `catch { }` leaves no node to span the arm against. The arm is spanned
    // against the clause instead rather than being dropped.
    let src = "fun f() {\n    try { risky() } catch (e: RuntimeException) {}\n}";
    let res = parse(src);
    assert_eq!(
        res.entities
            .iter()
            .filter(|e| e.tags.contains("try_arm"))
            .count(),
        2
    );
}

#[test]
fn flow_entities_follow_the_shared_id_and_parent_conventions() {
    // Load-bearing: the analyzer's branch-grouping pass materialises the
    // same shapes from tagged edges and deduplicates against these.
    let res = parse("fun f(a: Boolean) {\n    if (a) { if (a) { inner() } }\n}");
    let caller = res.entities.iter().find(|e| e.name == "f").unwrap();
    let nested = res.entities.iter().find(|e| e.name == "c1.c2").unwrap();
    assert_eq!(nested.id, format!("{}::branch::c1.c2", caller.id));
    assert_eq!(
        nested.parent_id.as_deref(),
        Some(format!("{}::branch::c1", caller.id).as_str())
    );
    assert_eq!(nested.qualified_name, format!("{}::c1.c2", caller.id));
    assert!(nested.tags.contains("branch_node"));
}

#[test]
fn conditions_stay_outside_the_arms_they_guard() {
    // The condition runs before any arm is chosen, so it belongs to the
    // enclosing flow, not to the arm.
    let res = parse("fun f() {\n    if (guard()) { act() }\n}");
    let tagged = branch_tagged(&res);
    assert!(tagged.contains(&("act".into(), "c1".into())), "{tagged:?}");
    assert!(
        !tagged.iter().any(|(t, _)| t == "guard"),
        "the condition must carry no branch tag: {tagged:?}"
    );
}

#[test]
fn a_call_outside_every_arm_carries_no_branch_key() {
    let res = parse("fun f() {\n    setup()\n    if (guard()) { act() }\n}");
    let tagged = branch_tagged(&res);
    assert!(!tagged.iter().any(|(t, _)| t == "setup"), "{tagged:?}");
}
