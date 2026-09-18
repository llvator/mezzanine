//! Integration tests for the Python parser, focused on try/except arm
//! emission and branch-path threading.

use super::PythonParser;
use crate::models::{CodeEntity, RelationshipKind};
use crate::parser::language_parser::{LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    let parser = PythonParser::new();
    parser.parse(Path::new("test.py"), src).expect("parse")
}

fn try_arms(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("try_arm"))
        .collect()
}

#[test]
fn try_except_emits_two_branch_arms() {
    let src = r#"
def f(x):
    try:
        a()
    except ValueError:
        b()
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(arms.len(), 2, "expected try body + except arms");
    assert!(arms.iter().any(|e| e.tags.contains("try_body_arm")));
    let except = arms.iter().find(|e| e.tags.contains("except_arm")).unwrap();
    assert_eq!(except.documentation.as_deref(), Some("caught: ValueError"));
    assert!(except.attributes.contains(&"caught:ValueError".to_string()));
}

#[test]
fn try_full_chain_emits_all_arms() {
    let src = r#"
def f():
    try:
        a()
    except ValueError:
        b()
    except (IOError, OSError):
        c()
    else:
        d()
    finally:
        e()
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(arms.len(), 5, "try + 2x except + else + finally");
    assert!(arms.iter().any(|e| e.tags.contains("try_body_arm")));
    assert_eq!(
        arms.iter()
            .filter(|e| e.tags.contains("except_arm"))
            .count(),
        2
    );
    assert!(arms.iter().any(|e| e.tags.contains("else_arm")));
    assert!(arms.iter().any(|e| e.tags.contains("finally_arm")));
    // Tuple of caught types travels through verbatim.
    assert!(arms.iter().any(|e| e
        .attributes
        .iter()
        .any(|a| a == "caught:(IOError, OSError)")));
}

#[test]
fn bare_except_has_no_caught_metadata() {
    let src = r#"
def f():
    try:
        a()
    except:
        b()
"#;
    let result = parse(src);
    let except = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("except_arm"))
        .unwrap();
    assert!(except.documentation.is_none());
    assert!(!except.attributes.iter().any(|a| a.starts_with("caught:")));
}

#[test]
fn except_group_clause_is_tagged_distinctly() {
    // PEP 654: except* MyError as e
    let src = r#"
def f():
    try:
        a()
    except* ValueError:
        b()
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert!(arms.iter().any(|e| e.tags.contains("except_group_arm")));
    assert!(!arms.iter().any(|e| e.tags.contains("except_arm")));
}

#[test]
fn calls_inside_except_arm_are_branch_tagged() {
    let src = r#"
def f():
    try:
        outer()
    except ValueError:
        inside_except()
"#;
    let result = parse(src);
    let except_id = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("except_arm"))
        .unwrap()
        .id
        .clone();
    // The branch path is the suffix after `::branch::`.
    let except_path = except_id.rsplit("::branch::").next().unwrap();
    let calls: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();
    let inside = calls
        .iter()
        .find(|r| r.target_id.ends_with("inside_except"))
        .expect("inside_except call recorded");
    assert_eq!(
        inside.metadata.get("branch").map(|s| s.as_str()),
        Some(except_path)
    );
}

#[test]
fn nested_try_increments_arm_path_under_outer_arm() {
    // Inner try lives inside the outer except — its arms should
    // carry the outer except's path as a prefix.
    let src = r#"
def f():
    try:
        a()
    except ValueError:
        try:
            b()
        except KeyError:
            c()
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    // Outer try_body, outer except, inner try_body, inner except.
    assert_eq!(arms.len(), 4);
    let inner_except = arms
        .iter()
        .filter(|e| e.tags.contains("except_arm"))
        .find(|e| e.attributes.iter().any(|a| a == "caught:KeyError"))
        .expect("inner KeyError arm");
    // Outer except is the second arm at module scope (c2); the
    // inner try's body and except live inside it.
    assert!(
        inner_except.id.contains("::branch::c2."),
        "inner except path should be nested under outer c2, got {}",
        inner_except.id
    );
}

#[test]
fn except_with_alias_strips_to_just_the_type() {
    // `except ValueError as e` should record `caught:ValueError`,
    // not `caught:ValueError as e`. Whether tree-sitter-python
    // exposes the alias as a sibling expression or wraps both in
    // an `as_pattern`, the captured `caught` text must not contain
    // the binding name.
    let src = r#"
def f():
    try:
        a()
    except ValueError as e:
        b()
"#;
    let result = parse(src);
    let except = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("except_arm"))
        .unwrap();
    let caught = except
        .attributes
        .iter()
        .find(|a| a.starts_with("caught:"))
        .expect("caught attribute present");
    assert!(
        !caught.contains(" as "),
        "caught attribute leaked the alias: {}",
        caught
    );
    assert!(
        caught.contains("ValueError"),
        "caught attribute should mention ValueError: {}",
        caught
    );
}

#[test]
fn try_inside_if_arm_nests_path_under_if() {
    // The if-arm is `c1` at function scope. The try inside it
    // becomes the next arm at the if-arm's scope, so the try body
    // is `c1.c1`. This ensures the try handler honours the
    // `current_branch` it was called with rather than emitting at
    // the outer scope.
    let src = r#"
def f(flag):
    if flag:
        try:
            a()
        except ValueError:
            b()
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(arms.len(), 2, "try body + except, both nested in if");
    for arm in &arms {
        assert!(
            arm.id.contains("::branch::c1."),
            "try arm should be nested under if (c1.*), got {}",
            arm.id
        );
    }
}

#[test]
fn empty_except_body_still_emits_entity() {
    // `except: pass` has no calls but the Branch entity must still
    // be emitted so the decision tree shows the arm was intentional
    // rather than missing.
    let src = r#"
def f():
    try:
        a()
    except ValueError:
        pass
"#;
    let result = parse(src);
    let arms = try_arms(&result);
    assert_eq!(arms.len(), 2);
    assert!(arms.iter().any(|e| e.tags.contains("except_arm")));
}

#[test]
fn calls_inside_try_body_are_branch_tagged() {
    // Symmetric to the except-arm test: calls in the try body
    // should reattach to the try_body Branch, not to the function.
    let src = r#"
def f():
    try:
        in_try()
    except ValueError:
        in_except()
"#;
    let result = parse(src);
    let try_body = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("try_body_arm"))
        .unwrap();
    let try_path = try_body.id.rsplit("::branch::").next().unwrap().to_string();
    let in_try = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("in_try"))
        .expect("in_try call recorded");
    assert_eq!(
        in_try.metadata.get("branch").map(|s| s.as_str()),
        Some(try_path.as_str())
    );
}

#[test]
fn finally_arm_carries_no_caught_metadata() {
    // Sanity: `finally` is unconditional cleanup, never tied to a
    // specific exception type. We must not accidentally bleed a
    // sibling except's caught type onto it.
    let src = r#"
def f():
    try:
        a()
    except ValueError:
        b()
    finally:
        c()
"#;
    let result = parse(src);
    let finally_arm = try_arms(&result)
        .into_iter()
        .find(|e| e.tags.contains("finally_arm"))
        .unwrap();
    assert!(finally_arm.documentation.is_none());
    assert!(!finally_arm
        .attributes
        .iter()
        .any(|a| a.starts_with("caught:")));
}

// ─── PY-002: match/case ─────────────────────────────────────────────────

fn case_arms(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("case_arm"))
        .collect()
}

#[test]
fn match_case_emits_one_arm_per_case() {
    let src = r#"
def f(x):
    match x:
        case 0:
            zero()
        case 1:
            one()
        case _:
            other()
"#;
    let result = parse(src);
    let arms = case_arms(&result);
    assert_eq!(arms.len(), 3, "three case clauses → three arms");
    // Patterns travel as documentation for the detail panel.
    let docs: Vec<&str> = arms
        .iter()
        .filter_map(|e| e.documentation.as_deref())
        .collect();
    assert!(docs.contains(&"pattern: 0"));
    assert!(docs.contains(&"pattern: 1"));
    assert!(docs.contains(&"pattern: _"));
}

#[test]
fn case_arms_share_arm_counter_with_if_branches() {
    // Per the ticket: case arms reuse the if/elif arm counter so a
    // sibling decision tree at the same scope numbers consistently.
    // The if's c1 + the three cases should be c2/c3/c4 — not their
    // own c1/c2/c3 sequence at a parallel scope.
    let src = r#"
def f(x):
    if x:
        a()
    match x:
        case 0:
            zero()
        case _:
            other()
"#;
    let result = parse(src);
    let case_paths: Vec<&str> = case_arms(&result)
        .into_iter()
        .map(|e| e.id.rsplit("::branch::").next().unwrap_or(""))
        .collect();
    assert!(
        case_paths.contains(&"c2"),
        "expected c2 in {:?}",
        case_paths
    );
    assert!(
        case_paths.contains(&"c3"),
        "expected c3 in {:?}",
        case_paths
    );
}

#[test]
fn match_subject_call_stays_in_outer_scope() {
    // A call inside the match subject runs once, before any arm —
    // its branch metadata should NOT be tagged with a case path.
    let src = r#"
def f():
    match get_subject():
        case 0:
            zero()
"#;
    let result = parse(src);
    let subject_call = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("get_subject"))
        .expect("get_subject call recorded");
    assert!(
        !subject_call.metadata.contains_key("branch"),
        "subject call should be in the outer scope, got branch={:?}",
        subject_call.metadata.get("branch")
    );
}

