//! Integration tests for the Groovy parser.
//!
//! GR-001 scaffolding, GR-002 try/catch/finally arms, GR-003 closure
//! body call extraction, GR-004 `@Field` script-scope state, GR-005
//! switch/case arms, GR-006 for/while/do-while loops, GR-007 Elvis
//! `?:` tagging, GR-013 if/else arms, GR-014 complexity metrics, and
//! GR-015 `def` as the absence of a type — plus the Java-parity
//! additions that came with them (`UsesType` edges, supertypes,
//! package-qualified names).

use super::GroovyParser;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    let parser = GroovyParser::new();
    parser.parse(Path::new("test.groovy"), src).expect("parse")
}

fn entities_named<'a>(result: &'a ParseResult, name: &str) -> Vec<&'a CodeEntity> {
    result.entities.iter().filter(|e| e.name == name).collect()
}

fn try_arms(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("try_arm"))
        .collect()
}

// ─── GR-001: parser scaffolding ─────────────────────────────────────────

#[test]
fn class_with_method_emits_class_and_method_entities() {
    let src = "class A { void m() { foo() } }";
    let result = parse(src);
    let class = entities_named(&result, "A")
        .into_iter()
        .next()
        .expect("class A");
    assert_eq!(class.kind, EntityKind::Class);
    let method = entities_named(&result, "m")
        .into_iter()
        .next()
        .expect("method m");
    assert_eq!(method.kind, EntityKind::Method);
    assert_eq!(method.parent_id.as_deref(), Some(class.id.as_str()));
}

#[test]
fn calls_inside_method_body_emit_calls_relationships() {
    let src = "class A { void m() { foo() } }";
    let result = parse(src);
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();
    assert!(
        calls.iter().any(|r| r.target_id.ends_with("foo")),
        "expected Calls edge to `foo`, got {:?}",
        calls.iter().map(|r| &r.target_id).collect::<Vec<_>>()
    );
}

#[test]
fn import_is_captured() {
    let src = "import com.foo.Bar\nclass A {}";
    let result = parse(src);
    assert_eq!(result.imports.len(), 1);
    assert_eq!(result.imports[0].path, "com.foo.Bar");
}

#[test]
fn script_file_emits_synthetic_file_container() {
    let src = "doIt()\n";
    let result = parse(src);
    let script = result
        .entities
        .iter()
        .find(|e| e.tags.contains("groovy_script"))
        .expect("script container present");
    assert_eq!(script.kind, EntityKind::File);
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();
    let call = calls
        .iter()
        .find(|r| r.target_id.ends_with("doIt"))
        .expect("doIt call recorded");
    assert_eq!(call.source_id, script.id);
}

#[test]
fn class_only_file_skips_script_container() {
    let src = "class A {}";
    let result = parse(src);
    assert!(
        !result
            .entities
            .iter()
            .any(|e| e.tags.contains("groovy_script")),
        "class-only files should not get a script container"
    );
}

// ─── GR-002: try / catch / finally ──────────────────────────────────────

#[test]
fn try_catch_emits_two_arms() {
    let src = r#"
class A {
    void m() {
        try { a() } catch (Exception e) { b() }
    }
}
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(
        arms.len(),
        2,
        "expected try body + catch arms, got {}",
        arms.len()
    );
    assert!(arms.iter().any(|e| e.tags.contains("try_body_arm")));
    let catch = arms.iter().find(|e| e.tags.contains("catch_arm")).unwrap();
    assert_eq!(catch.documentation.as_deref(), Some("caught: Exception"));
    assert!(catch.attributes.contains(&"caught:Exception".to_string()));
}

#[test]
fn try_full_chain_emits_all_arms() {
    let src = r#"
class A {
    void m() {
        try { a() }
        catch (FooException fe) { b() }
        catch (BarException be) { c() }
        finally { d() }
    }
}
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(arms.len(), 4, "try + 2x catch + finally");
    assert!(arms.iter().any(|e| e.tags.contains("try_body_arm")));
    assert_eq!(
        arms.iter().filter(|e| e.tags.contains("catch_arm")).count(),
        2
    );
    assert!(arms.iter().any(|e| e.tags.contains("finally_arm")));
}

#[test]
fn multi_catch_records_joined_types() {
    let src = r#"
class A {
    void m() {
        try { a() } catch (FooException | BarException e) { b() }
    }
}
"#;
    let result = parse(src);
    let catch = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("catch_arm"))
        .expect("catch arm present");
    let caught = catch
        .attributes
        .iter()
        .find(|a| a.starts_with("caught:"))
        .expect("caught attribute");
    assert!(
        caught.contains("FooException") && caught.contains("BarException"),
        "expected both types in {:?}",
        caught
    );
    assert!(caught.contains('|'), "multi-catch should join with `|`");
}

