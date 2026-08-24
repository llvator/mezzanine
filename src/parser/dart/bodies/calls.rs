//! Call-site extraction for Dart callable bodies.
//!
//! Walks a body emitting `Calls` / `Instantiates` relationships and
//! synthesising `Branch` / `Loop` entities for the arms of `if` / `switch` /
//! `try` / `for` / `while` / `do-while`, so the UI's decision-tree rendering
//! treats Dart the way it treats Java, Groovy and Python. Every relationship
//! emitted inside an arm carries a `branch` metadata key pointing at the
//! arm's path (`c1`, `c2.l1`), which is how the analyzer reattributes it to
//! the synthetic node.
//!
//! A call is a `call_expression` wrapping a callee and its `arguments`, and
//! the callee is either a bare `identifier` (`settle()`) or a
//! `member_expression` / `null_aware_member_expression` (`client.get`,
//! `repo?.find`). The one place that needs care is the *receiver*: the
//! grammar gives `this` and `super` as anonymous tokens, so the receiver is
//! read as the source text in front of the method name rather than as a
//! node. That also makes a chain fall out for free — in `a.b().c()` the
//! second call's receiver reads as `a.b()`, which is exactly what the Java
//! extractor produces for the same expression.
//!
//! A constructor is called without `new` in modern Dart, so a bare
//! uppercase-initial call is read as an instantiation — the rule the Kotlin
//! extractor uses for the same reason.
//!
//! Local function declarations are *not* skipped. A function declared inside
//! a body gets no entity of its own from this parser, so treating it as a
//! nested definition would drop its calls entirely; attributing them to the
//! enclosing callable is the honest alternative, and it is what closures
//! already do here and in the Java extractor.

use super::super::helpers::{child_of_kind, children_of_kind, declared_type};
use super::flow::{
    emit_branch_entity, emit_case_arm_entity, emit_loop_entity, emit_try_arm_entity,
};
use super::stdlib::is_core_method;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Who a call is attributed to. Fixed for the whole body and read-only, so a
/// helper that needs to know whose body it is walking takes this and cannot
/// reach the walk's counters.
pub(in crate::parser::dart) struct Caller<'a> {
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

/// Context threaded through [`extract_calls`] and its helpers. The two
/// halves are separate structs because they change for separate reasons: a
/// new counter is not a new thing to know about the caller, and only
/// [`Caller`] crosses the module edge.
pub(in crate::parser::dart) struct CallCtx<'a> {
    caller: Caller<'a>,
    walk: Walk<'a>,
}

