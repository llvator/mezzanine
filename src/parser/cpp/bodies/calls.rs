//! What a C++ body does: the calls it makes, the objects it builds, the
//! fields it writes, and the control-flow arms it does all of that inside.
//!
//! The walk is the one the Java and Go parsers use — a dispatcher that
//! either handles a node fully or lets the generic recursion take its
//! children, with a [`CallCtx`] threaded through so a handler carries one
//! reference instead of nine parameters. Three things are specific to C++
//! and worth reading before the code:
//!
//! * **A bare call is ambiguous.** `helper()` inside `Order::total` is
//!   `Order::helper` if the class has such a member and a free function
//!   otherwise, and the call site does not say which. So the name is
//!   qualified only when [`TypeIndex::declares_member`] confirms the
//!   member exists in this file, and left bare when it does not (§E1).
//! * **A declaration can be a constructor call.** `Order o(2);` and
//!   `Widget w{sum};` build objects without spelling `new`. They emit
//!   `Instantiates` — but only when the initializer is an argument list
//!   rather than a call, because `Matrix m = Matrix::identity();` is one
//!   dependency and recording it twice would inflate every coupling
//!   metric that reads it (§E4).
//! * **`std::` is filtered by qualifier, not by name.** `std::move(x)` is
//!   recognisable; `xs.push_back(y)` is not, and falls to the member-name
//!   table in [`super::stdlib`]. Both filters lift when the call's result
//!   is bound to a local, which is the caller saying the value matters.

use super::flow::{
    emit_branch_entity, emit_case_arm_entity, emit_loop_entity, emit_try_arm_entity, Arm,
};
use super::inference::{base_type, Locals, TypeIndex};
use super::stdlib::{is_std_member, is_std_qualified};
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span, ParseResult};
use crate::parser::working_set;
use std::collections::HashSet;
use std::path::Path;
use tree_sitter::Node;

/// Who a body's findings are attributed to. Fixed for the whole walk and
/// read-only, so a helper that needs to know whose body it is reading
/// cannot reach the walk's counters.
pub(in crate::parser::cpp) struct Caller<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub id: &'a str,
    pub name: &'a str,
    /// The class a member function belongs to — what `this->x()`, a bare
    /// sibling call and a field write are qualified by.
    pub owner: Option<&'a str>,
    pub types: &'a TypeIndex,
    pub locals: Locals,
}

/// What the walk accumulates: the call ordinal, the three path counters,
/// the value names already recorded, and the result they are written
/// into. Owned rather than borrowed — they start at zero with the body and
/// leave only as the paths and edges built from them.
struct Walk<'a> {
    call_order: u32,
    arm_counter: u32,
    loop_counter: u32,
    lambda_counter: u32,
    values: HashSet<String>,
    result: &'a mut ParseResult,
}

/// The context threaded through [`extract_calls`] and its handlers. The
/// two halves are separate structs because they change for separate
/// reasons: a new counter is not a new thing to know about the caller.
pub(in crate::parser::cpp) struct CallCtx<'a> {
    caller: Caller<'a>,
    walk: Walk<'a>,
}

impl<'a> CallCtx<'a> {
    /// Start a walk over one body. The counters begin at zero per body,
    /// which is what makes an arm path read relative to the callable
    /// rather than to the file.
    pub(in crate::parser::cpp) fn new(caller: Caller<'a>, result: &'a mut ParseResult) -> Self {
        Self {
            caller,
            walk: Walk {
                call_order: 0,
                arm_counter: 0,
                loop_counter: 0,
                lambda_counter: 0,
                values: HashSet::new(),
                result,
            },
        }
    }

    /// Consume the next call ordinal. Taken before any filtering, so a
    /// dropped call still burns its number and the edges that survive
    /// keep their source order (§C1).
    fn next_order(&mut self) -> u32 {
        self.walk.call_order += 1;
        self.walk.call_order
    }