#[test]
fn calls_inside_catch_arm_are_branch_tagged() {
    let src = r#"
class A {
    void m() {
        try { outer() } catch (Exception e) { inside_catch() }
    }
}
"#;
    let result = parse(src);
    let catch_id = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("catch_arm"))
        .unwrap()
        .id
        .clone();
    let catch_path = catch_id.rsplit("::branch::").next().unwrap();
    let call = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("inside_catch"))
        .expect("inside_catch call recorded");
    assert_eq!(
        call.metadata.get("branch").map(|s| s.as_str()),
        Some(catch_path)
    );
}

#[test]
fn nested_try_arms_nest_under_outer() {
    let src = r#"
class A {
    void m() {
        try { a() }
        catch (E1 e) {
            try { b() } catch (E2 e2) { c() }
        }
    }
}
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(arms.len(), 4, "outer + outer-catch + inner + inner-catch");
    let inner_catch = arms
        .iter()
        .filter(|e| e.tags.contains("catch_arm"))
        .find(|e| e.attributes.iter().any(|a| a == "caught:E2"))
        .expect("inner E2 catch");
    assert!(
        inner_catch.id.contains("::branch::c2."),
        "inner catch should nest under outer c2 (the outer catch arm), got {}",
        inner_catch.id
    );
}

#[test]
fn finally_arm_carries_no_caught_metadata() {
    let src = r#"
class A {
    void m() {
        try { a() } catch (Exception e) { b() } finally { c() }
    }
}
"#;
    let result = parse(src);
    let finally_arm = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("finally_arm"))
        .expect("finally arm");
    assert!(finally_arm.documentation.is_none());
    assert!(!finally_arm
        .attributes
        .iter()
        .any(|a| a.starts_with("caught:")));
}

#[test]
fn try_at_script_scope_attributes_to_script_container() {
    // Top-level `try` in a script file: the synthetic File container
    // is the caller, so the catch arm's parent_id chains back to it.
    let src = "try { a() } catch (Exception e) { b() } finally { c() }\n";
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(arms.len(), 3, "try + catch + finally at script scope");
}

// ─── GR-003: closures ──────────────────────────────────────────────────

#[test]
fn closure_body_calls_attribute_to_enclosing_function() {
    let src = r#"
class A {
    void m() {
        list.each { item -> consume(item) }
    }
}
"#;
    let result = parse(src);
    let method = entities_named(&result, "m").into_iter().next().unwrap();
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .filter(|r| r.source_id == method.id)
        .collect();
    assert!(
        calls.iter().any(|r| r.target_id.ends_with("consume")),
        "consume() call should reach enclosing method m: {:?}",
        calls.iter().map(|r| &r.target_id).collect::<Vec<_>>()
    );
}

#[test]
fn closure_inside_try_arm_inherits_branch_tag() {
    let src = r#"
class A {
    void m() {
        try {
            list.each { item -> in_try(item) }
        } catch (Exception e) {
            list.each { item -> in_catch(item) }
        }
    }
}
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    let try_body_path = arms
        .iter()
        .find(|e| e.tags.contains("try_body_arm"))
        .unwrap()
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    let catch_path = arms
        .iter()
        .find(|e| e.tags.contains("catch_arm"))
        .unwrap()
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    let in_try = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("in_try"))
        .expect("in_try recorded");
    assert_eq!(
        in_try.metadata.get("branch").map(|s| s.as_str()),
        Some(try_body_path.as_str())
    );
    let in_catch = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("in_catch"))
        .expect("in_catch recorded");
    assert_eq!(
        in_catch.metadata.get("branch").map(|s| s.as_str()),
        Some(catch_path.as_str())
    );
}

#[test]
fn nested_closures_dont_double_count() {
    let src = r#"
class A {
    void m() {
        items.each { a -> a.each { sub -> use_pair(a, sub) } }
    }
}
"#;
    let result = parse(src);
    let method = entities_named(&result, "m").into_iter().next().unwrap();
    let use_pair_calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .filter(|r| r.source_id == method.id)
        .filter(|r| r.target_id.ends_with("use_pair"))
        .collect();
    assert_eq!(
        use_pair_calls.len(),
        1,
        "use_pair should be emitted once, got {}",
        use_pair_calls.len()
    );
}

