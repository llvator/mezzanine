//! Call-site extraction for Go function and method bodies.
//!
//! Walks the body emitting `Calls` / `Instantiates` relationships and
//! synthesising `Branch` / `Loop` entities for the arms of `if`, `switch`,
//! type switch, `select` and `for`, so the UI's decision-tree rendering
//! treats Go like Java and Python. Every relationship emitted inside an arm
//! carries a `branch` metadata key naming that arm's path (`c1`, `c2.l1`),
//! which the analyzer reattributes to the synthetic node.
//!
//! Three things here are Go's alone:
//!
//! * **Targets are resolved through declared types, not through receiver
//!   text.** `s.repo.Find(id)` becomes `Repository.Find` by way of
//!   [`super::inference`], and an edge that cannot be resolved that far is
//!   dropped rather than pointed at a ghost named `s.repo.Find`.
//! * **A composite literal is an instantiation.** `Order{}` is how Go says
//!   `new Order()`, so it carries the same `Instantiates` edge that a
//!   constructor does elsewhere — with `new(Order)` alongside it.
//! * **`go` and `defer` are how a call is scheduled, not what it calls.**
//!   Both emit an ordinary edge tagged with the keyword that dispatched
//!   it, because "this call runs on another goroutine" and "this call runs
//!   when the function returns" are things a reader of the graph needs and
//!   cannot recover from the target.

use super::super::helpers::{field_texts, is_predeclared_type};
use super::super::packages::Imports;
use super::flow::{emit_branch_entity, emit_case_arm_entity, emit_loop_entity, Arm};
use super::inference::{Locals, TypeIndex};
use super::stdlib::is_builtin;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Who a call is attributed to, and everything fixed for the whole body
/// that deciding a target needs. Read-only, so a helper that resolves a
/// name cannot reach the walk's counters.
pub(in crate::parser::go) struct Caller<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub id: &'a str,
    pub name: &'a str,
    /// The type this callable hangs off, when it is a method.
    pub receiver_type: Option<&'a str>,
    /// What the file imported, for telling a package from a variable.
    pub imports: &'a Imports,
    /// What the file declares about struct fields and package-level vars.
    pub types: &'a TypeIndex,
    /// What this body's signature and assignments declare.
    pub locals: Locals,
}

/// What the walk accumulates as it descends: the call ordinal, the two arm
/// counters, and the result they are written into. Owned rather than
/// borrowed — they start at zero with the body, and leave only as the arm
/// paths built from them.
struct Walk<'a> {
    call_order: u32,
    arm_counter: u32,
    loop_counter: u32,
    result: &'a mut ParseResult,
}

/// Context threaded through the walk, so a recursive step passes one
/// reference rather than nine arguments. The two halves are separate
/// structs because they change for separate reasons: a new counter is not
/// a new thing to know about the caller.
pub(in crate::parser::go) struct CallCtx<'a> {
    caller: Caller<'a>,
    walk: Walk<'a>,
}

impl<'a> CallCtx<'a> {
    /// Start a walk over one body. The counters begin at zero per body,
    /// which is what makes an arm path read relative to the function
    /// rather than to the file.
    pub(in crate::parser::go) fn new(caller: Caller<'a>, result: &'a mut ParseResult) -> Self {
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
        nested_path(current_branch, 'c', self.walk.arm_counter)
    }

    /// Loops count on their own sequence so a sibling decision tree and
    /// loop at one scope read as `c1` / `l1` rather than colliding.
    fn next_loop_path(&mut self, current_branch: Option<&str>) -> String {
        self.walk.loop_counter += 1;
        nested_path(current_branch, 'l', self.walk.loop_counter)
    }

