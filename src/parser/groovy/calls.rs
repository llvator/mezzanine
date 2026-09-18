//! Call-site extraction for Groovy method/function bodies and
//! script-scope statements.
//!
//! Walks the AST emitting `Calls` / `Instantiates` / `WritesTo`
//! relationships and synthesising `Branch` entities for the arms of a
//! `try` statement (the try body itself, each `catch` clause, and
//! `finally`). Closure bodies are walked transparently: calls inside
//! attribute to the enclosing function/script — Groovy closures stay
//! anonymous in the graph rather than getting their own entities.

use super::super::language_parser::{node_text, node_to_span, ParseResult};
use super::flow::{
    emit_branch_entity, emit_case_arm_entity, emit_loop_entity, emit_try_arm_entity,
};
use super::helpers::{flow_keyword_invocation, has_field_annotation};
use super::stdlib::is_stdlib_method;
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, Visibility};
use std::path::Path;
use tree_sitter::Node;

/// Context threaded through the call-walk. `caller_id` is the full
/// entity id of whatever owns the body being walked — a method, a
/// free function, or the synthetic script container for top-level
/// statements. `parent_class` qualifies bare-receiver calls (in
/// Groovy, like Java, `foo()` inside class `Bar` means `Bar.foo()`).
pub(super) struct CallCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub caller_id: &'a str,
    pub caller_name: &'a str,
    pub parent_class: Option<&'a str>,
    pub call_order: &'a mut u32,
    pub arm_counter: &'a mut u32,
    pub loop_counter: &'a mut u32,
    pub result: &'a mut ParseResult,
}

/// Build the branch path for an arm at the given `current_branch`
/// level. Top-level arms read as `c1`, `c2`; nested ones append
/// `.c<idx>` so the path ancestry is left-to-right.
fn branch_path(current_branch: Option<&str>, idx: u32) -> String {
    match current_branch {
        Some(p) => format!("{}.c{}", p, idx),
        None => format!("c{}", idx),
    }
}

/// Loop-path counterpart of `branch_path`. Loop bodies use the `l`
/// prefix so a sibling decision tree, loop, and try chain at the
/// same scope read as `c1` / `l1` / `c2.…` rather than colliding on
/// the shared arm counter.
fn loop_path(current_branch: Option<&str>, idx: u32) -> String {
    match current_branch {
        Some(p) => format!("{}.l{}", p, idx),
        None => format!("l{}", idx),
    }
}

/// Recursive entry point. Dispatches on `node`'s own kind first, then
/// walks its children. Splitting the dispatch from the child-walk lets
/// us re-enter the function on a single sub-node (e.g. a
/// `lambda_expression`'s `body`, which can itself be a
/// `method_invocation`) without losing the kind-specific handling that
/// would otherwise only fire when the node is reached via its parent.
///
/// `current_branch` carries the enclosing arm path (if any) so calls
/// inside a `catch` body reattach via the relationship's `branch`
/// metadata.
pub(super) fn extract_calls(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if dispatch_node(node, ctx, current_branch) {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls(&child, ctx, current_branch);
    }
}

/// Per-kind handler. Returns `true` when the node was fully handled
/// (including any required recursion into its children) — the caller
/// then skips the default child-walk. Returning `false` lets the
/// generic recursion in `extract_calls` walk the children.
///
/// Split by concern: nested-definition skip → flow-control nodes →
/// call/write nodes. The flow-control arms route through
/// `dispatch_flow_node` so the outer dispatch stays under the
/// project's cyclomatic ceiling as the language picks up more
/// constructs.
fn dispatch_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    if is_nested_definition(node.kind()) {
        return true;
    }
    if dispatch_flow_node(node, ctx, current_branch) {
        return true;
    }
    match node.kind() {
        "method_invocation" => {
            handle_method_invocation(node, ctx, current_branch);
            walk_method_invocation_children(node, ctx, current_branch);
            true
        }
        "object_creation_expression" => {
            handle_object_creation(node, ctx, current_branch);
            false
        }
        "lambda_expression" => {
            // Recurse into the body only; parameters stay local to
            // the closure (per GR-003 — they must not leak as writes
            // to the enclosing scope).
            if let Some(body) = node.child_by_field_name("body") {
                extract_calls(&body, ctx, current_branch);
            }
            true
        }
        "local_variable_declaration" => {
            if has_field_annotation(node, ctx.source) {
                return true;
            }
            handle_local_write(node, ctx, current_branch);
            false
        }
        "assignment_expression" => {
            handle_local_write(node, ctx, current_branch);
            false
        }
        "field_access" => {
            handle_field_read(node, ctx, current_branch);
            false
        }
        _ => false,
    }
}