#[test]
fn closure_parameter_does_not_become_local_variable() {
    let src = r#"
class A {
    void m() {
        list.each { item -> consume(item) }
    }
}
"#;
    let result = parse(src);
    let locals: Vec<_> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("local_var") && e.name == "item")
        .collect();
    assert!(
        locals.is_empty(),
        "closure parameter `item` must not leak as a local of the enclosing method"
    );
}

// ─── GR-004: @Field script-scope state ─────────────────────────────────

#[test]
fn at_field_emits_module_state_variable() {
    let src = "@Field final String x = 'a'\n";
    let result = parse(src);
    let field = entities_named(&result, "x")
        .into_iter()
        .next()
        .expect("@Field x");
    assert_eq!(field.kind, EntityKind::Variable);
    assert!(field.tags.contains("module_state"));
    assert!(field.attributes.iter().any(|a| a == "final"));
    assert_eq!(field.return_type.as_deref(), Some("String"));
}

#[test]
fn at_field_writes_to_edge_from_script_container() {
    let src = "@Field final String x = 'a'\n";
    let result = parse(src);
    let field = entities_named(&result, "x").into_iter().next().unwrap();
    let script = result
        .entities
        .iter()
        .find(|e| e.tags.contains("groovy_script"))
        .expect("script container");
    let writes: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::WritesTo)
        .filter(|r| r.source_id == script.id && r.target_id == field.id)
        .collect();
    assert_eq!(
        writes.len(),
        1,
        "expected one WritesTo from script to @Field"
    );
}

#[test]
fn fully_qualified_field_annotation_recognized() {
    let src = "@groovy.transform.Field final String x = 'a'\n";
    let result = parse(src);
    let field = entities_named(&result, "x")
        .into_iter()
        .next()
        .expect("@Field x");
    assert!(field.tags.contains("module_state"));
}

#[test]
fn non_field_top_level_local_stays_local() {
    let src = "def y = init()\n";
    let result = parse(src);
    let local = result
        .entities
        .iter()
        .find(|e| e.name == "y")
        .expect("local y");
    assert!(
        !local.tags.contains("module_state"),
        "non-@Field locals should not be module_state"
    );
}

#[test]
fn at_field_get_bean_initializer_records_bean_name() {
    let src =
        "@Field final HybrisJdbcTemplate t = Registry.applicationContext.getBean('jdbcTemplate')\n";
    let result = parse(src);
    let field = entities_named(&result, "t")
        .into_iter()
        .next()
        .expect("@Field t");
    assert!(
        field.attributes.iter().any(|a| a == "bean:jdbcTemplate"),
        "expected bean:jdbcTemplate attribute, got {:?}",
        field.attributes
    );
}

// ─── GR-005: switch / case ──────────────────────────────────────────────

fn case_arms(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("case_arm"))
        .collect()
}

#[test]
fn switch_emits_one_arm_per_case() {
    let src = r#"
class A {
    void m(x) {
        switch (x) {
            case 1: a(); break;
            case 2: b(); break;
            default: c();
        }
    }
}
"#;
    let result = parse(src);
    let arms = case_arms(&result);
    assert_eq!(arms.len(), 3, "two cases + default → three arms");
    let docs: Vec<&str> = arms
        .iter()
        .filter_map(|e| e.documentation.as_deref())
        .collect();
    assert!(docs.contains(&"pattern: 1"));
    assert!(docs.contains(&"pattern: 2"));
    // The default label has no pattern, so it carries no `pattern:`
    // attribute or documentation — but it still appears as an arm.
    assert!(arms
        .iter()
        .any(|e| e.attributes.iter().all(|a| !a.starts_with("pattern:"))));
}

#[test]
fn switch_type_match_records_pattern() {
    let src = r#"
class A {
    void m(x) {
        switch (x) {
            case String: doStr(); break;
            case Integer: doInt(); break;
        }
    }
}
"#;
    let result = parse(src);
    let arms = case_arms(&result);
    assert_eq!(arms.len(), 2);
    assert!(arms
        .iter()
        .any(|e| e.attributes.iter().any(|a| a == "pattern:String")));
    assert!(arms
        .iter()
        .any(|e| e.attributes.iter().any(|a| a == "pattern:Integer")));
}

#[test]
fn switch_range_pattern_preserved() {
    let src = r#"
class A {
    void m(x) {
        switch (x) {
            case 1..10: small(); break;
        }
    }
}
"#;
    let result = parse(src);
    let arm = case_arms(&result).into_iter().next().expect("range arm");
    let doc = arm.documentation.as_deref().unwrap_or("");
    assert!(
        doc.contains("1") && doc.contains("10"),
        "range pattern survives: {:?}",
        doc
    );
}