    fn next_arm_path(&mut self, branch: Option<&str>) -> String {
        self.walk.arm_counter += 1;
        path_under(branch, 'c', self.walk.arm_counter)
    }

    fn next_loop_path(&mut self, branch: Option<&str>) -> String {
        self.walk.loop_counter += 1;
        path_under(branch, 'l', self.walk.loop_counter)
    }

    fn next_lambda(&mut self) -> u32 {
        self.walk.lambda_counter += 1;
        self.walk.lambda_counter
    }

    /// The address of an arm at `branch`, ready for the flow emitters.
    fn arm<'p>(&self, branch: Option<&'p str>, path: &'p str) -> Arm<'p>
    where
        'a: 'p,
    {
        Arm {
            caller_id: self.caller.id,
            parent_branch: branch,
            path,
            file: self.caller.path,
        }
    }
}

/// Where an arm sits under its parent. Top-level arms read as `c1` / `l1`;
/// nested ones append so the ancestry is left-to-right.
fn path_under(branch: Option<&str>, prefix: char, index: u32) -> String {
    match branch {
        Some(parent) => format!("{}.{}{}", parent, prefix, index),
        None => format!("{}{}", prefix, index),
    }
}

/// The immediately-enclosing call or construction has its result stored
/// into a named variable. Travels as metadata on the edge so a view can
/// render the binding without a node per local.
struct Binding {
    name: String,
    declared_type: Option<String>,
    is_reassignment: bool,
}

/// Walk one body, emitting its edges and synthesising its flow entities.
pub(in crate::parser::cpp) fn extract_calls(
    node: &Node,
    ctx: &mut CallCtx<'_>,
    branch: Option<&str>,
) {
    if dispatch_node(node, ctx, branch) {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls(&child, ctx, branch);
    }
}

/// Per-kind handler. Returns `true` when the node was fully handled —
/// including any recursion it owed its children — and the caller then
/// skips the generic child-walk.
fn dispatch_node(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) -> bool {
    if is_nested_definition(node.kind()) {
        return true;
    }
    if dispatch_flow_node(node, ctx, branch) {
        return true;
    }
    match node.kind() {
        "call_expression" => {
            handle_call(node, ctx, None, branch);
            walk_call_children(node, ctx, branch);
            true
        }
        "new_expression" => {
            handle_new(node, ctx, None, branch);
            false
        }
        "declaration" => {
            handle_declaration(node, ctx, branch);
            true
        }
        "assignment_expression" => {
            handle_assignment(node, ctx, branch);
            true
        }
        "lambda_expression" => {
            handle_lambda(node, ctx, branch);
            true
        }
        "throw_statement" => {
            handle_throw(node, ctx, branch);
            false
        }
        "identifier" => {
            handle_value_read(node, ctx, branch);
            true
        }
        _ => false,
    }
}

/// Definition kinds whose bodies belong to another entity. Skipping them
/// here keeps inner calls attributed to the inner entity, which the
/// declaration walk has already registered. Lambdas are deliberately
/// absent: they are handled, not skipped (§B7).
fn is_nested_definition(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "class_specifier"
            | "struct_specifier"
            | "union_specifier"
            | "enum_specifier"
            | "template_declaration"
    )
}

/// Flow-control dispatch. Returns `true` when the node was handled,
/// mirroring [`dispatch_node`]'s contract.
fn dispatch_flow_node(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) -> bool {
    match node.kind() {
        "if_statement" => handle_if(node, ctx, branch),
        "switch_statement" => handle_switch(node, ctx, branch),
        "try_statement" => handle_try(node, ctx, branch),
        "for_statement" => handle_for(node, ctx, branch),
        "for_range_loop" => handle_range_for(node, ctx, branch),
        "while_statement" => handle_while(node, ctx, branch),
        "do_statement" => handle_while(node, ctx, branch),
        _ => return false,
    }
    true
}