#[test]
fn calls_inside_case_body_are_branch_tagged() {
    let src = r#"
def f(x):
    match x:
        case 0:
            inside_zero()
        case _:
            inside_other()
"#;
    let result = parse(src);
    let zero_arm = case_arms(&result)
        .into_iter()
        .find(|e| e.attributes.iter().any(|a| a == "pattern:0"))
        .expect("zero-pattern arm");
    let zero_path = zero_arm.id.rsplit("::branch::").next().unwrap().to_string();
    let inside = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("inside_zero"))
        .expect("inside_zero call recorded");
    assert_eq!(
        inside.metadata.get("branch").map(|s| s.as_str()),
        Some(zero_path.as_str())
    );
}

#[test]
fn case_guard_calls_attach_to_the_arm() {
    // Guards (`case x if check(x):`) run only when the pattern
    // matches, so their calls are part of this arm's gate — they
    // attach to the arm, NOT the outer scope (which is the
    // difference from if_statement's condition).
    let src = r#"
def f(x):
    match x:
        case n if guard_check(n):
            body_call()
"#;
    let result = parse(src);
    let arm = case_arms(&result).into_iter().next().expect("one case arm");
    let arm_path = arm.id.rsplit("::branch::").next().unwrap().to_string();
    // Pattern text should reflect the guard.
    let doc = arm.documentation.as_deref().unwrap_or("");
    assert!(
        doc.contains("if guard_check(n)"),
        "expected guard in documentation: {:?}",
        doc
    );
    let guard_call = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("guard_check"))
        .expect("guard_check call recorded");
    assert_eq!(
        guard_call.metadata.get("branch").map(|s| s.as_str()),
        Some(arm_path.as_str()),
        "guard call should attach to the arm, not the outer scope"
    );
}

#[test]
fn multi_pattern_case_records_joined_pattern_text() {
    // `case 1 | 2 | 3:` is a single OR-pattern (case_pattern with
    // alternatives) — captured as one pattern. `case 1, 2:` is a
    // multi-pattern sequence — captured as the joined text. Both
    // shapes should survive into documentation.
    let src = r#"
def f(x):
    match x:
        case 1 | 2 | 3:
            small()
"#;
    let result = parse(src);
    let arm = case_arms(&result).into_iter().next().unwrap();
    let doc = arm.documentation.as_deref().unwrap_or("");
    assert!(
        doc.contains('1') && doc.contains('2') && doc.contains('3'),
        "all alternatives should appear in pattern: {:?}",
        doc
    );
}

#[test]
fn case_with_class_pattern_keeps_full_pattern_text() {
    // Class patterns like `Point(x=0, y=0)` capture more structure
    // than literal patterns and are useful in the detail panel as-is.
    let src = r#"
def f(p):
    match p:
        case Point(x=0, y=0):
            origin()
"#;
    let result = parse(src);
    let arm = case_arms(&result).into_iter().next().unwrap();
    let doc = arm.documentation.as_deref().unwrap_or("");
    assert!(
        doc.contains("Point(x=0, y=0)"),
        "class pattern should be preserved verbatim: {:?}",
        doc
    );
}

// ─── PY-003: with / async with ──────────────────────────────────────────

fn with_arms(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("with_node"))
        .collect()
}

#[test]
fn with_statement_emits_with_node_arm() {
    let src = r#"
def f(p):
    with open(p) as fh:
        consume(fh)
"#;
    let result = parse(src);
    let arms = with_arms(&result);
    assert_eq!(arms.len(), 1, "expected one with-block arm");
    let arm = arms[0];
    assert!(!arm.tags.contains("async"));
    let doc = arm.documentation.as_deref().unwrap_or("");
    assert!(
        doc.contains("open(p)"),
        "manager text should travel through documentation: {:?}",
        doc
    );
    assert!(arm
        .attributes
        .iter()
        .any(|a| a.starts_with("manager:") && a.contains("open(p)")));
}

#[test]
fn async_with_tags_async() {
    let src = r#"
async def f(p):
    async with acquire(p) as r:
        run(r)
"#;
    let result = parse(src);
    let arm = with_arms(&result).into_iter().next().expect("with arm");
    assert!(
        arm.tags.contains("async"),
        "async with should set async tag"
    );
    assert!(arm.attributes.iter().any(|a| a == "async"));
}

#[test]
fn calls_inside_with_body_are_branch_tagged() {
    let src = r#"
def f(p):
    with open(p) as fh:
        body_call()
"#;
    let result = parse(src);
    let arm = with_arms(&result).into_iter().next().unwrap();
    let arm_path = arm.id.rsplit("::branch::").next().unwrap().to_string();
    let body_call = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("body_call"))
        .expect("body_call recorded");
    assert_eq!(
        body_call.metadata.get("branch").map(|s| s.as_str()),
        Some(arm_path.as_str()),
    );
}

#[test]
fn with_manager_call_groups_under_with_arm() {
    // The `open(p)` call that produces the resource belongs under the
    // with-block — same as a for-loop's iterator expression — not the
    // outer scope. Without grouping, the manager call would float
    // separately from the body that uses it.
    let src = r#"
def f(p):
    with open(p) as fh:
        body_call()
"#;
    let result = parse(src);
    let arm = with_arms(&result).into_iter().next().unwrap();
    let arm_path = arm.id.rsplit("::branch::").next().unwrap().to_string();
    let mgr_call = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .find(|r| r.target_id.ends_with("open"))
        .expect("open(p) recorded");
    assert_eq!(
        mgr_call.metadata.get("branch").map(|s| s.as_str()),
        Some(arm_path.as_str()),
    );
}

#[test]
fn multi_item_with_captures_all_managers() {
    let src = r#"
def f(a, b):
    with open(a) as fa, open(b) as fb:
        use(fa, fb)
"#;
    let result = parse(src);
    let arm = with_arms(&result).into_iter().next().unwrap();
    let doc = arm.documentation.as_deref().unwrap_or("");
    assert!(
        doc.contains("open(a)") && doc.contains("open(b)"),
        "both managers should appear in documentation: {:?}",
        doc
    );
}

#[test]
fn with_uses_separate_counter_from_branches_and_loops() {
    // Sibling `if`, `for`, and `with` at the same scope must read as
    // c1 / l1 / w1 — not collide on a shared counter.
    let src = r#"
def f(flag, items, p):
    if flag:
        a()
    for item in items:
        b()
    with open(p) as fh:
        c()
"#;
    let result = parse(src);
    let with_arm = with_arms(&result).into_iter().next().unwrap();
    let with_path = with_arm.id.rsplit("::branch::").next().unwrap();
    assert_eq!(with_path, "w1", "with should be w1, got {}", with_path);
}

#[test]
fn nested_with_path_nests_under_outer_with() {
    let src = r#"
def f(a, b):
    with open(a) as fa:
        with open(b) as fb:
            use(fa, fb)
"#;
    let result = parse(src);
    let arms = with_arms(&result);
    assert_eq!(arms.len(), 2);
    let inner = arms
        .iter()
        .find(|e| e.id.contains("::branch::w1.w1"))
        .expect("inner with path is w1.w1");
    assert!(inner.tags.contains("with_node"));
}

#[test]
fn with_inside_if_nests_under_branch() {
    let src = r#"
def f(flag, p):
    if flag:
        with open(p) as fh:
            use(fh)
"#;
    let result = parse(src);
    let arm = with_arms(&result).into_iter().next().unwrap();
    assert!(
        arm.id.contains("::branch::c1.w1"),
        "with inside if should nest as c1.w1, got {}",
        arm.id
    );
}

#[test]
fn empty_with_body_still_emits_entity() {
    // A `with` whose body is just `pass` carries no calls — the arm
    // entity must still appear so the resource scope is visible in
    // the graph rather than disappearing entirely.
    let src = r#"
def f(p):
    with open(p):
        pass
"#;
    let result = parse(src);
    let arms = with_arms(&result);
    assert_eq!(arms.len(), 1);
}

// ─── PY-004: lambda ─────────────────────────────────────────────────────

fn lambda_entities(result: &ParseResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("lambda"))
        .collect()
}

#[test]
fn lambda_emits_synthetic_function_parented_to_caller() {
    let src = r#"
def f(xs):
    return sorted(xs, key=lambda x: x.lower())
"#;
    let result = parse(src);
    let lambdas = lambda_entities(&result);
    assert_eq!(lambdas.len(), 1, "expected one synthetic lambda entity");
    let lam = lambdas[0];
    assert!(
        lam.name.starts_with("<lambda@"),
        "lambda name should be a generated marker: {}",
        lam.name
    );
    let parent = lam.parent_id.as_deref().unwrap_or("");
    assert!(
        parent.ends_with(":f"),
        "lambda should be parented to f, got {}",
        parent
    );
}

#[test]
fn calls_inside_lambda_attach_to_lambda_not_outer() {
    // `.lower()` is invoked on a value the lambda receives — it
    // belongs to the lambda's scope, not f's. Without isolation, the
    // outer caller would falsely show as calling `.lower()`.
    let src = r#"
def f(xs):
    return sorted(xs, key=lambda x: x.lower())
"#;
    let result = parse(src);
    let lam = lambda_entities(&result).into_iter().next().expect("lambda");
    let lower_call = result
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::Calls && r.target_id.ends_with(".lower"))
        .expect("lower call recorded");
    assert_eq!(
        lower_call.source_id, lam.id,
        "lower() should be sourced to the lambda, not the outer fn"
    );
}

#[test]
fn nested_lambda_parents_to_outer_lambda() {
    let src = r#"
def f():
    return lambda x: lambda y: x + y
"#;
    let result = parse(src);
    let lambdas = lambda_entities(&result);
    assert_eq!(lambdas.len(), 2);
    let inner = lambdas
        .iter()
        .find(|e| {
            // The inner lambda's parent_id should reference the other lambda's id.
            let pid = e.parent_id.as_deref().unwrap_or("");
            pid.contains("::lambda::")
        })
        .expect("inner lambda parented to outer lambda");
    assert!(inner.tags.contains("lambda"));
}