#[test]
fn calls_inside_case_body_are_branch_tagged() {
    let src = r#"
class A {
    void m(x) {
        switch (x) {
            case 1: inside_one(); break;
            default: inside_default();
        }
    }
}
"#;
    let result = parse(src);
    let one_arm = case_arms(&result)
        .into_iter()
        .find(|e| e.attributes.iter().any(|a| a == "pattern:1"))
        .expect("pattern-1 arm");
    let one_path = one_arm.id.rsplit("::branch::").next().unwrap().to_string();
    let inside = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("inside_one"))
        .expect("inside_one recorded");
    assert_eq!(
        inside.metadata.get("branch").map(|s| s.as_str()),
        Some(one_path.as_str())
    );
}

#[test]
fn switch_subject_call_stays_in_outer_scope() {
    // The subject expression runs once before any arm — its calls
    // attribute to the enclosing method, not to a case branch.
    let src = r#"
class A {
    void m() {
        switch (get_subject()) {
            case 1: inside(); break;
        }
    }
}
"#;
    let result = parse(src);
    let subject = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("get_subject"))
        .expect("get_subject call recorded");
    assert!(
        !subject.metadata.contains_key("branch"),
        "subject call should be in outer scope, got branch={:?}",
        subject.metadata.get("branch")
    );
}

#[test]
fn multi_case_shared_body_emits_arm_per_label() {
    // `case 1: case 2: shared(); break;` parses as two groups in
    // tree-sitter-groovy — first group has only the label, second
    // has label + body. Both get arm entities; the body attributes
    // to the second label's arm.
    let src = r#"
class A {
    void m(x) {
        switch (x) {
            case 1:
            case 2:
                shared();
                break;
        }
    }
}
"#;
    let result = parse(src);
    let arms = case_arms(&result);
    assert_eq!(
        arms.len(),
        2,
        "one arm per `case` label, got {}",
        arms.len()
    );
    let case_two = arms
        .iter()
        .find(|e| e.attributes.iter().any(|a| a == "pattern:2"))
        .expect("pattern-2 arm");
    let case_two_path = case_two.id.rsplit("::branch::").next().unwrap().to_string();
    let shared = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("shared"))
        .expect("shared() recorded");
    assert_eq!(
        shared.metadata.get("branch").map(|s| s.as_str()),
        Some(case_two_path.as_str())
    );
}

#[test]
fn nested_switch_arm_paths_nest_under_outer() {
    let src = r#"
class A {
    void m(x, y) {
        switch (x) {
            case 1:
                switch (y) {
                    case 'a': inner(); break;
                }
                break;
        }
    }
}
"#;
    let result = parse(src);
    let inner = case_arms(&result).into_iter().find(|e| {
        e.attributes
            .iter()
            .any(|a| a == "pattern:'a'" || a == "pattern:\"a\"")
    });
    let inner = inner.expect("inner pattern-a arm");
    assert!(
        inner.id.contains("::branch::c1.c2") || inner.id.contains("::branch::c1.c1"),
        "inner case should nest under outer c1, got {}",
        inner.id
    );
}

// ─── GR-006: for / while / do-while loops ───────────────────────────────

fn loops(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("loop_node"))
        .collect()
}

#[test]
fn enhanced_for_emits_loop_entity() {
    let src = r#"
class A {
    void m(list) {
        for (item in list) {
            use(item);
        }
    }
}
"#;
    let result = parse(src);
    assert_eq!(loops(&result).len(), 1, "one loop entity for one for-in");
}

#[test]
fn enhanced_for_iterable_call_groups_under_loop() {
    // `for (row in queryForList(sql))` — the iterable expression's
    // `queryForList` call belongs under the loop, not the outer
    // method, mirroring how Python's for-iterator groups under the loop.
    let src = r#"
class A {
    void m(jdbcTemplate, sql) {
        for (row in jdbcTemplate.queryForList(sql)) {
            use(row);
        }
    }
}
"#;
    let result = parse(src);
    let loop_path = loops(&result)
        .into_iter()
        .next()
        .expect("loop entity")
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    let q = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("queryForList"))
        .expect("queryForList recorded");
    assert_eq!(
        q.metadata.get("branch").map(|s| s.as_str()),
        Some(loop_path.as_str())
    );
}

