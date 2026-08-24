//! Call-site extraction for Java method/constructor bodies.
//!
//! Walks the AST emitting `Calls` / `Instantiates` relationships and
//! synthesising `Branch` / `Loop` entities for the arms of `if` /
//! `switch` / `try` / `for` / `while` / `do-while` constructs so the
//! UI's decision-tree rendering treats Java like Groovy and Python.
//! Every relationship emitted inside a flow arm carries a `branch`
//! metadata key pointing at the arm's path (e.g. `c1`, `c2.l1`) so the
//! analyzer reattributes it to the synthetic Branch/Loop node.

use super::flow::{
    emit_branch_entity, emit_case_arm_entity, emit_loop_entity, emit_try_arm_entity,
};
use super::stdlib::is_stdlib_method;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Who a call is attributed to. Fixed for the whole body and read-only,
/// so a helper that needs to know whose body it is walking takes this
/// and cannot reach the walk's counters.
pub(in crate::parser::java) struct Caller<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub id: &'a str,
    pub name: &'a str,
    pub parent_class: Option<&'a str>,
}

/// What the walk accumulates as it descends: the call ordinal, the two
/// arm counters, and the result they are written into. The counters are
/// owned rather than borrowed — they start at zero with the body and
/// nothing outside reads them back, so they leave only as the paths
/// built from them.
struct Walk<'a> {
    call_order: u32,
    arm_counter: u32,
    loop_counter: u32,
    result: &'a mut ParseResult,
}

/// Context threaded through `extract_calls` and its helpers, so the
/// recursive walker passes one reference instead of plumbing nine
/// arguments per call. The two halves are separate structs because they
/// change for separate reasons: a new counter is not a new thing to
/// know about the caller, and only [`Caller`] crosses the module edge.
pub(in crate::parser::java) struct CallCtx<'a> {
    caller: Caller<'a>,
    walk: Walk<'a>,
}

impl<'a> CallCtx<'a> {
    /// Start a walk over one callable's body. The counters begin at zero
    /// per body, which is what makes an arm path (`c1`, `l1`) read
    /// relative to the method rather than to the file.
    pub(in crate::parser::java) fn new(caller: Caller<'a>, result: &'a mut ParseResult) -> Self {
        Self {
            caller,
            walk: Walk {
                call_order: 0,
                arm_counter: 0,
                loop_counter: 0,
                result,
            },
        }
    }

    /// Consume the next call ordinal. Taken before any filtering, so a
    /// dropped call still burns its number and the edges that survive
    /// keep their source order.
    fn next_order(&mut self) -> u32 {
        self.walk.call_order += 1;
        self.walk.call_order
    }

    /// Consume the next arm number and return its path under
    /// `current_branch`.
    fn next_arm_path(&mut self, current_branch: Option<&str>) -> String {
        self.walk.arm_counter += 1;
        branch_path(current_branch, self.walk.arm_counter)
    }

    /// Loop counterpart of `next_arm_path`. Loops count separately so a
    /// sibling decision tree and loop at the same scope read as `c1` /
    /// `l1` rather than colliding on one counter.
    fn next_loop_path(&mut self, current_branch: Option<&str>) -> String {
        self.walk.loop_counter += 1;
        loop_path(current_branch, self.walk.loop_counter)
    }
}

/// Captures that the immediately-enclosing call/instantiation has its
/// result stored into a named variable. Attached as metadata on the edge
/// so downstream views can render the binding without adding a separate
/// node per local.
struct Binding {
    name: String,
    declared_type: Option<String>,
    is_reassignment: bool,
}

/// Build the branch path for an arm at the given `current_branch` level.
/// Top-level arms read as `c1`, `c2`; nested ones append `.c<idx>` so
/// the path ancestry is left-to-right.
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