/// Recurse into a call's receiver and arguments, but not into its callee
/// name — [`handle_call`] has already spoken for that.
fn walk_call_children(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    if let Some(function) = node.child_by_field_name("function") {
        if function.kind() == "field_expression" {
            if let Some(receiver) = function.child_by_field_name("argument") {
                extract_calls(&receiver, ctx, branch);
            }
        }
    }
    if let Some(arguments) = node.child_by_field_name("arguments") {
        extract_calls(&arguments, ctx, branch);
    }
}

// --- calls, constructions, writes -----------------------------------------

fn handle_call(node: &Node, ctx: &mut CallCtx<'_>, binding: Option<&Binding>, branch: Option<&str>) {
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let order = ctx.next_order();
    let Some(target) = call_target(&function, ctx) else {
        return;
    };
    if target.text == ctx.caller.name {
        return;
    }
    if binding.is_none() && is_filtered(&target) {
        return;
    }
    let rel = Relationship::new(ctx.caller.id.to_string(), target.text, RelationshipKind::Calls);
    add_edge(ctx, rel, order, binding, branch);
}

fn handle_new(node: &Node, ctx: &mut CallCtx<'_>, binding: Option<&Binding>, branch: Option<&str>) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let order = ctx.next_order();
    let target = base_type(node_text(&type_node, ctx.caller.source)).to_string();
    if is_std_qualified(&target) {
        return;
    }
    let rel = Relationship::new(
        ctx.caller.id.to_string(),
        target,
        RelationshipKind::Instantiates,
    );
    add_edge(ctx, rel, order, binding, branch);
}

/// `Type name(args);` / `Type name{args};` / `Type name;` — a construction
/// with no `new` in sight, and `Type name = expr;`, which is not one.
fn handle_declaration(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let declared = declared_type_name(node, ctx.caller.source);
    let mut cursor = node.walk();
    let declarators: Vec<Node> = node.children_by_field_name("declarator", &mut cursor).collect();
    for declarator in declarators {
        let value = initializer(&declarator);
        match value {
            // `Order o(2)` / `Widget w{sum}` — the initializer is an
            // argument list, so the declared type is what gets built.
            Some(v) if matches!(v.kind(), "argument_list" | "initializer_list") => {
                instantiate_declared(ctx, declared.as_deref(), &declarator, branch);
                extract_calls(&v, ctx, branch);
            }
            // `Order o = make()` — one dependency, already on the value.
            Some(v) => bind_value(&v, ctx, &binding_for(node, &declarator, ctx), branch),
            None => instantiate_declared(ctx, declared.as_deref(), &declarator, branch),
        }
    }
}

/// Emit the `Instantiates` edge a bare or argument-list declaration
/// implies, when its type is a named one rather than `int` or `auto`.
fn instantiate_declared(
    ctx: &mut CallCtx<'_>,
    declared: Option<&str>,
    declarator: &Node,
    branch: Option<&str>,
) {
    let Some(target) = declared else { return };
    // A function declaration inside a body constructs nothing.
    if super::super::helpers::function_declarator(declarator).is_some() {
        return;
    }
    if is_std_qualified(target) {
        return;
    }
    let order = ctx.next_order();
    let rel = Relationship::new(
        ctx.caller.id.to_string(),
        target.to_string(),
        RelationshipKind::Instantiates,
    );
    add_edge(ctx, rel, order, None, branch);
}

/// `lhs = rhs`. A write into a field of the enclosing class is a
/// `WritesTo` edge (§C5); the right-hand side is walked either way.
fn handle_assignment(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return;
    };
    if let Some(target) = written_field(&left, ctx) {
        let rel = Relationship::new(
            ctx.caller.id.to_string(),
            target,
            RelationshipKind::WritesTo,
        );
        let order = ctx.next_order();
        add_edge(ctx, rel, order, None, branch);
    } else {
        extract_calls(&left, ctx, branch);
    }
    let binding = Binding {
        name: node_text(&left, ctx.caller.source).to_string(),
        declared_type: None,
        is_reassignment: true,
    };
    bind_value(&right, ctx, &binding, branch);
}