    /// Address one synthetic flow entity: this caller, this enclosing arm,
    /// this path.
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

/// Captures that the enclosing call or literal has its result stored into
/// a name. Travels as metadata on the edge so a view can render the
/// binding without a node per local.
struct Binding {
    name: String,
    declared_type: Option<String>,
    is_reassignment: bool,
}

/// An arm's path under its parent: `c1`, then `c1.l2` for a loop inside it.
fn nested_path(current_branch: Option<&str>, prefix: char, idx: u32) -> String {
    match current_branch {
        Some(p) => format!("{}.{}{}", p, prefix, idx),
        None => format!("{}{}", prefix, idx),
    }
}

/// Walk a function or method body, emitting relationships and flow
/// entities. `current_branch` is the arm the walk is currently inside, or
/// `None` at the body's top level.
pub(in crate::parser::go) fn extract_calls(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
) {
    if dispatch_node(node, ctx, current_branch) {
        return;
    }
    walk_children(node, ctx, current_branch);
}

/// Recurse into every child of a node with no handling of the node itself.
fn walk_children(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls(&child, ctx, current_branch);
    }
}

/// Per-kind handler. Returns `true` when the node was fully handled —
/// including any recursion it needed — so the caller skips the default
/// child walk. `false` lets the generic recursion take over.
fn dispatch_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    // A nested function declaration owns its own scope. It cannot appear
    // inside a body in legal Go, but the grammar accepts top-level
    // statements in a fragment, so the guard costs nothing and keeps a
    // snippet's inner calls off the outer caller.
    if matches!(node.kind(), "function_declaration" | "method_declaration") {
        return true;
    }
    if dispatch_flow_node(node, ctx, current_branch) {
        return true;
    }
    match node.kind() {
        "call_expression" => {
            handle_call(node, ctx, None, None, current_branch);
            walk_call_children(node, ctx, current_branch);
            true
        }
        "composite_literal" => {
            handle_composite_literal(node, ctx, None, current_branch);
            // The literal's body holds the field values, which may call.
            if let Some(body) = node.child_by_field_name("body") {
                extract_calls(&body, ctx, current_branch);
            }
            true
        }
        "short_var_declaration" => {
            handle_binding(node, ctx, current_branch, false);
            true
        }
        "assignment_statement" => {
            handle_binding(node, ctx, current_branch, true);
            true
        }
        "var_spec" => {
            handle_var_spec(node, ctx, current_branch);
            true
        }
        "go_statement" => {
            handle_dispatched(node, ctx, current_branch, "goroutine");
            true
        }
        "defer_statement" => {
            handle_dispatched(node, ctx, current_branch, "deferred");
            true
        }
        _ => false,
    }
}

/// Flow-control dispatch. Returns `true` if the node was handled, mirroring
/// the [`dispatch_node`] contract.
fn dispatch_flow_node(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) -> bool {
    match node.kind() {
        "if_statement" => handle_if_statement(node, ctx, current_branch),
        "for_statement" => handle_for_statement(node, ctx, current_branch),
        "expression_switch_statement" | "type_switch_statement" => {
            handle_switch_statement(node, ctx, current_branch)
        }
        "select_statement" => handle_select_statement(node, ctx, current_branch),
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------
// Calls and instantiations
// ---------------------------------------------------------------------

/// Emit the edge for one `call_expression`.
fn handle_call(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    dispatch: Option<&str>,
    current_branch: Option<&str>,
) {
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let order = ctx.next_order();
    // `new(Order)` is spelled as a call and means an instantiation.
    if let Some(allocated) = allocated_type(node, ctx) {
        if let Some(target) = literal_target(&allocated, ctx) {
            emit(
                ctx,
                target,
                RelationshipKind::Instantiates,
                order,
                Emission {
                    binding,
                    dispatch,
                    current_branch,
                },
            );
        }
        return;
    }
    let Some(target) = callee_target(&function, ctx) else {
        return;
    };
    emit(
        ctx,
        target,
        RelationshipKind::Calls,
        order,
        Emission {
            binding,
            dispatch,
            current_branch,
        },
    );
}

/// Emit the `Instantiates` edge for `Order{…}` — Go's constructor call.
fn handle_composite_literal(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    binding: Option<&Binding>,
    current_branch: Option<&str>,
) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Some(target) = literal_target(&type_node, ctx) else {
        return;
    };
    let order = ctx.next_order();
    emit(
        ctx,
        target,
        RelationshipKind::Instantiates,
        order,
        Emission {
            binding,
            dispatch: None,
            current_branch,
        },
    );
}

/// The optional facts an edge carries beyond its source, target and kind.
/// Grouped so the emitter takes four arguments rather than seven.
struct Emission<'a> {
    binding: Option<&'a Binding>,
    dispatch: Option<&'a str>,
    current_branch: Option<&'a str>,
}