/// Walk the body of a method/constructor, emitting `Calls` and
/// `Instantiates` relationships and synthesising Branch / Loop entities
/// for arms of control-flow constructs. `ctx.caller.id` is the full
/// entity ID (used as the relationship source — never ambiguous);
/// `caller_name` is the bare method name, used only to suppress obvious
/// self-recursion; `parent_class` qualifies receiver-less calls (in
/// Java `foo()` inside class `Bar` means `Bar.foo()`).
pub(in crate::parser::java) fn extract_calls(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
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
            handle_method_invocation(node, ctx, None, current_branch);
            walk_method_invocation_children(node, ctx, current_branch);
            true
        }
        "object_creation_expression" => {
            handle_object_creation(node, ctx, None, current_branch);
            // Walk the argument list (and any anonymous-class body) so
            // nested calls still emit edges.
            false
        }
        "local_variable_declaration" => {
            handle_local_variable_declaration(node, ctx, current_branch);
            true
        }
        "assignment_expression" => {
            handle_assignment_expression(node, ctx, current_branch);
            true
        }
        _ => false,
    }
}

/// Nested-definition kinds whose bodies own their own scope. Skipping
/// them at the call-walk keeps inner calls attributed to the inner
/// entity (already registered in `mod.rs`) instead of leaking onto the
/// outer caller.
fn is_nested_definition(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "annotation_type_declaration"
            | "record_declaration"
            | "method_declaration"
            | "constructor_declaration"
    )
}

/// Flow-control dispatch: `if`, `try` (both plain and try-with-resources),
/// `switch`, the four loop kinds. Returns `true` if the node was handled,
/// mirroring the outer `dispatch_node` contract. `ternary_expression` is
/// not special-cased — its sub-expressions still recurse through the
/// generic child-walk so calls inside both branches still emit edges
/// (no Elvis in Java, so nothing extra to tag).
fn dispatch_flow_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    match node.kind() {
        "if_statement" => handle_if_statement(node, ctx, current_branch),
        "try_statement" | "try_with_resources_statement" => {
            handle_try_statement(node, ctx, current_branch)
        }
        "switch_expression" => handle_switch_expression(node, ctx, current_branch),
        "enhanced_for_statement" => handle_enhanced_for(node, ctx, current_branch),
        "for_statement" => handle_classic_for(node, ctx, current_branch),
        "while_statement" => handle_while(node, ctx, current_branch),
        "do_statement" => handle_do_while(node, ctx, current_branch),
        _ => return false,
    }
    true
}

/// Recurse into the children of a `method_invocation` other than its
/// callee identifier — the receiver and the argument list. Done
/// explicitly so we don't accidentally re-emit a Calls edge for the
/// invocation's own `name` identifier (which is already handled by
/// `handle_method_invocation`).
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
}

/// `Type name = rhs;` (one or more declarators). When `rhs` is a direct
/// call or `new`, tag that edge with `binds_to` + `binds_type`.
fn handle_local_variable_declaration(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    let declared_type = node
        .child_by_field_name("type")
        .map(|t| node_text(&t, ctx.caller.source).to_string());

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(value_node) = child.child_by_field_name("value") else {
            continue;
        };

        let binding = Binding {
            name: node_text(&name_node, ctx.caller.source).to_string(),
            declared_type: declared_type.clone(),
            is_reassignment: false,
        };
        emit_with_binding(&value_node, ctx, &binding, current_branch);
    }
}

/// `lhs = rhs` (and only `=` — compound forms like `+=` are not type-bearing
/// rebindings). When `rhs` is a direct call or `new`, tag that edge with
/// `rebinds_to`. The declared type is unknown at the reassignment site, so
/// `binds_type` is omitted.
fn handle_assignment_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let operator_is_eq = node.child(1).map(|n| n.kind() == "=").unwrap_or(false);
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        // Malformed — fall back to generic walk so nested calls aren't dropped.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            extract_calls(&child, ctx, current_branch);
        }
        return;
    };

    if !operator_is_eq {
        extract_calls(&left, ctx, current_branch);
        extract_calls(&right, ctx, current_branch);
        return;
    }

    let binding = Binding {
        name: node_text(&left, ctx.caller.source).to_string(),
        declared_type: None,
        is_reassignment: true,
    };
    emit_with_binding(&right, ctx, &binding, current_branch);
}

