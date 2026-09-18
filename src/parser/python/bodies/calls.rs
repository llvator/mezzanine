//! Call-site extraction for Python function bodies.
//!
//! Walks the AST of a function body emitting `Calls` / `Instantiates` /
//! `WritesTo` relationships, and synthesizing `Branch` / `Loop` entities
//! for if/elif/else, for, while, and try/except arms. Each arm/loop body
//! is recursed into with its `current_branch` set to the path of that
//! arm so calls and writes inside attribute-tag onto the right node.

use super::flow::{
    emit_branch_entity, emit_case_arm_entity, emit_loop_entity, emit_try_arm_entity,
    emit_with_entity, Arm,
};
use super::inference::{Locals, Returns};
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, Visibility};
use crate::parser::language_parser::{node_text, ParseResult};
use crate::parser::working_set;
use std::path::Path;
use tree_sitter::Node;

/// Who a call is attributed to, and everything fixed for the whole body that
/// deciding a target needs. Read-only, so a helper that resolves a name
/// cannot reach the walk's counters.
pub(in crate::parser::python) struct Caller<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub id: &'a str,
    pub name: &'a str,
    /// The class this callable hangs off, when it is a method — what `self`,
    /// `cls` and a bare sibling-method call resolve against.
    pub parent_class: Option<&'a str>,
    /// What this body's class and signature declare about the types its
    /// receivers hold, so `self.store.save()` can resolve to `Store.save`
    /// (PY-031). Empty for a body that annotates nothing, which leaves every
    /// receiver spelled the way the source spells it.
    pub locals: &'a Locals,
    /// What a call to each name in this file evaluates to, so the links of a
    /// fluent chain resolve (PY-032).
    pub returns: &'a Returns,
}

/// What the walk accumulates as it descends: the call ordinal, and the
/// result it is written into. Owned rather than borrowed — the ordinal
/// starts at zero with the body and leaves only as edge metadata.
struct Walk<'a> {
    call_order: u32,
    result: &'a mut ParseResult,
}

/// Context threaded through the walk, so a recursive step passes one
/// reference rather than nine arguments. The two halves are separate structs
/// because they change for separate reasons: a new counter is not a new
/// thing to know about the caller.
pub(in crate::parser::python) struct CallCtx<'a> {
    caller: Caller<'a>,
    walk: Walk<'a>,
}

impl<'a> CallCtx<'a> {
    /// Start a walk over one callable's body. The call ordinal begins at
    /// zero per body, which is what makes `order` metadata read relative to
    /// the callable rather than to the file.
    pub(in crate::parser::python) fn new(caller: Caller<'a>, result: &'a mut ParseResult) -> Self {
        Self {
            caller,
            walk: Walk {
                call_order: 0,
                result,
            },
        }
    }

    /// Consume the next call ordinal. Taken before any filtering, so a
    /// dropped call still burns its number and the edges that survive keep
    /// their source order.
    fn next_order(&mut self) -> u32 {
        self.walk.call_order += 1;
        self.walk.call_order
    }

    /// Address one synthetic flow entity: this caller, this enclosing arm,
    /// this path, this file.
    fn arm<'p>(&self, parent_branch: Option<&'p str>, path: &'p str) -> Arm<'p>
    where
        'a: 'p,
    {
        Arm {
            caller_id: self.caller.id,
            parent_branch,
            path,
            file: self.caller.path,
        }
    }
}

/// The three per-scope counters.
///
/// [`extract_calls`] starts a fresh set for every block it walks — a method
/// body, a branch arm, a loop body — which is what makes `c1` / `l1` / `w1`
/// read relative to their container instead of continuing the outer scope's
/// sequence. Arms, loops and `with` blocks count on separate sequences so
/// the first `if` and the first `for` at one scope are `c1` and `l1` rather
/// than `c1` and `c2`.
#[derive(Default)]
struct Counters {
    arm: u32,
    loops: u32,
    withs: u32,
}

impl Counters {
    fn next_arm(&mut self, current_branch: Option<&str>) -> String {
        self.arm += 1;
        nested_path(current_branch, 'c', self.arm)
    }

    fn next_loop(&mut self, current_branch: Option<&str>) -> String {
        self.loops += 1;
        nested_path(current_branch, 'l', self.loops)
    }

    fn next_with(&mut self, current_branch: Option<&str>) -> String {
        self.withs += 1;
        nested_path(current_branch, 'w', self.withs)
    }
}

/// An arm's path under its parent: `c1`, then `c1.l2` for a loop inside it.
fn nested_path(current_branch: Option<&str>, prefix: char, idx: u32) -> String {
    match current_branch {
        Some(p) => format!("{}.{}{}", p, prefix, idx),
        None => format!("{}{}", prefix, idx),
    }
}

/// One edge out of the callable being walked, tagged with the arm it fired
/// in so the analyzer can re-parent it onto that arm's node.
fn edge(
    caller_id: &str,
    target: String,
    kind: RelationshipKind,
    current_branch: Option<&str>,
) -> Relationship {
    let mut rel = Relationship::new(caller_id.to_string(), target, kind);
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    rel
}

/// Walk a child that may not be there. Every control-flow handler reaches
/// for optional grammar fields, and spelling the `if let` out at each one is
/// what pushed those handlers over the names-in-view line.
fn walk_opt(target: Option<Node>, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(target) = target {
        extract_calls(&target, ctx, current_branch);
    }
}

pub(in crate::parser::python) fn extract_calls(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    // Per-node dispatch: call sites, assignments, walrus writes, and
    // lambda scope creation. Pulled into a helper so this function's
    // top doesn't grow a new branch per node-kind we want to react to.
    // Returns true when the dispatch handled `node` as its own scope
    // (currently: only lambda) and the rest of this call should be
    // skipped — otherwise the walk continues normally.
    if run_node_handlers(node, ctx, current_branch) {
        return;
    }
    // Per-scope counters: each recursive call (method body, branch body,
    // loop body, module body) has its own numbering reset, so nested
    // control-flow labels restart at 1 under their container instead of
    // continuing the outer counter.
    let mut counters = Counters::default();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            // Nested `def`/`class` (and their decorated form) own
            // their own scope. Skip them entirely here — otherwise
            // calls and `x = …` writes from the inner body would be
            // attributed to the enclosing function. `handle_function`
            // re-enters them via `extract_entities` so they still
            // get registered as their own entities.
            "function_definition" | "class_definition" | "decorated_definition" => {
                continue;
            }
            "if_statement" => handle_if_statement(&child, ctx, current_branch, &mut counters),
            "for_statement" => handle_for_statement(&child, ctx, current_branch, &mut counters),
            "conditional_expression" => handle_ternary(&child, ctx, current_branch),
            "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression" => handle_comprehension(&child, ctx, current_branch),
            "try_statement" => handle_try_statement(&child, ctx, current_branch, &mut counters),
            "match_statement" => handle_match_statement(&child, ctx, current_branch, &mut counters),
            "with_statement" => handle_with_statement(&child, ctx, current_branch, &mut counters),
            "while_statement" => handle_while_statement(&child, ctx, current_branch, &mut counters),
            _ => extract_calls(&child, ctx, current_branch),
        }
    }
}