fn emit(
    ctx: &mut CallCtx<'_>,
    target: String,
    kind: RelationshipKind,
    order: u32,
    extra: Emission<'_>,
) {
    let mut rel = Relationship::new(ctx.caller.id.to_string(), target, kind);
    rel.metadata.insert("order".to_string(), order.to_string());
    if let Some(branch) = extra.current_branch {
        rel.metadata
            .insert("branch".to_string(), branch.to_string());
    }
    if let Some(key) = extra.dispatch {
        rel.metadata.insert(key.to_string(), "true".to_string());
    }
    if let Some(b) = extra.binding {
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
    ctx.walk.result.add_relationship(rel);
}

/// What a call's `function` expression names, as a target the resolver can
/// look up — or `None` when it names nothing worth an edge.
fn callee_target(function: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    match function.kind() {
        "identifier" => {
            let name = node_text(function, ctx.caller.source);
            // A bare call in Go is a call to this package. The only bare
            // names that are not are the built-ins, and a name matching
            // the caller's own, which is recursion.
            (!is_builtin(name) && name != ctx.caller.name).then(|| name.to_string())
        }
        "selector_expression" => selector_target(function, ctx),
        // `Parse[Order](…)` instantiates a generic before calling it, and
        // `(fn)(…)` parenthesises one. Neither changes the name.
        "index_expression" => callee_target(&function.child_by_field_name("operand")?, ctx),
        "parenthesized_expression" => callee_target(&first_named_child(function)?, ctx),
        _ => None,
    }
}

/// `x.Method()` — resolved through what the file declares about `x`.
fn selector_target(function: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    let name = node_text(&function.child_by_field_name("field")?, ctx.caller.source);
    let operand = function.child_by_field_name("operand")?;
    let qualifier = qualifier_of(&operand, ctx)?;
    // `s.handle()` inside `func (s *Server) handle()` is recursion.
    if name == ctx.caller.name && Some(qualifier.as_str()) == ctx.caller.receiver_type {
        return None;
    }
    Some(format!("{}.{}", qualifier, name))
}

/// The name a selector's operand stands for: a package, or a type.
///
/// Returning `None` drops the edge, which is the right answer whenever the
/// operand's type is not stated in this file. The alternative — falling
/// back to the operand's source text — is what fills a graph with ghost
/// nodes named after expressions.
fn qualifier_of(operand: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    match operand.kind() {
        "identifier" => {
            let text = node_text(operand, ctx.caller.source);
            if let Some(declared) = ctx.caller.locals.type_of(text) {
                return usable_type(declared, ctx);
            }
            if ctx.caller.imports.is_stdlib_qualifier(text) {
                return None;
            }
            if let Some(package) = ctx.caller.imports.package_for(text) {
                return Some(package.to_string());
            }
            ctx.caller
                .types
                .global_type(text)
                .and_then(|declared| usable_type(declared, ctx))
        }
        // `s.repo` — a field, on something whose type resolves one level up.
        "selector_expression" => {
            let owner = qualifier_of(&operand.child_by_field_name("operand")?, ctx)?;
            let field = node_text(&operand.child_by_field_name("field")?, ctx.caller.source);
            usable_type(ctx.caller.types.field_type(&owner, field)?, ctx)
        }
        // A pointer dereference or an address-of leaves the type alone.
        "parenthesized_expression" => qualifier_of(&first_named_child(operand)?, ctx),
        "unary_expression" => qualifier_of(&operand.child_by_field_name("operand")?, ctx),
        _ => None,
    }
}

/// The bare name a recorded type resolves to as a call qualifier, or
/// `None` when the call through it is not worth drawing.
///
/// Two kinds are dropped. A predeclared type — `err.Error()` names the
/// built-in `error`, and `error.Error` is noise in every graph. And a type
/// belonging to a standard-library package: `var wg sync.WaitGroup`
/// followed by `wg.Add(1)` is the same call to the same standard library
/// as `fmt.Println`, reached through a variable instead of directly, and
/// the package qualifier is the only thing that says so.
fn usable_type(declared: &str, ctx: &CallCtx<'_>) -> Option<String> {
    match declared.rsplit_once('.') {
        Some((package, name)) => {
            (!ctx.caller.imports.is_stdlib_qualifier(package)).then(|| name.to_string())
        }
        None => (!is_predeclared_type(declared)).then(|| declared.to_string()),
    }
}

/// The type node `new(T)` allocates, when the call is `new` at all. `new`
/// is the one built-in worth an edge: it constructs a named type the same
/// way a composite literal does.
fn allocated_type<'t>(node: &Node<'t>, ctx: &CallCtx<'_>) -> Option<Node<'t>> {
    let function = node.child_by_field_name("function")?;
    if node_text(&function, ctx.caller.source) != "new" {
        return None;
    }
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let first = arguments.children(&mut cursor).find(|c| c.is_named());
    first
}

/// The type a composite literal builds, when it names one. A map, slice,
/// array or anonymous-struct literal names no type the graph holds.
fn literal_target(type_node: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    match type_node.kind() {
        "type_identifier" => {
            let name = node_text(type_node, ctx.caller.source);
            (!is_predeclared_type(name)).then(|| name.to_string())
        }
        "generic_type" => literal_target(&type_node.child_by_field_name("type")?, ctx),
        // `new(*Order)` allocates a pointer to a pointer; the named type
        // inside is still the one being depended on.
        "pointer_type" | "slice_type" | "array_type" => {
            literal_target(&first_named_child(type_node)?, ctx)
        }
        "qualified_type" => {
            let package = node_text(
                &type_node.child_by_field_name("package")?,
                ctx.caller.source,
            );
            if ctx.caller.imports.is_stdlib_qualifier(package) {
                return None;
            }
            let name = node_text(&type_node.child_by_field_name("name")?, ctx.caller.source);
            let package = ctx.caller.imports.package_for(package).unwrap_or(package);
            Some(format!("{}.{}", package, name))
        }
        _ => None,
    }
}

/// Recurse into a call's parts other than the callee name itself — the
/// receiver it hangs off and the arguments it is given — so calls nested
/// there still emit their own edges.
fn walk_call_children(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    if let Some(function) = node.child_by_field_name("function") {
        match function.kind() {
            "selector_expression" | "index_expression" => {
                if let Some(operand) = function.child_by_field_name("operand") {
                    extract_calls(&operand, ctx, current_branch);
                }
            }
            // `go func() { … }()` and `f()()` both put real work here.
            "func_literal" | "call_expression" | "parenthesized_expression" => {
                extract_calls(&function, ctx, current_branch)
            }
            _ => {}
        }
    }
    if let Some(arguments) = node.child_by_field_name("arguments") {
        extract_calls(&arguments, ctx, current_branch);
    }
}

// ---------------------------------------------------------------------
// Bindings
// ---------------------------------------------------------------------

/// `x := rhs` and `x = rhs`. When the right-hand side is a single call or
/// literal, the edge it emits is tagged with the name the result lands in.
///
/// Only a single right-hand value is tagged. `a, b := f()` still emits the
/// call — it binds both names, joined, which is what the source reads like
/// and what a view can show. The blank identifier is left out of that
/// list: `_` names nothing.
fn handle_binding(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    is_reassignment: bool,
) {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        walk_children(node, ctx, current_branch);
        return;
    };
    // Compound assignment (`+=`, `<<=`) rebinds nothing type-bearing.
    let plain = node
        .child_by_field_name("operator")
        .map(|op| op.kind() == "=")
        .unwrap_or(true);
    if !plain {
        extract_calls(&left, ctx, current_branch);
        extract_calls(&right, ctx, current_branch);
        return;
    }

    extract_calls(&left, ctx, current_branch);
    let values = named_children(&right);
    let Some(value) = single(&values) else {
        extract_calls(&right, ctx, current_branch);
        return;
    };
    let names = bound_names(&left, ctx.caller.source);
    if names.is_empty() {
        extract_calls(value, ctx, current_branch);
        return;
    }
    let binding = Binding {
        name: names.join(", "),
        declared_type: None,
        is_reassignment,
    };
    emit_with_binding(value, ctx, &binding, current_branch);
}