/// Apply `binding` to the outermost call/`new` in `value_node`, then
/// continue walking the same node so nested calls (e.g. arguments,
/// chained receivers) are still emitted as plain edges.
fn emit_with_binding(
    value_node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: &Binding,
    current_branch: Option<&str>,
) {
    match value_node.kind() {
        "method_invocation" => {
            handle_method_invocation(value_node, ctx, Some(binding), current_branch);
            walk_method_invocation_children(value_node, ctx, current_branch);
        }
        "object_creation_expression" => {
            handle_object_creation(value_node, ctx, Some(binding), current_branch);
            let mut cursor = value_node.walk();
            for child in value_node.children(&mut cursor) {
                extract_calls(&child, ctx, current_branch);
            }
        }
        _ => extract_calls(value_node, ctx, current_branch),
    }
}

fn handle_method_invocation(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let method_name = node_text(&name_node, ctx.caller.source).to_string();
    let order = ctx.next_order();
    if method_name == ctx.caller.name {
        return;
    }
    // Stdlib filter normally drops common names (`format`, `get`,
    // `isPresent`, …) to reduce noise. Skip the filter when the result
    // is bound to a local — the binding shows the caller cares about
    // the returned value, so the edge is structurally relevant.
    if binding.is_none() && is_stdlib_method(&method_name) {
        return;
    }
    let receiver = node
        .child_by_field_name("object")
        .map(|obj| node_text(&obj, ctx.caller.source));
    let callee = qualify_callee(receiver, &method_name, ctx.caller.parent_class);
    let mut rel = Relationship::new(ctx.caller.id.to_string(), callee, RelationshipKind::Calls);
    rel.metadata
        .insert("order".to_string(), order.to_string());
    apply_binding_metadata(&mut rel, binding);
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    ctx.walk.result.add_relationship(rel);
}

fn handle_object_creation(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let type_name = node_text(&type_node, ctx.caller.source).to_string();
    let order = ctx.next_order();
    let mut rel = Relationship::new(
        ctx.caller.id.to_string(),
        type_name,
        RelationshipKind::Instantiates,
    );
    rel.metadata
        .insert("order".to_string(), order.to_string());
    apply_binding_metadata(&mut rel, binding);
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    ctx.walk.result.add_relationship(rel);
}

fn apply_binding_metadata(rel: &mut Relationship, binding: Option<&Binding>) {
    let Some(b) = binding else { return };
    let key = if b.is_reassignment {
        "rebinds_to"
    } else {
        "binds_to"
    };
    rel.metadata.insert(key.to_string(), b.name.clone());
    if let Some(t) = &b.declared_type {
        rel.metadata.insert("binds_type".to_string(), t.clone());
    }
}

/// Qualify a called method name with its most specific known owner.
///
/// - `this.method()` / `super.method()` / bare `method()` → qualify with
///   the enclosing class so the resolver can find `Class.method`.
/// - `receiver.method()` → qualify with the receiver text as-is; the
///   resolver tries `Receiver.method` via `typed_method_to_id`.
/// - No receiver and no enclosing class → return the bare method name.
fn qualify_callee(receiver: Option<&str>, method_name: &str, parent_class: Option<&str>) -> String {
    match receiver {
        Some("this") | Some("super") | None => match parent_class {
            Some(cls) => format!("{}.{}", cls, method_name),
            None => method_name.to_string(),
        },
        Some(other) => format!("{}.{}", other, method_name),
    }
}