impl<'a> CallCtx<'a> {
    /// Start a walk over one callable's body. The counters begin at zero per
    /// body, which is what makes an arm path (`c1`, `l1`) read relative to
    /// the callable rather than to the file.
    pub(in crate::parser::dart) fn new(caller: Caller<'a>, result: &'a mut ParseResult) -> Self {
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
    /// dropped call still burns its number and the edges that survive keep
    /// their source order.
    fn next_order(&mut self) -> u32 {
        self.walk.call_order += 1;
        self.walk.call_order
    }

    fn next_arm_path(&mut self, current_branch: Option<&str>) -> String {
        self.walk.arm_counter += 1;
        path_under(current_branch, 'c', self.walk.arm_counter)
    }

    /// Loop counterpart of [`Self::next_arm_path`]. Loops count separately so
    /// a sibling decision tree and loop at the same scope read as `c1` / `l1`
    /// rather than colliding on one counter.
    fn next_loop_path(&mut self, current_branch: Option<&str>) -> String {
        self.walk.loop_counter += 1;
        path_under(current_branch, 'l', self.walk.loop_counter)
    }
}

/// Captures that a call's result is stored into a named variable. Attached
/// as metadata on the edge so downstream views can render the binding
/// without adding a node per local.
struct Binding {
    name: String,
    declared_type: Option<String>,
    is_reassignment: bool,
}

/// Build an arm path at the given nesting level. Top-level arms read as `c1`
/// / `l1`; nested ones append `.c2` so ancestry is left-to-right.
fn path_under(current_branch: Option<&str>, prefix: char, idx: u32) -> String {
    match current_branch {
        Some(parent) => format!("{}.{}{}", parent, prefix, idx),
        None => format!("{}{}", prefix, idx),
    }
}

/// Walk a callable body, emitting relationships and control-flow arms.
pub(in crate::parser::dart) fn extract_calls(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    if dispatch_node(node, ctx, current_branch) {
        return;
    }
    walk_children(node, ctx, current_branch);
}

/// Per-kind handler. Returns `true` when the node was fully handled,
/// including any recursion it owed its children — the caller then skips the
/// default child-walk.
fn dispatch_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    if is_nested_definition(node.kind()) {
        return true;
    }
    if dispatch_flow_node(node, ctx, current_branch) {
        return true;
    }
    match node.kind() {
        "call_expression" | "cascade_call_expression" => {
            handle_call(node, ctx, None, current_branch);
            true
        }
        "new_expression" | "const_object_expression" => {
            handle_instantiation(node, ctx, None, current_branch);
            // Fall through to the generic walk so the arguments are seen.
            false
        }
        "initialized_variable_definition" | "initialized_identifier" => {
            handle_binding_definition(node, ctx, current_branch);
            true
        }
        "assignment_expression" => {
            handle_assignment(node, ctx, current_branch);
            true
        }
        _ => false,
    }
}

/// Declaration kinds that own their own scope, so the walk stops at them
/// rather than folding a nested type's members into the callable that
/// happens to contain it. Local *functions* are deliberately absent — see
/// the module header.
fn is_nested_definition(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "mixin_declaration"
            | "extension_declaration"
            | "extension_type_declaration"
            | "enum_declaration"
            | "type_alias"
            | "class_member"
    )
}