/// Per-node dispatch: call sites, assignment writes (self / local),
/// walrus writes, and lambda scope creation. Returns `true` when the
/// node was treated as its own scope and the caller should stop
/// walking children — only lambdas trigger that, since they own their
/// own caller id and the recursion happens inside `handle_lambda`.
///
/// Other handlers fire alongside the normal child-walk: `handle_call`
/// on a `call` node, the assignment handlers on `assignment` /
/// `augmented_assignment`, and `handle_walrus_write` on
/// `named_expression`. `handle_call` runs even when `extract_calls` was
/// passed the call directly (e.g. a `for k, v in mapping.items():`
/// where the iterator field IS the call) so the call edge isn't lost.
fn run_node_handlers(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    match node.kind() {
        "lambda" => {
            handle_lambda(node, ctx);
            return true;
        }
        "call" => handle_call(node, ctx, current_branch),
        "raise_statement" => handle_raise(node, ctx, current_branch),
        _ => handle_write_forms(node, ctx, current_branch),
    }
    false
}

/// The node kinds that bind a name: `x = …`, `x += …`, and the walrus
/// `(x := …)`. Grouped into one dispatch arm so adding a node-kind handler to
/// `run_node_handlers` doesn't widen its match every time.
fn handle_write_forms(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    match node.kind() {
        "assignment" | "augmented_assignment" => {
            handle_self_write(node, ctx, current_branch);
            handle_local_write(node, ctx, current_branch);
        }
        "named_expression" => handle_walrus_write(node, ctx, current_branch),
        _ => {}
    }
}

/// `raise ValueError("nope")` makes this function depend on `ValueError`
/// (PY-010). Emits a `References` edge to the raised type, tagged `raises`.
///
/// Both shapes land on the same edge. `raise Err(...)` is a call, so the walk
/// also produces an `Instantiates` edge — but that edge cannot say whether
/// the instance was thrown or merely stored, which is the question "what does
/// this function raise?" needs answered. `raise err` and `raise mod.Err` are
/// not calls and produced nothing at all before.
///
/// `References` is deliberately not a dependency kind, so the extra edge
/// annotates the graph without inflating fan-out or coupling metrics. The
/// target is the same name the `Instantiates` edge uses, so the two share one
/// node rather than minting a second ghost.
///
/// A bare `raise` (the re-raise inside an `except`) names nothing and is
/// skipped. The branch tag is honoured — raising inside an `except` arm is
/// the common case, and the edge belongs to that arm.
fn handle_raise(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(raised) = node.named_child(0) else {
        return;
    };
    let source = ctx.caller.source;
    let named = match raised.kind() {
        "call" => raised
            .child_by_field_name("function")
            .and_then(|f| receiver_name(&f, source)),
        _ => receiver_name(&raised, source),
    };
    let Some(target) = named else {
        return;
    };
    let mut rel = edge(
        ctx.caller.id,
        target,
        RelationshipKind::References,
        current_branch,
    );
    rel.metadata
        .insert("raises".to_string(), "true".to_string());
    ctx.walk.result.add_relationship(rel);
}

/// `a if cond else b` is a control-flow split inside an expression (PY-011):
/// calls in `a` only fire when the condition holds, calls in `b` only when it
/// doesn't. Treating all three as unconditional made a ternary look like it
/// always did both.
///
/// Branch entities are emitted only when an arm actually contains a call.
/// Ternaries are overwhelmingly `x if x else 0` — value-only — and minting
/// two empty Branch nodes for each of those would bury the decision trees
/// that matter under one-liner noise. The condition always stays in the
/// enclosing scope; it is evaluated either way.
fn handle_ternary(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    // Positional children, in source order: consequence, condition,
    // alternative — `a if cond else b`.
    let consequence = node.named_child(0);
    let condition = node.named_child(1);
    let alternative = node.named_child(2);

    walk_opt(condition, ctx, current_branch);

    let worth_branching = [consequence, alternative]
        .into_iter()
        .flatten()
        .any(|arm| contains_call(&arm));
    if !worth_branching {
        walk_opt(consequence, ctx, current_branch);
        walk_opt(alternative, ctx, current_branch);
        return;
    }

    let base = flow_path_segment(node, "t");
    for (index, arm) in [consequence, alternative].into_iter().flatten().enumerate() {
        let branch_path = match current_branch {
            Some(p) => format!("{}.{}_{}", p, base, index + 1),
            None => format!("{}_{}", base, index + 1),
        };
        emit_branch_entity(
            &ctx.arm(current_branch, &branch_path),
            &arm,
            ctx.walk.result,
        );
        extract_calls(&arm, ctx, Some(&branch_path));
    }
}

/// A path segment for a construct that is an *expression*, not a statement.
///
/// The `c1` / `l1` / `w1` counters belong to a statement block: `extract_calls`
/// resets them for every scope it walks, which works because every construct
/// that uses them is a direct child of a block. A ternary or a comprehension
/// is not — it hides inside an assignment or an argument list, several
/// recursion levels down, where the enclosing block's counter is long out of
/// reach and a fresh one starts at 1 again. Numbering them that way collided:
/// a ternary arm and the `if` arm two lines below it both became `c1`, and the
/// analyzer merged them into one node.
///
/// Position is the one thing that is unique without threading state through
/// the whole walk. `<lambda@12:8>` already names entities this way for the
/// same reason.
fn flow_path_segment(node: &Node, prefix: &str) -> String {
    let start = node.start_position();
    format!("{}{}_{}", prefix, start.row + 1, start.column + 1)
}