/// Per-arm dispatch for `if (cond) {} else if (cond2) {} else {}`.
///
/// Java parses `else if` as the alternative slot containing another
/// `if_statement`, but the user-visible model is a flat list of arms.
/// We flatten the chain here so siblings read as `c1` / `c2` / `c3`
/// instead of nesting (`c1` / `c1.c2` / `c1.c2.c3`), matching how the
/// Python parser flattens `elif_clause` siblings.
///
/// The condition expression stays in `current_branch` — it's evaluated
/// before any arm runs and belongs to the outer flow.
fn handle_if_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(cond) = node.child_by_field_name("condition") {
        extract_calls(&cond, ctx, current_branch);
    }
    if let Some(body) = node.child_by_field_name("consequence") {
        let path = ctx.next_arm_path(current_branch);
        emit_branch_entity(
            ctx.caller.id,
            current_branch,
            &path,
            &body,
            ctx.caller.path,
            ctx.walk.result,
        );
        extract_calls(&body, ctx, Some(&path));
    }

    let mut alt = node.child_by_field_name("alternative");
    while let Some(a) = alt {
        if a.kind() == "if_statement" {
            // `else if (...) { ... }` — emit as a sibling arm, then
            // walk into its own alternative.
            if let Some(cond) = a.child_by_field_name("condition") {
                extract_calls(&cond, ctx, current_branch);
            }
            if let Some(body) = a.child_by_field_name("consequence") {
                let path = ctx.next_arm_path(current_branch);
                emit_branch_entity(
                    ctx.caller.id,
                    current_branch,
                    &path,
                    &body,
                    ctx.caller.path,
                    ctx.walk.result,
                );
                extract_calls(&body, ctx, Some(&path));
            }
            alt = a.child_by_field_name("alternative");
        } else {
            // Terminal `else { ... }` — `a` is the body statement
            // itself (usually a `block`).
            let path = ctx.next_arm_path(current_branch);
            emit_branch_entity(
                ctx.caller.id,
                current_branch,
                &path,
                &a,
                ctx.caller.path,
                ctx.walk.result,
            );
            extract_calls(&a, ctx, Some(&path));
            alt = None;
        }
    }
}

/// Per-arm dispatch for `try { } catch { } catch { } finally { }`,
/// including the try-with-resources form.
///
/// The try body itself becomes the first arm (tagged `try_body_arm`);
/// each `catch_clause` and the optional `finally_clause` follow. Caught
/// exception types travel as documentation + `caught:<type>` attribute,
/// with multi-catch (`catch (Foo | Bar e)`) joined by ` | `.
///
/// Resources of a try-with-resources statement are walked under the
/// `try_body_arm` path — they're part of the try's setup and disposal
/// semantics, so grouping them with the body matches how a loop's
/// iterator expression groups with the loop.
fn handle_try_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let body = node.child_by_field_name("body");
    let try_body_path = body.map(|body| {
        let path = ctx.next_arm_path(current_branch);
        emit_try_arm_entity(
            ctx.caller.id,
            current_branch,
            &path,
            &body,
            ctx.caller.path,
            "try_body",
            None,
            ctx.walk.result,
        );
        path
    });
    if let Some(resources) = node.child_by_field_name("resources") {
        let scope = try_body_path.as_deref().or(current_branch);
        extract_calls(&resources, ctx, scope);
    }
    if let (Some(body), Some(path)) = (body, try_body_path.as_deref()) {
        extract_calls(&body, ctx, Some(path));
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
    let caught = caught_types_text(clause, ctx.caller.source);
    let Some(body) = clause.child_by_field_name("body") else {
        return;
    };
    let path = ctx.next_arm_path(current_branch);
    emit_try_arm_entity(
        ctx.caller.id,
        current_branch,
        &path,
        &body,
        ctx.caller.path,
        "catch",
        caught.as_deref(),
        ctx.walk.result,
    );
    extract_calls(&body, ctx, Some(&path));
}

fn handle_finally_clause(clause: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    // tree-sitter-java exposes the finally block as the trailing
    // `block` named child rather than via a `body` field.
    let body = find_child_kind(clause, "block");
    let Some(body) = body else { return };
    let path = ctx.next_arm_path(current_branch);
    emit_try_arm_entity(
        ctx.caller.id,
        current_branch,
        &path,
        &body,
        ctx.caller.path,
        "finally",
        None,
        ctx.walk.result,
    );
    extract_calls(&body, ctx, Some(&path));
}

