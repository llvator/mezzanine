//! Call-site extraction for Kotlin function bodies.
//!
//! In Kotlin's tree-sitter grammar, a `call_expression` is either
//! `simple_identifier + call_suffix` (plain `foo(...)`) or
//! `navigation_expression + call_suffix` (member `obj.foo(...)`).
//!
//! The same walk synthesises `Branch` / `Loop` entities for the arms of
//! `if` / `when` / `try` / `for` / `while` / `do…while` (KT-002), so the UI's
//! decision-tree rendering treats Kotlin like Java, Groovy and Python. Every
//! relationship emitted inside a flow arm carries a `branch` metadata key
//! pointing at the arm's path (e.g. `c1`, `c2.l1`) so the analyzer
//! reattributes it to the synthetic Branch/Loop node.
//!
//! `tree-sitter-kotlin 0.3` names no fields at all, so where Java reads
//! `child_by_field_name("condition")` this file locates each part
//! positionally. The [`parts`] helpers own that, and are the only place the
//! child ordering of a construct is assumed.

use super::flow::{
    emit_branch_entity, emit_case_arm_entity, emit_loop_entity, emit_try_arm_entity,
};
use super::stdlib::is_stdlib_method;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Who a call is attributed to. Fixed for the whole body and read-only, so a
/// helper that needs to know whose body it is walking takes this and cannot
/// reach the walk's counters.
pub(in crate::parser::kotlin) struct Caller<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub id: &'a str,
    pub name: &'a str,
    pub parent_class: Option<&'a str>,
}

/// What the walk accumulates as it descends: the call ordinal, the two arm
/// counters, and the result they are written into.
struct Walk<'a> {
    call_order: u32,
    arm_counter: u32,
    loop_counter: u32,
    result: &'a mut ParseResult,
}

/// Context threaded through `extract_calls` and its helpers, so the recursive
/// walker passes one reference instead of plumbing eight arguments per call.
/// The two halves are separate structs because they change for separate
/// reasons: a new counter is not a new thing to know about the caller, and
/// only [`Caller`] crosses the module edge.
pub(in crate::parser::kotlin) struct CallCtx<'a> {
    caller: Caller<'a>,
    walk: Walk<'a>,
}

impl<'a> CallCtx<'a> {
    /// Start a walk over one function's body. The counters begin at zero per
    /// body, which is what makes an arm path (`c1`, `l1`) read relative to
    /// the function rather than to the file.
    pub(in crate::parser::kotlin) fn new(caller: Caller<'a>, result: &'a mut ParseResult) -> Self {
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

    /// Consume the next call ordinal, so the edges that survive filtering
    /// keep their source order.
    fn next_order(&mut self) -> u32 {
        self.walk.call_order += 1;
        self.walk.call_order
    }

    /// Emit a plain Branch arm for `body` and return its path.
    fn open_arm(&mut self, current_branch: Option<&str>, body: &Node) -> String {
        let path = self.next_arm_path(current_branch);
        emit_branch_entity(
            self.caller.id,
            current_branch,
            &path,
            body,
            self.caller.path,
            self.walk.result,
        );
        path
    }

    /// Consume the next arm number and return its path under
    /// `current_branch`. Top-level arms read as `c1`, `c2`; nested ones
    /// append `.c<idx>` so the path ancestry is left-to-right.
    fn next_arm_path(&mut self, current_branch: Option<&str>) -> String {
        self.walk.arm_counter += 1;
        match current_branch {
            Some(p) => format!("{}.c{}", p, self.walk.arm_counter),
            None => format!("c{}", self.walk.arm_counter),
        }
    }

    /// Loop counterpart of `next_arm_path`. Loops count separately so a
    /// sibling decision tree and loop at the same scope read as `c1` / `l1`
    /// rather than colliding on one counter.
    fn next_loop_path(&mut self, current_branch: Option<&str>) -> String {
        self.walk.loop_counter += 1;
        match current_branch {
            Some(p) => format!("{}.l{}", p, self.walk.loop_counter),
            None => format!("l{}", self.walk.loop_counter),
        }
    }
}

/// Walk the body of a function, emitting `Calls` / `Instantiates`
/// relationships and synthesising Branch / Loop entities for the arms of
/// control-flow constructs.
pub(in crate::parser::kotlin) fn extract_calls(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    if dispatch_flow_node(node, ctx, current_branch) {
        return;
    }
    if node.kind() == "call_expression" {
        handle_call_expression(node, ctx, current_branch);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls(&child, ctx, current_branch);
    }
}

/// Flow-control dispatch: `if`, `when`, `try`, and the three loop kinds.
/// Returns `true` when the node was fully handled — including any required
/// recursion into its children — so the caller skips the default child-walk.
///
/// `elvis_expression` is deliberately absent: like Java's ternary it scores
/// for complexity but gets no Branch entity, and its two sides still recurse
/// through the generic child-walk so calls in either emit edges.
fn dispatch_flow_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    match node.kind() {
        "if_expression" => handle_if_expression(node, ctx, current_branch),
        "when_expression" => handle_when_expression(node, ctx, current_branch),
        "try_expression" => handle_try_expression(node, ctx, current_branch),
        "for_statement" => handle_for_statement(node, ctx, current_branch),
        "while_statement" | "do_while_statement" => handle_while(node, ctx, current_branch),
        _ => return false,
    }
    true
}

