//! Call-site extraction for TypeScript function/method bodies.
//!
//! Walks the AST emitting `Calls` / `Instantiates` relationships and
//! synthesising `Branch` / `Loop` entities for the arms of `if` / `switch` /
//! `try` / `for` / `for…of` / `while` / `do…while` (TS-002), so the UI's
//! decision-tree rendering treats TypeScript like Java and Python. Every
//! relationship emitted inside a flow arm carries a `branch` metadata key
//! holding the arm's path (`c1`, `c2.l1`, …) so the analyzer re-sources it
//! onto the synthetic node.
//!
//! Receivers are typed through [`super::inference`] where the file's
//! declarations reach: `this.api.run()` becomes `ApiClient.run` rather than
//! the unmatched `this.api.run`.

use super::super::ctx::ExtractCtx;
use super::flow::{
    emit_branch_entity, emit_case_arm_entity, emit_loop_entity, emit_try_arm_entity,
};
use super::inference::{infer_local_types, infer_param_types, TypeEnv};
use super::stdlib::is_stdlib_function;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{find_child_by_kind, node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Build the type environment for one callable and walk its body.
///
/// The single entry point every callable kind uses — top-level functions,
/// class methods and arrow-function constants — so the inference wiring
/// cannot drift between them. Parameters are inserted first and locals
/// second, so a `const` shadowing a parameter wins.
pub(in crate::parser::typescript) fn extract_body_calls(
    body: &Node,
    caller_id: &str,
    caller_name: &str,
    parameters: Option<Node>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let mut locals = parameters
        .map(|p| infer_param_types(&p, ctx.source))
        .unwrap_or_default();
    locals.extend(infer_local_types(body, ctx.source));
    let env = TypeEnv {
        locals,
        members: ctx.members,
        self_type,
    };
    let mut call_order = 0u32;
    let mut arm_counter = 0u32;
    let mut loop_counter = 0u32;
    let mut call_ctx = CallCtx {
        source: ctx.source,
        path: ctx.path,
        caller_id,
        caller_name,
        env: &env,
        call_order: &mut call_order,
        arm_counter: &mut arm_counter,
        loop_counter: &mut loop_counter,
        result: &mut *ctx.result,
    };
    extract_calls(body, &mut call_ctx, None);
}

/// Context threaded through `extract_calls` and its helpers: the caller's
/// identity and type environment, plus the mutable walk state.
pub(super) struct CallCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub caller_id: &'a str,
    pub caller_name: &'a str,
    pub env: &'a TypeEnv<'a>,
    pub call_order: &'a mut u32,
    pub arm_counter: &'a mut u32,
    pub loop_counter: &'a mut u32,
    pub result: &'a mut ParseResult,
}

/// Captures that the immediately-enclosing call or instantiation has its
/// result stored into a named variable. Rides the edge as metadata so
/// downstream views can render the binding without a node per local.
struct Binding {
    name: String,
    declared_type: Option<String>,
    is_reassignment: bool,
}

/// Branch path for an arm at the given nesting level. Top-level arms read as
/// `c1`, `c2`; nested ones append `.c<idx>` so ancestry reads left-to-right.
fn branch_path(current_branch: Option<&str>, idx: u32) -> String {
    match current_branch {
        Some(p) => format!("{}.c{}", p, idx),
        None => format!("c{}", idx),
    }
}

/// Loop-path counterpart of `branch_path`, using the `l` prefix so a sibling
/// decision tree and loop at one scope don't collide on a shared counter.
fn loop_path(current_branch: Option<&str>, idx: u32) -> String {
    match current_branch {
        Some(p) => format!("{}.l{}", p, idx),
        None => format!("l{}", idx),
    }
}

/// Walk a function/method body emitting call edges and flow entities.
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
/// (including any recursion it needed) — the caller then skips the default
/// child-walk.
fn dispatch_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    if dispatch_flow_node(node, ctx, current_branch) {
        return true;
    }
    match node.kind() {
        "call_expression" => {
            handle_call_expression(node, ctx, None, current_branch);
            walk_call_children(node, ctx, current_branch);
            true
        }
        "new_expression" => {
            handle_new_expression(node, ctx, None, current_branch);
            // Fall through to the generic walk so calls in the argument list
            // still emit edges.
            false
        }
        "lexical_declaration" | "variable_declaration" => {
            handle_variable_declaration(node, ctx, current_branch);
            true
        }
        "assignment_expression" => {
            handle_assignment_expression(node, ctx, current_branch);
            true
        }
        _ => false,
    }
}