/// `var x T = rhs` — the one binding form that states the type outright.
fn handle_var_spec(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(right) = node.child_by_field_name("value") else {
        walk_children(node, ctx, current_branch);
        return;
    };
    let values = named_children(&right);
    let Some(value) = single(&values) else {
        extract_calls(&right, ctx, current_branch);
        return;
    };
    let names: Vec<String> = field_texts(node, "name", ctx.caller.source);
    if names.is_empty() {
        extract_calls(value, ctx, current_branch);
        return;
    }
    let binding = Binding {
        name: names.join(", "),
        declared_type: node
            .child_by_field_name("type")
            .map(|t| node_text(&t, ctx.caller.source).to_string()),
        is_reassignment: false,
    };
    emit_with_binding(value, ctx, &binding, current_branch);
}

/// Apply a binding to the outermost call or literal in `value`, then keep
/// walking it so nested calls still emit plain edges.
fn emit_with_binding(
    value: &Node,
    ctx: &mut CallCtx<'_>,
    binding: &Binding,
    current_branch: Option<&str>,
) {
    match value.kind() {
        "call_expression" => {
            handle_call(value, ctx, Some(binding), None, current_branch);
            walk_call_children(value, ctx, current_branch);
        }
        "composite_literal" => {
            handle_composite_literal(value, ctx, Some(binding), current_branch);
            if let Some(body) = value.child_by_field_name("body") {
                extract_calls(&body, ctx, current_branch);
            }
        }
        // `x := &Order{}` — the address-of is not what is being built.
        "unary_expression" => match value.child_by_field_name("operand") {
            Some(operand) => emit_with_binding(&operand, ctx, binding, current_branch),
            None => extract_calls(value, ctx, current_branch),
        },
        _ => extract_calls(value, ctx, current_branch),
    }
}