/// `throw SomeError(…)` names a type without depending on its behaviour.
/// `References` is outside `is_dependency`, which is the point: it
/// annotates the graph without inflating fan-out (§C6).
fn handle_throw(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let mut cursor = node.walk();
    let thrown = node
        .children(&mut cursor)
        .find(|c| matches!(c.kind(), "call_expression" | "new_expression"))
        .and_then(|c| c.child_by_field_name(if c.kind() == "new_expression" { "type" } else { "function" }));
    let Some(thrown) = thrown else { return };
    let target = base_type(node_text(&thrown, ctx.caller.source)).to_string();
    let mut rel = Relationship::new(
        ctx.caller.id.to_string(),
        target,
        RelationshipKind::References,
    );
    if let Some(arm) = branch {
        rel.metadata.insert("branch".to_string(), arm.to_string());
    }
    ctx.walk.result.add_relationship(rel);
}

/// A `SCREAMING_CASE` name read without being called — a `#define`, an
/// `enum` constant, a `constexpr` limit (§C4).
///
/// The naming rule is the one ADR 0021 took from Rust, and C++ shares the
/// convention for exactly this category. Names the body binds itself are
/// skipped, and so is anything already recorded once: a constant read in
/// a loop is one dependency, not ten.
fn handle_value_read(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let name = node_text(node, ctx.caller.source);
    if !is_screaming_case(name) || ctx.caller.locals.type_of(name).is_some() {
        return;
    }
    if !ctx.walk.values.insert(name.to_string()) {
        return;
    }
    let mut rel = Relationship::new(
        ctx.caller.id.to_string(),
        name.to_string(),
        RelationshipKind::UsesValue,
    );
    if let Some(arm) = branch {
        rel.metadata.insert("branch".to_string(), arm.to_string());
    }
    ctx.walk.result.add_relationship(rel);
}

/// Apply a binding to the outermost call or construction in `value`, then
/// keep walking it so nested calls still emit plain edges.
fn bind_value(value: &Node, ctx: &mut CallCtx<'_>, binding: &Binding, branch: Option<&str>) {
    match value.kind() {
        "call_expression" => {
            handle_call(value, ctx, Some(binding), branch);
            walk_call_children(value, ctx, branch);
        }
        "new_expression" => {
            handle_new(value, ctx, Some(binding), branch);
            let mut cursor = value.walk();
            for child in value.children(&mut cursor) {
                extract_calls(&child, ctx, branch);
            }
        }
        _ => extract_calls(value, ctx, branch),
    }
}

/// Add an edge with the metadata every call site carries: its order, the
/// arm it fired in, and the local it was bound to.
fn add_edge(
    ctx: &mut CallCtx<'_>,
    mut rel: Relationship,
    order: u32,
    binding: Option<&Binding>,
    branch: Option<&str>,
) {
    rel.metadata.insert("order".to_string(), order.to_string());
    if let Some(arm) = branch {
        rel.metadata.insert("branch".to_string(), arm.to_string());
    }
    if let Some(bound) = binding {
        let key = if bound.is_reassignment { "rebinds_to" } else { "binds_to" };
        rel.metadata.insert(key.to_string(), bound.name.clone());
        if let Some(type_name) = &bound.declared_type {
            rel.metadata
                .insert("binds_type".to_string(), type_name.clone());
        }
    }
    ctx.walk.result.add_relationship(rel);
}

// --- how a target is spelled ----------------------------------------------

/// How a call site was spelled, once resolved.
struct Target {
    text: String,
    /// The call goes through a receiver this file could not type, so the
    /// owner half of `text` is the receiver's own name. Only these are
    /// subject to the standard-library member-name filter: a name that
    /// might be `std::vector::size` is worth dropping, and the same name
    /// on a type the file *did* declare is not.
    untyped_receiver: bool,
}