#[test]
fn assignment_to_lambda_emits_lambda_entity() {
    // `f = lambda x: x + 1` — the lambda sits as the RHS of an
    // assignment. We must still create a synthetic Function entity for
    // it, regardless of where in an expression tree it lives.
    let src = r#"
def outer():
    f = lambda x: x + 1
    return f(2)
"#;
    let result = parse(src);
    let lambdas = lambda_entities(&result);
    assert_eq!(lambdas.len(), 1);
}

// ─── PY-005: yield / generators ─────────────────────────────────────────

fn entity_named<'a>(result: &'a ParseResult, name: &str) -> &'a CodeEntity {
    result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named {}", name))
}

#[test]
fn function_with_yield_is_tagged_generator() {
    let src = r#"
def gen():
    yield 1
    yield 2
"#;
    let result = parse(src);
    let gen = entity_named(&result, "gen");
    assert!(gen.tags.contains("generator"), "expected generator tag");
    assert!(!gen.tags.contains("async_generator"));
}

#[test]
fn function_with_yield_from_is_tagged_generator() {
    let src = r#"
def gen(xs):
    yield from xs
"#;
    let result = parse(src);
    let gen = entity_named(&result, "gen");
    assert!(gen.tags.contains("generator"));
}

#[test]
fn async_function_with_yield_is_async_generator() {
    let src = r#"
async def agen():
    yield 1
"#;
    let result = parse(src);
    let agen = entity_named(&result, "agen");
    assert!(agen.tags.contains("generator"));
    assert!(agen.tags.contains("async_generator"));
}

#[test]
fn yield_inside_nested_block_still_tags_outer_function() {
    let src = r#"
def gen(items):
    for item in items:
        if item:
            yield item
"#;
    let result = parse(src);
    let gen = entity_named(&result, "gen");
    assert!(gen.tags.contains("generator"));
}

#[test]
fn yield_in_nested_function_does_not_taint_outer() {
    // The inner function is the generator; the outer just defines and
    // returns it. Without scoping the scan, the outer would also be
    // mis-tagged as a generator.
    let src = r#"
def outer():
    def inner():
        yield 1
    return inner
"#;
    let result = parse(src);
    let outer = entity_named(&result, "outer");
    assert!(
        !outer.tags.contains("generator"),
        "outer should not be a generator"
    );
    let inner = entity_named(&result, "inner");
    assert!(inner.tags.contains("generator"));
}

#[test]
fn function_without_yield_is_not_tagged_generator() {
    let src = r#"
def f():
    return 1
"#;
    let result = parse(src);
    let f = entity_named(&result, "f");
    assert!(!f.tags.contains("generator"));
}

// ─── PY-006: walrus operator ────────────────────────────────────────────

fn local_var<'a>(result: &'a ParseResult, name: &str) -> Option<&'a CodeEntity> {
    result
        .entities
        .iter()
        .find(|e| e.name == name && e.tags.contains("local_var"))
}

#[test]
fn walrus_in_if_condition_emits_local() {
    let src = r#"
def f(xs):
    if (n := len(xs)) > 10:
        use(n)
"#;
    let result = parse(src);
    let n = local_var(&result, "n").expect("walrus-introduced local n");
    assert!(n.tags.contains("local_var"));
}

#[test]
fn walrus_write_carries_branch_tag_when_inside_arm() {
    // The walrus lives in the if's *condition*, which extract_calls
    // recurses with the OUTER scope — the write should not carry the
    // arm's branch tag because it runs before the arm is entered.
    let src = r#"
def f(xs):
    if (n := len(xs)) > 10:
        use(n)
"#;
    let result = parse(src);
    let writes: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::WritesTo && r.target_id.ends_with("::n"))
        .collect();
    assert_eq!(writes.len(), 1);
    assert!(
        !writes[0].metadata.contains_key("branch"),
        "walrus in if-condition should not carry an arm tag"
    );
}

#[test]
fn walrus_inside_arm_inherits_branch() {
    // A walrus that lives inside the arm body runs only when the arm
    // is taken, so its write should be tagged with the arm's path.
    //
    // Since PY-012 the comprehension is its own Loop scope, so the path is
    // the more specific `c1.l1` — the walrus runs once per iteration of a
    // generator that only runs when the arm is taken. Still nested under
    // `c1`, which is what this test is about.
    let src = r#"
def f(xs):
    if xs:
        total = sum((m := compute(x)) for x in xs)
"#;
    let result = parse(src);
    let m_write = result
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::WritesTo && r.target_id.ends_with("::m"))
        .expect("walrus write for m");
    assert!(
        m_write
            .metadata
            .get("branch")
            .is_some_and(|b| b.starts_with("c1.l")),
        "walrus inside arm should inherit the arm's branch path, got {:?}",
        m_write.metadata.get("branch")
    );
}

// ─── PY-007: tuple / starred unpacking ──────────────────────────────────

#[test]
fn tuple_assignment_emits_one_local_per_target() {
    let src = r#"
def f():
    a, b = compute()
"#;
    let result = parse(src);
    assert!(local_var(&result, "a").is_some(), "a should be a local");
    assert!(local_var(&result, "b").is_some(), "b should be a local");
}

#[test]
fn parenthesized_tuple_assignment_emits_locals() {
    let src = r#"
def f():
    (a, b) = compute()
"#;
    let result = parse(src);
    assert!(local_var(&result, "a").is_some());
    assert!(local_var(&result, "b").is_some());
}

#[test]
fn starred_unpack_tags_splat() {
    let src = r#"
def f(xs):
    head, *tail = xs
"#;
    let result = parse(src);
    let head = local_var(&result, "head").expect("head local");
    assert!(!head.tags.contains("splat"));
    let tail = local_var(&result, "tail").expect("tail local");
    assert!(tail.tags.contains("splat"), "*tail should carry splat tag");
}

#[test]
fn nested_tuple_pattern_still_collects_all_targets() {
    let src = r#"
def f():
    (a, (b, c)) = compute()
"#;
    let result = parse(src);
    for n in ["a", "b", "c"] {
        assert!(local_var(&result, n).is_some(), "{} should be a local", n);
    }
}

// ─── PY-008: chained assignment ─────────────────────────────────────────

#[test]
fn chained_assignment_emits_local_for_every_target() {
    let src = r#"
def f():
    x = y = compute()
"#;
    let result = parse(src);
    let x = local_var(&result, "x").expect("x local");
    let y = local_var(&result, "y").expect("y local");
    // Both share the innermost RHS as documentation.
    assert_eq!(x.documentation.as_deref(), Some("= compute()"));
    assert_eq!(y.documentation.as_deref(), Some("= compute()"));
}

#[test]
fn chained_assignment_emits_one_writes_to_per_target() {
    let src = r#"
def f():
    x = y = compute()
"#;
    let result = parse(src);
    let writes: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::WritesTo)
        .collect();
    let x_writes = writes
        .iter()
        .filter(|r| r.target_id.ends_with("::x"))
        .count();
    let y_writes = writes
        .iter()
        .filter(|r| r.target_id.ends_with("::y"))
        .count();
    assert_eq!(x_writes, 1, "x should have exactly one WritesTo edge");
    assert_eq!(y_writes, 1, "y should have exactly one WritesTo edge");
}

#[test]
fn chained_assignment_combined_with_tuple_unpack() {
    // PY-007 + PY-008 cross-test: tuple LHS in a chain.
    let src = r#"
def f():
    (a, b) = c = compute()
"#;
    let result = parse(src);
    for n in ["a", "b", "c"] {
        assert!(local_var(&result, n).is_some(), "{} should be a local", n);
    }
}

// ─── PY-025: UsesType edges from annotations ────────────────────────────

fn uses_type_targets(result: &ParseResult, source_suffix: &str) -> Vec<String> {
    let mut targets: Vec<String> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::UsesType && r.source_id.ends_with(source_suffix))
        .map(|r| r.target_id.clone())
        .collect();
    targets.sort();
    targets
}

#[test]
fn annotated_signature_emits_uses_type_edges() {
    let src = r#"
def process(o: Order, items: list[LineItem]) -> Receipt:
    pass
"#;
    let result = parse(src);
    assert_eq!(
        uses_type_targets(&result, "process"),
        vec!["LineItem", "Order", "Receipt"]
    );
}

#[test]
fn quoted_annotation_resolves_like_plain() {
    let src = r#"
def load(ref: "Order") -> "Order":
    pass
"#;
    let result = parse(src);
    // Dedup: parameter and return both mention Order → one edge.
    assert_eq!(uses_type_targets(&result, "load"), vec!["Order"]);
}

#[test]
fn unannotated_code_emits_no_uses_type_edges() {
    let src = r#"
def process(o, items):
    return items

class Plain:
    def method(self, x):
        self.x = x
"#;
    let result = parse(src);
    assert!(
        !result
            .relationships
            .iter()
            .any(|r| r.kind == RelationshipKind::UsesType),
        "absent annotations must emit nothing"
    );
}

#[test]
fn dataclass_annotated_attributes_emit_uses_type_edges() {
    let src = r#"
from dataclasses import dataclass

@dataclass
class Invoice:
    order: Order
    items: list[LineItem]
    total: float
"#;
    let result = parse(src);
    assert_eq!(
        uses_type_targets(&result, "Invoice"),
        vec!["LineItem", "Order"]
    );
}

#[test]
fn method_mention_of_owning_class_is_skipped() {
    let src = r#"
class Order:
    def merge(self, other: "Order") -> "Order":
        return self

    def receipt(self) -> Receipt:
        pass
"#;
    let result = parse(src);
    let targets: Vec<String> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::UsesType)
        .map(|r| r.target_id.clone())
        .collect();
    assert_eq!(
        targets,
        vec!["Receipt"],
        "owning-class mentions carry no signal"
    );
}