/// Does this expression contain a call of its own?
///
/// Stops at nested scopes: a call inside a `lambda` belongs to the lambda,
/// which becomes its own entity, so it must not decide whether the enclosing
/// ternary arm is worth a Branch node.
fn contains_call(node: &Node) -> bool {
    if matches!(
        node.kind(),
        "lambda" | "function_definition" | "class_definition"
    ) {
        return false;
    }
    if node.kind() == "call" {
        return true;
    }
    let mut cursor = node.walk();
    let mut children = node.named_children(&mut cursor);
    children.any(|child| contains_call(&child))
}

/// A comprehension is a loop written as an expression (PY-012).
///
/// `[transform(x) for x in source() if keep(x)]` makes three calls, and all
/// three used to land flat in the enclosing function's scope — so a function
/// whose whole body is one comprehension looked like straight-line code. It
/// gets a `Loop` entity, and everything inside it — the iterator expression,
/// the filters, the element expression — groups under that loop, exactly as
/// the body of an explicit `for` does.
///
/// Nested `for` / `if` clauses need no special handling: they are children of
/// the same comprehension node, so they land in the same loop scope.
fn handle_comprehension(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let segment = flow_path_segment(node, "l");
    let loop_path = match current_branch {
        Some(p) => format!("{}.{}", p, segment),
        None => segment,
    };
    let is_async = comprehension_is_async(node, ctx.caller.source);
    emit_loop_entity(
        &ctx.arm(current_branch, &loop_path),
        node,
        is_async,
        ctx.walk.result,
    );
    // A comprehension nested directly inside this one (`[[c for c in row]
    // for row in rows]`) is its own loop, but arrives as a child node rather
    // than as a child of a body — the dispatch table only ever sees children,
    // so it would never match. Recurse explicitly so the nested loop lands
    // under this one.
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_comprehension_kind(child.kind()) {
            handle_comprehension(&child, ctx, Some(&loop_path));
        } else {
            extract_calls(&child, ctx, Some(&loop_path));
        }
    }
}

/// The four comprehension node kinds. Kept in one place so the dispatch table
/// and the nested-comprehension recursion cannot drift apart.
fn is_comprehension_kind(kind: &str) -> bool {
    matches!(
        kind,
        "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression"
    )
}

/// `[x async for x in stream()]` — the `async` sits on the `for_in_clause`,
/// not on the comprehension, so the outer node's text never starts with it.
fn comprehension_is_async(node: &Node, source: &str) -> bool {
    let mut cursor = node.walk();
    let mut clauses = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "for_in_clause");
    clauses.any(|c| is_async_construct(&c, source))
}

/// Per-arm dispatch for `if / elif / else`.
///
/// Each arm becomes its own `Branch` entity so calls inside group under the
/// arm rather than under the function. The condition stays in the enclosing
/// scope — it runs whichever way the branch goes.
///
/// Extracted from `extract_calls` for the same reason `handle_try_statement`
/// and `handle_match_statement` were: the dispatch table has to stay at its
/// grandfathered complexity, and it cannot if every control-flow construct
/// spells its handling out inline.
fn handle_if_statement(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    // Condition expression stays in the enclosing scope — it's part of the
    // outer flow, not a new arm.
    walk_opt(node.child_by_field_name("condition"), ctx, current_branch);
    // The `if` arm is the first arm at this scope.
    walk_arm(
        node.child_by_field_name("consequence"),
        ctx,
        current_branch,
        counters,
    );
    // Every `elif_clause` AND the final `else_clause` sit as sibling
    // `alternative` fields on the if_statement itself (per
    // tree-sitter-python's grammar), so we iterate them here instead of
    // recursing into a chain.
    let mut alt_cursor = node.walk();
    for alt in node.children_by_field_name("alternative", &mut alt_cursor) {
        match alt.kind() {
            "elif_clause" => {
                walk_opt(alt.child_by_field_name("condition"), ctx, current_branch);
                walk_arm(
                    alt.child_by_field_name("consequence"),
                    ctx,
                    current_branch,
                    counters,
                );
            }
            "else_clause" => walk_arm(
                alt.child_by_field_name("body"),
                ctx,
                current_branch,
                counters,
            ),
            _ => extract_calls(&alt, ctx, current_branch),
        }
    }
}

/// Open one branch arm over `body` — take the next arm number at this scope,
/// place the `Branch` entity, and walk the body inside it.
///
/// Every arm of an `if`, and the `else` of a loop, is this same three-step
/// move; writing it once is what lets `handle_if_statement` read as the list
/// of arms it dispatches rather than as the bookkeeping each one costs.
fn walk_arm(
    body: Option<Node>,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let Some(body) = body else { return };
    let branch_path = counters.next_arm(current_branch);
    emit_branch_entity(
        &ctx.arm(current_branch, &branch_path),
        &body,
        ctx.walk.result,
    );
    extract_calls(&body, ctx, Some(&branch_path));
}

/// `for` / `async for`: one `Loop` entity, with the iterator expression, the
/// body, and any `else` clause grouped under it.
fn handle_for_statement(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let loop_path = counters.next_loop(current_branch);
    let is_async = is_async_construct(node, ctx.caller.source);
    // Emit one Loop entity per for-loop, using the body span so the entity
    // carries the real arm's lines.
    if let Some(body) = node.child_by_field_name("body") {
        emit_loop_entity(
            &ctx.arm(current_branch, &loop_path),
            &body,
            is_async,
            ctx.walk.result,
        );
    }
    // The iterator expression (`for k, v in kwargs.items()`) semantically
    // belongs to the loop — the user sees `kwargs.items()` as part of the
    // loop setup, so we group it under the Loop node instead of leaving it
    // at the outer scope.
    walk_opt(node.child_by_field_name("right"), ctx, Some(&loop_path));
    walk_opt(node.child_by_field_name("body"), ctx, Some(&loop_path));
    walk_loop_else(node, ctx, current_branch, &loop_path, counters);
}

/// A `while` loop, handled as its own Loop scope. Mirrors
/// `handle_for_statement`: the condition (re-evaluated each iteration) and
/// the body group under the loop, and a trailing `else` becomes a branch
/// arm inside it. Split out of the `extract_calls` dispatch table so the
/// walk stays a flat one-arm-per-node-kind match rather than carrying this
/// construct's nesting inline.
fn handle_while_statement(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let loop_path = counters.next_loop(current_branch);
    if let Some(body) = node.child_by_field_name("body") {
        // A `while` is never async.
        emit_loop_entity(
            &ctx.arm(current_branch, &loop_path),
            &body,
            false,
            ctx.walk.result,
        );
    }
    // The condition runs on every iteration, so group any calls inside it
    // with the loop too.
    walk_opt(node.child_by_field_name("condition"), ctx, Some(&loop_path));
    walk_opt(node.child_by_field_name("body"), ctx, Some(&loop_path));
    walk_loop_else(node, ctx, current_branch, &loop_path, counters);
}

