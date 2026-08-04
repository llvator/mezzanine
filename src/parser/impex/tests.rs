//! Integration tests for the Impex parser. Cover the four IM-NNN
//! tickets that landed together: IM-001 scaffolding, IM-002
//! reference-column type-to-type edges, IM-003 `#%` script blocks
//! (statements, hooks, conditionals), and IM-004 macros.

use super::ImpexParser;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    let parser = ImpexParser::new();
    parser
        .parse(Path::new("test.impex"), src)
        .expect("parse")
}

fn writes_to(result: &ParseResult) -> Vec<&crate::models::Relationship> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::WritesTo)
        .collect()
}

fn references(result: &ParseResult) -> Vec<&crate::models::Relationship> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::References)
        .collect()
}

fn entities_named<'a>(result: &'a ParseResult, name: &str) -> Vec<&'a CodeEntity> {
    result.entities.iter().filter(|e| e.name == name).collect()
}

fn branches(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Branch)
        .collect()
}

// ─── IM-001: scaffolding ──────────────────────────────────────────────

#[test]
fn empty_file_emits_only_module_container() {
    let result = parse("");
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].kind, EntityKind::Module);
    assert!(result.entities[0].tags.contains("impex_file"));
}

#[test]
fn comment_only_file_emits_only_module_container() {
    let result = parse("# header comment\n# another\n");
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.relationships.len(), 0);
}

#[test]
fn header_emits_writes_to_target_type() {
    let result = parse("INSERT_UPDATE Foo; code[unique=true]\n");
    let edges = writes_to(&result);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, "Foo");
    assert_eq!(
        edges[0].metadata.get("operation").map(|s| s.as_str()),
        Some("INSERT_UPDATE")
    );
}

#[test]
fn all_four_operations_dispatch() {
    let src = r#"
INSERT Foo; code
UPDATE Bar; code
INSERT_UPDATE Baz; code
REMOVE Qux; code
"#;
    let result = parse(src);
    let edges = writes_to(&result);
    let ops: Vec<&str> = edges
        .iter()
        .filter_map(|r| r.metadata.get("operation").map(|s| s.as_str()))
        .collect();
    assert!(ops.contains(&"INSERT"));
    assert!(ops.contains(&"UPDATE"));
    assert!(ops.contains(&"INSERT_UPDATE"));
    assert!(ops.contains(&"REMOVE"));
}

#[test]
fn processor_modifier_emits_class_reference() {
    let src = "UPDATE Foo[processor=com.example.MyProcessor]; key[unique=true]; value\n";
    let result = parse(src);
    let class_refs: Vec<_> = references(&result)
        .into_iter()
        .filter(|r| r.target_id == "com.example.MyProcessor")
        .collect();
    assert_eq!(class_refs.len(), 1);
    assert_eq!(
        class_refs[0].metadata.get("from").map(|s| s.as_str()),
        Some("impex_header_modifier")
    );
}

#[test]
fn data_rows_are_skipped() {
    let src = r#"
INSERT_UPDATE Foo; code; value
; row1; v1
; row2; v2
"#;
    let result = parse(src);
    // Only the header's WritesTo edge — no per-data-row entities.
    let edges = writes_to(&result);
    assert_eq!(edges.len(), 1);
}

// ─── IM-002: reference columns ────────────────────────────────────────

#[test]
fn reference_column_emits_type_to_type_edge() {
    let src = "INSERT_UPDATE UnitMapping; code[unique=true]; unit(code); country(isocode)[unique=true]\n";
    let result = parse(src);
    let fk_edges: Vec<_> = references(&result)
        .into_iter()
        .filter(|r| {
            r.metadata.get("from").map(|s| s.as_str()) == Some("impex_reference_column")
        })
        .collect();
    assert_eq!(fk_edges.len(), 2);
    let unit_edge = fk_edges
        .iter()
        .find(|r| r.target_id == "Unit")
        .expect("unit FK present");
    // The FK is sourced from the row's TARGET TYPE, not the file.
    assert_eq!(unit_edge.source_id, "UnitMapping");
    let country_edge = fk_edges
        .iter()
        .find(|r| r.target_id == "Country")
        .expect("country FK present");
    assert_eq!(country_edge.source_id, "UnitMapping");
    assert_eq!(
        country_edge.metadata.get("identity").map(|s| s.as_str()),
        Some("true"),
        "[unique=true] should mark the FK as identity"
    );
}