#[test]
fn typing_and_builtin_names_are_filtered() {
    let src = r#"
from typing import Optional

def find(key: str) -> Optional[Order]:
    pass

def unions(x: Order | None) -> dict[str, LineItem]:
    pass
"#;
    let result = parse(src);
    assert_eq!(uses_type_targets(&result, "find"), vec!["Order"]);
    assert_eq!(
        uses_type_targets(&result, "unions"),
        vec!["LineItem", "Order"]
    );
}

// ---------------------------------------------------------------------
// PY-027 — complexity metrics
// ---------------------------------------------------------------------

/// `(cyclomatic, cognitive, max_nesting)` for the named callable.
fn metrics_of(result: &ParseResult, name: &str) -> (u32, u32, u32) {
    let e = result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named {}", name));
    (
        e.metrics.cyclomatic.expect("cyclomatic"),
        e.metrics.cognitive_complexity.expect("cognitive"),
        e.metrics.max_nesting.expect("max_nesting"),
    )
}

/// The cross-language reference fixture from the parser-readiness audit,
/// ported from Java. Java scores it 12 / 31 / 6; the whole point of
/// modelling `elif_clause` as a nesting increment and `else_clause` as a
/// flat one is that Python lands on the same numbers. Pinned exactly so a
/// tree-sitter grammar bump can't silently re-score it.
#[test]
fn gnarly_fixture_scores_the_same_as_java() {
    let src = r#"
class A:
    def gnarly(self, a, b, c, d):
        if a > 0:
            for x in range(b):
                if x > 0 and c > 0:
                    while d > 0:
                        try:
                            if x > 1:
                                pass
                            elif x < 0:
                                pass
                            else:
                                pass
                        except RuntimeError:
                            pass
        elif b > 0 or c > 0:
            return 1
        return 2
"#;
    let result = parse(src);
    assert_eq!(metrics_of(&result, "gnarly"), (12, 31, 6));
}

#[test]
fn straight_line_function_scores_one_and_zero() {
    let result = parse("def f(a, b):\n    return a + b\n");
    assert_eq!(metrics_of(&result, "f"), (1, 0, 0));
    let e = result.entities.iter().find(|e| e.name == "f").unwrap();
    assert_eq!(e.metrics.loc, 2);
    assert_eq!(e.metrics.param_count, Some(2));
}

/// A bodyless stub must be *measured as trivial*, not left unmeasured —
/// `None` reads as "clean" to the hotspot ranking, which is the failure
/// mode PY-027 exists to close.
#[test]
fn stub_bodies_are_measured_not_skipped() {
    let src = r#"
from typing import overload

def ellipsis_stub(x): ...

def pass_stub(x):
    pass

@overload
def over(x: int) -> int: ...
"#;
    let result = parse(src);
    for name in ["ellipsis_stub", "pass_stub", "over"] {
        assert_eq!(metrics_of(&result, name), (1, 0, 0), "{}", name);
    }
}

/// `.pyi` files are all stubs, so every callable in one must land on the
/// trivial score rather than picking up phantom branches from annotations.
#[test]
fn pyi_stub_file_scores_trivially() {
    let parser = PythonParser::new();
    let src = "def find(key: str) -> Optional[User]: ...\n\nclass Repo:\n    def all(self) -> List[User]: ...\n";
    let result = parser.parse(Path::new("repo.pyi"), src).expect("parse");
    assert_eq!(metrics_of(&result, "find"), (1, 0, 0));
    assert_eq!(metrics_of(&result, "all"), (1, 0, 0));
}

/// A comprehension's loop and filter each branch; the comprehension node
/// itself only opens the nesting scope.
#[test]
fn comprehension_loop_and_filter_are_branches() {
    let result = parse("def f(xs):\n    return [y for y in xs if y > 0]\n");
    assert_eq!(metrics_of(&result, "f"), (3, 2, 1));
}

#[test]
fn nested_comprehension_deepens_nesting() {
    let result = parse("def f(rows):\n    return [c for r in rows for c in r]\n");
    // Two `for_in_clause` branches, one comprehension scope.
    assert_eq!(metrics_of(&result, "f"), (3, 1, 1));
}

/// Each `case` is a branch; the `match` itself carries the cognitive
/// nesting increment. Same split Java uses for `switch` / `switch_label`.
#[test]
fn match_statement_counts_one_branch_per_case() {
    let src = r#"
def f(xs):
    match xs:
        case []:
            pass
        case [a]:
            pass
        case _:
            pass
"#;
    let result = parse(src);
    assert_eq!(metrics_of(&result, "f"), (4, 1, 2));
}

#[test]
fn ternary_is_a_branch_but_with_is_not() {
    let ternary = parse("def f(xs):\n    return 1 if xs else 2\n");
    assert_eq!(metrics_of(&ternary, "f"), (2, 1, 1));

    let with_block = parse("def g(p):\n    with open(p) as fh:\n        fh.read()\n");
    assert_eq!(metrics_of(&with_block, "g"), (1, 0, 0));
}

/// `finally` takes no branch; each `except` arm does.
#[test]
fn except_arms_branch_and_finally_does_not() {
    let src = r#"
def f(p):
    try:
        go()
    except ValueError:
        pass
    except OSError:
        pass
    finally:
        done()
"#;
    let result = parse(src);
    assert_eq!(metrics_of(&result, "f"), (3, 2, 1));
}

/// `async for` / `async with` are the same node kinds as their sync forms,
/// so they must score identically without special-casing.
#[test]
fn async_loops_score_as_their_sync_forms() {
    let sync = parse("def f(xs):\n    for x in xs:\n        use(x)\n");
    let asynchronous = parse("async def f(xs):\n    async for x in xs:\n        use(x)\n");
    assert_eq!(metrics_of(&sync, "f"), metrics_of(&asynchronous, "f"));
}

#[test]
fn each_boolean_operator_adds_one_to_both_scores() {
    let result = parse("def f(a, b, c):\n    return a and b or c\n");
    assert_eq!(metrics_of(&result, "f"), (3, 2, 0));
}

/// A lambda is a callable entity of its own, so it carries the same metric
/// set as a `def` — and also contributes to its container, exactly as a
/// Java lambda contributes to the enclosing method.
#[test]
fn lambda_carries_its_own_metrics() {
    let result = parse("def f(xs):\n    return sorted(xs, key=lambda t: t.a if t else t.b)\n");
    let lam = result
        .entities
        .iter()
        .find(|e| e.tags.contains("lambda"))
        .expect("lambda entity");
    assert_eq!(lam.metrics.cyclomatic, Some(2));
    assert_eq!(lam.metrics.param_count, Some(1));
    // The enclosing function sees the lambda as a nesting scope plus the
    // ternary inside it.
    assert_eq!(metrics_of(&result, "f"), (2, 3, 2));
}

#[test]
fn classes_carry_size_and_encapsulation_metrics() {
    let src = r#"
class Account:
    owner: str

    def __init__(self, owner):
        self.owner = owner
        self._balance = 0
        self.__pin = None
"#;
    let result = parse(src);
    let cls = result
        .entities
        .iter()
        .find(|e| e.name == "Account")
        .expect("class");
    assert_eq!(cls.metrics.field_count, Some(3));
    assert!(cls.metrics.loc >= 7);
    // `owner` public, `_balance` protected, `__pin` private.
    assert_eq!(cls.metrics.public_field_ratio, Some(1.0 / 3.0));
}

#[test]
fn tuple_return_annotation_scores_return_complexity() {
    let src = r#"
def pair() -> tuple[int, str]:
    return 1, "a"

def triple() -> Tuple[int, str, bool]:
    return 1, "a", True

def homogeneous() -> tuple[int, ...]:
    return (1, 2)

def single() -> int:
    return 1
"#;
    let result = parse(src);
    let rc = |name: &str| {
        result
            .entities
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .metrics
            .return_complexity
    };
    assert_eq!(rc("pair"), Some(2));
    assert_eq!(rc("triple"), Some(3));
    assert_eq!(rc("homogeneous"), None);
    assert_eq!(rc("single"), None);
}

// ---------------------------------------------------------------------
// PY-026 — call-chain callee names
// ---------------------------------------------------------------------

fn call_targets(result: &ParseResult) -> Vec<String> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .map(|r| r.target_id.clone())
        .collect()
}

#[test]
fn single_line_chain_names_each_link_by_its_method() {
    let src = r#"
def make():
    return builder.set_size("medium").set_dough("thin").build()
"#;
    let targets = call_targets(&parse(src));
    assert!(
        targets.contains(&"builder.set_size".to_string()),
        "{:?}",
        targets
    );
    assert!(
        targets.contains(&"set_size.set_dough".to_string()),
        "{:?}",
        targets
    );
    assert!(
        targets.contains(&"set_dough.build".to_string()),
        "{:?}",
        targets
    );
}

/// The case that fails today: a chain broken across lines used to carry
/// its own newlines and indentation into the callee name.
#[test]
fn multi_line_chain_reduces_to_the_same_names() {
    let src = "def make():\n    return (\n        builder\n        .set_size(\"medium\")\n        .set_dough(\"thin\")\n        .build()\n    )\n";
    let targets = call_targets(&parse(src));
    assert!(
        targets.contains(&"builder.set_size".to_string()),
        "{:?}",
        targets
    );
    assert!(
        targets.contains(&"set_size.set_dough".to_string()),
        "{:?}",
        targets
    );
    assert!(
        targets.contains(&"set_dough.build".to_string()),
        "{:?}",
        targets
    );
}

/// A dot inside a string literal must never reach a name. The receiver is
/// named by its type instead.
#[test]
fn string_literal_receiver_is_named_by_its_type() {
    let src = r#"
def f():
    return "a.b.c".split(".").pop()
"#;
    let targets = call_targets(&parse(src));
    assert!(targets.contains(&"str.split".to_string()), "{:?}", targets);
    assert!(targets.contains(&"split.pop".to_string()), "{:?}", targets);
}