#[test]
fn classic_for_groups_init_cond_update_under_loop() {
    // `next`/`size`/etc. are filtered by `is_stdlib_method` so the
    // call sites here pick names that don't collide with the stdlib
    // table — the assertion is about loop-branch attribution, not the
    // identity of the called function.
    let src = r#"
class A {
    void m() {
        for (int i = init_v(); i < bound(); i = step(i)) {
            body_call();
        }
    }
}
"#;
    let result = parse(src);
    let loop_path = loops(&result)
        .into_iter()
        .next()
        .expect("loop entity")
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    for callee in ["init_v", "bound", "step", "body_call"] {
        let r = result
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .find(|r| r.target_id.ends_with(callee))
            .unwrap_or_else(|| panic!("{} call recorded", callee));
        assert_eq!(
            r.metadata.get("branch").map(|s| s.as_str()),
            Some(loop_path.as_str()),
            "{} call should be under loop",
            callee
        );
    }
}

#[test]
fn while_loop_groups_condition_and_body() {
    let src = r#"
class A {
    void m() {
        while (cond_call()) {
            body_call();
        }
    }
}
"#;
    let result = parse(src);
    let loop_path = loops(&result)
        .into_iter()
        .next()
        .expect("loop entity")
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    for callee in ["cond_call", "body_call"] {
        let r = result
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .find(|r| r.target_id.ends_with(callee))
            .unwrap_or_else(|| panic!("{} recorded", callee));
        assert_eq!(
            r.metadata.get("branch").map(|s| s.as_str()),
            Some(loop_path.as_str())
        );
    }
}

#[test]
fn do_while_emits_loop_entity() {
    let src = r#"
class A {
    void m() {
        do { body(); } while (cond());
    }
}
"#;
    let result = parse(src);
    assert_eq!(loops(&result).len(), 1, "one loop entity for do-while");
}

#[test]
fn nested_loops_path_nest_under_outer() {
    // The outer loop is l1; the inner loop is its child and gets the
    // `l1.l<N>` shape. Counters accumulate across the recursive walk
    // (matching the existing GR-002 arm-counter behaviour) rather
    // than resetting per scope, so the inner is `l1.l2` (counter=2
    // after outer increment), not `l1.l1`. The path uniquely
    // identifies the nested loop either way; what matters is that
    // it nests under the outer.
    let src = r#"
class A {
    void m(xs, ys) {
        for (a in xs) {
            for (b in ys) {
                pair(a, b);
            }
        }
    }
}
"#;
    let result = parse(src);
    let inner = loops(&result)
        .into_iter()
        .find(|e| {
            let path = e.id.rsplit("::branch::").next().unwrap_or("");
            path.starts_with("l1.l") && path != "l1"
        })
        .expect("inner loop should nest under l1");
    assert!(inner.tags.contains("loop_node"));
}

#[test]
fn loop_uses_separate_counter_from_branches() {
    // A sibling `if` and `for` at the same scope must read as `c1`
    // and `l1` — not collide on the shared arm counter.
    let src = r#"
class A {
    void m(flag, items) {
        if (flag) { a(); }
        for (item in items) { b(); }
    }
}
"#;
    let result = parse(src);
    let loop_entity = loops(&result).into_iter().next().expect("loop entity");
    let loop_path = loop_entity
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    assert_eq!(loop_path, "l1", "loop should be l1, got {}", loop_path);
}

#[test]
fn closure_inside_for_attributes_to_loop() {
    // GR-003 closures compose with GR-006: `list.each { … }` inside a
    // `for (item in xs)` body — the closure's interior calls should
    // land on the loop branch, not the outer method.
    let src = r#"
class A {
    void m(xs) {
        for (item in xs) {
            sub.each { x -> consume(x) }
        }
    }
}
"#;
    let result = parse(src);
    let loop_path = loops(&result)
        .into_iter()
        .next()
        .expect("loop entity")
        .id
        .rsplit("::branch::")
        .next()
        .unwrap()
        .to_string();
    let consume = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("consume"))
        .expect("consume recorded");
    assert_eq!(
        consume.metadata.get("branch").map(|s| s.as_str()),
        Some(loop_path.as_str()),
    );
}

// ─── GR-007: Elvis `?:` tagging ─────────────────────────────────────────

#[test]
fn elvis_call_on_alternative_tagged_null_safe() {
    let src = r#"
class A {
    void m(a) {
        def r = a ?: fallback();
    }
}
"#;
    let result = parse(src);
    let fallback = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("fallback"))
        .expect("fallback recorded");
    assert_eq!(
        fallback.metadata.get("null_safe").map(|s| s.as_str()),
        Some("true"),
        "fallback call should be tagged null_safe"
    );
}