/// Per-arm dispatch for `switch (subj) { case A: ...; case B: ...; default: ... }`.
///
/// Tree-sitter-java parses this as a `switch_expression` whose body is
/// a `switch_block` containing one or more `switch_block_statement_group`s.
/// Each group is a sequence of `switch_label`s followed by statements;
/// multi-label fall-through splits across groups in the AST, so emitting
/// one arm per `switch_label` gives fidelity over de-duplication.
///
/// The subject expression stays in `current_branch` — it runs once
/// before any arm and is part of the outer flow.
fn handle_switch_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(cond) = node.child_by_field_name("condition") {
        extract_calls(&cond, ctx, current_branch);
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for group in body.children(&mut cursor) {
        if group.kind() != "switch_block_statement_group" {
            // Java 14+ also parses arrow-form rules as `switch_rule`
            // children. Walk them so their calls still emit edges
            // (no per-rule branch entity yet — out of scope here).
            extract_calls(&group, ctx, current_branch);
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
                let arm_path = ctx.next_arm_path(current_branch);
                let pattern = switch_label_pattern(&child, ctx.caller.source);
                emit_case_arm_entity(
                    ctx.caller.id,
                    current_branch,
                    &arm_path,
                    &child,
                    ctx.caller.path,
                    pattern.as_deref(),
                    ctx.walk.result,
                );
                last_arm_path = Some(arm_path);
            }
            _ => {
                // Statements after the labels. Attribute to the most
                // recent label's branch.
                let arm = last_arm_path.as_deref().or(current_branch);
                extract_calls(&child, ctx, arm);
            }
        }
    }
}

/// Read the pattern text from a `switch_label`. Returns `None` for the
/// `default:` label (no named children) so the emitter knows not to
/// record a `pattern:` attribute. For `case <expr>:` the first named
/// child is the pattern node — we stringify it verbatim so the detail
/// panel reads the same as the source.
fn switch_label_pattern(label: &Node, source: &str) -> Option<String> {
    let mut cursor = label.walk();
    for child in label.children(&mut cursor) {
        if child.is_named() {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

/// `for (Type item : iter) { body }` — the enhanced-for shape. The
/// iterable expression's calls (`for (Row r : queryForList(sql))`)
/// group under the loop, mirroring Python's `for ... in iter:`
/// handling.
fn handle_enhanced_for(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let path = ctx.next_loop_path(current_branch);
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        emit_loop_entity(
            ctx.caller.id,
            current_branch,
            &path,
            &body,
            ctx.caller.path,
            ctx.walk.result,
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
    let path = ctx.next_loop_path(current_branch);
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        emit_loop_entity(
            ctx.caller.id,
            current_branch,
            &path,
            &body,
            ctx.caller.path,
            ctx.walk.result,
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
    let path = ctx.next_loop_path(current_branch);
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        emit_loop_entity(
            ctx.caller.id,
            current_branch,
            &path,
            &body,
            ctx.caller.path,
            ctx.walk.result,
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

/// Linear scan for a named child by kind. Used where
/// `child_by_field_name` returns None and we still need the named
/// child — e.g. `finally_clause`'s body block, which the grammar
/// exposes as a sibling rather than a field.
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

/// Read the caught type(s) from a `catch_clause`. Tree-sitter-java
/// nests them as `catch_formal_parameter` → `catch_type` → one or more
/// type nodes. Multi-catch (`Foo | Bar`) shows up as multiple type
/// siblings under one catch_type.
fn caught_types_text(clause: &Node, source: &str) -> Option<String> {
    let formal = find_child_kind(clause, "catch_formal_parameter")?;
    let catch_type = find_child_kind(&formal, "catch_type")?;
    let mut types: Vec<String> = Vec::new();
    let mut cursor = catch_type.walk();
    for tc in catch_type.children(&mut cursor) {
        if tc.is_named() && tc.kind() != "modifiers" {
            types.push(node_text(&tc, source).to_string());
        }
    }
    if types.is_empty() {
        None
    } else {
        Some(types.join(" | "))
    }
}