/// Flow-control dispatch. Returns `true` if the node was handled, mirroring
/// the `dispatch_node` contract. `ternary_expression` is deliberately absent:
/// its arms recurse through the generic walk so their calls still emit edges,
/// but a one-expression arm is not worth a graph node.
fn dispatch_flow_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    match node.kind() {
        "if_statement" => handle_if_statement(node, ctx, current_branch),
        "try_statement" => handle_try_statement(node, ctx, current_branch),
        "switch_statement" => handle_switch_statement(node, ctx, current_branch),
        "for_statement" => handle_classic_for(node, ctx, current_branch),
        "for_in_statement" => handle_for_in(node, ctx, current_branch),
        "while_statement" => handle_while(node, ctx, current_branch),
        "do_statement" => handle_do_while(node, ctx, current_branch),
        _ => return false,
    }
    true
}

/// Recurse into the parts of a `call_expression` other than the callee name:
/// the receiver expression and the argument list. Done explicitly so the
/// callee identifier isn't re-visited and double-counted.
fn walk_call_children(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(func) = node.child_by_field_name("function") {
        match func.kind() {
            // The property identifier is not itself a call; only the object
            // side can contain one (`getClient().run()`).
            "member_expression" => {
                if let Some(obj) = func.child_by_field_name("object") {
                    extract_calls(&obj, ctx, current_branch);
                }
            }
            "identifier" => {}
            // `(cb)()`, `handlers[k]()`, `foo()()` — walk the whole callee.
            _ => extract_calls(&func, ctx, current_branch),
        }
    }
    if let Some(args) = node.child_by_field_name("arguments") {
        extract_calls(&args, ctx, current_branch);
    }
}

/// `const x: T = rhs` / `let x = rhs` (one or more declarators). When `rhs` is
/// a direct call or `new`, that edge is tagged `binds_to` + `binds_type`.
fn handle_variable_declaration(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let Some(value) = child.child_by_field_name("value") else {
            continue;
        };
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let binding = Binding {
            name: node_text(&name_node, ctx.source).to_string(),
            declared_type: child
                .child_by_field_name("type")
                .map(|t| super::super::helpers::extract_type_text(&t, ctx.source)),
            is_reassignment: false,
        };
        emit_with_binding(&value, ctx, &binding, current_branch);
    }
}

/// `lhs = rhs`. The grammar keeps compound forms (`+=`) in a separate
/// `augmented_assignment_expression` node, so every `assignment_expression`
/// is a plain rebinding. The declared type is unknown at a reassignment site,
/// so only `rebinds_to` is recorded.
fn handle_assignment_expression(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return;
    };
    extract_calls(&left, ctx, current_branch);
    let binding = Binding {
        name: node_text(&left, ctx.source).to_string(),
        declared_type: None,
        is_reassignment: true,
    };
    emit_with_binding(&right, ctx, &binding, current_branch);
}

/// Apply `binding` to the outermost call/`new` in `value`, then keep walking
/// so nested calls still emit plain edges.
fn emit_with_binding(
    value: &Node,
    ctx: &mut CallCtx<'_>,
    binding: &Binding,
    current_branch: Option<&str>,
) {
    match value.kind() {
        "call_expression" => {
            handle_call_expression(value, ctx, Some(binding), current_branch);
            walk_call_children(value, ctx, current_branch);
        }
        "new_expression" => {
            handle_new_expression(value, ctx, Some(binding), current_branch);
            let mut cursor = value.walk();
            for child in value.children(&mut cursor) {
                extract_calls(&child, ctx, current_branch);
            }
        }
        // `await foo()` / `(await foo())` — unwrap so the binding still
        // reaches the call it is really binding.
        "await_expression" | "parenthesized_expression" | "non_null_expression" => {
            match value.named_child(0) {
                Some(inner) => emit_with_binding(&inner, ctx, binding, current_branch),
                None => extract_calls(value, ctx, current_branch),
            }
        }
        _ => extract_calls(value, ctx, current_branch),
    }
}