#[test]
fn no_call_target_carries_source_punctuation() {
    let src = "def make():\n    return (\n        builder\n        .set_size(\"medium\")\n        .add(items[0].name)\n        .build()\n    )\n";
    for target in call_targets(&parse(src)) {
        assert!(
            !target.contains('\n') && !target.contains('"') && !target.contains('('),
            "leaked source text: {:?}",
            target
        );
    }
}

/// Attribute receivers keep resolving to their last segment, and `self` /
/// `super()` keep their existing rewrite onto the enclosing class.
#[test]
fn attribute_and_super_receivers_are_unchanged() {
    let src = r#"
class Child(Base):
    def run(self):
        self.repo.find(1)
        self.helper()
        super().run_base()
"#;
    let targets = call_targets(&parse(src));
    assert!(targets.contains(&"repo.find".to_string()), "{:?}", targets);
    assert!(
        targets.contains(&"Child.helper".to_string()),
        "{:?}",
        targets
    );
    assert!(
        targets.contains(&"Child.run_base".to_string()),
        "{:?}",
        targets
    );
}

/// Whole-graph guard for the acceptance criterion: nothing the parser
/// names may contain a newline or a quote.
#[test]
fn no_entity_name_contains_source_punctuation() {
    let src = "class B:\n    def go(self):\n        return (\n            self.b\n            .set(\"x\")\n            .build()\n        )\n";
    let result = parse(src);
    for e in &result.entities {
        assert!(
            !e.name.contains('\n') && !e.name.contains('"') && !e.name.contains('\''),
            "leaked source text in entity name: {:?}",
            e.name
        );
    }
}

// ---------------------------------------------------------------------
// PY-014 / PY-015 / PY-016 / PY-022 — class-level semantics
// ---------------------------------------------------------------------

fn class_named<'a>(result: &'a ParseResult, name: &str) -> &'a CodeEntity {
    result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named {}", name))
}

/// PY-014: `NamedTuple` and `TypedDict` are `@dataclass` said differently.
#[test]
fn namedtuple_and_typeddict_are_dataclasses() {
    let src = r#"
import typing
from typing import NamedTuple, TypedDict

class Point(NamedTuple):
    x: int

class Config(typing.TypedDict):
    host: str

class Plain:
    pass
"#;
    let result = parse(src);
    let point = class_named(&result, "Point");
    assert_eq!(point.kind, crate::models::EntityKind::Dataclass);
    assert!(point.tags.contains("namedtuple"), "{:?}", point.tags);
    assert!(point.tags.contains("dataclass"));

    let config = class_named(&result, "Config");
    assert_eq!(config.kind, crate::models::EntityKind::Dataclass);
    assert!(config.tags.contains("typeddict"), "{:?}", config.tags);

    assert_eq!(
        class_named(&result, "Plain").kind,
        crate::models::EntityKind::Class
    );
}

/// Abstract still outranks the record kinds.
#[test]
fn abstract_still_wins_over_dataclass_promotion() {
    let src = r#"
from abc import ABC
from dataclasses import dataclass

@dataclass
class Shape(ABC):
    sides: int
"#;
    let result = parse(src);
    let shape = class_named(&result, "Shape");
    assert_eq!(shape.kind, crate::models::EntityKind::AbstractClass);
    assert!(shape.tags.contains("dataclass"));
}

/// PY-015: either a builtin exception base or the naming convention.
#[test]
fn exception_classes_are_tagged() {
    let src = r#"
class ConfigError(Exception):
    pass

class AppError(ValueError):
    pass

class TimeoutWarning(Warning):
    pass

# Base is a project class this pass never sees — the name carries it.
class ParseError(AppError):
    pass

class Repository:
    pass
"#;
    let result = parse(src);
    for name in ["ConfigError", "AppError", "TimeoutWarning", "ParseError"] {
        assert!(
            class_named(&result, name).tags.contains("exception"),
            "{} not tagged",
            name
        );
    }
    assert!(!class_named(&result, "Repository")
        .tags
        .contains("exception"));
}

/// PY-016: the base of `class Foo(Bag[int])` is `Bag`, not `Bag[int]` — the
/// literal text matched no entity and the inheritance edge was lost.
#[test]
fn subscripted_bases_record_the_base_not_the_subscript() {
    let src = r#"
from typing import Generic, TypeVar

class Bag:
    pass

class IntBag(Bag[int]):
    pass

class Pair(Generic[str, int]):
    pass
"#;
    let result = parse(src);
    let int_bag = class_named(&result, "IntBag");
    assert_eq!(int_bag.extends, vec!["Bag"]);
    assert_eq!(int_bag.generics, vec!["int"]);

    let pair = class_named(&result, "Pair");
    assert_eq!(pair.extends, vec!["Generic"]);
    assert_eq!(pair.generics, vec!["str", "int"]);
}

/// PY-022: the names inside `__slots__` are the fields; `__slots__` is not.
#[test]
fn slots_declare_fields_and_slots_itself_does_not() {
    let src = r#"
class Account:
    __slots__ = ("owner", "_balance")

    def __init__(self, owner):
        self.owner = owner
"#;
    let result = parse(src);
    let account = class_named(&result, "Account");
    let names: Vec<&str> = account.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["owner", "_balance"]);
    assert!(account.tags.contains("has_slots"));
    // `owner` is also assigned in __init__ — it must not be recorded twice.
    assert_eq!(account.metrics.field_count, Some(2));
}

#[test]
fn slots_accept_every_literal_form() {
    let list_form = parse("class A:\n    __slots__ = [\"a\", \"b\"]\n");
    assert_eq!(
        class_named(&list_form, "A")
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );

    let single = parse("class B:\n    __slots__ = \"only\"\n");
    assert_eq!(
        class_named(&single, "B")
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        vec!["only"]
    );

    // A dict's keys are the slots; the values are per-slot docstrings.
    let dict_form = parse("class C:\n    __slots__ = {\"a\": \"the a\", \"b\": \"the b\"}\n");
    assert_eq!(
        class_named(&dict_form, "C")
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );

    let empty = parse("class D:\n    __slots__ = ()\n");
    let d = class_named(&empty, "D");
    assert!(d.tags.contains("has_slots"));
    assert!(d.fields.is_empty());
}

// ---------------------------------------------------------------------
// PY-013 / PY-019 / PY-023 — module-level bindings
// ---------------------------------------------------------------------

fn kind_of(result: &ParseResult, name: &str) -> crate::models::EntityKind {
    result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named {}", name))
        .kind
}

/// PY-013: a subscript of a type constructor, or an explicit `: TypeAlias`.
#[test]
fn type_aliases_are_promoted_from_variables() {
    let src = r#"
from typing import Callable, Dict, Optional, TypeAlias

Handler = Callable[[int], None]
MaybeUser = Optional[User]
JSON: TypeAlias = dict
Rows = list[dict]
MAX_RETRIES = 3
registry = {}
"#;
    let result = parse(src);
    use crate::models::EntityKind;
    assert_eq!(kind_of(&result, "Handler"), EntityKind::TypeAlias);
    assert_eq!(kind_of(&result, "MaybeUser"), EntityKind::TypeAlias);
    assert_eq!(kind_of(&result, "JSON"), EntityKind::TypeAlias);
    assert_eq!(kind_of(&result, "Rows"), EntityKind::TypeAlias);
    // Ordinary bindings keep their kinds.
    assert_eq!(kind_of(&result, "MAX_RETRIES"), EntityKind::Constant);
    assert_eq!(kind_of(&result, "registry"), EntityKind::Variable);
}

/// The alias rule outranks the UPPER_CASE constant rule.
#[test]
fn an_upper_case_alias_is_still_an_alias() {
    let result = parse("from typing import Dict, Any\n\nJSON = Dict[str, Any]\n");
    assert_eq!(
        kind_of(&result, "JSON"),
        crate::models::EntityKind::TypeAlias
    );
}

/// PEP 695 `type X = …` is its own statement kind and reached no arm before.
#[test]
fn pep695_type_statement_becomes_a_type_alias() {
    let result = parse("type Vec3 = tuple[float, float, float]\n");
    let alias = result
        .entities
        .iter()
        .find(|e| e.name == "Vec3")
        .expect("Vec3");
    assert_eq!(alias.kind, crate::models::EntityKind::TypeAlias);
    assert_eq!(
        alias.return_type.as_deref(),
        Some("tuple[float, float, float]")
    );
}

/// PY-019: a module maintained only with `+=` used to look stateless.
#[test]
fn module_augmented_assignment_declares_state() {
    let result = parse("COUNTER += 1\n");
    assert_eq!(
        kind_of(&result, "COUNTER"),
        crate::models::EntityKind::Constant
    );
}

/// Rebinding is not redeclaring — first write wins, as it does for locals.
#[test]
fn a_rebound_module_name_yields_one_entity() {
    let result = parse("COUNTER = 0\nCOUNTER += 1\nCOUNTER = 5\n");
    let hits = result
        .entities
        .iter()
        .filter(|e| e.name == "COUNTER")
        .count();
    assert_eq!(hits, 1, "module rebinding produced duplicate entities");
}

/// PY-023: `T = TypeVar("T")` stays a Variable at runtime, but is flagged as
/// a type-system participant.
#[test]
fn typevar_factories_are_tagged() {
    let src = r#"
import typing
from typing import TypeVar, ParamSpec

T = TypeVar("T")
P = ParamSpec("P")
Ts = typing.TypeVarTuple("Ts")
counter = int("3")
"#;
    let result = parse(src);
    let tagged = |name: &str| {
        result
            .entities
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .tags
            .contains("type_param")
    };
    assert!(tagged("T"));
    assert!(tagged("P"));
    assert!(tagged("Ts"));
    assert!(!tagged("counter"));
}