#[test]
fn elvis_tags_both_sides() {
    let src = r#"
class A {
    void m() {
        def r = primary() ?: fallback();
    }
}
"#;
    let result = parse(src);
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();
    let primary = calls
        .iter()
        .find(|r| r.target_id.ends_with("primary"))
        .expect("primary recorded");
    let fallback = calls
        .iter()
        .find(|r| r.target_id.ends_with("fallback"))
        .expect("fallback recorded");
    assert_eq!(
        primary.metadata.get("null_safe").map(|s| s.as_str()),
        Some("true")
    );
    assert_eq!(
        fallback.metadata.get("null_safe").map(|s| s.as_str()),
        Some("true")
    );
}

#[test]
fn regular_ternary_does_not_tag_null_safe() {
    // `a ? b() : c()` — both b() and c() are real calls but neither
    // is null-safe; only Elvis (`a ?: b()`, where consequence is
    // missing) gets the tag.
    let src = r#"
class A {
    void m(a) {
        def r = a ? b() : c();
    }
}
"#;
    let result = parse(src);
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .filter(|r| r.target_id.ends_with("b") || r.target_id.ends_with("c"))
        .collect();
    assert_eq!(calls.len(), 2, "both ternary branches recorded");
    for r in &calls {
        assert!(
            !r.metadata.contains_key("null_safe"),
            "regular ternary branch should not be null_safe: {}",
            r.target_id
        );
    }
}

// ─── GR-013: if / else-if / else arms ───────────────────────────────────

/// The deliberately gnarly fixture from the parser-readiness audit,
/// ported from Java. Java yields 8 Branch + 2 Loop entities and the
/// metric triple 12 / 31 / 6; Groovy must land in the same place or the
/// two languages' scores are not comparable.
const GNARLY: &str = r#"class A { int gnarly(int a, int b, int c, int d) {
  if (a>0) { for (int x=0;x<b;x++) { if (x>0 && c>0) { while (d>0) { try { if (x>1) {} else if (x<0) {} else {} } catch (RuntimeException e) {} } } } }
  else if (b>0 || c>0) { return 1 }
  return 2 } }"#;

fn branches(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Branch)
        .collect()
}

/// Cloned rather than borrowed so callers can pass a freshly-parsed
/// temporary: `metrics_of(&parse(src), "m")`.
fn metrics_of(result: &ParseResult, name: &str) -> crate::models::entity::EntityMetrics {
    entities_named(result, name)
        .into_iter()
        .next()
        .expect("entity")
        .metrics
        .clone()
}

#[test]
fn if_else_emits_one_arm_per_branch() {
    let src = "class A { void m() { if (a) { foo() } else { bar() } } }";
    let result = parse(src);
    let arms = branches(&result);
    assert_eq!(arms.len(), 2, "if + else are two arms: {:?}", arms);
    assert_eq!(arms[0].name, "c1");
    assert_eq!(arms[1].name, "c2");
}

#[test]
fn else_if_chain_flattens_into_sibling_arms() {
    // `else if` nests in the AST but reads as a flat decision list, so the
    // arms are siblings (`c1`/`c2`/`c3`), not `c1`/`c1.c2`/`c1.c2.c3`.
    let src = "class A { void m() { if (a) { foo() } else if (b) { bar() } else { baz() } } }";
    let result = parse(src);
    let names: Vec<&str> = branches(&result).iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["c1", "c2", "c3"]);
}

#[test]
fn if_without_else_emits_a_single_arm() {
    let src = "class A { void m() { if (a) { foo() } } }";
    let result = parse(src);
    assert_eq!(branches(&result).len(), 1);
}

#[test]
fn calls_inside_an_if_arm_are_branch_tagged() {
    let src = "class A { void m() { if (a) { foo() } else { bar() } } }";
    let result = parse(src);
    let branch_of = |target: &str| {
        result
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .find(|r| r.target_id.ends_with(target))
            .and_then(|r| r.metadata.get("branch").cloned())
    };
    assert_eq!(branch_of("foo").as_deref(), Some("c1"));
    assert_eq!(branch_of("bar").as_deref(), Some("c2"));
}

#[test]
fn if_condition_call_stays_in_the_outer_scope() {
    // The condition runs before any arm is chosen — it belongs to the
    // enclosing flow, matching how the switch subject is handled.
    let src = "class A { void m() { if (ready()) { foo() } } }";
    let result = parse(src);
    let ready = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("ready"))
        .expect("condition call recorded");
    assert!(
        !ready.metadata.contains_key("branch"),
        "condition call should not be inside an arm"
    );
}

#[test]
fn nested_if_arms_nest_under_the_outer_arm() {
    let src = "class A { void m() { if (a) { if (b) { foo() } } } }";
    let result = parse(src);
    let names: Vec<&str> = branches(&result).iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["c1", "c1.c2"]);
}