fn handle_call_expression(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let Some(func) = node.child_by_field_name("function") else {
        return;
    };
    let Some((callee, member)) = callee_of(&func, ctx) else {
        return;
    };
    *ctx.call_order += 1;
    if member == ctx.caller_name {
        return;
    }
    // The stdlib filter normally drops common names (`map`, `filter`, `then`)
    // to keep the graph readable. Skip it when the result is bound to a
    // variable — the binding shows the caller cares about the value, so the
    // edge is structurally relevant. Same rule as the Java parser.
    if binding.is_none() && is_stdlib_function(&member) {
        return;
    }
    let mut rel = Relationship::new(ctx.caller_id.to_string(), callee, RelationshipKind::Calls);
    tag_edge(&mut rel, ctx.call_order, binding, current_branch);
    ctx.result.add_relationship(rel);
}

fn handle_new_expression(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let Some(ctor) = node.child_by_field_name("constructor") else {
        return;
    };
    let type_name = node_text(&ctor, ctx.source).to_string();
    *ctx.call_order += 1;
    let mut rel = Relationship::new(
        ctx.caller_id.to_string(),
        type_name,
        RelationshipKind::Instantiates,
    );
    tag_edge(&mut rel, ctx.call_order, binding, current_branch);
    ctx.result.add_relationship(rel);
}