#[test]
fn multi_key_reference_column() {
    let src = "INSERT_UPDATE A; b(c, d)\n";
    let result = parse(src);
    // Target type is `B` (capitalised from the `b(...)` column name)
    // per Hybris convention; the original attribute name lives in
    // the relationship's `field` metadata.
    let edge = references(&result)
        .into_iter()
        .find(|r| r.source_id == "A" && r.target_id == "B")
        .expect("multi-key FK present");
    assert_eq!(
        edge.metadata.get("target_attrs").map(|s| s.as_str()),
        Some("c,d")
    );
    assert_eq!(edge.metadata.get("field").map(|s| s.as_str()), Some("b"));
}

#[test]
fn non_reference_column_emits_no_fk() {
    let src = "INSERT_UPDATE Foo; code[unique=true]; value\n";
    let result = parse(src);
    let fk_edges: Vec<_> = references(&result)
        .into_iter()
        .filter(|r| {
            r.metadata.get("from").map(|s| s.as_str()) == Some("impex_reference_column")
        })
        .collect();
    assert!(fk_edges.is_empty());
}

// ─── IM-003: #% script blocks ─────────────────────────────────────────

#[test]
fn script_statement_emits_groovy_relationships() {
    // The `#%` body is a Groovy expression; the Groovy parser
    // produces a `Calls` edge for `getBean(...)` that attributes to
    // the Impex file's module entity.
    let src = "\"#% Registry.applicationContext.getBean(\\\"jdbcTemplate\\\");\"\n";
    let result = parse(src);
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();
    assert!(
        calls.iter().any(|r| r.target_id.ends_with("getBean")),
        "expected getBean call from `#%` body, got {:?}",
        calls.iter().map(|r| &r.target_id).collect::<Vec<_>>()
    );
    let getbean = calls
        .iter()
        .find(|r| r.target_id.ends_with("getBean"))
        .unwrap();
    assert_eq!(
        getbean.metadata.get("from").map(|s| s.as_str()),
        Some("impex_script")
    );
}

#[test]
fn if_endif_emits_branch_arm() {
    let src = r#"
#% if: platformInfoService.isExtensionAvailable("catalogtest")
UPDATE ApplicationConfiguration; key[unique=true]; value
#% endif:
"#;
    let result = parse(src);
    let arms = branches(&result);
    assert_eq!(arms.len(), 1);
    let arm = &arms[0];
    assert!(arm.tags.contains("impex_condition"));
    assert!(arm.tags.contains("if_node"));
    let cond_attr = arm
        .attributes
        .iter()
        .find(|a| a.starts_with("condition:"))
        .expect("condition attribute present");
    assert!(cond_attr.contains("catalogtest"));
}

#[test]
fn header_inside_if_carries_branch_metadata() {
    let src = r#"
#% if: enabled
UPDATE Foo; code
#% endif:
"#;
    let result = parse(src);
    let arm_path = branches(&result)
        .into_iter()
        .next()
        .expect("if arm")
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    let edge = writes_to(&result)
        .into_iter()
        .find(|r| r.target_id == "Foo")
        .expect("Foo WritesTo edge");
    assert_eq!(
        edge.metadata.get("branch").map(|s| s.as_str()),
        Some(arm_path.as_str())
    );
}

#[test]
fn nested_if_endif_nests_arm_paths() {
    let src = r#"
#% if: outer_cond
#% if: inner_cond
UPDATE Foo; code
#% endif:
#% endif:
"#;
    let result = parse(src);
    let arms = branches(&result);
    assert_eq!(arms.len(), 2);
    let inner = arms
        .iter()
        .find(|a| a.id.contains("::branch::c1.c2"))
        .expect("inner arm nested under outer");
    assert!(inner.tags.contains("impex_condition"));
}