/// Flow-control dispatch. Returns `true` if the node was handled, mirroring
/// the [`dispatch_node`] contract. `conditional_expression` is not
/// special-cased: both of its branches still recurse through the generic
/// child-walk, so calls inside them emit edges without an arm entity.
fn dispatch_flow_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    match node.kind() {
        "if_statement" => handle_if(node, ctx, current_branch),
        "try_statement" => handle_try(node, ctx, current_branch),
        "switch_statement" => handle_switch(node, ctx, current_branch),
        "for_statement" | "while_statement" | "do_statement" => {
            handle_loop(node, ctx, current_branch)
        }
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------

/// Emit the edge for one `call_expression`, then walk what it contains.
fn handle_call(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let source = ctx.caller.source;
    let order = ctx.next_order();
    let callee = first_named_child(node);
    // A cascade names no receiver of its own — see [`cascade_receiver`].
    let implicit = cascade_receiver(node, source);

    match callee.map(|c| (c, c.kind())) {
        Some((callee, "identifier")) => {
            let name = node_text(&callee, source).to_string();
            emit_call(
                ctx,
                implicit.as_deref(),
                &name,
                binding,
                current_branch,
                order,
            );
        }
        Some((callee, "member_expression" | "null_aware_member_expression")) => {
            if let Some((receiver, name)) = split_member(&callee, source) {
                emit_call(
                    ctx,
                    receiver.as_deref(),
                    &name,
                    binding,
                    current_branch,
                    order,
                );
            }
            // The receiver may hold calls of its own — `a.b().c()`.
            walk_receiver(&callee, ctx, current_branch);
        }
        // A callee this parser cannot name — `(handlers[i])()`. The ordinal
        // is still spent, so surviving edges keep their source order.
        _ => {
            if let Some(callee) = callee {
                extract_calls(&callee, ctx, current_branch);
            }
        }
    }

    if let Some(arguments) = child_of_kind(node, "arguments") {
        extract_calls(&arguments, ctx, current_branch);
    }
}

/// The expression a cascade is applied to, for a `cascade_call_expression`.
///
/// `builder..open()..seal()` puts the receiver outside the cascade entirely:
/// it is the sibling the whole run of `cascade_section`s hangs off. So it is
/// found by climbing out of the section and walking back past any earlier
/// ones. Every other call kind carries its own receiver and gets `None`.
fn cascade_receiver(node: &Node, source: &str) -> Option<String> {
    if node.kind() != "cascade_call_expression" {
        return None;
    }
    let mut previous = node.parent()?.prev_sibling();
    while let Some(sibling) = previous {
        if sibling.kind() == "cascade_section" {
            previous = sibling.prev_sibling();
            continue;
        }
        if !sibling.is_named() {
            return None;
        }
        return Some(node_text(&sibling, source).to_string());
    }
    None
}

/// Split a member expression into the receiver's source text and the method
/// name.
///
/// The name is the last `identifier`; the receiver is everything written in
/// front of it, minus the `.` or `?.` that joins them. Reading the receiver
/// as text rather than as a node is what makes `this`, `super` and a nested
/// call all work — the first two are anonymous tokens, and the third is an
/// expression no single name describes.
fn split_member(expr: &Node, source: &str) -> Option<(Option<String>, String)> {
    let identifiers = children_of_kind(expr, "identifier");
    let name_node = identifiers.last()?;
    let name = node_text(name_node, source).to_string();

    let receiver = source[expr.start_byte()..name_node.start_byte()]
        .trim()
        .trim_end_matches('.')
        .trim_end_matches('?')
        .trim();

    // A receiver that spans lines is not a name — it is a closure or a
    // multi-line expression the call happens to hang off, and qualifying
    // with it would mint a ghost entity named after a block of source.
    // `buckets.sort((a, b) => … a.label.compareTo(b.label))` is the shape
    // that found this. Returning `None` drops the edge but not the walk:
    // the caller still recurses through the receiver, so the real calls
    // inside it are still found.
    if receiver.contains('\n') {
        return None;
    }
    Some(((!receiver.is_empty()).then(|| receiver.to_string()), name))
}

/// Walk everything in a member expression except the method name it ends on.
fn walk_receiver(expr: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let identifiers = children_of_kind(expr, "identifier");
    let name_id = identifiers.last().map(|n| n.id());
    let mut cursor = expr.walk();
    let children: Vec<Node> = expr.children(&mut cursor).collect();
    for child in children {
        if child.is_named() && Some(child.id()) != name_id {
            extract_calls(&child, ctx, current_branch);
        }
    }
}

/// Decide what kind of edge one call is, and record it.
fn emit_call(
    ctx: &mut CallCtx<'_>,
    receiver: Option<&str>,
    name: &str,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
    order: u32,
) {
    if name == ctx.caller.name {
        return;
    }
    // The core-library filter drops the names every collection and string in
    // the language answers to. It is skipped when the result is bound to a
    // local: the binding shows the caller cares about the value, so the edge
    // carries structure rather than noise.
    if binding.is_none() && is_core_method(name) {
        return;
    }

    let (target, kind) = match receiver {
        // A bare uppercase name is a constructor call, not a function.
        None if starts_upper(name) => (name.to_string(), RelationshipKind::Instantiates),
        None | Some("this") | Some("super") => (
            qualify(ctx.caller.parent_class, name),
            RelationshipKind::Calls,
        ),
        Some(receiver) => (format!("{}.{}", receiver, name), RelationshipKind::Calls),
    };

    push_relationship(ctx, target, kind, order, binding, current_branch);
}

/// `new Foo(…)` / `const Foo(…)`. Modern Dart drops both keywords, and those
/// calls are read as instantiations by [`emit_call`] instead — this covers
/// the explicit spelling, which is still legal and still common in older
/// code.
fn handle_instantiation(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let order = ctx.next_order();
    let Some(target) = instantiated_class(node, ctx.caller.source) else {
        return;
    };
    push_relationship(
        ctx,
        target,
        RelationshipKind::Instantiates,
        order,
        binding,
        current_branch,
    );
}

/// The class an explicit `new` / `const` expression builds.
///
/// The written type may carry a library prefix — `new http.Client()` — and
/// the prefix is an import alias, not something the graph holds an entity
/// for. Both parts arrive as `type_identifier` children of the one `type`
/// node, so the last is the class. A generic argument is nested a level
/// deeper and never shadows it, which is what leaves `List<Order>` reading
/// as `List`.
fn instantiated_class(node: &Node, source: &str) -> Option<String> {
    let type_node = child_of_kind(node, "type")?;
    let names = children_of_kind(&type_node, "type_identifier");
    names.last().map(|n| node_text(n, source).to_string())
}

fn push_relationship(
    ctx: &mut CallCtx<'_>,
    target: String,
    kind: RelationshipKind,
    order: u32,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let mut rel = Relationship::new(ctx.caller.id.to_string(), target, kind);
    rel.metadata.insert("order".to_string(), order.to_string());
    if let Some(b) = binding {
        let key = if b.is_reassignment {
            "rebinds_to"
        } else {
            "binds_to"
        };
        rel.metadata.insert(key.to_string(), b.name.clone());
        if let Some(declared) = &b.declared_type {
            rel.metadata
                .insert("binds_type".to_string(), declared.clone());
        }
    }
    if let Some(branch) = current_branch {
        rel.metadata
            .insert("branch".to_string(), branch.to_string());
    }
    ctx.walk.result.add_relationship(rel);
}

/// Qualify a receiver-less call with the enclosing class, so the resolver can
/// find a sibling method by `Class.method`.
fn qualify(parent_class: Option<&str>, name: &str) -> String {
    match parent_class {
        Some(class) => format!("{}.{}", class, name),
        None => name.to_string(),
    }
}

fn starts_upper(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

fn first_named_child<'t>(node: &Node<'t>) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|c| c.is_named());
    found
}