/// Per-arm dispatch for `if (cond) { } else if (cond2) { } else { }`.
///
/// Kotlin parses `else if` as a `control_structure_body` wrapping another
/// `if_expression`, but the user-visible model is a flat list of arms. The
/// chain is flattened here so siblings read as `c1` / `c2` / `c3` instead of
/// nesting, matching Java and the Python `elif_clause` handling.
///
/// Each condition stays in `current_branch` — it is evaluated before its arm
/// runs and belongs to the outer flow.
fn handle_if_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut current = *node;
    loop {
        let parts = parts::if_parts(&current);
        if let Some(condition) = parts.condition {
            extract_calls(&condition, ctx, current_branch);
        }
        if let Some(consequence) = parts.consequence {
            let path = ctx.open_arm(current_branch, &consequence);
            extract_calls(&consequence, ctx, Some(&path));
        }
        let Some(alternative) = parts.alternative else {
            return;
        };
        match parts::chained_if(&alternative) {
            // `else if (…) { … }` — a sibling arm, not a nested one.
            Some(chained) => current = chained,
            None => {
                let path = ctx.open_arm(current_branch, &alternative);
                extract_calls(&alternative, ctx, Some(&path));
                return;
            }
        }
    }
}

/// Per-arm dispatch for `when (subject) { a -> …; b, c -> …; else -> … }`.
///
/// Each `when_entry` becomes one arm tagged `case_arm`, carrying its
/// condition list verbatim as the `pattern:` attribute. The subject and the
/// conditions stay in `current_branch`: they are evaluated to choose an arm,
/// not inside one.
fn handle_when_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "when_subject" => extract_calls(&child, ctx, current_branch),
            "when_entry" => handle_when_entry(&child, ctx, current_branch),
            _ => {}
        }
    }
}

fn handle_when_entry(entry: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut conditions: Vec<String> = Vec::new();
    let mut body: Option<Node> = None;
    let mut cursor = entry.walk();
    for child in entry.children(&mut cursor) {
        match child.kind() {
            "when_condition" => {
                extract_calls(&child, ctx, current_branch);
                conditions.push(node_text(&child, ctx.caller.source).to_string());
            }
            "control_structure_body" => body = Some(child),
            _ => {}
        }
    }
    // An `else ->` entry parses with no `when_condition`; the arm is still
    // emitted, with no pattern, exactly as Java emits `default:`.
    let pattern = if conditions.is_empty() {
        None
    } else {
        Some(conditions.join(", "))
    };
    let arm = body.unwrap_or(*entry);
    let path = ctx.next_arm_path(current_branch);
    emit_case_arm_entity(
        ctx.caller.id,
        current_branch,
        &path,
        &arm,
        ctx.caller.path,
        pattern.as_deref(),
        ctx.walk.result,
    );
    if let Some(body) = body {
        extract_calls(&body, ctx, Some(&path));
    }
}

/// Per-arm dispatch for `try { } catch (e: E) { } finally { }`.
///
/// The try body itself becomes the first arm (tagged `try_body_arm`); each
/// `catch_block` and the optional `finally_block` follow. The caught type
/// travels as documentation plus a `caught:<type>` attribute.
fn handle_try_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let body = parts::block_statements(node);
    let path = ctx.next_arm_path(current_branch);
    emit_try_arm_entity(
        ctx.caller.id,
        current_branch,
        &path,
        &body.unwrap_or(*node),
        ctx.caller.path,
        "try_body",
        None,
        ctx.walk.result,
    );
    if let Some(body) = body {
        extract_calls(&body, ctx, Some(&path));
    }

    let mut cursor = node.walk();
    for clause in node.children(&mut cursor) {
        match clause.kind() {
            "catch_block" => handle_catch_block(&clause, ctx, current_branch),
            "finally_block" => handle_try_clause(&clause, ctx, current_branch, "finally", None),
            _ => {}
        }
    }
}