/// The names on the left of an assignment, minus the blank identifier.
fn bound_names(left: &Node, source: &str) -> Vec<String> {
    named_children(left)
        .iter()
        .map(|n| node_text(n, source).to_string())
        .filter(|name| name != "_")
        .collect()
}

// ---------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------

/// `if init; cond { … } else if … { … } else { … }`.
///
/// The chain is flattened: Go nests `else if` in the `alternative` field as
/// another `if_statement`, but a reader sees a flat list of arms, so
/// siblings read as `c1` / `c2` / `c3` rather than nesting one inside the
/// next. The initializer and condition stay in the enclosing branch —
/// both run before any arm is chosen.
fn handle_if_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let mut current = Some(*node);
    while let Some(arm) = current {
        for field in ["initializer", "condition"] {
            if let Some(part) = arm.child_by_field_name(field) {
                extract_calls(&part, ctx, current_branch);
            }
        }
        if let Some(body) = arm.child_by_field_name("consequence") {
            emit_arm(ctx, current_branch, &body);
        }
        match arm.child_by_field_name("alternative") {
            // `else if …` — a sibling arm, so keep flattening.
            Some(alt) if alt.kind() == "if_statement" => current = Some(alt),
            // A terminal `else { … }`.
            Some(alt) => {
                emit_arm(ctx, current_branch, &alt);
                current = None;
            }
            None => current = None,
        }
    }
}

/// Emit one Branch arm for `body`, then walk the body inside it.
fn emit_arm(ctx: &mut CallCtx<'_>, current_branch: Option<&str>, body: &Node) {
    let path = ctx.next_arm_path(current_branch);
    emit_branch_entity(&ctx.arm(current_branch, &path), body, ctx.walk.result);
    extract_calls(body, ctx, Some(&path));
}