/// Attach the order, binding and enclosing-branch metadata every emitted edge
/// carries, so the three call sites can't drift apart.
fn tag_edge(
    rel: &mut Relationship,
    call_order: &u32,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    rel.metadata
        .insert("order".to_string(), call_order.to_string());
    if let Some(b) = binding {
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
    if let Some(b) = current_branch {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
}

/// `(callee as the resolver will see it, bare member name)` for a call's
/// callee expression, or `None` for a shape that names no callee.
///
/// A receiver-less `foo()` stays **unqualified**. In TypeScript a bare call
/// inside a method is a module-level function, not a sibling method — those
/// need `this.`. Qualifying it with the enclosing class produced `Class.foo`,
/// which misses `typed_method_to_id` and is then dropped rather than retried
/// as a bare name, so every call to an imported helper vanished from the
/// graph.
fn callee_of(func: &Node, ctx: &CallCtx<'_>) -> Option<(String, String)> {
    match func.kind() {
        "identifier" => {
            let name = node_text(func, ctx.source).to_string();
            Some((name.clone(), name))
        }
        "member_expression" => {
            let property = func.child_by_field_name("property")?;
            let member = node_text(&property, ctx.source).to_string();
            let object = func.child_by_field_name("object")?;
            let receiver = node_text(&object, ctx.source);
            Some((qualify_member(ctx.env, receiver, &member), member))
        }
        _ => None,
    }
}

/// Qualify `receiver.member` with the receiver's type where the file's
/// declarations reach it.
///
/// Three outcomes, in order:
///
/// 1. The receiver types → `Type.member`, which the resolver looks up like
///    any other qualified callee.
/// 2. The receiver is a plain dotted name we simply couldn't type →
///    `receiver.member`, unchanged. It lands on a ghost: an unresolved
///    receiver stays visibly unresolved rather than collapsing onto a
///    same-named method somewhere else in the tree.
/// 3. The receiver is an *expression* — a fluent chain, a cast, an index —
///    → `<nearest named object>.member`, e.g. `nodeSel.select('c').attr(…)`
///    becomes `nodeSel.attr`.
///
/// Case 3 exists because shipping the expression text verbatim mints a
/// hundred-character entity name, and a fresh one per call site: on this
/// repo's own `ui/` tree that single shape was 40% of all ghosts, each a
/// unique node. Reducing to the nearest named object collapses them onto one
/// stable ghost per (object, member) pair.
///
/// The result stays *qualified* on purpose. Falling back to the bare `member`
/// would let the resolver's bare-name lookup collapse `d3.select(x).attr()`
/// onto any project function called `attr` — measured on `ui/`, that bought
/// 41 edges whose targets were `on`, `select`, `text`, `data` and `id`, which
/// is a precision loss dressed up as recall.
fn qualify_member(env: &TypeEnv<'_>, receiver: &str, member: &str) -> String {
    let compact: String = receiver.chars().filter(|c| !c.is_whitespace()).collect();
    if compact == "super" {
        return match env.self_type {
            Some(ty) => format!("{}.{}", ty, member),
            None => member.to_string(),
        };
    }
    if let Some(ty) = env.resolve_receiver(&compact) {
        return format!("{}.{}", ty, member);
    }
    if is_dotted_name(&compact) {
        return format!("{}.{}", compact, member);
    }
    match nearest_named_object(&compact) {
        Some(object) => format!("{}.{}", object, member),
        // Nothing in the receiver is even a name (`(await f()).g()`). The
        // bare member is all that's left; it is at least an identifier.
        None => member.to_string(),
    }
}

/// Is this receiver text a plain dotted identifier path (`a`, `a.b.c`)?
fn is_dotted_name(text: &str) -> bool {
    !text.is_empty() && text.split('.').all(is_identifier)
}

fn is_identifier(segment: &str) -> bool {
    segment.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$')
        && segment
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// The last plainly-named object in a receiver expression: the innermost
/// thing the chain is anchored to that we can still name. `nodeSel.select(…)`
/// → `nodeSel`; `simulation?.alpha(0.3)` → `simulation` (the optional-chain
/// marker is not part of the name).
fn nearest_named_object(receiver: &str) -> Option<String> {
    receiver
        .split('.')
        .map(|segment| segment.trim_end_matches('?'))
        .rfind(|segment| is_identifier(segment))
        .map(str::to_string)
}

/// Bump the arm counter, emit a Branch entity for `body`, and return its path.
fn emit_arm(ctx: &mut CallCtx<'_>, current_branch: Option<&str>, body: &Node) -> String {
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
    path
}

/// Bump the loop counter, emit a Loop entity for `body`, and return its path.
fn emit_loop(ctx: &mut CallCtx<'_>, current_branch: Option<&str>, body: &Node) -> String {
    *ctx.loop_counter += 1;
    let path = loop_path(current_branch, *ctx.loop_counter);
    emit_loop_entity(
        ctx.caller_id,
        current_branch,
        &path,
        body,
        ctx.path,
        ctx.result,
    );
    path
}

/// Per-arm dispatch for `if (c) {} else if (c2) {} else {}`.
///
/// The grammar nests a chained `else if` inside an `else_clause`, but the
/// user-visible model is a flat list of arms, so the chain is flattened here
/// and siblings read as `c1` / `c2` / `c3`. Conditions stay in the *outer*
/// branch — they are evaluated before any arm runs.
fn handle_if_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut current = Some(*node);
    while let Some(if_node) = current {
        if let Some(cond) = if_node.child_by_field_name("condition") {
            extract_calls(&cond, ctx, current_branch);
        }
        if let Some(body) = if_node.child_by_field_name("consequence") {
            let path = emit_arm(ctx, current_branch, &body);
            extract_calls(&body, ctx, Some(&path));
        }
        let alternative = if_node
            .child_by_field_name("alternative")
            .and_then(|clause| else_body(&clause));
        current = match alternative {
            // `else if (…)` — continue the chain as a sibling arm.
            Some(next) if next.kind() == "if_statement" => Some(next),
            // Terminal `else { … }`.
            Some(body) => {
                let path = emit_arm(ctx, current_branch, &body);
                extract_calls(&body, ctx, Some(&path));
                None
            }
            None => None,
        };
    }
}

/// The statement an `else_clause` wraps. The grammar shape is
/// `else_clause → 'else' statement`, so the first named child is the body.
fn else_body<'a>(clause: &Node<'a>) -> Option<Node<'a>> {
    if clause.kind() == "else_clause" {
        clause.named_child(0)
    } else {
        // Defensive: a grammar that puts the statement directly in the
        // `alternative` field still works.
        Some(*clause)
    }
}

/// Per-arm dispatch for `try { } catch (e) { } finally { }`.
///
/// The try body is the first arm (tagged `try_body_arm`), then the catch and
/// finally clauses. TypeScript catches carry a binding rather than an
/// exception type, so the binding text is what travels as `caught:`.
fn handle_try_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(body) = node.child_by_field_name("body") {
        let path = emit_arm_tagged(ctx, current_branch, &body, "try_body", None);
        extract_calls(&body, ctx, Some(&path));
    }
    if let Some(handler) = node.child_by_field_name("handler") {
        let caught = catch_binding_text(&handler, ctx.source);
        if let Some(body) = handler.child_by_field_name("body") {
            let path = emit_arm_tagged(ctx, current_branch, &body, "catch", caught.as_deref());
            extract_calls(&body, ctx, Some(&path));
        }
    }
    if let Some(finalizer) = node.child_by_field_name("finalizer") {
        let body = finalizer
            .child_by_field_name("body")
            .or_else(|| find_child_by_kind(&finalizer, "statement_block"));
        if let Some(body) = body {
            let path = emit_arm_tagged(ctx, current_branch, &body, "finally", None);
            extract_calls(&body, ctx, Some(&path));
        }
    }
}