/// Python's `for`/`while` can carry an `else` clause that runs only when the
/// loop completes without a `break`. Treat it as a branch arm *inside* the
/// loop so calls there don't leak back to the outer scope — the arm's number
/// still comes from the enclosing scope's sequence, which is what keeps it
/// distinct from an arm of a sibling `if`.
fn walk_loop_else(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    loop_path: &str,
    counters: &mut Counters,
) {
    let Some(body) = node
        .child_by_field_name("alternative")
        .and_then(|c| c.child_by_field_name("body"))
    else {
        return;
    };
    let arm_path = counters.next_arm(current_branch);
    emit_branch_entity(&ctx.arm(Some(loop_path), &arm_path), &body, ctx.walk.result);
    extract_calls(&body, ctx, Some(&arm_path));
}

/// Per-arm dispatch for `try: ... except ...: ... else: ... finally:`.
///
/// Each arm of the construct becomes its own `Branch` entity so calls
/// inside group visually under the arm. Tags emitted: `try_arm` on
/// every arm plus a kind-specific tag (`try_body_arm`, `except_arm`,
/// `except_group_arm` for PEP 654 `except*`, `else_arm`,
/// `finally_arm`). Caught exception types travel as documentation +
/// `caught:<type>` attribute on the except arms.
///
/// The arm counter is shared with if/elif/else (and case arms) so a
/// sibling decision tree at the same scope numbers consistently —
/// extracted into this helper so the call-walk dispatch table in
/// `extract_calls` stays at its grandfathered complexity rather than
/// growing each time we add a new control-flow node-kind handler.
fn handle_try_statement(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    if let Some(body) = node.child_by_field_name("body") {
        let branch_path = counters.next_arm(current_branch);
        emit_try_arm_entity(
            &ctx.arm(current_branch, &branch_path),
            &body,
            "try_body",
            None,
            ctx.walk.result,
        );
        extract_calls(&body, ctx, Some(&branch_path));
    }

    // except / except* / else / finally are plain children (not in
    // any field), so iterate the children list.
    let mut clause_cursor = node.walk();
    for clause in node.children(&mut clause_cursor) {
        match clause.kind() {
            "except_clause" | "except_group_clause" => {
                handle_except_clause(&clause, ctx, current_branch, counters)
            }
            "else_clause" | "finally_clause" => {
                handle_try_simple_clause(&clause, ctx, current_branch, counters)
            }
            _ => {}
        }
    }
}

/// Body for the no-payload arms of a try-statement: `else` and
/// `finally`. Both have a body block but neither carries a caught
/// type. Kept distinct from `handle_except_clause` because the way
/// the body is reached differs — `else_clause` exposes a `body`
/// field, `finally_clause` doesn't and we have to find the trailing
/// `block` named child instead.
fn handle_try_simple_clause(
    clause: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let is_else = clause.kind() == "else_clause";
    // `named` must outlive the `find` result on the finally branch
    // because the returned Node borrows from the cursor.
    let mut named = clause.walk();
    let body = if is_else {
        clause.child_by_field_name("body")
    } else {
        clause
            .named_children(&mut named)
            .find(|nc| nc.kind() == "block")
    };
    let Some(b) = body else { return };
    let arm_kind = if is_else { "else" } else { "finally" };
    let branch_path = counters.next_arm(current_branch);
    emit_try_arm_entity(
        &ctx.arm(current_branch, &branch_path),
        &b,
        arm_kind,
        None,
        ctx.walk.result,
    );
    extract_calls(&b, ctx, Some(&branch_path));
}

/// Body of one `except`/`except*` clause. Splits the clause into the
/// caught type (first non-block named child, drilling through any
/// `as_pattern` to drop the `as e` alias) and the body block, then
/// emits the arm entity and recurses.
fn handle_except_clause(
    clause: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let mut caught: Option<String> = None;
    let mut body: Option<Node> = None;
    let mut named = clause.walk();
    for nc in clause.named_children(&mut named) {
        if nc.kind() == "block" {
            body = Some(nc);
        } else if caught.is_none() {
            let type_node = if nc.kind() == "as_pattern" {
                nc.named_child(0).unwrap_or(nc)
            } else {
                nc
            };
            caught = Some(node_text(&type_node, ctx.caller.source).to_string());
        }
    }
    let Some(b) = body else { return };
    let arm_kind = if clause.kind() == "except_group_clause" {
        "except_group"
    } else {
        "except"
    };
    let branch_path = counters.next_arm(current_branch);
    emit_try_arm_entity(
        &ctx.arm(current_branch, &branch_path),
        &b,
        arm_kind,
        caught.as_deref(),
        ctx.walk.result,
    );
    extract_calls(&b, ctx, Some(&branch_path));
}

/// Per-arm dispatch for `match SUBJECT: case PAT: ...` (PEP 634).
///
/// The match `subject` expression(s) stay in the enclosing scope —
/// they're the gate, not a per-arm action. Each `case_clause` becomes
/// its own `Branch` arm sharing the caller's arm counter so a
/// sibling decision tree at the same scope (an if/elif and a match in
/// the same function) numbers consistently.
///
/// Within a case arm: the optional `guard` (`case x if x > 0:`) runs
/// only when the pattern matches, so its calls reattach to the arm's
/// branch — not the outer scope, which is the difference from the
/// if-condition handling. The body (`consequence` field) recurses
/// with the new arm as `current_branch`.
fn handle_match_statement(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let mut subj_cursor = node.walk();
    for subj in node.children_by_field_name("subject", &mut subj_cursor) {
        extract_calls(&subj, ctx, current_branch);
    }

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut case_cursor = body.walk();
    for case in body.children(&mut case_cursor) {
        if case.kind() != "case_clause" {
            continue;
        }
        handle_case_clause(&case, ctx, current_branch, counters);
    }
}