/// Nested-definition kinds whose bodies own their own scope. Skipping
/// them at the call-walk keeps inner calls / writes attributed to the
/// inner entity (already registered in `mod.rs`) instead of leaking
/// onto the outer caller.
fn is_nested_definition(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "method_declaration"
            | "function_definition"
            | "constructor_declaration"
    )
}

/// Flow-control dispatch: `if`, `try`, `switch`, the four loop kinds,
/// and `ternary_expression` (Elvis detection). Returns `true` if the node
/// was handled, mirroring the outer `dispatch_node` contract.
fn dispatch_flow_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    if let Some(keyword) = flow_keyword_invocation(node, ctx.source) {
        handle_degraded_flow(node, keyword, ctx, current_branch);
        return true;
    }
    match node.kind() {
        "if_statement" => handle_if_statement(node, ctx, current_branch),
        "try_statement" => handle_try_statement(node, ctx, current_branch),
        "switch_expression" | "switch_statement" => {
            handle_switch_expression(node, ctx, current_branch)
        }
        "enhanced_for_statement" => handle_enhanced_for(node, ctx, current_branch),
        "for_statement" => handle_classic_for(node, ctx, current_branch),
        "while_statement" => handle_while(node, ctx, current_branch),
        "do_statement" => handle_do_while(node, ctx, current_branch),
        "ternary_expression" => handle_ternary_expression(node, ctx, current_branch),
        _ => return false,
    }
    true
}

/// Recurse into the children of a `method_invocation` other than its
/// callee identifier — the receiver, the argument list, and any
/// trailing closure body. Done explicitly so we don't accidentally
/// re-emit a Calls edge for the invocation's own `name` identifier
/// (which is already handled by `handle_method_invocation`).
fn walk_method_invocation_children(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    if let Some(obj) = node.child_by_field_name("object") {
        extract_calls(&obj, ctx, current_branch);
    }
    if let Some(args) = node.child_by_field_name("arguments") {
        extract_calls(&args, ctx, current_branch);
    }
    if let Some(body) = node.child_by_field_name("body") {
        extract_calls(&body, ctx, current_branch);
    }
}

fn handle_method_invocation(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let method_name = node_text(&name_node, ctx.source).to_string();
    if method_name.is_empty() {
        return;
    }
    *ctx.call_order += 1;
    if method_name == ctx.caller_name || is_stdlib_method(&method_name) {
        return;
    }
    let receiver_text = node
        .child_by_field_name("object")
        .map(|obj| node_text(&obj, ctx.source).to_string());
    let callee = qualify_callee(receiver_text.as_deref(), &method_name, ctx.parent_class);
    let mut rel = Relationship::new(ctx.caller_id.to_string(), callee, RelationshipKind::Calls);
    rel.metadata
        .insert("order".to_string(), ctx.call_order.to_string());
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    ctx.result.add_relationship(rel);
}

fn handle_object_creation(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let type_name = node_text(&type_node, ctx.source).to_string();
    *ctx.call_order += 1;
    let mut rel = Relationship::new(
        ctx.caller_id.to_string(),
        type_name,
        RelationshipKind::Instantiates,
    );
    rel.metadata
        .insert("order".to_string(), ctx.call_order.to_string());
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    ctx.result.add_relationship(rel);
}

/// `this.method()` / `super.method()` / bare `method()` qualify with
/// the enclosing class so the resolver can find `Class.method`.
/// `receiver.method()` qualifies with the receiver text as-is.
fn qualify_callee(receiver: Option<&str>, method_name: &str, parent_class: Option<&str>) -> String {
    match receiver {
        Some("this") | Some("super") | None => match parent_class {
            Some(cls) => format!("{}.{}", cls, method_name),
            None => method_name.to_string(),
        },
        Some(other) => format!("{}.{}", other, method_name),
    }
}