/// PEP 695 bracket groups on classes and functions.
#[test]
fn pep695_type_parameters_are_recorded_as_generics() {
    let result =
        parse("class Box[T]:\n    pass\n\ndef first[T](xs: list[T]) -> T:\n    return xs[0]\n");
    let generics = |name: &str| {
        result
            .entities
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .generics
            .clone()
    };
    assert_eq!(generics("Box"), vec!["T"]);
    assert_eq!(generics("first"), vec!["T"]);
}

// ---------------------------------------------------------------------
// PY-018 / PY-024 — import shape
// ---------------------------------------------------------------------

/// PY-018: an empty `items` used to mean either "captured nothing" or "asked
/// for everything". Now the star says so.
#[test]
fn star_imports_are_flagged() {
    let result = parse("from foo.bar import *\nfrom baz import thing\n");
    let star = result
        .imports
        .iter()
        .find(|i| i.path == "foo.bar")
        .expect("foo.bar");
    assert!(star.is_wildcard);
    assert!(star.items.is_empty());

    let named = result
        .imports
        .iter()
        .find(|i| i.path == "baz")
        .expect("baz");
    assert!(!named.is_wildcard);
    assert_eq!(named.items, vec!["thing"]);
}

/// PY-024: `try` says "one of these", `if` says "when the gate opens".
#[test]
fn conditional_imports_record_their_wrapper() {
    let src = r#"
import os

try:
    import ujson as json
except ImportError:
    import json

if TYPE_CHECKING:
    from models import User
"#;
    let result = parse(src);
    use crate::parser::language_parser::ImportCondition;
    let condition = |path: &str| {
        result
            .imports
            .iter()
            .find(|i| i.path == path)
            .unwrap_or_else(|| panic!("no import of {}", path))
            .condition
    };
    assert_eq!(condition("os"), None);
    assert_eq!(condition("ujson"), Some(ImportCondition::Fallback));
    assert_eq!(condition("json"), Some(ImportCondition::Fallback));
    assert_eq!(condition("models"), Some(ImportCondition::Guarded));
}

/// The wrapper in force is restored on the way out — a `try` inside an `if`
/// must not leave the rest of the `if` looking like a fallback.
#[test]
fn nested_conditionals_restore_the_outer_wrapper() {
    let src = r#"
if sys.version_info >= (3, 11):
    try:
        import tomllib
    except ImportError:
        pass
    from typing import Self
"#;
    let result = parse(src);
    use crate::parser::language_parser::ImportCondition;
    let condition = |path: &str| {
        result
            .imports
            .iter()
            .find(|i| i.path == path)
            .unwrap()
            .condition
    };
    assert_eq!(condition("tomllib"), Some(ImportCondition::Fallback));
    assert_eq!(condition("typing"), Some(ImportCondition::Guarded));
}

/// Imports inside an `if` still have to be *found* — the new arm must recurse
/// exactly as the default one did.
#[test]
fn conditional_imports_are_still_collected() {
    let result = parse("if True:\n    import os\n");
    assert_eq!(result.imports.len(), 1);
}

// ---------------------------------------------------------------------
// PY-010 / PY-017 / PY-020 / PY-021
// ---------------------------------------------------------------------

/// PY-010: what a function raises was invisible unless the raise happened to
/// be a call, and even then it was indistinguishable from constructing one.
#[test]
fn raises_produce_reference_edges() {
    let src = r#"
def check(v):
    if v is None:
        raise ValueError("nope")
    raise ConfigError
"#;
    let result = parse(src);
    let raised: Vec<&str> = result
        .relationships
        .iter()
        .filter(|r| r.metadata.get("raises").map(String::as_str) == Some("true"))
        .map(|r| r.target_id.as_str())
        .collect();
    assert!(raised.contains(&"ValueError"), "{:?}", raised);
    assert!(raised.contains(&"ConfigError"), "{:?}", raised);
}

/// A bare `raise` re-raises whatever is in flight — it names nothing.
#[test]
fn a_bare_reraise_names_nothing() {
    let src = r#"
def f():
    try:
        go()
    except ValueError:
        raise
"#;
    let result = parse(src);
    assert!(result
        .relationships
        .iter()
        .all(|r| !r.metadata.contains_key("raises")));
}

/// Raising inside an `except` arm is the common case; the edge belongs to
/// that arm, not to the function.
#[test]
fn a_raise_inside_an_arm_carries_the_branch_tag() {
    let src = r#"
def f():
    try:
        go()
    except ValueError:
        raise ConfigError("bad")
"#;
    let result = parse(src);
    let rel = result
        .relationships
        .iter()
        .find(|r| r.target_id == "ConfigError" && r.metadata.contains_key("raises"))
        .expect("raises edge");
    assert!(rel.metadata.contains_key("branch"), "{:?}", rel.metadata);
}

/// PY-017: a project's own decorators are dependencies, and were invisible.
#[test]
fn decorators_produce_call_edges() {
    let src = r#"
@retry
@router.get("/users")
def list_users():
    pass

@register
class Service:
    pass
"#;
    let result = parse(src);
    let decorated: Vec<&str> = result
        .relationships
        .iter()
        .filter(|r| r.metadata.get("decorator").map(String::as_str) == Some("true"))
        .map(|r| r.target_id.as_str())
        .collect();
    assert!(decorated.contains(&"retry"), "{:?}", decorated);
    // A factory call depends on the factory, not on the call.
    assert!(decorated.contains(&"router.get"), "{:?}", decorated);
    assert!(decorated.contains(&"register"), "{:?}", decorated);
}

/// Language-protocol decorators are already tags; an edge to them would be
/// noise on every method in a codebase.
#[test]
fn builtin_decorators_produce_no_edges() {
    let src = r#"
class A:
    @property
    def value(self):
        return 1

    @staticmethod
    def make():
        return A()
"#;
    let result = parse(src);
    assert!(
        result
            .relationships
            .iter()
            .all(|r| !r.metadata.contains_key("decorator")),
        "builtin decorator produced an edge"
    );
}

/// PY-020: overload stubs stay in the graph but say what they are.
#[test]
fn overload_stubs_are_tagged() {
    let src = r#"
from typing import overload

@overload
def get(key: str) -> str: ...

@overload
def get(key: int) -> int: ...

def get(key):
    return key
"#;
    let result = parse(src);
    let stubs = result
        .entities
        .iter()
        .filter(|e| e.name == "get" && e.tags.contains("overload_stub"))
        .count();
    let impls = result
        .entities
        .iter()
        .filter(|e| e.name == "get" && !e.tags.contains("overload_stub"))
        .count();
    assert_eq!(stubs, 2);
    assert_eq!(impls, 1);
}

/// PY-021: `async for` awaits once per iteration — same node kind as `for`,
/// so it needed the source slice to tell them apart.
#[test]
fn async_loops_and_with_blocks_are_tagged() {
    let src = r#"
async def stream(source):
    async for chunk in source:
        use(chunk)
    for x in [1]:
        use(x)
    async with lock():
        use(1)
"#;
    let result = parse(src);
    let loops: Vec<bool> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("loop_node"))
        .map(|e| e.tags.contains("async_loop"))
        .collect();
    assert_eq!(loops, vec![true, false], "one async loop then one sync one");

    let with_node = result
        .entities
        .iter()
        .find(|e| e.tags.contains("with_node"))
        .expect("with entity");
    assert!(with_node.tags.contains("async_with"));
}

/// A `while` loop is its own Loop scope: the condition (re-evaluated every
/// iteration) and the body both group under it, and a `while ... else`
/// clause becomes a branch arm nested inside the loop. Characterises the
/// while handling, which otherwise had no dedicated test.
#[test]
fn while_loop_groups_condition_body_and_else() {
    let src = r#"
def f(x):
    while cond(x):
        body(x)
    else:
        done(x)
"#;
    let result = parse(src);

    // Exactly one Loop entity, at path l1, and it is never async.
    let loop_node = result
        .entities
        .iter()
        .find(|e| e.tags.contains("loop_node"))
        .expect("while loop entity");
    assert_eq!(loop_node.name, "l1");
    assert!(!loop_node.tags.contains("async_loop"));

    // The else clause becomes a branch arm parented on the loop.
    let branch = result
        .entities
        .iter()
        .find(|e| e.tags.contains("branch_node"))
        .expect("while-else branch entity");
    assert_eq!(
        branch.parent_id.as_deref(),
        Some(loop_node.id.as_str()),
        "while-else arm should nest under the loop"
    );

    // Condition and body calls group under the loop; the else call under
    // its arm.
    let branch_of = |callee: &str| -> String {
        result
            .relationships
            .iter()
            .find(|r| r.target_id.ends_with(callee))
            .unwrap_or_else(|| panic!("no call to {}", callee))
            .metadata
            .get("branch")
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(branch_of("cond"), "l1", "condition call groups under loop");
    assert_eq!(branch_of("body"), "l1", "body call groups under loop");
    assert_eq!(
        branch_of("done"),
        branch.name,
        "else call groups under its arm"
    );
}

// ---------------------------------------------------------------------
// PY-009 — global / nonlocal
// ---------------------------------------------------------------------

/// A `global` write mutates module state. Recording it as a fresh local
/// hid exactly the thing that makes the function stateful.
#[test]
fn a_global_write_targets_the_module_binding() {
    let src = r#"
COUNTER = 0

def bump():
    global COUNTER
    COUNTER = COUNTER + 1
"#;
    let result = parse(src);
    assert!(
        !result
            .entities
            .iter()
            .any(|e| e.id.contains("::local::COUNTER")),
        "global write still minted a synthetic local"
    );
    let write = result
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::WritesTo && r.target_id == "COUNTER")
        .expect("WritesTo COUNTER");
    assert_eq!(
        write.metadata.get("scope").map(String::as_str),
        Some("global")
    );
}