/// One `case PAT [if GUARD]:` arm of a `match`.
fn handle_case_clause(
    case: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let Some(consequence) = case.child_by_field_name("consequence") else {
        return;
    };
    let branch_path = counters.next_arm(current_branch);
    let pattern_text = case_pattern_text(case, ctx.caller.source);
    emit_case_arm_entity(
        &ctx.arm(current_branch, &branch_path),
        &consequence,
        Some(pattern_text.as_str()).filter(|t| !t.is_empty()),
        ctx.walk.result,
    );
    walk_opt(case.child_by_field_name("guard"), ctx, Some(&branch_path));
    extract_calls(&consequence, ctx, Some(&branch_path));
}

/// Build the human-readable pattern string for one `case_clause`.
///
/// Tree-sitter-python exposes one or more `case_pattern` named
/// children (multi-pattern: `case 1, 2, 3:`); the optional `guard`
/// field is an `if_clause`. We join the patterns with `, ` and append
/// `if <guard>` so the rendered string reads like the source — useful
/// for the detail panel without requiring a re-parse.
fn case_pattern_text(case: &Node, source: &str) -> String {
    let mut patterns: Vec<String> = Vec::new();
    let mut named = case.walk();
    for nc in case.named_children(&mut named) {
        if nc.kind() == "case_pattern" {
            patterns.push(node_text(&nc, source).to_string());
        }
    }
    let mut text = patterns.join(", ");
    if let Some(guard) = case.child_by_field_name("guard") {
        let raw = node_text(&guard, source);
        let trimmed = raw.trim();
        // The if_clause node text reads "if <expr>"; strip the
        // leading keyword so we can prefix it ourselves and avoid
        // double "if if" when the source layout is unusual.
        let stripped = trimmed.strip_prefix("if").map(str::trim).unwrap_or(trimmed);
        if !text.is_empty() {
            text.push_str(" if ");
            text.push_str(stripped);
        }
    }
    text
}

/// Is this `for` / `with` its `async` form? tree-sitter-python uses one node
/// kind for both, with `async` as a leading anonymous token, so the source
/// slice is the only place the distinction survives.
fn is_async_construct(node: &Node, source: &str) -> bool {
    node_text(node, source).trim_start().starts_with("async")
}

/// Per-block dispatch for `with` / `async with` (PEP 343 context
/// managers).
///
/// The body is treated as a synthetic `Branch`-like entity tagged
/// `with_node` so calls inside group visually under the resource.
/// The manager expression itself recurses with `current_branch` set
/// to the with-arm — the `open(path)` in `with open(path) as f:` is
/// what brings the resource into scope, so it belongs under the with
/// node, not the enclosing function (mirroring how a `for` loop
/// groups its iterator).
///
/// The async variant adds an `async` tag/attribute. Detection sniffs
/// the leading source text since tree-sitter-python exposes the
/// `async` keyword as an anonymous token rather than a named field
/// or child — same approach `handle_function` uses for `async def`.
///
/// `with` blocks count on their own sequence (separate from arms and
/// loops) so a sibling decision tree, loop, and with-block at the same
/// scope read as `c1`/`l1`/`w1` rather than colliding on one shared
/// sequence.
fn handle_with_statement(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    counters: &mut Counters,
) {
    let is_async = is_async_construct(node, ctx.caller.source);

    let mut clause_cursor = node.walk();
    let with_clause = node
        .children(&mut clause_cursor)
        .find(|c| c.kind() == "with_clause");

    let with_path = counters.next_with(current_branch);

    let manager_text = with_clause
        .as_ref()
        .map(|c| node_text(c, ctx.caller.source).to_string());

    if let Some(body) = node.child_by_field_name("body") {
        emit_with_entity(
            &ctx.arm(current_branch, &with_path),
            &body,
            is_async,
            manager_text.as_deref(),
            ctx.walk.result,
        );
    }

    if let Some(clause) = with_clause {
        let mut item_cursor = clause.walk();
        for item in clause.children(&mut item_cursor) {
            if item.kind() != "with_item" {
                continue;
            }
            walk_opt(item.child_by_field_name("value"), ctx, Some(&with_path));
        }
    }

    walk_opt(node.child_by_field_name("body"), ctx, Some(&with_path));
}

/// One call site: whatever the callee resolves to, recorded against the arm
/// the call fires in and numbered in source order.
fn handle_call(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(function_node) = node.child_by_field_name("function") else {
        return;
    };
    // Taken before the callee is resolved, so a dropped call still burns its
    // number and the edges that survive keep their source order.
    let order = ctx.next_order();
    let named = match function_node.kind() {
        "identifier" => bare_callee(&function_node, &ctx.caller),
        "attribute" => attribute_callee(&function_node, &ctx.caller),
        _ => None,
    };
    let Some((target, kind)) = named else { return };
    let mut rel = edge(ctx.caller.id, target, kind, current_branch);
    rel.metadata.insert("order".to_string(), order.to_string());
    ctx.walk.result.add_relationship(rel);
}

/// What a bare `foo(...)` names, and which kind of edge that makes.
///
/// The name is emitted as written, whatever encloses the call. This looks
/// like a missing step and is not: Python resolves a bare name by LEGB —
/// local, enclosing, *module*, builtin — and class scope is not in that
/// chain. `foo()` inside a method never finds a sibling method; reaching one
/// needs `self.foo()`, which is `attribute_callee`'s business.
///
/// Until PY-031 this function prefixed the name with the enclosing class,
/// on the reasoning that a bare call in a method usually means a sibling.
/// It cannot: a module-level `normalize_string` called from a method of
/// `StringProcessor` became `StringProcessor.normalize_string`, which exists
/// nowhere and resolved to a ghost — while the real function sat two
/// definitions below in the same file. The resolver already ranks a bare
/// name by locality (AN-011), which is the same job done correctly.
///
/// Self-recursion is the only name dropped. Builtin calls (`print`, `len`,
/// `isinstance`, …) are emitted — the unresolved targets become ghost
/// entities tagged `ghost_stdlib` by the graph builder, and the UI's
/// stdlib-ghost toggle decides whether to show them. This keeps the data
/// layer faithful while letting the reader collapse the builtin noise when
/// it gets in the way.
fn bare_callee(function_node: &Node, caller: &Caller<'_>) -> Option<(String, RelationshipKind)> {
    let func_name = node_text(function_node, caller.source).to_string();
    if func_name == caller.name {
        return None;
    }
    // Capitalized name → likely class instantiation.
    if func_name.chars().next().is_some_and(char::is_uppercase) {
        return Some((func_name, RelationshipKind::Instantiates));
    }
    Some((func_name, RelationshipKind::Calls))
}