#[test]
fn gnarly_fixture_matches_javas_branch_and_loop_counts() {
    let result = parse(GNARLY);
    assert_eq!(branches(&result).len(), 8, "Java yields 8 Branch entities");
    assert_eq!(loops(&result).len(), 2, "Java yields 2 Loop entities");
}

// ─── GR-014: complexity metrics ─────────────────────────────────────────

#[test]
fn gnarly_fixture_matches_javas_metric_triple() {
    let m = metrics_of(&parse(GNARLY), "gnarly");
    assert_eq!(m.cyclomatic, Some(12));
    assert_eq!(m.cognitive_complexity, Some(31));
    assert_eq!(m.max_nesting, Some(6));
}

#[test]
fn a_straight_line_method_is_cyclomatic_one() {
    let m = metrics_of(&parse("class A { void m() { foo(); bar() } }"), "m");
    assert_eq!(m.cyclomatic, Some(1));
    assert_eq!(m.cognitive_complexity, Some(0));
    assert_eq!(m.max_nesting, Some(0));
}

#[test]
fn a_bodyless_method_is_measured_rather_than_left_unset() {
    // An empty result reads as "clean"; `None` reads as "not measured".
    // An abstract signature has exactly one straight-through path.
    let m = metrics_of(&parse("interface I { void m() }"), "m");
    assert_eq!(m.cyclomatic, Some(1));
    assert_eq!(m.max_nesting, Some(0));
    assert_eq!(m.cognitive_complexity, Some(0));
}

#[test]
fn short_circuit_operators_each_add_a_path() {
    let m = metrics_of(
        &parse("class A { void m() { if (a && b || c) { foo() } } }"),
        "m",
    );
    assert_eq!(m.cyclomatic, Some(4), "base + if + && + ||");
}

#[test]
fn elvis_counts_as_a_branch() {
    // Groovy's `?:` parses as a ternary with a missing consequence, so it
    // rides on the ternary increment rather than needing its own.
    let m = metrics_of(
        &parse("class A { void m(a) { def r = a ?: fallback(); } }"),
        "m",
    );
    assert_eq!(m.cyclomatic, Some(2));
}

#[test]
fn a_braced_arm_is_a_block_not_a_closure() {
    // The grammar spells both `{ … }` forms `closure`. If an `if` arm were
    // counted as a closure, its cognitive cost would be charged twice.
    let m = metrics_of(&parse("class A { void m() { if (a) { foo() } } }"), "m");
    assert_eq!(
        m.cognitive_complexity,
        Some(1),
        "the `if` alone, not if + closure"
    );
}

#[test]
fn a_closure_argument_nests_without_branching() {
    // Closures are the dominant Groovy idiom: they open a nesting scope
    // (Java's `lambda_expression` rule) but add nothing to cyclomatic.
    let m = metrics_of(
        &parse("class A { void m() { run { if (a) { foo() } } } }"),
        "m",
    );
    assert_eq!(
        m.cyclomatic,
        Some(2),
        "the `if` only — the closure does not branch"
    );
    assert_eq!(
        m.cognitive_complexity,
        Some(3),
        "closure (+1) + if nested inside it (+2)"
    );
}

#[test]
fn a_script_container_carries_its_own_complexity() {
    // A Gradle build file keeps its logic in top-level statements, so a
    // script scoring 0 would be reported as clean rather than unmeasured.
    let result = parse("if (a) { foo() }\nfor (x in list) { bar(x) }\n");
    let script = result
        .entities
        .iter()
        .find(|e| e.tags.contains("groovy_script"))
        .expect("script container");
    assert_eq!(script.metrics.cyclomatic, Some(3), "base + if + for");
    assert!(script.metrics.cognitive_complexity.unwrap_or(0) > 0);
}

#[test]
fn a_class_in_a_script_is_not_folded_into_the_script() {
    // Nested definitions own their metrics; the script counts only what
    // attributes to it, matching how calls are attributed.
    let result = parse("foo()\nclass A { void m() { if (a) { bar() } } }\n");
    let script = result
        .entities
        .iter()
        .find(|e| e.tags.contains("groovy_script"))
        .expect("script container");
    assert_eq!(
        script.metrics.cyclomatic,
        Some(1),
        "the class's `if` is not the script's"
    );
    assert_eq!(metrics_of(&result, "m").cyclomatic, Some(2));
}

// ─── Control flow the grammar degrades inside closures ──────────────────