impl Target {
    fn resolved(text: String) -> Self {
        Self { text, untyped_receiver: false }
    }
}

/// The target of a call, keyed the way the resolver keys entities (§E1),
/// or `None` when the callee has no name that describes it — a call
/// through an expression, say, where dropping the edge is the correct
/// answer (§E3).
fn call_target(function: &Node, ctx: &CallCtx<'_>) -> Option<Target> {
    match function.kind() {
        "identifier" => {
            let name = node_text(function, ctx.caller.source);
            Some(Target::resolved(qualify_bare(name, ctx)))
        }
        "qualified_identifier" | "field_identifier" => Some(Target::resolved(
            strip_template_args(node_text(function, ctx.caller.source)),
        )),
        "field_expression" => member_target(function, ctx),
        // `foo<int>(x)` and the cast operators wear the same shape.
        "template_function" => call_target(&function.child_by_field_name("name")?, ctx),
        _ => None,
    }
}

/// A bare `helper()` qualified by the enclosing class, but only when this
/// file says the class has such a member. Anything else stays bare and is
/// ranked by locality, which is what a free function needs.
fn qualify_bare(name: &str, ctx: &CallCtx<'_>) -> String {
    match ctx.caller.owner {
        Some(owner) if ctx.caller.types.declares_member(owner, name) => {
            format!("{}::{}", owner, name)
        }
        _ => name.to_string(),
    }
}

/// `receiver.member()` / `receiver->member()`, spelled through the
/// receiver's declared type when this file states one (§E2) and through
/// the receiver's own text when it does not.
fn member_target(node: &Node, ctx: &CallCtx<'_>) -> Option<Target> {
    let receiver = node.child_by_field_name("argument")?;
    let field = node.child_by_field_name("field")?;
    let member = node_text(&field, ctx.caller.source);
    let (owner, untyped_receiver) = match receiver_type(&receiver, ctx) {
        Some(type_name) => (type_name, false),
        None => (receiver_text(&receiver, ctx)?, true),
    };
    Some(Target {
        text: format!("{}::{}", owner, member),
        untyped_receiver,
    })
}

/// The declared type of a receiver expression, following the hops this
/// file can actually see: `this` to the enclosing class, a name to its
/// local or parameter declaration, a field to the class body that
/// declares it.
fn receiver_type(node: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    match node.kind() {
        "this" => ctx.caller.owner.map(str::to_string),
        "identifier" | "field_identifier" => named_type(node, ctx),
        "field_expression" => {
            let inner = receiver_type(&node.child_by_field_name("argument")?, ctx)?;
            let field = node.child_by_field_name("field")?;
            ctx.caller
                .types
                .field_type(&inner, node_text(&field, ctx.caller.source))
                .map(str::to_string)
        }
        "parenthesized_expression" => receiver_type(&node.named_child(0)?, ctx),
        "pointer_expression" => receiver_type(&node.child_by_field_name("argument")?, ctx),
        _ => None,
    }
}

/// What a written name holds: a local or parameter first, then a field of
/// the enclosing class, then a namespace-scope object.
fn named_type(node: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    let name = node_text(node, ctx.caller.source);
    if let Some(type_name) = ctx.caller.locals.type_of(name) {
        return Some(type_name.to_string());
    }
    if let Some(owner) = ctx.caller.owner {
        if let Some(type_name) = ctx.caller.types.field_type(owner, name) {
            return Some(type_name.to_string());
        }
    }
    ctx.caller.types.global_type(name).map(str::to_string)
}

/// The receiver's source text, for the untyped case — but only when the
/// receiver *is* a name. `(a + b).foo()` has no text that describes it,
/// and an expression used as an entity name is the mistake §E3 names.
fn receiver_text(node: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    match node.kind() {
        "identifier" | "field_identifier" => {
            Some(node_text(node, ctx.caller.source).to_string())
        }
        "this" => None,
        _ => None,
    }
}