/// Handle a control-flow statement the grammar degraded into a
/// `method_invocation` — see [`flow_keyword_invocation`] for when that
/// happens. The condition (the invocation's `arguments`) runs in the
/// outer flow; the guarded block (its `body`) becomes an arm or a loop.
///
/// No `Calls` edge is emitted: `if` is not a method. What the degraded
/// form cannot carry is the rest of the structure — an `else` chain, the
/// `case` labels of a `switch` — so `switch` yields no arm at all rather
/// than a fabricated one, and its body is walked in the outer flow.
fn handle_degraded_flow(
    node: &Node,
    keyword: &str,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    if let Some(args) = node.child_by_field_name("arguments") {
        extract_calls(&args, ctx, current_branch);
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    match keyword {
        "while" | "for" => {
            *ctx.loop_counter += 1;
            let path = loop_path(current_branch, *ctx.loop_counter);
            emit_loop_entity(
                ctx.caller_id,
                current_branch,
                &path,
                &body,
                ctx.path,
                ctx.result,
            );
            extract_calls(&body, ctx, Some(&path));
        }
        "if" => emit_arm(&body, ctx, current_branch),
        _ => extract_calls(&body, ctx, current_branch),
    }
}

/// Per-arm dispatch for `if (cond) {} else if (cond2) {} else {}` (GR-013).
///
/// Groovy parses `else if` as the alternative slot holding another
/// `if_statement`, exactly like Java, but the user-visible model is a flat
/// list of arms — so the chain is flattened here and siblings read as
/// `c1` / `c2` / `c3` rather than nesting.
///
/// Two `tree-sitter-groovy` quirks the arm bodies come wrapped in, both
/// harmless because the emitters only need *a* node to span:
/// a braced arm is a `closure` rather than a `block` (the grammar makes no
/// distinction in statement position), and a terminal `else { … }` arrives
/// as an `expression_statement` wrapping that closure.
///
/// The condition expression stays in `current_branch` — it's evaluated
/// before any arm runs and belongs to the outer flow.
fn handle_if_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    emit_if_arm(node, ctx, current_branch);
    let mut alt = node.child_by_field_name("alternative");
    while let Some(a) = alt {
        if a.kind() == "if_statement" {
            emit_if_arm(&a, ctx, current_branch);
            alt = a.child_by_field_name("alternative");
        } else {
            // Terminal `else { … }` — `a` is the body statement itself.
            emit_arm(&a, ctx, current_branch);
            alt = None;
        }
    }
}

/// Walk one `if (cond) <consequence>` level of a chain: the condition
/// runs in the outer flow, the consequence becomes a sibling arm.
fn emit_if_arm(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(cond) = node.child_by_field_name("condition") {
        extract_calls(&cond, ctx, current_branch);
    }
    if let Some(body) = node.child_by_field_name("consequence") {
        emit_arm(&body, ctx, current_branch);
    }
}

/// Emit one Branch entity for `body` and walk it with that arm as the
/// current branch.
fn emit_arm(body: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    *ctx.arm_counter += 1;
    let path = branch_path(current_branch, *ctx.arm_counter);
    emit_branch_entity(
        ctx.caller_id,
        current_branch,
        &path,
        body,
        ctx.path,
        ctx.result,
    );
    extract_calls(body, ctx, Some(&path));
}

/// Per-arm dispatch for `try { } catch { } catch { } finally { }`.
///
/// The try body itself becomes the first arm (tagged `try_body_arm`);
/// each `catch_clause` and the optional `finally_clause` follow.
/// Caught exception types travel as documentation + `caught:<type>`
/// attribute, with multi-catch (`catch (Foo | Bar e)`) joined by ` | `
/// to mirror PY-001's tuple rendering.
fn handle_try_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(body) = node.child_by_field_name("body") {
        *ctx.arm_counter += 1;
        let path = branch_path(current_branch, *ctx.arm_counter);
        emit_try_arm_entity(
            ctx.caller_id,
            current_branch,
            &path,
            &body,
            ctx.path,
            "try_body",
            None,
            ctx.result,
        );
        extract_calls(&body, ctx, Some(&path));
    }

    let mut cursor = node.walk();
    for clause in node.children(&mut cursor) {
        match clause.kind() {
            "catch_clause" => handle_catch_clause(&clause, ctx, current_branch),
            "finally_clause" => handle_finally_clause(&clause, ctx, current_branch),
            _ => {}
        }
    }
}