/// What a `receiver.method(...)` names.
///
/// Self-recursion is the only name dropped. A builtin-shaped name is not
/// filtered out here either: an attribute call like `obj.update(x)` can target
/// a user-defined `Observer.update` method, and dropping it on the basis of
/// the bare name colliding with `dict.update` produced false negatives.
/// Unresolved targets fall through to ghost nodes, where the UI's ghost toggle
/// already gives the user control.
fn attribute_callee(
    function_node: &Node,
    caller: &Caller<'_>,
) -> Option<(String, RelationshipKind)> {
    let (receiver, method_name) = extract_attribute_parts(function_node, caller.source)?;
    if method_name == caller.name {
        return None;
    }
    // `self` / `cls` / `super()` all name the enclosing class, when there is
    // one; outside a class they name nothing better than the method itself.
    let callee = match (receiver.as_str(), caller.parent_class) {
        ("self" | "cls" | "super()", Some(cls)) => format!("{}.{}", cls, method_name),
        ("self" | "cls" | "super()", None) => method_name,
        _ => {
            let typed = declared_receiver(function_node, &receiver, caller);
            format!("{}.{}", typed.as_deref().unwrap_or(&receiver), method_name)
        }
    };
    Some((callee, RelationshipKind::Calls))
}

/// The type a receiver is declared to hold, when something declared it
/// (PY-031).
///
/// Two shapes carry a declaration, and `receiver_name` has already reduced
/// both to their last segment, which is why the tree is consulted again
/// rather than the reduced text: `self.store` and a local named `store` both
/// arrive here as `"store"`, and only one of them is a field.
///
/// Answering `None` is the common case and the safe one — it leaves the
/// receiver spelled as the source spells it, which is what every Python body
/// without annotations got before this existed.
fn declared_receiver(function_node: &Node, receiver: &str, caller: &Caller<'_>) -> Option<String> {
    if caller.locals.is_empty() && caller.returns.is_empty() {
        return None;
    }
    let object = function_node.child_by_field_name("object")?;
    let declared = match object.kind() {
        // `store.save()` — a parameter of this callable.
        "identifier" => caller.locals.param(receiver),
        // `self.store.save()` — a field of the enclosing class. Anything
        // else on the left of the dot is a chain this pass cannot type.
        "attribute" => {
            let inner = object.child_by_field_name("object")?;
            (node_text(&inner, caller.source) == "self")
                .then(|| caller.locals.field(receiver))
                .flatten()
        }
        // `builder.set_size(...).set_dough()` — the receiver is the *result*
        // of `set_size`, so its type is whatever that method returns
        // (PY-032). `receiver_name` has already reduced the call to the name
        // of the method that produced it, which is the key `Returns` holds.
        "call" => caller.returns.of(receiver),
        _ => None,
    };
    declared.map(str::to_string)
}

fn extract_attribute_parts(node: &Node, source: &str) -> Option<(String, String)> {
    let object = node.child_by_field_name("object")?;
    let attribute = node.child_by_field_name("attribute")?;
    // `receiver_name` declines for receivers that no single name describes;
    // zipping propagates that decline as "no call to record".
    receiver_name(&object, source).zip(Some(node_text(&attribute, source).to_string()))
}

/// A lambda is a callable like any other, so it carries the same metric set
/// as a `def` — otherwise `mezz quality` would rank a `key=lambda …` holding
/// a three-way conditional as if it had no body at all. It also still
/// contributes to its enclosing callable's score, the way a Java lambda
/// contributes to the method it sits in.
fn populate_lambda_metrics(node: &Node, source: &str, entity: &mut CodeEntity) {
    entity.metrics.loc = (entity.span.end.line - entity.span.start.line + 1) as u32;
    entity.metrics.param_count = Some(
        node.child_by_field_name("parameters")
            .map_or(0, |p| p.named_child_count() as u32),
    );
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        let (cc, nesting, cog) = super::complexity::compute_complexity(&body);
        entity.metrics.cyclomatic = Some(cc);
        entity.metrics.max_nesting = Some(nesting);
        entity.metrics.cognitive_complexity = Some(cog);
    }
    working_set::populate(entity, body.as_ref(), source);
    crate::parser::loops::populate(entity, body.as_ref());
}

/// Reduce a receiver expression to the single identifier that names it.
///
/// PY-026: this used to be `node_text` of the whole subtree, so a fluent
/// chain shipped its own source — newlines, indentation and string
/// literals included — as the callee name, and from there as an entity
/// name into every MCP response and UI payload. Nine such names came out
/// of one builder file in `data/python`.
///
/// Each arm answers "what names the value this receiver evaluates to?":
///
/// - `self.repo.find()` — the receiver is `self.repo`; its last segment,
///   `repo`, is the part a type can be attached to.
/// - `builder.set_size(...).set_dough()` — the receiver is the *result* of
///   `set_size`, so the link is named after the method that produced it.
///   Multi-line and single-line chains reduce identically, since this
///   reads the tree rather than the text.
/// - `items[0].name()` — a subscript yields an element of `items`.
/// - `"a.b".split(".")` — a literal receiver is named by its type, which
///   keeps the literal's dots and quotes out of the graph entirely.
///
/// Returning `None` drops the call edge, which only happens for receivers
/// that no single name describes (`(a + b).foo()`). Those produced garbage
/// names before; a missing edge is the honest outcome and matches how the
/// resolver already declines rather than guesses elsewhere.
fn receiver_name(node: &Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node_text(node, source).to_string()),
        "attribute" => node
            .child_by_field_name("attribute")
            .map(|a| node_text(&a, source).to_string()),
        "call" => {
            let function = node.child_by_field_name("function")?;
            // `super()` keeps its call form: `attribute_callee` matches on it
            // to rewrite the callee onto the enclosing class.
            match receiver_name(&function, source) {
                Some(name) if name == "super" => Some("super()".to_string()),
                other => other,
            }
        }
        "subscript" => node
            .child_by_field_name("value")
            .and_then(|v| receiver_name(&v, source)),
        "parenthesized_expression" | "await" => {
            node.named_child(0).and_then(|c| receiver_name(&c, source))
        }
        kind => literal_type_name(kind).map(String::from),
    }
}

/// The Python type a literal receiver evaluates to, so `[1, 2].append(x)`
/// reads as `list.append`. Unknown kinds return `None`, which
/// `receiver_name` turns into "no edge".
fn literal_type_name(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "string" | "concatenated_string" => "str",
        "integer" => "int",
        "float" => "float",
        "true" | "false" => "bool",
        "none" => "None",
        "list" | "list_comprehension" => "list",
        "dictionary" | "dictionary_comprehension" => "dict",
        "set" | "set_comprehension" => "set",
        "tuple" => "tuple",
        _ => return None,
    })
}