/// `emit_arm` for a try/catch/finally arm, layering the try-specific tags on.
fn emit_arm_tagged(
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    body: &Node,
    arm_kind: &str,
    caught: Option<&str>,
) -> String {
    *ctx.arm_counter += 1;
    let path = branch_path(current_branch, *ctx.arm_counter);
    emit_try_arm_entity(
        ctx.caller_id,
        current_branch,
        &path,
        body,
        ctx.path,
        arm_kind,
        caught,
        ctx.result,
    );
    path
}

/// Text of a `catch_clause`'s bound parameter (`catch (err: unknown)` →
/// `err: unknown`), or `None` for the bare `catch { … }` form.
fn catch_binding_text(clause: &Node, source: &str) -> Option<String> {
    let parameter = clause.child_by_field_name("parameter")?;
    let mut text = node_text(&parameter, source).to_string();
    if let Some(type_ann) = clause.child_by_field_name("type") {
        text.push_str(node_text(&type_ann, source));
    }
    Some(text)
}

/// Per-arm dispatch for `switch (v) { case A: …; default: … }`.
///
/// One arm per `switch_case` / `switch_default`, matching how the Java parser
/// emits one per `switch_label`: multi-label fall-through then shows every
/// label as an intentional branch rather than collapsing them. The subject
/// expression stays in the outer branch — it runs once, before any arm.
fn handle_switch_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(value) = node.child_by_field_name("value") {
        extract_calls(&value, ctx, current_branch);
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for arm in body.children(&mut cursor) {
        match arm.kind() {
            "switch_case" => handle_switch_arm(&arm, ctx, current_branch, true),
            "switch_default" => handle_switch_arm(&arm, ctx, current_branch, false),
            _ => {}
        }
    }
}

/// Emit one case/default arm and walk its statements under that arm's path.
/// The `case <expr>:` value travels as the arm's `pattern:` attribute; it is
/// evaluated as part of the decision, so it belongs to the arm.
fn handle_switch_arm(
    arm: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    has_value: bool,
) {
    let pattern = has_value
        .then(|| arm.child_by_field_name("value"))
        .flatten()
        .map(|v| node_text(&v, ctx.source).to_string());
    *ctx.arm_counter += 1;
    let path = branch_path(current_branch, *ctx.arm_counter);
    emit_case_arm_entity(
        ctx.caller_id,
        current_branch,
        &path,
        arm,
        ctx.path,
        pattern.as_deref(),
        ctx.result,
    );
    let mut cursor = arm.walk();
    for child in arm.children(&mut cursor) {
        extract_calls(&child, ctx, Some(&path));
    }
}

/// Classic `for (init; cond; update) { body }`. All three header expressions
/// run as part of the iteration cycle, so their calls group under the loop.
fn handle_classic_for(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let path = emit_loop(ctx, current_branch, &body);
    for field in ["initializer", "condition", "increment"] {
        if let Some(part) = node.child_by_field_name(field) {
            extract_calls(&part, ctx, Some(&path));
        }
    }
    extract_calls(&body, ctx, Some(&path));
}

/// `for (const x of xs)` and `for (const k in o)` — one grammar node for both.
/// The iterated expression's calls group under the loop, mirroring how Java
/// treats the enhanced-for's value and Python its `for … in`.
fn handle_for_in(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let path = emit_loop(ctx, current_branch, &body);
    if let Some(right) = node.child_by_field_name("right") {
        extract_calls(&right, ctx, Some(&path));
    }
    extract_calls(&body, ctx, Some(&path));
}

/// `while (cond) { body }`. The condition runs on every iteration, so its
/// calls group under the loop too.
fn handle_while(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let path = emit_loop(ctx, current_branch, &body);
    if let Some(cond) = node.child_by_field_name("condition") {
        extract_calls(&cond, ctx, Some(&path));
    }
    extract_calls(&body, ctx, Some(&path));
}

/// `do { body } while (cond)`. The condition runs after each iteration; call
/// attribution doesn't depend on that ordering, so this is structurally
/// identical to `handle_while`.
fn handle_do_while(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    handle_while(node, ctx, current_branch);
}