fn handle_catch_clause(clause: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let caught = caught_types_text(clause, ctx.source);
    let Some(body) = clause.child_by_field_name("body") else {
        return;
    };
    *ctx.arm_counter += 1;
    let path = branch_path(current_branch, *ctx.arm_counter);
    emit_try_arm_entity(
        ctx.caller_id,
        current_branch,
        &path,
        &body,
        ctx.path,
        "catch",
        caught.as_deref(),
        ctx.result,
    );
    extract_calls(&body, ctx, Some(&path));
}

fn handle_finally_clause(clause: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    // tree-sitter-groovy exposes the finally block as the trailing
    // `block` named child rather than via a `body` field.
    let mut named_cursor = clause.walk();
    let body = clause
        .named_children(&mut named_cursor)
        .find(|nc| nc.kind() == "block");
    let Some(body) = body else { return };
    *ctx.arm_counter += 1;
    let path = branch_path(current_branch, *ctx.arm_counter);
    emit_try_arm_entity(
        ctx.caller_id,
        current_branch,
        &path,
        &body,
        ctx.path,
        "finally",
        None,
        ctx.result,
    );
    extract_calls(&body, ctx, Some(&path));
}

/// Per-arm dispatch for `switch (subj) { case A: ...; case B: ...; default: ... }`.
///
/// Tree-sitter-groovy parses this as a `switch_expression` (Groovy
/// supports switch as an expression) whose body is a `switch_block`
/// containing one or more `switch_block_statement_group`s. Each group
/// is a sequence of `switch_label`s followed by statements; multi-
/// label fall-through (`case 1: case 2: shared(); break;`) splits
/// across two groups in the AST — the first carries only the label,
/// the second carries the body — so emitting one arm per `switch_label`
/// gives "fidelity over de-duplication" per the ticket.
///
/// Statements within a group attribute to the *last* label of that
/// group (the typical case is one label per group; the multi-label
/// shape above puts the body's group at the second label so the
/// attribution lands there naturally).
///
/// The subject expression stays in `current_branch` — it runs once
/// before any arm and is part of the outer flow, mirroring how
/// Python's `match SUBJECT:` keeps the subject outside.
fn handle_switch_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(cond) = node.child_by_field_name("condition") {
        extract_calls(&cond, ctx, current_branch);
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut group_cursor = body.walk();
    for group in body.children(&mut group_cursor) {
        if group.kind() != "switch_block_statement_group" {
            continue;
        }
        handle_switch_group(&group, ctx, current_branch);
    }
}

/// Emit one Branch arm per `switch_label` in the group, then walk the
/// group's statements with the last label's branch path as
/// `current_branch`. Empty groups (multi-label fall-through, e.g. a
/// bare `case 1:` whose body lives in the next group) still produce
/// arm entities so the decision tree shows the label was intentional.
fn handle_switch_group(group: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut last_arm_path: Option<String> = None;
    let mut cursor = group.walk();
    for child in group.children(&mut cursor) {
        match child.kind() {
            "switch_label" => {
                *ctx.arm_counter += 1;
                let arm_path = branch_path(current_branch, *ctx.arm_counter);
                let pattern = switch_label_pattern(&child, ctx.source);
                emit_case_arm_entity(
                    ctx.caller_id,
                    current_branch,
                    &arm_path,
                    &child,
                    ctx.path,
                    pattern.as_deref(),
                    ctx.result,
                );
                last_arm_path = Some(arm_path);
            }
            _ => {
                // Statements (or `break_statement`) after the labels.
                // Attribute to the most recent label's branch.
                let arm = last_arm_path.as_deref().or(current_branch);
                extract_calls(&child, ctx, arm);
            }
        }
    }
}