fn last_named_child<'t>(node: &Node<'t>) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).filter(|c| c.is_named()).last();
    found
}

fn walk_children(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls(&child, ctx, current_branch);
    }
}

// ---------------------------------------------------------------------
// Bindings
// ---------------------------------------------------------------------

/// `Type name = rhs` — one declarator. When `rhs` is a call or a `new`, that
/// edge is tagged `binds_to`, plus `binds_type` when a type was written.
fn handle_binding_definition(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let (Some(name_node), Some(value)) =
        (child_of_kind(node, "identifier"), value_of_definition(node))
    else {
        walk_children(node, ctx, current_branch);
        return;
    };
    let binding = Binding {
        name: node_text(&name_node, ctx.caller.source).to_string(),
        // The type sits on the enclosing declaration rather than on the
        // definition, so it is looked for on both.
        declared_type: declared_type(node, ctx.caller.source).or_else(|| {
            node.parent()
                .and_then(|parent| declared_type(&parent, ctx.caller.source))
        }),
        is_reassignment: false,
    };
    emit_with_binding(&value, ctx, &binding, current_branch);
}

/// `lhs = rhs`, and only `=` — a compound form like `+=` is not a
/// type-bearing rebinding. The declared type is unknown at a reassignment
/// site, so `binds_type` is omitted.
fn handle_assignment(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let plain_assign = node.child(1).map(|n| n.kind() == "=").unwrap_or(false);
    let (Some(target), Some(value)) = (first_named_child(node), last_named_child(node)) else {
        walk_children(node, ctx, current_branch);
        return;
    };
    if !plain_assign || target.id() == value.id() {
        walk_children(node, ctx, current_branch);
        return;
    }
    let binding = Binding {
        name: node_text(&target, ctx.caller.source).to_string(),
        declared_type: None,
        is_reassignment: true,
    };
    emit_with_binding(&value, ctx, &binding, current_branch);
}