/// The named type a declaration declares, or `None` for `auto`, `int`,
/// and the other spellings that name no entity.
fn declared_type_name(node: &Node, source: &str) -> Option<String> {
    let type_node = node.child_by_field_name("type")?;
    if !matches!(
        type_node.kind(),
        "type_identifier" | "qualified_identifier" | "template_type"
    ) {
        return None;
    }
    Some(base_type(node_text(&type_node, source)).to_string())
}

/// The value an `init_declarator` initialises with, if any.
fn initializer<'a>(declarator: &Node<'a>) -> Option<Node<'a>> {
    if declarator.kind() != "init_declarator" {
        return None;
    }
    declarator.child_by_field_name("value")
}

fn binding_for(node: &Node, declarator: &Node, ctx: &CallCtx<'_>) -> Binding {
    let name = super::super::helpers::declared_name(declarator)
        .map(|n| node_text(&n, ctx.caller.source).to_string())
        .unwrap_or_default();
    Binding {
        name,
        declared_type: node
            .child_by_field_name("type")
            .map(|t| node_text(&t, ctx.caller.source).to_string()),
        is_reassignment: false,
    }
}

/// The field of the enclosing class an assignment writes into — `this->x`
/// or, where the class body is in this file, a bare `x_`.
fn written_field(left: &Node, ctx: &CallCtx<'_>) -> Option<String> {
    let owner = ctx.caller.owner?;
    match left.kind() {
        "field_expression" => {
            let receiver = left.child_by_field_name("argument")?;
            let holder = receiver_type(&receiver, ctx)?;
            let field = left.child_by_field_name("field")?;
            Some(format!(
                "{}::{}",
                holder,
                node_text(&field, ctx.caller.source)
            ))
        }
        "identifier" => {
            let name = node_text(left, ctx.caller.source);
            if ctx.caller.locals.type_of(name).is_some() {
                return None;
            }
            ctx.caller
                .types
                .field_type(owner, name)
                .map(|_| format!("{}::{}", owner, name))
        }
        _ => None,
    }
}

/// Whether a target is standard-library noise.
///
/// A `std::`-rooted name says so outright. Everything else is judged only
/// when the receiver went untyped: `xs.push_back(y)` on an `xs` this file
/// never declared is almost certainly a container, while `Session::begin`
/// on a `Session` it did declare is a project call that happens to share
/// a name with one. Free functions are never judged by this table at all
/// — a bare `first()` is a call to something the project wrote.
fn is_filtered(target: &Target) -> bool {
    if is_std_qualified(&target.text) {
        return true;
    }
    if !target.untyped_receiver {
        return false;
    }
    let member = target.text.rsplit("::").next().unwrap_or(&target.text);
    is_std_member(member)
}