/// Read the pattern text from a `switch_label`. Returns `None` for the
/// `default:` label (no children) so the emitter knows not to record
/// a `pattern:` attribute. For `case <expr>:` the first non-keyword
/// named child is the pattern node — we stringify it verbatim so the
/// detail panel reads the same as the source.
fn switch_label_pattern(label: &Node, source: &str) -> Option<String> {
    let mut cursor = label.walk();
    for child in label.children(&mut cursor) {
        if child.is_named() {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

/// `for (item in iter) { body }` — the enhanced-for shape. The
/// iterable expression's calls (`for (row in queryForList(sql))`)
/// group under the loop, mirroring Python's `for ... in iter:`
/// handling at [python/calls.rs:242](../python/calls.rs#L242).
fn handle_enhanced_for(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    *ctx.loop_counter += 1;
    let path = loop_path(current_branch, *ctx.loop_counter);
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        emit_loop_entity(
            ctx.caller_id,
            current_branch,
            &path,
            &body,
            ctx.path,
            ctx.result,
        );
    }
    if let Some(value) = node.child_by_field_name("value") {
        extract_calls(&value, ctx, Some(&path));
    }
    if let Some(body) = body {
        extract_calls(&body, ctx, Some(&path));
    }
}

/// Classic `for (init; cond; update) { body }`. All three header
/// expressions run as part of the loop's iteration cycle, so they
/// recurse with `current_branch` set to the loop path.
fn handle_classic_for(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    *ctx.loop_counter += 1;
    let path = loop_path(current_branch, *ctx.loop_counter);
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        emit_loop_entity(
            ctx.caller_id,
            current_branch,
            &path,
            &body,
            ctx.path,
            ctx.result,
        );
    }
    for field in ["init", "condition", "update"] {
        if let Some(part) = node.child_by_field_name(field) {
            extract_calls(&part, ctx, Some(&path));
        }
    }
    if let Some(body) = body {
        extract_calls(&body, ctx, Some(&path));
    }
}

/// `while (cond) { body }`. The condition runs on every iteration,
/// so its calls group under the loop too.
fn handle_while(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    *ctx.loop_counter += 1;
    let path = loop_path(current_branch, *ctx.loop_counter);
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        emit_loop_entity(
            ctx.caller_id,
            current_branch,
            &path,
            &body,
            ctx.path,
            ctx.result,
        );
    }
    if let Some(cond) = node.child_by_field_name("condition") {
        extract_calls(&cond, ctx, Some(&path));
    }
    if let Some(body) = body {
        extract_calls(&body, ctx, Some(&path));
    }
}

/// `do { body } while (cond)`. The condition runs after each
/// iteration; the call-graph attribution doesn't care about that
/// ordering — every call inside still belongs to the loop — so this
/// is structurally identical to `handle_while`.
fn handle_do_while(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    handle_while(node, ctx, current_branch);
}

/// Detect Groovy's Elvis (`a ?: b()`) inside a `ternary_expression`.
///
/// Tree-sitter-groovy parses Elvis as a `ternary_expression` whose
/// `consequence` child is synthesised as a missing token (the absent
/// `c` in `a ? c : b`). When that's the case, both sides are the same
/// expression in source — `a` is both the test and the value-when-
/// truthy. Calls in either side are tagged `null_safe` so the UI can
/// distinguish defensive lookups from regular control flow. Plain
/// ternaries (consequence not missing) recurse normally without the
/// tag.
///
/// Null-safe `obj?.foo()` is *not* matched here: the published
/// tree-sitter-groovy grammar exposes `?.` as an `ERROR` sibling
/// rather than as a distinct node kind, so the receiver and call end
/// up split across `expression_statement` children with no detectable
/// link. Calls inside still reach the graph via the default ERROR
/// recursion; they just don't carry the `null_safe` tag.
fn handle_ternary_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let is_elvis = node
        .child_by_field_name("consequence")
        .is_some_and(|c| c.is_missing());
    let before = ctx.result.relationships.len();
    if let Some(cond) = node.child_by_field_name("condition") {
        extract_calls(&cond, ctx, current_branch);
    }
    if let Some(consequence) = node.child_by_field_name("consequence") {
        if !consequence.is_missing() {
            extract_calls(&consequence, ctx, current_branch);
        }
    }
    if let Some(alt) = node.child_by_field_name("alternative") {
        extract_calls(&alt, ctx, current_branch);
    }
    if is_elvis {
        for rel in &mut ctx.result.relationships[before..] {
            if rel.kind == RelationshipKind::Calls {
                rel.metadata
                    .insert("null_safe".to_string(), "true".to_string());
            }
        }
    }
}

/// Linear scan for a named child by kind. Used where
/// `child_by_field_name` returns None and we still need the named
/// child — e.g. `finally_clause`'s body block, which the grammar
/// exposes as a sibling rather than a field. The explicit loop
/// (rather than `Iterator::find`) keeps the cursor in scope until
/// after the early return.
#[allow(clippy::manual_find)]
fn find_child_kind<'a>(node: &Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