/// Emit a `WritesTo` edge for `self.<ident> = …` assignments inside
/// a method body. The target is formatted `{ClassName}.{attr}` —
/// matching how `attribute_callee` resolves self-method calls — so the
/// resolver can link the write to the synthetic field Variable that
/// `create_field_entities` created from the parser's per-class
/// `fields` list. Unresolved targets fall through to ghosts the
/// same way call targets do.
fn handle_self_write(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(cls) = ctx.caller.parent_class else {
        return;
    };
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    if left.kind() != "attribute" {
        return;
    }
    let Some(obj) = left.child_by_field_name("object") else {
        return;
    };
    let Some(attr) = left.child_by_field_name("attribute") else {
        return;
    };
    if node_text(&obj, ctx.caller.source) != "self" {
        return;
    }
    let field_name = node_text(&attr, ctx.caller.source);
    if field_name.is_empty() {
        return;
    }
    let target = format!("{}.{}", cls, field_name);
    let rel = edge(
        ctx.caller.id,
        target,
        RelationshipKind::WritesTo,
        current_branch,
    );
    ctx.walk.result.add_relationship(rel);
}

/// One identifier introduced by an assignment LHS. Carries the source
/// name plus a `splat` flag for the `*tail` arm of a starred unpack
/// (`head, *tail = xs`) — useful in the UI to show that this binding
/// captures the rest of an iterable rather than a single element.
struct LocalTarget {
    name: String,
    splat: bool,
}

/// Push a `LocalTarget` for `node` if it's a non-empty bare identifier
/// other than `self`/`cls`. Shared by the identifier and splat arms of
/// `collect_local_targets` so the validation lives in one place.
fn push_target_if_named(node: &Node, source: &str, splat: bool, out: &mut Vec<LocalTarget>) {
    if node.kind() != "identifier" {
        return;
    }
    let name = node_text(node, source).to_string();
    if name.is_empty() || name == "self" || name == "cls" {
        return;
    }
    out.push(LocalTarget { name, splat });
}

/// Walk an assignment LHS pattern and append every leaf identifier it
/// introduces to `out`. Handles single names, tuple/list-style
/// `pattern_list` and `tuple_pattern`, and the `*tail` form
/// (`list_splat_pattern`). Subscript / attribute LHS are mutations on
/// existing values, not declarations, so they're skipped.
fn collect_local_targets(pattern: &Node, source: &str, out: &mut Vec<LocalTarget>) {
    match pattern.kind() {
        "identifier" => push_target_if_named(pattern, source, false, out),
        "pattern_list" | "tuple_pattern" => {
            let mut cursor = pattern.walk();
            for child in pattern.named_children(&mut cursor) {
                collect_local_targets(&child, source, out);
            }
        }
        "list_splat_pattern" => {
            if let Some(inner) = pattern.named_child(0) {
                push_target_if_named(&inner, source, true, out);
            }
        }
        _ => {}
    }
}

/// Route one assignment target to the binding it actually mutates.
///
/// PY-009: `global x` and `nonlocal x` say that a write inside this function
/// targets an *outer* binding. Without reading them, every such write minted
/// a `func::local::x` — a variable that does not exist, shadowing the real
/// module-level or enclosing-function one and hiding the mutation that makes
/// the function stateful.
fn emit_write(
    target: &LocalTarget,
    span_node: &Node,
    rhs_text: Option<&str>,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    match outer_scope_declaration(span_node, &target.name, ctx.caller.source) {
        Some(scope) => emit_outer_write(&target.name, scope, ctx, current_branch),
        None => emit_local_write(target, span_node, rhs_text, ctx, current_branch),
    }
}

/// A `WritesTo` edge against the bare name, for a write the function declared
/// `global` or `nonlocal`.
///
/// The target is the plain name rather than a scoped id, so it resolves the
/// way every other bare name in this parser does — locality-ranked against
/// the module-level entity for `global`, and against the enclosing function's
/// local for `nonlocal`. The `scope` metadata keeps the distinction the two
/// keywords make, which no id shape could carry.
fn emit_outer_write(
    name: &str,
    scope: &'static str,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    let mut rel = edge(
        ctx.caller.id,
        name.to_string(),
        RelationshipKind::WritesTo,
        current_branch,
    );
    rel.metadata.insert("scope".to_string(), scope.to_string());
    ctx.walk.result.add_relationship(rel);
}

/// Did the enclosing function declare `name` as `global` or `nonlocal`?
///
/// Answered by looking, rather than by threading a per-scope set through
/// `extract_calls` — the declaration can sit anywhere in the function
/// (including inside an `if`), so a set would have to be gathered in a
/// pre-pass anyway, and the walk up is cheap on function-sized bodies.
///
/// A write at module scope has no enclosing function and no outer binding to
/// target, so it answers `None`.
fn outer_scope_declaration(node: &Node, name: &str, source: &str) -> Option<&'static str> {
    let mut current = *node;
    let function = loop {
        let parent = current.parent()?;
        if parent.kind() == "function_definition" {
            break parent;
        }
        current = parent;
    };
    let body = function.child_by_field_name("body")?;
    find_scope_declaration(&body, name, source)
}

/// Scan a function body for `global name` / `nonlocal name`, without
/// descending into nested definitions — an inner function's `global` is its
/// own business.
fn find_scope_declaration(node: &Node, name: &str, source: &str) -> Option<&'static str> {
    if matches!(
        node.kind(),
        "function_definition" | "class_definition" | "lambda"
    ) {
        return None;
    }
    let keyword = match node.kind() {
        "global_statement" => Some("global"),
        "nonlocal_statement" => Some("nonlocal"),
        _ => None,
    };
    if let Some(keyword) = keyword {
        let mut cursor = node.walk();
        if node
            .named_children(&mut cursor)
            .any(|c| node_text(&c, source) == name)
        {
            return Some(keyword);
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_scope_declaration(&child, name, source) {
            return Some(found);
        }
    }
    None
}