#[test]
fn if_inside_a_parameterized_closure_is_a_branch_not_a_call() {
    // `{ it -> if (…) { … } }` parses the `if` as a method invocation
    // named "if". Left alone that emits a Calls edge to a keyword and
    // loses the branch entirely.
    let src = "class A { void m() { items.each { it -> if (ok(it)) { use(it) } } } }";
    let result = parse(src);
    assert!(
        !result
            .relationships
            .iter()
            .any(|r| r.kind == RelationshipKind::Calls && r.target_id.ends_with("if")),
        "no Calls edge to the `if` keyword"
    );
    assert_eq!(branches(&result).len(), 1, "the guarded block is an arm");
    let used = result
        .relationships
        .iter()
        .find(|r| r.target_id.ends_with("use"))
        .expect("call inside the arm");
    assert!(used.metadata.contains_key("branch"));
}

// ─── GR-015: `def` is not a type ────────────────────────────────────────

#[test]
fn def_is_not_recorded_as_a_return_type() {
    let result = parse("class A { def compute() { return 1 } }");
    let method = entities_named(&result, "compute")
        .into_iter()
        .next()
        .expect("method");
    assert_eq!(method.return_type, None, "`def` is the absence of a type");
}

#[test]
fn a_declared_return_type_is_still_recorded() {
    // The skip must be keyword-specific, not "Groovy has no types".
    let result = parse("class A { Release deploy() { return null } }");
    let method = entities_named(&result, "deploy")
        .into_iter()
        .next()
        .expect("method");
    assert_eq!(method.return_type.as_deref(), Some("Release"));
}

#[test]
fn a_def_field_and_a_def_parameter_record_no_type() {
    let result = parse("class A { def cache\n void m(def raw) { } }");
    let field = entities_named(&result, "cache")
        .into_iter()
        .next()
        .expect("field");
    assert_eq!(field.return_type, None);
    let method = entities_named(&result, "m")
        .into_iter()
        .next()
        .expect("method");
    assert_eq!(method.parameters[0].type_name, None);
}

// ─── UsesType edges (JV-001 parity) ─────────────────────────────────────

fn uses_type_targets(result: &ParseResult, source_name: &str) -> Vec<String> {
    let source = entities_named(result, source_name)
        .into_iter()
        .next()
        .expect("source entity");
    let mut targets: Vec<String> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::UsesType && r.source_id == source.id)
        .map(|r| r.target_id.clone())
        .collect();
    targets.sort();
    targets
}

#[test]
fn a_signature_emits_uses_type_edges_for_its_named_types() {
    let src = "class A { Release deploy(Artifact a, List<Manifest> ms) { } }";
    let result = parse(src);
    assert_eq!(
        uses_type_targets(&result, "deploy"),
        vec!["Artifact", "Manifest", "Release"],
        "`List` is a std name and is dropped"
    );
}

#[test]
fn a_field_type_emits_a_uses_type_edge() {
    let result = parse("class A { ArtifactRepository repo\n def cache }");
    assert_eq!(
        uses_type_targets(&result, "repo"),
        vec!["ArtifactRepository"]
    );
    assert!(
        uses_type_targets(&result, "cache").is_empty(),
        "`def` has no type"
    );
}

#[test]
fn script_scope_field_state_emits_uses_type_edges() {
    let result = parse("import groovy.transform.Field\n@Field HybrisJdbcTemplate template\n");
    assert_eq!(
        uses_type_targets(&result, "template"),
        vec!["HybrisJdbcTemplate"]
    );
}

#[test]
fn a_self_referential_field_emits_no_edge() {
    let result = parse("class Node { Node next }");
    assert!(uses_type_targets(&result, "next").is_empty());
}

// ─── Container metadata (Java parity) ───────────────────────────────────

#[test]
fn a_class_records_its_supertypes() {
    let result = parse("class Deployer extends BaseDeployer implements Auditable, Closeable { }");
    let class = entities_named(&result, "Deployer")
        .into_iter()
        .next()
        .expect("class");
    assert_eq!(class.extends, vec!["BaseDeployer"]);
    assert_eq!(class.implements, vec!["Auditable", "Closeable"]);
}

#[test]
fn a_package_declaration_qualifies_its_containers() {
    let result = parse("package com.acme.deploy\nclass Deployer { }");
    let class = entities_named(&result, "Deployer")
        .into_iter()
        .next()
        .expect("class");
    assert_eq!(class.qualified_name, "com.acme.deploy.Deployer");
}

#[test]
fn a_class_without_a_package_keeps_its_bare_name() {
    let result = parse("class Deployer { }");
    let class = entities_named(&result, "Deployer")
        .into_iter()
        .next()
        .expect("class");
    assert_eq!(class.qualified_name, "Deployer");
}