/// Read the caught type(s) from a `catch_clause`. Tree-sitter-groovy
/// nests them as `catch_formal_parameter` → `catch_type` → one or more
/// `type_identifier`. Multi-catch (`Foo | Bar`) shows up as multiple
/// type_identifier siblings under one catch_type.
fn caught_types_text(clause: &Node, source: &str) -> Option<String> {
    let formal = match clause.child_by_field_name("parameter") {
        Some(n) => n,
        None => find_child_kind(clause, "catch_formal_parameter")?,
    };
    let catch_type = match formal.child_by_field_name("type") {
        Some(n) => n,
        None => find_child_kind(&formal, "catch_type")?,
    };
    let mut types: Vec<String> = Vec::new();
    let mut cursor = catch_type.walk();
    for tc in catch_type.children(&mut cursor) {
        if tc.kind() == "type_identifier" || tc.kind() == "scoped_type_identifier" {
            types.push(node_text(&tc, source).to_string());
        }
    }
    if types.is_empty() {
        None
    } else {
        Some(types.join(" | "))
    }
}

/// Emit a synthetic local-variable entity + `WritesTo` edge for an
/// `<ident> = …` (or `def x = …`, or `Type x = …`) inside a callable
/// body. Mirrors Python's `handle_local_write` shape: locals are
/// promoted to graph nodes so the reader can see what a callable
/// computes, not just what it calls. The first write registers the
/// entity; subsequent writes only emit additional edges.
fn handle_local_write(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some((name, span)) = local_write_target(node, ctx.source) else {
        return;
    };
    if name.is_empty() || name == "this" {
        return;
    }
    let local_id = format!("{}::local::{}", ctx.caller_id, name);
    let already_exists = ctx.result.entities.iter().any(|e| e.id == local_id);
    if !already_exists {
        let mut entity = CodeEntity::new(&name, EntityKind::Variable, ctx.path, span);
        entity.id = local_id.clone();
        entity.qualified_name = format!("{}::{}", ctx.caller_id, name);
        entity.parent_id = Some(ctx.caller_id.to_string());
        entity.tags.insert("local_var".to_string());
        entity.visibility = Visibility::Private;
        ctx.result.add_entity(entity);
    }
    let mut rel = Relationship::new(
        ctx.caller_id.to_string(),
        local_id,
        RelationshipKind::WritesTo,
    );
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    ctx.result.add_relationship(rel);
}

fn local_write_target(node: &Node, source: &str) -> Option<(String, crate::models::Span)> {
    match node.kind() {
        "local_variable_declaration" => {
            let mut cursor = node.walk();
            let declarator = node
                .children(&mut cursor)
                .find(|c| c.kind() == "variable_declarator")?;
            let name_node = declarator.child_by_field_name("name")?;
            Some((
                node_text(&name_node, source).to_string(),
                node_to_span(&declarator),
            ))
        }
        "assignment_expression" => {
            let left = node.child_by_field_name("left")?;
            if left.kind() != "identifier" {
                return None;
            }
            Some((node_text(&left, source).to_string(), node_to_span(node)))
        }
        _ => None,
    }
}

/// Emit a `Reads` edge for a bare `field_access` whose receiver is
/// `this` or unspecified — i.e. a read of a class field or a script's
/// `@Field`-declared module-state. Receiver-qualified accesses
/// (`other.x`) are left untouched: the resolver can't tell whether
/// they refer to a state field without type inference, and dropping
/// them in as `Reads` would produce noise.
fn handle_field_read(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(field) = node.child_by_field_name("field") else {
        return;
    };
    let field_name = node_text(&field, ctx.source).to_string();
    if field_name.is_empty() {
        return;
    }
    let receiver = node
        .child_by_field_name("object")
        .map(|o| node_text(&o, ctx.source).to_string());
    let target = match receiver.as_deref() {
        Some("this") | None => match ctx.parent_class {
            Some(cls) => format!("{}.{}", cls, field_name),
            None => field_name,
        },
        // Scripts use the synthetic file container as the implicit
        // owner for `@Field` reads. Any unprefixed identifier that
        // matches a declared field name will resolve through the
        // analyzer's name-resolution pass.
        Some(other) => format!("{}.{}", other, field_name),
    };
    let mut rel = Relationship::new(
        ctx.caller_id.to_string(),
        target,
        RelationshipKind::ReadsFrom,
    );
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    ctx.result.add_relationship(rel);
}