fn handle_catch_block(clause: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let caught = parts::caught_type(clause).map(|t| node_text(&t, ctx.caller.source).to_string());
    handle_try_clause(clause, ctx, current_branch, "catch", caught.as_deref());
}

/// Emit one `catch` / `finally` arm and walk its statements.
///
/// An empty block (`catch (e: E) { }`) leaves no `statements` node behind —
/// this grammar inlines the braces into the clause — so the arm is spanned
/// against the clause itself rather than dropped.
fn handle_try_clause(
    clause: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    arm_kind: &str,
    caught: Option<&str>,
) {
    let body = parts::block_statements(clause);
    let path = ctx.next_arm_path(current_branch);
    emit_try_arm_entity(
        ctx.caller.id,
        current_branch,
        &path,
        &body.unwrap_or(*clause),
        ctx.caller.path,
        arm_kind,
        caught,
        ctx.walk.result,
    );
    if let Some(body) = body {
        extract_calls(&body, ctx, Some(&path));
    }
}

/// `for (item in iterable) { body }`. The iterable expression's calls
/// (`for (row in queryRows(sql))`) group under the loop, mirroring Java's
/// enhanced-for and Python's `for … in iter:`.
fn handle_for_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    handle_loop(node, ctx, current_branch, parts::for_iterable(node));
}

/// `while (cond) { body }` and `do { body } while (cond)`. The condition runs
/// on every iteration, so its calls group under the loop too; the two forms
/// differ only in child order, which [`parts::loop_condition`] absorbs.
fn handle_while(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    handle_loop(node, ctx, current_branch, parts::loop_condition(node));
}

/// Shared body of the three loop handlers: take a loop path, emit the Loop
/// entity for the body, then walk the header expression and the body under
/// that path.
fn handle_loop(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    header: Option<Node>,
) {
    let path = ctx.next_loop_path(current_branch);
    let body = parts::control_body(node);
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
    if let Some(header) = header {
        extract_calls(&header, ctx, Some(&path));
    }
    if let Some(body) = body {
        extract_calls(&body, ctx, Some(&path));
    }
}

fn handle_call_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut cursor = node.walk();
    let Some(target) = node.children(&mut cursor).next() else {
        return;
    };

    match target.kind() {
        "simple_identifier" => {
            let name = node_text(&target, ctx.caller.source).to_string();
            let order = ctx.next_order();
            if name == ctx.caller.name || is_stdlib_method(&name) {
                return;
            }
            // A leading uppercase means a constructor call — Kotlin has no
            // `new`, so the name is the only signal available.
            let (callee, kind) = if name.starts_with(|c: char| c.is_uppercase()) {
                (name, RelationshipKind::Instantiates)
            } else {
                (
                    qualify(None, &name, ctx.caller.parent_class),
                    RelationshipKind::Calls,
                )
            };
            emit_edge(ctx, callee, kind, order, current_branch);
        }
        "navigation_expression" => {
            let Some((receiver, method)) = extract_navigation_parts(&target, ctx.caller.source)
            else {
                return;
            };
            let order = ctx.next_order();
            if method == ctx.caller.name || is_stdlib_method(&method) {
                return;
            }
            let callee = qualify(Some(&receiver), &method, ctx.caller.parent_class);
            emit_edge(ctx, callee, RelationshipKind::Calls, order, current_branch);
        }
        _ => {}
    }
}

/// Qualify a called name with its most specific known owner: `this.f()`,
/// `super.f()` and a bare `f()` take the enclosing class; any other receiver
/// is used verbatim so the resolver can try `Receiver.f`.
fn qualify(receiver: Option<&str>, method: &str, parent_class: Option<&str>) -> String {
    match receiver {
        Some("this") | Some("super") | None => match parent_class {
            Some(cls) => format!("{}.{}", cls, method),
            None => method.to_string(),
        },
        Some(other) => format!("{}.{}", other, method),
    }
}

/// Record one call/instantiation edge, tagged with its source order and —
/// when the call sits inside a flow arm — that arm's branch path.
fn emit_edge(
    ctx: &mut CallCtx<'_>,
    callee: String,
    kind: RelationshipKind,
    order: u32,
    current_branch: Option<&str>,
) {
    let mut rel = Relationship::new(ctx.caller.id.to_string(), callee, kind);
    rel.metadata.insert("order".to_string(), order.to_string());
    if let Some(branch) = current_branch {
        rel.metadata
            .insert("branch".to_string(), branch.to_string());
    }
    ctx.walk.result.add_relationship(rel);
}