#[test]
fn a_nonlocal_write_is_marked_as_such() {
    let src = r#"
def outer():
    total = 0

    def inner():
        nonlocal total
        total = total + 1

    return inner
"#;
    let result = parse(src);
    let write = result
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::WritesTo && r.target_id == "total")
        .expect("WritesTo total");
    assert_eq!(
        write.metadata.get("scope").map(String::as_str),
        Some("nonlocal")
    );
    // The enclosing function's own `total = 0` is still a real local.
    assert!(result
        .entities
        .iter()
        .any(|e| e.id.ends_with("::local::total")));
}

/// The declaration can sit anywhere in the function, including inside an
/// `if` — so a shallow scan of the body's top level is not enough.
#[test]
fn a_global_declared_inside_a_block_still_counts() {
    let src = r#"
def bump(flag):
    if flag:
        global COUNTER
    COUNTER = 1
"#;
    let result = parse(src);
    assert!(!result
        .entities
        .iter()
        .any(|e| e.id.contains("::local::COUNTER")));
}

/// An inner function's `global` is its own business — it must not silence
/// the outer function's genuinely local write of the same name.
#[test]
fn an_inner_functions_global_does_not_leak_outward() {
    let src = r#"
def outer():
    def inner():
        global value
        value = 1
    value = 2
    return inner
"#;
    let result = parse(src);
    let outer_local = result
        .entities
        .iter()
        .find(|e| e.id.ends_with("::local::value") && e.id.contains("outer"));
    assert!(
        outer_local.is_some(),
        "outer's own local write was suppressed"
    );
}

/// Writes with no `global`/`nonlocal` declaration are untouched.
#[test]
fn an_ordinary_write_is_still_a_local() {
    let result = parse("def f():\n    x = 1\n    return x\n");
    assert!(result.entities.iter().any(|e| e.id.ends_with("::local::x")));
    assert!(result
        .relationships
        .iter()
        .all(|r| !r.metadata.contains_key("scope")));
}

// ---------------------------------------------------------------------
// PY-011 / PY-012 — expression-level control flow
// ---------------------------------------------------------------------

/// PY-011: calls in a ternary's arms are conditional, and used to be recorded
/// as if the function always made both.
#[test]
fn ternary_arms_with_calls_become_branches() {
    let src = r#"
def pick(flag):
    return compute_a() if flag else compute_b()
"#;
    let result = parse(src);
    // Expression-level flow is numbered by position, not by the enclosing
    // block's sibling counter — see `flow_path_segment`.
    let arms: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("branch_node"))
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(arms, vec!["t3_12_1", "t3_12_2"]);

    let branch_of = |target: &str| {
        result
            .relationships
            .iter()
            .find(|r| r.target_id.ends_with(target))
            .and_then(|r| r.metadata.get("branch").cloned())
    };
    assert_eq!(branch_of("compute_a").as_deref(), Some("t3_12_1"));
    assert_eq!(branch_of("compute_b").as_deref(), Some("t3_12_2"));
}

/// The overwhelmingly common ternary is value-only. Two empty Branch nodes
/// for each of those would bury the decision trees that matter.
#[test]
fn a_value_only_ternary_emits_no_branches() {
    let result = parse("def f(x):\n    return x if x else 0\n");
    assert!(
        !result
            .entities
            .iter()
            .any(|e| e.tags.contains("branch_node")),
        "value-only ternary polluted the graph with empty arms"
    );
}

/// The condition runs either way, so its calls stay in the enclosing scope.
#[test]
fn a_ternary_condition_stays_outside_the_arms() {
    let src = r#"
def pick(x):
    return compute_a() if is_ready(x) else compute_b()
"#;
    let result = parse(src);
    let cond = result
        .relationships
        .iter()
        .find(|r| r.target_id.ends_with("is_ready"))
        .expect("is_ready call");
    assert!(!cond.metadata.contains_key("branch"), "{:?}", cond.metadata);
}

/// PY-012: a comprehension is a loop. Its calls used to land flat in the
/// enclosing scope, so a function whose body was one comprehension read as
/// straight-line code.
#[test]
fn a_comprehension_becomes_a_loop_scope() {
    let src = r#"
def f(source):
    return [transform(x) for x in source() if keep(x)]
"#;
    let result = parse(src);
    let loops: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("loop_node"))
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(loops, vec!["l3_12"]);

    for callee in ["transform", "source", "keep"] {
        let rel = result
            .relationships
            .iter()
            .find(|r| r.target_id.ends_with(callee))
            .unwrap_or_else(|| panic!("no call to {}", callee));
        assert_eq!(
            rel.metadata.get("branch").map(String::as_str),
            Some("l3_12"),
            "{} did not group under the comprehension's loop",
            callee
        );
    }
}

#[test]
fn every_comprehension_form_emits_a_loop() {
    for src in [
        "def f(xs):\n    return [g(x) for x in xs]\n",
        "def f(xs):\n    return {g(x) for x in xs}\n",
        "def f(xs):\n    return {g(x): h(x) for x in xs}\n",
        "def f(xs):\n    return (g(x) for x in xs)\n",
    ] {
        let result = parse(src);
        assert_eq!(
            result
                .entities
                .iter()
                .filter(|e| e.tags.contains("loop_node"))
                .count(),
            1,
            "no loop for {}",
            src.trim()
        );
    }
}

/// Nested clauses land in the same loop scope; a comprehension inside a
/// comprehension nests under it.
#[test]
fn nested_comprehensions_nest_their_loops() {
    let src = r#"
def f(rows):
    return [[cell(c) for c in row] for row in rows]
"#;
    let result = parse(src);
    let mut loops: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("loop_node"))
        .map(|e| e.name.as_str())
        .collect();
    loops.sort();
    assert_eq!(loops, vec!["l3_12", "l3_12.l3_13"]);
}

/// `async for` inside a comprehension awaits per element, same as the
/// statement form (PY-021).
#[test]
fn an_async_comprehension_is_tagged() {
    let result = parse("async def f(stream):\n    return [x async for x in stream]\n");
    let loop_node = result
        .entities
        .iter()
        .find(|e| e.tags.contains("loop_node"))
        .expect("loop");
    assert!(
        loop_node.tags.contains("async_loop"),
        "{:?}",
        loop_node.tags
    );
}

// ------------------------------------------------------------------
//  Type-checking imports (AN-022)
// ------------------------------------------------------------------

/// The Python half of "the build erases this edge". `if TYPE_CHECKING:` is
/// the one guard whose body never runs, so its imports are the same
/// category as TypeScript's `import type`.
#[test]
fn a_type_checking_guard_marks_its_imports_erased() {
    let src = r#"
import os

if TYPE_CHECKING:
    from models import User

if typing.TYPE_CHECKING:
    from other import Thing
"#;
    let result = parse(src);
    let type_only = |path: &str| {
        result
            .imports
            .iter()
            .find(|i| i.path == path)
            .unwrap_or_else(|| panic!("no import of {}", path))
            .is_type_only
    };
    assert!(!type_only("os"));
    assert!(type_only("models"));
    assert!(
        type_only("other"),
        "a qualified TYPE_CHECKING is the same gate"
    );
}

/// `Guarded` is broader than this ticket. A platform or version gate is
/// just as conditional and very much executed, so marking every `if` as
/// erased would be the same lie the mark exists to end.
#[test]
fn an_ordinary_guard_is_conditional_without_being_erased() {
    let src = r#"
if sys.version_info >= (3, 11):
    import tomllib

try:
    import ujson
except ImportError:
    pass
"#;
    let result = parse(src);
    use crate::parser::language_parser::ImportCondition;
    let import = |path: &str| {
        result
            .imports
            .iter()
            .find(|i| i.path == path)
            .unwrap_or_else(|| panic!("no import of {}", path))
    };
    assert_eq!(import("tomllib").condition, Some(ImportCondition::Guarded));
    assert!(!import("tomllib").is_type_only);
    assert!(!import("ujson").is_type_only);
}

/// A `try` written inside the gate is still inside it. The wrapper is
/// replaced on the way down; being erased is not.
#[test]
fn a_try_inside_the_type_checking_gate_is_still_erased() {
    let src = r#"
if TYPE_CHECKING:
    try:
        from models import User
    except ImportError:
        from fallback import User
    from other import Thing
"#;
    let result = parse(src);
    assert!(
        result.imports.iter().all(|i| i.is_type_only),
        "{:?}",
        result.imports
    );
}

/// And the gate does not leak past its own block.
#[test]
fn imports_after_the_type_checking_gate_are_not_erased() {
    let src = r#"
if TYPE_CHECKING:
    from models import User

import os
"#;
    let result = parse(src);
    let os = result.imports.iter().find(|i| i.path == "os").unwrap();
    assert!(!os.is_type_only);
}

// ---------------------------------------------------------------------
// PY-029: the module docstring travels as file documentation
// ---------------------------------------------------------------------

#[test]
fn module_docstring_becomes_file_documentation() {
    let src =
        "\"\"\"What this module is for.\n\nA second paragraph.\n\"\"\"\n\ndef f():\n    pass\n";
    assert_eq!(
        parse(src).file_documentation.as_deref(),
        Some("What this module is for.\n\nA second paragraph.")
    );
}

#[test]
fn a_licence_comment_above_the_docstring_does_not_hide_it() {
    let src = "# SPDX-License-Identifier: MIT\n\"\"\"The module.\"\"\"\n";
    assert_eq!(
        parse(src).file_documentation.as_deref(),
        Some("The module.")
    );
}

#[test]
fn a_module_that_opens_with_code_has_no_file_documentation() {
    let src = "import os\n\n\"\"\"Not a docstring — it is the second statement.\"\"\"\n";
    assert_eq!(parse(src).file_documentation, None);
}