/// `MAX_RETRIES` yes, `MaxRetries` and `max_retries` no. A single
/// uppercase letter is a template parameter, not a constant.
fn is_screaming_case(name: &str) -> bool {
    name.len() > 1
        && name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Drop the template arguments from a qualified name so the target is a
/// key rather than a spelling: `Cache<int>::get` → `Cache::get`.
fn strip_template_args(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

// --- lambdas ---------------------------------------------------------------

/// A lambda owns its own scope: the calls inside it are its calls, not the
/// enclosing function's (§B7). It gets an entity with a positional name,
/// the same metric set as any other callable (§D4), and the enclosing
/// callable as parent.
fn handle_lambda(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let index = ctx.next_lambda();
    let name = format!(
        "<lambda@{}:{}>",
        node.start_position().row + 1,
        node.start_position().column
    );
    let mut entity = CodeEntity::new(&name, EntityKind::Function, ctx.caller.path, node_to_span(node));
    entity.id = format!("{}::lambda::{}", ctx.caller.id, index);
    entity.qualified_name = format!("{}::{}", ctx.caller.id, name);
    entity.parent_id = Some(ctx.caller.id.to_string());
    entity.visibility = Visibility::Private;
    entity.tags.insert("lambda".to_string());
    entity.tags.insert("closure".to_string());
    entity.source_code = Some(node_text(node, ctx.caller.source).to_string());
    if let Some(declarator) = node.child_by_field_name("declarator") {
        if let Some(params) = declarator.child_by_field_name("parameters") {
            entity.parameters = super::super::helpers::parse_parameters(&params, ctx.caller.source);
        }
    }
    entity.metrics.param_count = Some(entity.parameters.len() as u32);
    entity.metrics.loc = super::super::helpers::line_count(&entity);

    let body = node.child_by_field_name("body");
    super::super::complexity::score_body(&mut entity, body.as_ref());
    working_set::populate(&mut entity, body.as_ref(), ctx.caller.source);
    crate::parser::loops::populate(&mut entity, body.as_ref());

    let lambda_id = entity.id.clone();
    let lambda_name = entity.name.clone();
    ctx.walk.result.add_relationship(lambda_edge(ctx, &lambda_id, branch));
    ctx.walk.result.add_entity(entity);

    let Some(body) = body else { return };
    let caller = Caller {
        source: ctx.caller.source,
        path: ctx.caller.path,
        id: &lambda_id,
        name: &lambda_name,
        owner: ctx.caller.owner,
        types: ctx.caller.types,
        locals: Locals::for_body(node, &body, ctx.caller.source),
    };
    let mut inner = CallCtx::new(caller, &mut *ctx.walk.result);
    extract_calls(&body, &mut inner, None);
}

/// The enclosing body names the lambda without calling it there — the
/// same fact `UsesFn` records for a function handed to a dispatcher.
fn lambda_edge(ctx: &CallCtx<'_>, lambda_id: &str, branch: Option<&str>) -> Relationship {
    let mut rel = Relationship::new(
        ctx.caller.id.to_string(),
        lambda_id.to_string(),
        RelationshipKind::UsesFn,
    );
    if let Some(arm) = branch {
        rel.metadata.insert("branch".to_string(), arm.to_string());
    }
    rel
}

// --- control flow ----------------------------------------------------------

/// `if (cond) … else if (cond2) … else …`. C++ spells the else as an
/// `else_clause`, and the chained form wraps another `if_statement` in
/// one. The user-visible model is a flat list of arms, so the chain is
/// flattened here: siblings read as `c1` / `c2` / `c3` rather than
/// nesting, matching how Java and Python present the same source.
fn handle_if(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let mut current = Some(*node);
    while let Some(statement) = current {
        if let Some(condition) = statement.child_by_field_name("condition") {
            extract_calls(&condition, ctx, branch);
        }
        if let Some(body) = statement.child_by_field_name("consequence") {
            emit_arm(ctx, branch, &body);
        }
        current = next_alternative(&statement, ctx, branch);
    }
}

/// Walk one `else`, and hand back the `if` it chains to when it is an
/// `else if`. A terminal `else` becomes an arm here and ends the chain.
fn next_alternative<'t>(
    statement: &Node<'t>,
    ctx: &mut CallCtx<'_>,
    branch: Option<&str>,
) -> Option<Node<'t>> {
    let clause = statement.child_by_field_name("alternative")?;
    let mut cursor = clause.walk();
    let body = clause.children(&mut cursor).find(|c| c.is_named())?;
    if body.kind() == "if_statement" {
        return Some(body);
    }
    emit_arm(ctx, branch, &body);
    None
}

/// Emit one branch arm and walk its body under that arm's path.
fn emit_arm(ctx: &mut CallCtx<'_>, branch: Option<&str>, body: &Node) {
    let path = ctx.next_arm_path(branch);
    emit_branch_entity(&ctx.arm(branch, &path), body, ctx.walk.result);
    extract_calls(body, ctx, Some(&path));
}