/// Extract (receiver, method_name) from a `navigation_expression` node.
fn extract_navigation_parts(node: &Node, source: &str) -> Option<(String, String)> {
    // navigation_expression has: child[0] = receiver, then navigation_suffix containing `.` + simple_identifier
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    let receiver_node = children.first()?;
    let nav_suffix = children.iter().find(|c| c.kind() == "navigation_suffix")?;

    let receiver = match receiver_node.kind() {
        "simple_identifier" => node_text(receiver_node, source).to_string(),
        "this_expression" => "this".to_string(),
        "super_expression" => "super".to_string(),
        // For chained calls like `a.b.c()`, take the full text as receiver
        _ => node_text(receiver_node, source).to_string(),
    };

    // navigation_suffix contains `.` and `simple_identifier`
    let mut sc = nav_suffix.walk();
    let method_name = nav_suffix
        .children(&mut sc)
        .find(|c| c.kind() == "simple_identifier")
        .map(|c| node_text(&c, source).to_string())?;

    Some((receiver, method_name))
}

/// Positional readers for the control-flow constructs.
///
/// `tree-sitter-kotlin 0.3` declares no field names, so every part of an
/// `if` / loop / `try` has to be found by kind and order. Keeping that in one
/// module means a grammar bump is checked here rather than hunted for across
/// six handlers.
mod parts {
    use tree_sitter::Node;

    /// The three positional parts of an `if_expression`. Any of them can be
    /// absent: `if (a) ;` has no consequence, and an `if` without `else` has
    /// no alternative.
    pub(super) struct IfParts<'t> {
        pub condition: Option<Node<'t>>,
        pub consequence: Option<Node<'t>>,
        pub alternative: Option<Node<'t>>,
    }

    /// Split an `if_expression` on its anonymous `else` token: the first
    /// non-body named child is the condition, and the
    /// `control_structure_body` before/after the token is the
    /// consequence/alternative.
    pub(super) fn if_parts<'t>(node: &Node<'t>) -> IfParts<'t> {
        let mut parts = IfParts {
            condition: None,
            consequence: None,
            alternative: None,
        };
        let mut seen_else = false;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "else" {
                seen_else = true;
            } else if child.kind() == "control_structure_body" {
                let slot = if seen_else {
                    &mut parts.alternative
                } else {
                    &mut parts.consequence
                };
                *slot = Some(child);
            } else if child.is_named() && parts.condition.is_none() {
                parts.condition = Some(child);
            }
        }
        parts
    }

    /// The `if_expression` an `else if` alternative wraps, if this
    /// alternative is a chained one rather than a terminal `else` body.
    pub(super) fn chained_if<'t>(alternative: &Node<'t>) -> Option<Node<'t>> {
        let mut cursor = alternative.walk();
        let chained = alternative
            .children(&mut cursor)
            .find(|c| c.kind() == "if_expression");
        chained
    }

    /// The `control_structure_body` of a loop, absent for `while (x) ;`.
    pub(super) fn control_body<'t>(node: &Node<'t>) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        let body = node
            .children(&mut cursor)
            .find(|c| c.kind() == "control_structure_body");
        body
    }

    /// The condition of a `while` / `do…while` — the only named child that
    /// is not the body. Order-insensitive, so it reads both forms.
    pub(super) fn loop_condition<'t>(node: &Node<'t>) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        let condition = node
            .children(&mut cursor)
            .find(|c| c.is_named() && c.kind() != "control_structure_body");
        condition
    }

    /// The iterable of a `for (item in iterable)` — the named child that
    /// follows the anonymous `in` token, past the loop variable.
    pub(super) fn for_iterable<'t>(node: &Node<'t>) -> Option<Node<'t>> {
        named_after(node, "in")
    }

    /// The declared exception type of a `catch (e: E)` — the named child
    /// that follows the `:` separating it from the binding.
    pub(super) fn caught_type<'t>(clause: &Node<'t>) -> Option<Node<'t>> {
        named_after(clause, ":")
    }

    /// The `statements` of a block the grammar inlined into its parent — the
    /// body of a `try`, a `catch` or a `finally`. `None` for an empty block:
    /// with the braces inlined there is no node left to point at.
    pub(super) fn block_statements<'t>(node: &Node<'t>) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        let statements = node
            .children(&mut cursor)
            .find(|c| c.kind() == "statements");
        statements
    }

    /// The first named child appearing after the given anonymous token.
    fn named_after<'t>(node: &Node<'t>, token: &str) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        let mut seen = false;
        for child in node.children(&mut cursor) {
            if child.kind() == token {
                seen = true;
            } else if seen && child.is_named() {
                return Some(child);
            }
        }
        None
    }
}