#[test]
fn beforeeach_hook_attributed_to_file() {
    let src = "#% beforeeach: prepareRow()\n";
    let result = parse(src);
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();
    let prepare = calls
        .iter()
        .find(|r| r.target_id.ends_with("prepareRow"))
        .expect("prepareRow recorded");
    assert_eq!(
        prepare.metadata.get("impex_hook").map(|s| s.as_str()),
        Some("beforeeach")
    );
}

// ─── IM-004: macros ───────────────────────────────────────────────────

#[test]
fn macro_substitutes_in_header_modifier() {
    let src = r#"
$catalog = my-catalog
INSERT_UPDATE Foo[default=$catalog]; code
"#;
    let result = parse(src);
    // The substituted value isn't surfaced as a relationship target
    // (it's a literal, not a class), but we can check no warning was
    // emitted — i.e. the reference resolved.
    assert!(
        result.warnings.is_empty(),
        "no warnings expected, got {:?}",
        result.warnings
    );
}

#[test]
fn macro_recursive_definition_substitutes_through() {
    let src = r#"
$inner = bar
$outer = $inner
INSERT_UPDATE Foo[default=$outer]; code
"#;
    let result = parse(src);
    assert!(result.warnings.is_empty());
}

#[test]
fn out_of_order_reference_warns() {
    // Reference appears BEFORE the definition — Hybris semantics
    // say this resolves to the literal `$cat`, not to the eventual
    // value. We log a warning so the user can see what went unbound.
    let src = r#"
INSERT_UPDATE Foo[default=$cat]; code
$cat = real-value
"#;
    let result = parse(src);
    assert!(
        result.warnings.iter().any(|w| w.contains("$cat")),
        "expected unresolved-macro warning, got {:?}",
        result.warnings
    );
}

#[test]
fn cycle_bottoms_out_with_warning() {
    let src = r#"
$a = $b
$b = $a
INSERT_UPDATE Foo[default=$a]; code
"#;
    let result = parse(src);
    // Either an unresolved warning OR a cycle warning is acceptable —
    // both signal the same situation. The key contract is "doesn't
    // panic and produces a diagnostic."
    assert!(
        !result.warnings.is_empty(),
        "expected at least one warning for the cycle"
    );
}

#[test]
fn macro_value_with_brackets_and_commas() {
    let src = r#"
$contentCV = catalogVersion(CatalogVersion.catalog(Catalog.id),CatalogVersion.version)
INSERT_UPDATE Foo; code
"#;
    let result = parse(src);
    // Just verifying we don't crash on a complex macro value. The
    // header parses normally.
    assert_eq!(writes_to(&result).len(), 1);
}

#[test]
fn block_marker_directive_is_ignored() {
    // `$START_USERRIGHTS` looks like a macro reference but isn't a
    // `$x = value` definition. The dispatcher should silently
    // ignore it (or treat as a marker) — never panic, never warn.
    let src = r#"
$START_USERRIGHTS
INSERT_UPDATE Foo; code
$END_USERRIGHTS
"#;
    let result = parse(src);
    assert_eq!(writes_to(&result).len(), 1);
}

// ─── Integration: file gets a `Module` container with the right name ──

#[test]
fn file_container_named_after_stem() {
    let src = "INSERT_UPDATE Foo; code\n";
    let result = parse(src);
    let containers: Vec<&CodeEntity> = result
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Module && e.tags.contains("impex_file"))
        .collect();
    assert_eq!(containers.len(), 1);
    assert_eq!(containers[0].name, "test");
}

#[test]
fn writes_to_uses_file_container_as_source() {
    let src = "INSERT_UPDATE Foo; code\n";
    let result = parse(src);
    let container = entities_named(&result, "test").into_iter().next().unwrap();
    let edge = writes_to(&result)
        .into_iter()
        .find(|r| r.target_id == "Foo")
        .unwrap();
    assert_eq!(edge.source_id, container.id);
}