/// The initialiser of a variable definition: the last named child, as long
/// as it is not the name itself.
fn value_of_definition<'t>(node: &Node<'t>) -> Option<Node<'t>> {
    let value = last_named_child(node)?;
    let name = child_of_kind(node, "identifier")?;
    (value.id() != name.id()).then_some(value)
}

/// Apply `binding` to the call the value expression produces, then keep
/// walking so nested calls in its arguments still emit plain edges.
///
/// `await`, `!` and the other prefix forms wrap the expression they apply
/// to, so they are peeled first: `final r = await client.get(u)` binds `r`
/// to `get`, not to the `await`.
fn emit_with_binding(
    value: &Node,
    ctx: &mut CallCtx<'_>,
    binding: &Binding,
    current_branch: Option<&str>,
) {
    let value = peel_prefixes(value);
    match value.kind() {
        "call_expression" => handle_call(&value, ctx, Some(binding), current_branch),
        "new_expression" | "const_object_expression" => {
            handle_instantiation(&value, ctx, Some(binding), current_branch);
            walk_children(&value, ctx, current_branch);
        }
        _ => extract_calls(&value, ctx, current_branch),
    }
}

/// Strip the expression wrappers that carry no call of their own.
fn peel_prefixes<'t>(node: &Node<'t>) -> Node<'t> {
    let mut current = *node;
    while matches!(current.kind(), "unary_expression" | "await_expression") {
        match first_named_child(&current) {
            Some(inner) => current = inner,
            None => break,
        }
    }
    current
}

// ---------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------

/// Register one arm and walk its body under the arm's path.
fn walk_arm(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let path = ctx.next_arm_path(current_branch);
    emit_branch_entity(
        ctx.caller.id,
        current_branch,
        &path,
        node,
        ctx.caller.path,
        ctx.walk.result,
    );
    extract_calls(node, ctx, Some(&path));
}

/// `if (cond) … else if (cond) … else …`.
///
/// The grammar nests `else if` as another `if_statement` in the alternative
/// slot, but the user-visible model is a flat list of arms, so the chain is
/// flattened here: siblings read as `c1` / `c2` / `c3` rather than `c1` /
/// `c1.c2` / `c1.c2.c3`. The conditions stay at the outer level — each is
/// evaluated before any arm runs.
fn handle_if(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut current = Some(*node);
    while let Some(statement) = current {
        let parts = if_parts(&statement);
        if let Some(condition) = parts.condition {
            extract_calls(&condition, ctx, current_branch);
        }
        if let Some(consequence) = parts.consequence {
            walk_arm(&consequence, ctx, current_branch);
        }
        current = match parts.alternative {
            Some(alt) if alt.kind() == "if_statement" => Some(alt),
            Some(alt) => {
                walk_arm(&alt, ctx, current_branch);
                None
            }
            None => None,
        };
    }
}

/// The three parts of an `if_statement`.
#[derive(Default)]
struct IfParts<'t> {
    condition: Option<Node<'t>>,
    consequence: Option<Node<'t>>,
    alternative: Option<Node<'t>>,
}

/// Read an `if_statement` positionally, which is all the grammar allows — it
/// names no fields. The condition is the first named child, the alternative
/// is the first named child after the `else` token, and the consequence is
/// whatever falls between. Each slot keeps the first node offered to it, so
/// a trailing token can never overwrite a part.
fn if_parts<'t>(node: &Node<'t>) -> IfParts<'t> {
    let mut parts = IfParts::default();
    let mut seen_else = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "else" {
            seen_else = true;
            continue;
        }
        if !child.is_named() {
            continue;
        }
        let slot = if seen_else {
            &mut parts.alternative
        } else if parts.condition.is_none() {
            &mut parts.condition
        } else {
            &mut parts.consequence
        };
        slot.get_or_insert(child);
    }
    parts
}