/// `for`, in all four spellings: infinite, condition-only, three-clause,
/// and `range`. Go has one loop keyword, so there is one handler.
///
/// Everything in the header groups under the loop rather than beside it.
/// The condition of a `for` is re-evaluated on every iteration, and the
/// expression a `range` walks is what the loop is over — a call in either
/// belongs to the loop the way `for _, r := range db.Rows()` reads.
fn handle_for_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    let Some(body) = node.child_by_field_name("body") else {
        walk_children(node, ctx, current_branch);
        return;
    };
    let path = ctx.next_loop_path(current_branch);
    emit_loop_entity(&ctx.arm(current_branch, &path), &body, ctx.walk.result);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            extract_calls(&child, ctx, Some(&path));
        }
    }
}

/// `switch` and `switch x.(type)`. The initializer and the subject run once
/// before any arm, so they stay in the enclosing branch; each case becomes
/// an arm carrying its label as a `pattern:`.
fn handle_switch_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    for field in ["initializer", "value"] {
        if let Some(part) = node.child_by_field_name(field) {
            extract_calls(&part, ctx, current_branch);
        }
    }
    handle_cases(node, ctx, current_branch, false);
}

/// `select { case <-ch: … }`. Every arm is a case over channel readiness;
/// there is no subject expression to evaluate first.
fn handle_select_statement(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>) {
    handle_cases(node, ctx, current_branch, true);
}

/// Emit one arm per case of a switch, type switch, or select, and walk each
/// case's statements under it.
fn handle_cases(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    current_branch: Option<&str>,
    from_select: bool,
) {
    let mut cursor = node.walk();
    let cases: Vec<Node> = node
        .children(&mut cursor)
        .filter(|c| {
            matches!(
                c.kind(),
                "expression_case" | "type_case" | "communication_case" | "default_case"
            )
        })
        .collect();
    for case in cases {
        let pattern = case_pattern(&case, ctx.caller.source);
        let path = ctx.next_arm_path(current_branch);
        emit_case_arm_entity(
            &ctx.arm(current_branch, &path),
            &case,
            pattern.as_deref(),
            from_select,
            ctx.walk.result,
        );
        let mut inner = case.walk();
        let children: Vec<Node> = case.children(&mut inner).collect();
        for child in children {
            extract_calls(&child, ctx, Some(&path));
        }
    }
}

/// The label a case shows: the matched values, the matched types joined by
/// ` | `, or the channel operation. `default` has none.
fn case_pattern(case: &Node, source: &str) -> Option<String> {
    match case.kind() {
        "expression_case" => {
            Some(node_text(&case.child_by_field_name("value")?, source).to_string())
        }
        "type_case" => {
            let mut cursor = case.walk();
            let types: Vec<String> = case
                .children_by_field_name("type", &mut cursor)
                .map(|t| node_text(&t, source).to_string())
                .collect();
            (!types.is_empty()).then(|| types.join(" | "))
        }
        "communication_case" => {
            Some(node_text(&case.child_by_field_name("communication")?, source).to_string())
        }
        _ => None,
    }
}

/// `go f()` and `defer f()`. The call is ordinary; how it is scheduled is
/// not, so the edge carries the keyword as a tag.
fn handle_dispatched(node: &Node, ctx: &mut CallCtx<'_>, current_branch: Option<&str>, key: &str) {
    let Some(expression) = last_named_child(node) else {
        return;
    };
    if expression.kind() == "call_expression" {
        handle_call(&expression, ctx, None, Some(key), current_branch);
        walk_call_children(&expression, ctx, current_branch);
    } else {
        extract_calls(&expression, ctx, current_branch);
    }
}

// ---------------------------------------------------------------------
// Node helpers
// ---------------------------------------------------------------------

fn named_children<'a>(node: &Node<'a>) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| c.is_named())
        .collect()
}

/// The one element of a slice, or `None` when there are none or several.
fn single<'a, 'b>(nodes: &'b [Node<'a>]) -> Option<&'b Node<'a>> {
    (nodes.len() == 1).then(|| &nodes[0])
}

fn first_named_child<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|c| c.is_named());
    found
}

fn last_named_child<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).filter(|c| c.is_named()).last()
}