/// Emit a synthetic Variable entity (first-write-wins) plus a
/// `WritesTo` edge for one local target. Shared by `handle_local_write`
/// and `handle_walrus_write` so both `x = compute()` and `(x :=
/// compute())` produce the same shape of node.
fn emit_local_write(
    target: &LocalTarget,
    span_node: &Node,
    rhs_text: Option<&str>,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    let local_id = format!("{}::local::{}", ctx.caller.id, target.name);
    let already_exists = ctx.walk.result.entities.iter().any(|e| e.id == local_id);
    if !already_exists {
        let entity = local_entity(target, span_node, rhs_text, ctx.caller.id, ctx.caller.path);
        ctx.walk.result.add_entity(entity);
    }
    let rel = edge(
        ctx.caller.id,
        local_id,
        RelationshipKind::WritesTo,
        current_branch,
    );
    ctx.walk.result.add_relationship(rel);
}

/// The synthetic node one local binding gets: private, parented on the
/// callable that binds it, and documented with the expression it was bound
/// from so the detail panel can show what the name holds.
fn local_entity(
    target: &LocalTarget,
    span_node: &Node,
    rhs_text: Option<&str>,
    caller_id: &str,
    path: &Path,
) -> CodeEntity {
    let span = crate::parser::language_parser::node_to_span(span_node);
    let mut entity = CodeEntity::new(&target.name, EntityKind::Variable, path, span);
    entity.id = format!("{}::local::{}", caller_id, target.name);
    entity.qualified_name = format!("{}::{}", caller_id, target.name);
    entity.parent_id = Some(caller_id.to_string());
    entity.tags.insert("local_var".to_string());
    if target.splat {
        entity.tags.insert("splat".to_string());
    }
    entity.visibility = Visibility::Private;
    if let Some(rhs) = rhs_text.filter(|r| !r.is_empty()) {
        entity.documentation = Some(format!("= {}", rhs));
    }
    entity
}

/// Emit a synthetic local-variable entity + `WritesTo` edge for a
/// `<lhs> = …` or `<lhs> += …` assignment inside a function body.
/// Locals are promoted to graph nodes so the reader can see what a
/// method computes, not just what it calls.
///
/// LHS shapes handled: bare identifier, `pattern_list` / `tuple_pattern`
/// (`a, b = …`, `(a, b) = …`), and `list_splat_pattern` (`head, *tail
/// = xs`). Each leaf identifier becomes its own write — first-write-
/// wins on the entity, one `WritesTo` edge per target. Subscript /
/// attribute-chain targets are mutations, not declarations.
///
/// Chained assignment (`x = y = compute()`) is handled by walking the
/// nested `assignment` chain along the `right` field and collecting
/// every LHS along the way. The innermost RHS provides the shared
/// documentation. To avoid double-emission when the call walk
/// recursively descends into the inner assignment, we early-out when
/// the parent node is itself an assignment — the outer call already
/// covered every target.
fn handle_local_write(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(parent) = node.parent() {
        if matches!(parent.kind(), "assignment" | "augmented_assignment") {
            return;
        }
    }
    let source = ctx.caller.source;

    let mut targets: Vec<LocalTarget> = Vec::new();
    let mut innermost_rhs: Option<Node> = None;
    let mut current = *node;
    loop {
        if let Some(left) = current.child_by_field_name("left") {
            collect_local_targets(&left, source, &mut targets);
        }
        let Some(right) = current.child_by_field_name("right") else {
            break;
        };
        if matches!(right.kind(), "assignment" | "augmented_assignment") {
            current = right;
        } else {
            innermost_rhs = Some(right);
            break;
        }
    }

    let rhs_text = innermost_rhs.map(|r| node_text(&r, source).to_string());
    for target in &targets {
        emit_write(target, node, rhs_text.as_deref(), ctx, current_branch);
    }
}

/// Walrus form: `(n := compute())`. The grammar guarantees `name` is
/// a bare identifier — no tuple unpacking, no chaining — so this is a
/// thin wrapper around `emit_local_write` with the named_expression's
/// `value` field as the RHS for documentation.
fn handle_walrus_write(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let source = ctx.caller.source;
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    if name_node.kind() != "identifier" {
        return;
    }
    let name = node_text(&name_node, source).to_string();
    if name.is_empty() || name == "self" || name == "cls" {
        return;
    }
    let target = LocalTarget { name, splat: false };
    let rhs_text = node
        .child_by_field_name("value")
        .map(|v| node_text(&v, source).to_string());
    emit_write(&target, node, rhs_text.as_deref(), ctx, current_branch);
}

/// Treat a `lambda` expression as a small anonymous function. Without
/// this, calls inside the body (`sorted(xs, key=lambda x: x.lower())`)
/// would attribute `.lower()` to the outer caller — wrong, since the
/// lambda owns its own scope and its own return value.
///
/// Emits a synthetic `Function` entity named `<lambda@line:col>` with
/// the enclosing callable as its parent, then recurses `extract_calls`
/// into the body with the lambda as the new caller (and
/// `current_branch` reset to None — the lambda is its own scope, not
/// part of the outer control-flow path). `parent_class` carries
/// through so `self.foo()` calls inside a lambda defined in a method
/// still resolve to the surrounding class.
fn handle_lambda(node: &Node, ctx: &mut CallCtx<'_>) {
    let span = crate::parser::language_parser::node_to_span(node);
    let line = span.start.line + 1;
    let col = span.start.column + 1;
    let name = format!("<lambda@{}:{}>", line, col);
    let lambda_id = format!("{}::lambda::{}:{}", ctx.caller.id, line, col);

    let mut entity = CodeEntity::new(&name, EntityKind::Function, ctx.caller.path, span);
    entity.id = lambda_id.clone();
    entity.qualified_name = format!("{}::{}", ctx.caller.id, name);
    entity.parent_id = Some(ctx.caller.id.to_string());
    entity.tags.insert("lambda".to_string());
    entity.visibility = Visibility::Private;
    entity.source_code = Some(node_text(node, ctx.caller.source).to_string());
    populate_lambda_metrics(node, ctx.caller.source, &mut entity);
    ctx.walk.result.add_entity(entity);

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    // The lambda is its own caller, with its own call ordinal sequence.
    // A lambda reads the enclosing body's names, so the receiver tables
    // carry through — as does the class, for the `self.foo()` reason above.
    let inner = Caller {
        source: ctx.caller.source,
        path: ctx.caller.path,
        id: &lambda_id,
        name: &name,
        parent_class: ctx.caller.parent_class,
        locals: ctx.caller.locals,
        returns: ctx.caller.returns,
    };
    let mut inner_ctx = CallCtx::new(inner, ctx.walk.result);
    extract_calls(&body, &mut inner_ctx, None);
}