/// `try { } on T catch (e) { } catch (e) { } finally { }`.
///
/// Dart's grammar keeps a handler's body *outside* the clause that
/// introduces it: `try_statement` is a flat run of the try block, then
/// `type` / `catch_clause` markers each followed by their own `block`. So
/// the run is walked left to right, remembering which handler is open, and
/// each `block` closes the one before it. The first block is the try body.
fn handle_try(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut caught: Option<String> = None;
    let mut seen_try_body = false;
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();

    for child in children {
        match child.kind() {
            "type" => caught = Some(node_text(&child, ctx.caller.source).to_string()),
            "block" if !seen_try_body => {
                seen_try_body = true;
                walk_try_arm(&child, ctx, current_branch, "try_body", None);
            }
            "block" => {
                let caught = caught.take();
                walk_try_arm(&child, ctx, current_branch, "catch", caught.as_deref());
            }
            "finally_clause" => {
                if let Some(body) = child_of_kind(&child, "block") {
                    walk_try_arm(&body, ctx, current_branch, "finally", None);
                }
            }
            _ => {}
        }
    }
}

fn walk_try_arm(
    body: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    arm_kind: &str,
    caught: Option<&str>,
) {
    let path = ctx.next_arm_path(current_branch);
    emit_try_arm_entity(
        ctx.caller.id,
        current_branch,
        &path,
        body,
        ctx.caller.path,
        arm_kind,
        caught,
        ctx.walk.result,
    );
    extract_calls(body, ctx, Some(&path));
}

/// `switch (subject) { case A: … default: … }`. The subject runs once before
/// any arm, so it stays at the outer level. Each case and the default become
/// their own arm, and the pattern a case matches on travels with it.
fn handle_switch(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(subject) = child_of_kind(node, "parenthesized_expression") {
        extract_calls(&subject, ctx, current_branch);
    }
    let Some(block) = child_of_kind(node, "switch_block") else {
        return;
    };

    let mut cursor = block.walk();
    let arms: Vec<Node> = block
        .children(&mut cursor)
        .filter(|c| {
            matches!(
                c.kind(),
                "switch_statement_case" | "switch_statement_default"
            )
        })
        .collect();

    for arm in arms {
        let path = ctx.next_arm_path(current_branch);
        let pattern = case_pattern(&arm, ctx.caller.source);
        emit_case_arm_entity(
            ctx.caller.id,
            current_branch,
            &path,
            &arm,
            ctx.caller.path,
            pattern,
            ctx.walk.result,
        );
        extract_calls(&arm, ctx, Some(&path));
    }
}

/// What a `case` matches on. `default` matches on nothing, and the grammar
/// gives it a node kind of its own, so the absence is unambiguous.
fn case_pattern<'a>(arm: &Node, source: &'a str) -> Option<&'a str> {
    if arm.kind() != "switch_statement_case" {
        return None;
    }
    first_named_child(arm).map(|pattern| node_text(&pattern, source))
}

/// `for` / `while` / `do-while`. Everything but the body — the loop header,
/// the iterable, the trailing `while` condition — runs on every iteration,
/// so its calls group under the loop too, matching the Java pass.
fn handle_loop(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let path = ctx.next_loop_path(current_branch);
    let body = loop_body(node);
    if let Some(body) = &body {
        emit_loop_entity(
            ctx.caller.id,
            current_branch,
            &path,
            body,
            ctx.caller.path,
            ctx.walk.result,
        );
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node
        .children(&mut cursor)
        .filter(|c| c.is_named())
        .collect();
    for child in children {
        if Some(child.id()) == body.map(|b| b.id()) {
            continue;
        }
        extract_calls(&child, ctx, Some(&path));
    }
    if let Some(body) = &body {
        extract_calls(body, ctx, Some(&path));
    }
}

/// The statement a loop repeats. `do { … } while (c);` puts its body first
/// and its condition last, so it is read from the front; every other loop
/// puts the body last.
fn loop_body<'t>(node: &Node<'t>) -> Option<Node<'t>> {
    if node.kind() == "do_statement" {
        return first_named_child(node);
    }
    child_of_kind(node, "block").or_else(|| last_named_child(node))
}