#[test]
fn a_function_docstring_is_not_the_modules() {
    let src = "def f():\n    \"\"\"Mine, not the file's.\"\"\"\n";
    let result = parse(src);
    assert_eq!(result.file_documentation, None);
    let f = result.entities.iter().find(|e| e.name == "f").expect("f");
    assert_eq!(f.documentation.as_deref(), Some("Mine, not the file's."));
}

// ---------------------------------------------------------------------
// PY-030: `UsesValue` edges from imported names read as values
// ---------------------------------------------------------------------

fn uses_value_targets(result: &ParseResult) -> Vec<&str> {
    let mut targets: Vec<&str> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::UsesValue)
        .map(|r| r.target_id.as_str())
        .collect();
    targets.sort_unstable();
    targets
}

#[test]
fn an_imported_constant_read_in_a_body_is_a_dependency() {
    let src = "from .config import LIMIT\n\ndef f(rows):\n    return rows[:LIMIT]\n";
    assert_eq!(uses_value_targets(&parse(src)), vec!["config::LIMIT"]);
}

#[test]
fn an_imported_function_passed_as_a_value_is_a_dependency() {
    let src =
        "from .config import normalise\n\ndef f(items):\n    return sorted(items, key=normalise)\n";
    assert_eq!(uses_value_targets(&parse(src)), vec!["config::normalise"]);
}

#[test]
fn a_call_to_an_imported_name_is_not_also_a_value_read() {
    // The call is already a `Calls` edge; a second kind for the same site
    // would double-count one dependency.
    let src = "from .config import normalise\n\ndef f(x):\n    return normalise(x)\n";
    let result = parse(src);
    assert!(uses_value_targets(&result).is_empty());
    assert!(result
        .relationships
        .iter()
        .any(|r| r.kind == RelationshipKind::Calls && r.target_id == "normalise"));
}

#[test]
fn an_absolute_import_names_nothing_this_analysis_walked() {
    let src = "from django.conf import settings\n\ndef f():\n    return settings\n";
    assert!(uses_value_targets(&parse(src)).is_empty());
}

#[test]
fn a_type_checking_import_is_erased_and_carries_no_value_edge() {
    let src = "from typing import TYPE_CHECKING\n\nif TYPE_CHECKING:\n    from .schema import ROWS\n\ndef f():\n    return ROWS\n";
    assert!(uses_value_targets(&parse(src)).is_empty());
}

#[test]
fn a_shadowed_import_name_is_declined_rather_than_guessed() {
    let src = "from .config import LIMIT\n\ndef g(LIMIT):\n    return LIMIT\n\ndef f(rows):\n    return rows[:LIMIT]\n";
    assert!(uses_value_targets(&parse(src)).is_empty());
}

#[test]
fn a_keyword_argument_label_is_not_a_read_of_the_import() {
    let src = "from .config import limit\n\ndef f(rows):\n    return render(rows, limit=5)\n";
    assert!(uses_value_targets(&parse(src)).is_empty());
}

#[test]
fn an_annotation_is_a_type_edge_not_a_value_edge() {
    // `UsesType` already records it (PY-025); a second dependency kind for
    // the same mention would double-count it.
    let src = "from .store import Store\n\ndef f(s: Store) -> Store:\n    return s\n";
    assert!(uses_value_targets(&parse(src)).is_empty());
}

#[test]
fn a_value_read_is_sourced_from_the_callable_not_the_local_it_binds() {
    let src = "from .config import LIMIT\n\ndef f():\n    cap = LIMIT\n    return cap\n";
    let result = parse(src);
    let rel = result
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::UsesValue)
        .expect("one value edge");
    assert!(
        !rel.source_id.contains("::local::"),
        "the local is the result of the read, not the reader: {}",
        rel.source_id
    );
    assert!(
        rel.source_id.ends_with(":f"),
        "sourced from f: {}",
        rel.source_id
    );
}

// ---------------------------------------------------------------------
// PY-031: receivers resolve through their declared types
// ---------------------------------------------------------------------

fn calls_to(src: &str, target: &str) -> bool {
    call_targets(&parse(src)).iter().any(|t| t == target)
}

#[test]
fn a_field_copied_from_an_annotated_parameter_types_its_receiver() {
    let src = "class Service:\n    def __init__(self, store: Store):\n        self.store = store\n\n    def place(self, order):\n        self.store.save(order)\n";
    assert!(
        calls_to(src, "Store.save"),
        "expected Store.save, got {:?}",
        call_targets(&parse(src))
    );
}

#[test]
fn a_class_level_annotation_types_its_receiver() {
    let src =
        "class Service:\n    cache: Cache\n\n    def warm(self):\n        self.cache.fill()\n";
    assert!(calls_to(src, "Cache.fill"));
}

#[test]
fn an_annotated_parameter_types_its_own_receiver() {
    let src =
        "class Service:\n    def place(self, backup: Optional[Store]):\n        backup.save(1)\n";
    assert!(calls_to(src, "Store.save"));
}

#[test]
fn an_unannotated_receiver_keeps_the_text_the_source_gave_it() {
    let src = "class Service:\n    def place(self, thing):\n        thing.whatever()\n";
    assert!(calls_to(src, "thing.whatever"));
}

#[test]
fn a_container_annotation_types_the_receiver_as_the_container() {
    let src = "class Service:\n    def __init__(self, rows: list[Row]):\n        self.rows = rows\n\n    def add(self, r):\n        self.rows.append(r)\n";
    assert!(calls_to(src, "list.append"));
}

#[test]
fn an_imported_name_called_bare_in_a_method_is_not_a_sibling_method() {
    let src = "from .config import normalise\n\nclass Renderer:\n    def render(self, label):\n        return normalise(label)\n";
    assert!(
        calls_to(src, "normalise"),
        "got {:?}",
        call_targets(&parse(src))
    );
    assert!(!calls_to(src, "Renderer.normalise"));
}

#[test]
fn a_module_level_function_called_bare_in_a_method_is_not_a_sibling_method() {
    // Python's LEGB lookup skips class scope: `helper(label)` inside a method
    // finds the module-level `helper`, never `Renderer.helper` — which needs
    // `self.`. Prefixing it produced a name that exists nowhere (PY-031).
    let src = "def helper(x):\n    return x\n\nclass Renderer:\n    def render(self, label):\n        return helper(label)\n";
    assert!(
        calls_to(src, "helper"),
        "got {:?}",
        call_targets(&parse(src))
    );
    assert!(!calls_to(src, "Renderer.helper"));
}

#[test]
fn a_sibling_method_is_still_reached_through_self() {
    let src = "class Renderer:\n    def render(self, label):\n        return self.helper(label)\n";
    assert!(calls_to(src, "Renderer.helper"));
}

// ---------------------------------------------------------------------
// PY-032: a receiver whose type the code shows without annotating it
// ---------------------------------------------------------------------

#[test]
fn a_field_built_by_a_constructor_types_its_receiver() {
    let src = "class Facade:\n    def __init__(self):\n        self._renderer = VideoRenderer()\n\n    def play(self):\n        self._renderer.render()\n";
    assert!(
        calls_to(src, "VideoRenderer.render"),
        "got {:?}",
        call_targets(&parse(src))
    );
}

#[test]
fn a_dotted_constructor_is_named_by_its_last_segment() {
    let src = "class Facade:\n    def __init__(self):\n        self._store = models.Store()\n\n    def save(self, x):\n        self._store.put(x)\n";
    assert!(calls_to(src, "Store.put"));
}

#[test]
fn a_lowercase_callee_is_not_a_constructor() {
    // `json.loads` returns something the code never names, so the field
    // stays untyped and the receiver keeps its own text.
    let src = "class Facade:\n    def __init__(self, text):\n        self._data = json.loads(text)\n\n    def names(self):\n        return self._data.keys()\n";
    assert!(
        calls_to(src, "_data.keys"),
        "got {:?}",
        call_targets(&parse(src))
    );
}

#[test]
fn an_annotated_return_types_the_next_link_of_a_chain() {
    let src = "class PizzaBuilder:\n    def set_size(self, size) -> \"PizzaBuilder\":\n        return self\n\n    def set_dough(self, d):\n        return self\n\ndef make(builder):\n    return builder.set_size(\"m\").set_dough(\"thin\")\n";
    assert!(
        calls_to(src, "PizzaBuilder.set_dough"),
        "got {:?}",
        call_targets(&parse(src))
    );
}

#[test]
fn an_unannotated_return_self_types_the_next_link_too() {
    // The fluent shape is a declaration even without the annotation: the
    // body says the call yields the class it is a method of.
    let src = "class Builder:\n    def step(self):\n        return self\n\n    def build(self):\n        return 1\n\ndef make(b):\n    return b.step().build()\n";
    assert!(
        calls_to(src, "Builder.build"),
        "got {:?}",
        call_targets(&parse(src))
    );
}

#[test]
fn a_method_name_two_classes_claim_differently_is_declined() {
    // `open` yields a `Door` in one class and a `Window` in the other, and a
    // chain gives us only the bare name — so neither answer is emitted.
    let src = "class A:\n    def open(self) -> Door:\n        return Door()\n\nclass B:\n    def open(self) -> Window:\n        return Window()\n\ndef use(x):\n    return x.open().shut()\n";
    let targets = call_targets(&parse(src));
    assert!(
        targets.iter().any(|t| t == "open.shut"),
        "got {:?}",
        targets
    );
    assert!(!targets
        .iter()
        .any(|t| t == "Door.shut" || t == "Window.shut"));
}

#[test]
fn a_chain_off_a_name_the_file_does_not_declare_keeps_its_text() {
    let src = "def use(client):\n    return client.get(\"/\").json()\n";
    assert!(
        calls_to(src, "get.json"),
        "got {:?}",
        call_targets(&parse(src))
    );
}