/// `switch (subject) { case A: … default: … }`. The subject runs once
/// before any arm and stays in the enclosing flow; each `case_statement`
/// — `default:` included — becomes one arm, tagged with its label.
fn handle_switch(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    if let Some(condition) = node.child_by_field_name("condition") {
        extract_calls(&condition, ctx, branch);
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    let cases: Vec<Node> = body.children(&mut cursor).collect();
    for case in cases {
        if case.kind() != "case_statement" {
            extract_calls(&case, ctx, branch);
            continue;
        }
        let pattern = case
            .child_by_field_name("value")
            .map(|v| node_text(&v, ctx.caller.source).to_string());
        let path = ctx.next_arm_path(branch);
        emit_case_arm_entity(
            &ctx.arm(branch, &path),
            &case,
            pattern.as_deref(),
            ctx.walk.result,
        );
        walk_children(&case, ctx, Some(&path));
    }
}

/// `try { … } catch (…) { … }`. The try body is the first arm; each
/// `catch_clause` follows with its declared exception type recorded.
fn handle_try(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    if let Some(body) = node.child_by_field_name("body") {
        let path = ctx.next_arm_path(branch);
        emit_try_arm_entity(&ctx.arm(branch, &path), &body, "try_body", None, ctx.walk.result);
        extract_calls(&body, ctx, Some(&path));
    }
    let mut cursor = node.walk();
    let clauses: Vec<Node> = node
        .children(&mut cursor)
        .filter(|c| c.kind() == "catch_clause")
        .collect();
    for clause in clauses {
        handle_catch(&clause, ctx, branch);
    }
}

fn handle_catch(clause: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let Some(body) = clause.child_by_field_name("body") else {
        return;
    };
    let caught = clause
        .child_by_field_name("parameters")
        .map(|p| node_text(&p, ctx.caller.source).trim_matches(['(', ')']).to_string());
    let path = ctx.next_arm_path(branch);
    emit_try_arm_entity(
        &ctx.arm(branch, &path),
        &body,
        "catch",
        caught.as_deref(),
        ctx.walk.result,
    );
    extract_calls(&body, ctx, Some(&path));
}

/// `for (init; cond; update) { … }`. All three header clauses run as part
/// of the loop's cycle, so their calls group under the loop path.
fn handle_for(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let path = open_loop(node, ctx, branch);
    for field in ["initializer", "condition", "update"] {
        if let Some(part) = node.child_by_field_name(field) {
            extract_calls(&part, ctx, Some(&path));
        }
    }
    walk_body(node, ctx, &path);
}

/// `for (auto& x : xs) { … }`. The iterated expression groups under the
/// loop for the same reason a `while` condition does.
fn handle_range_for(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let path = open_loop(node, ctx, branch);
    if let Some(right) = node.child_by_field_name("right") {
        extract_calls(&right, ctx, Some(&path));
    }
    walk_body(node, ctx, &path);
}

/// `while (cond) { … }` and `do { … } while (cond)`. The two are the same
/// shape for attribution: every call inside belongs to the loop, and the
/// order the condition runs in does not change whose loop it is.
fn handle_while(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let path = open_loop(node, ctx, branch);
    if let Some(condition) = node.child_by_field_name("condition") {
        extract_calls(&condition, ctx, Some(&path));
    }
    walk_body(node, ctx, &path);
}

/// Take the next loop path and emit its entity when the loop has a body.
fn open_loop(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) -> String {
    let path = ctx.next_loop_path(branch);
    if let Some(body) = node.child_by_field_name("body") {
        emit_loop_entity(&ctx.arm(branch, &path), &body, ctx.walk.result);
    }
    path
}

fn walk_body(node: &Node, ctx: &mut CallCtx<'_>, path: &str) {
    if let Some(body) = node.child_by_field_name("body") {
        extract_calls(&body, ctx, Some(path));
    }
}

fn walk_children(node: &Node, ctx: &mut CallCtx<'_>, branch: Option<&str>) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        extract_calls(&child, ctx, branch);
    }
}
