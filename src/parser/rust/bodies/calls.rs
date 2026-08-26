//! Call-site extraction for Rust function bodies.
//!
//! Resolves `Self::foo`, `self.foo()`, and `var.foo()` (via local-type
//! inference) into qualified `Type::method` callees so the graph resolver
//! can find them.

use super::super::ctx::ExtractCtx;
use super::inference::{
    closure_bindings, infer_local_types, infer_param_types, ReceiverPath, TypeEnv,
};
use super::stdlib::is_stdlib_function;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use crate::parser::rust_type_names::strip_generics;
use tree_sitter::Node;

/// The single entry point both callable kinds use — a free function and a
/// method in an `impl` block — so the inference wiring cannot drift between
/// them. Parameters are bound first and locals second, so a `let` rebinding
/// shadows the parameter of the same name.
///
/// `decl` is the whole `function_item`, whose parameter list types the
/// receivers; `body` is the block those calls are read from. `self_type` is
/// the enclosing impl's type name, which expands `Self::foo` / `self.foo()`
/// into `{Type}::foo` so the graph resolver can match them.
pub(in crate::parser::rust) fn extract_body_calls(
    decl: &Node,
    body: &Node,
    caller_id: &str,
    caller_name: &str,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let mut locals = infer_param_types(decl, ctx.source);
    locals.extend(infer_local_types(body, ctx.source));
    let env = TypeEnv {
        closures: closure_bindings(body, ctx.source),
        locals,
        fields: ctx.struct_fields,
        self_type,
    };
    let mut call_order = 0u32;
    extract_calls(
        body,
        ctx.source,
        caller_id,
        caller_name,
        &env,
        &mut call_order,
        ctx.result,
    );
}

/// Extract function/method calls from a code block with order tracking.
///
/// `caller_id` is the entity ID of the enclosing function/method — used
/// directly as the relationship source so callers are never ambiguous.
/// `caller_name` is kept only to suppress obvious recursive self-calls.
/// `self_type` is the enclosing impl's type name (if any), used to expand
/// `Self::foo` and `self.foo()` into `{Type}::foo` for disambiguation.
/// `local_types` maps variable names to inferred type names within the
/// current function body (populated by `infer_local_types`).
#[allow(clippy::too_many_arguments)]
fn extract_calls(
    node: &Node,
    source: &str,
    caller_id: &str,
    caller_name: &str,
    env: &TypeEnv<'_>,
    call_order: &mut u32,
    result: &mut ParseResult,
) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        // Unconditional, and a no-op for every kind but `arguments`: an
        // extra match arm here would charge this walker a branch it does
        // not need, and the check belongs with the thing it guards.
        record_fn_values(&child, source, caller_id, env, result);
        match child.kind() {
            "call_expression" => {
                if let Some(callee) = callee_of(&child, source, env) {
                    *call_order += 1;
                    if callee.callee != caller_name && !is_stdlib_function(&callee.callee) {
                        let mut rel = Relationship::new(
                            caller_id.to_string(),
                            callee.callee.clone(),
                            RelationshipKind::Calls,
                        );
                        rel.metadata
                            .insert("order".to_string(), call_order.to_string());
                        callee.tag(&mut rel);
                        if let Some(func_node) = child.child_by_field_name("function") {
                            record_call_site(&mut rel, &lsp_ident_node(&func_node));
                        }
                        result.add_relationship(rel);
                    }
                }
                extract_calls(
                    &child,
                    source,
                    caller_id,
                    caller_name,
                    env,
                    call_order,
                    result,
                );
            }
            "method_call_expression" => {
                // `receiver.method(args)` — qualify with the enclosing type
                // when the receiver is `self`, otherwise resolve the receiver
                // through local_types (e.g., `walker` → `FileWalker`) or
                // fall back to the receiver text.
                if let Some(method_node) = child.child_by_field_name("name") {
                    let method_name = node_text(&method_node, source).to_string();
                    *call_order += 1;
                    let method_call_ident = method_node;
                    let receiver_node = child.child_by_field_name("value");
                    let receiver_text = receiver_node
                        .as_ref()
                        .map(|v| node_text(v, source).to_string());
                    // Resolve the receiver — `self`, a variable, or a dotted
                    // path walked through parameter/field types.
                    let callee = member_callee(env, receiver_text.as_deref(), &method_name);
                    if method_name != caller_name && !is_stdlib_function(&method_name) {
                        let mut rel = Relationship::new(
                            caller_id.to_string(),
                            callee.callee.clone(),
                            RelationshipKind::Calls,
                        );
                        rel.metadata
                            .insert("order".to_string(), call_order.to_string());
                        callee.tag(&mut rel);
                        record_call_site(&mut rel, &method_call_ident);
                        result.add_relationship(rel);
                    }
                }
                extract_calls(
                    &child,
                    source,
                    caller_id,
                    caller_name,
                    env,
                    call_order,
                    result,
                );
            }
            _ => {
                extract_calls(
                    &child,
                    source,
                    caller_id,
                    caller_name,
                    env,
                    call_order,
                    result,
                );
            }
        }
    }
}

/// Record the functions this argument list *names* rather than calls.
///
/// Only path-qualified arguments count. `enums::parse_enum` handed to a
/// dispatcher is a dependency on `enums`; a bare `foo` in the same position
/// is overwhelmingly a local variable, and treating those as references
/// would bury the real edges under one ghost per argument in the codebase.
/// The qualified form is also the one the module-stem index keys on, so the
/// target resolves the same way a call to it would.
fn record_fn_values(
    arguments: &Node,
    source: &str,
    caller_id: &str,
    env: &TypeEnv<'_>,
    result: &mut ParseResult,
) {
    if arguments.kind() != "arguments" {
        return;
    }
    let mut cursor = arguments.walk();
    for arg in arguments.children(&mut cursor) {
        if arg.kind() != "scoped_identifier" {
            continue;
        }
        let Some(named) = scoped_value_name(&arg, source, env) else {
            continue;
        };
        if !is_fn_shaped(&named) {
            continue;
        }
        let mut rel = Relationship::new(caller_id.to_string(), named, RelationshipKind::UsesFn);
        record_call_site(&mut rel, &lsp_ident_node(&arg));
        result.add_relationship(rel);
    }
}

/// Whether a collapsed path names a function rather than a variant or a
/// borrowed built-in.
///
/// Measured before this filter existed: of 626 path-qualified arguments in
/// this repo, essentially none were function references. They were enum
/// variants — `EntityKind::Function`, `ShapePattern::Hierarchical`,
/// `Ordering::Relaxed` — which are values, not functions, and admitting
/// them put 626 spurious dependency edges into every folder's drawing.
///
/// Rust's own naming settles it: a module and a free function are
/// snake_case, a type or a variant is CamelCase. Requiring both halves to
/// be snake_case keeps `enums::parse_enum` and drops `EntityKind::Function`
/// and `String::from` alike — the latter is a real reference, but to a type
/// whose dependency `UsesType` already carries, and matching it bound 60
/// copies to an unrelated local `from`.
fn is_fn_shaped(collapsed: &str) -> bool {
    let mut segments = collapsed.split("::");
    let (Some(owner), Some(last)) = (segments.next(), segments.next()) else {
        return false;
    };
    let lowercase = |s: &str| s.chars().next().is_some_and(char::is_lowercase);
    // Both halves must read as a module path to a free function. `String::from`
    // and `Self::helper` are type-associated and belong to whatever owns the
    // type, which `UsesType` already records; admitting them bound 60 copies of
    // `String::from` to an unrelated local `from`.
    lowercase(owner) && lowercase(last) && !is_stdlib_function(last)
}

/// `path::to::Thing::name` collapsed to the `Thing::name` the resolver is
/// keyed on, with `Self::` expanded — the same reduction a call to the very
/// same path would get, so the two cannot disagree about what they point at.
fn scoped_value_name(node: &Node, source: &str, env: &TypeEnv<'_>) -> Option<String> {
    let cleaned: String = node_text(node, source)
        .chars()
        .filter(|c| *c != ' ')
        .collect();
    let stripped = strip_generics(&cleaned.replace("::<", "<"));
    let mut segments: Vec<&str> = stripped.split("::").collect();
    let last = segments.pop()?.to_string();
    Some(match (segments.pop(), env.self_type) {
        (Some("Self"), Some(ty)) => format!("{ty}::{last}"),
        (Some(owner), _) => format!("{owner}::{last}"),
        (None, _) => last,
    })
}

/// A callee string, plus the hint the analyzer needs when the receiver's
/// field types live outside this file.
pub(super) struct MemberCallee {
    /// What the resolver sees: `Type::member` when the receiver typed here,
    /// otherwise the receiver's source text, unchanged from before AN-012.
    pub callee: String,
    /// `(dotted type-and-field path, member name)` when the receiver typed
    /// down to a struct whose fields are declared elsewhere — `CodeEntity.kind`
    /// / `is_callable` for `entity.kind.is_callable()`. Rides the relationship
    /// as transient metadata until [`crate::analyzer`] finishes the walk.
    pub deferred: Option<(String, String)>,
}

impl MemberCallee {
    /// A callee that needs nothing further from the analyzer.
    fn plain(callee: String) -> Self {
        Self {
            callee,
            deferred: None,
        }
    }

    /// Copy the deferred-receiver hint onto a relationship's metadata, where
    /// the analyzer's cross-file pass picks it up. Nothing is written for a
    /// receiver that already resolved, so the keys only exist on the edges
    /// that pay for them.
    pub(super) fn tag(&self, rel: &mut Relationship) {
        if let Some((path, member)) = &self.deferred {
            rel.metadata.insert("recv_path".to_string(), path.clone());
            rel.metadata
                .insert("recv_member".to_string(), member.clone());
        }
    }
}

/// Qualify `receiver.member` with the receiver's type.
///
/// The one place both call shapes — `recv.method(args)` and `recv.field(args)`
/// — decide what the callee looks like, so `self`-qualification, dotted-path
/// resolution and the receiver-text fallback cannot drift apart between them.
fn member_callee(env: &TypeEnv<'_>, receiver: Option<&str>, member: &str) -> MemberCallee {
    let Some(receiver) = receiver else {
        return MemberCallee::plain(member.to_string());
    };
    if receiver == "self" {
        return MemberCallee::plain(match env.self_type {
            Some(ty) => format!("{}::{}", ty, member),
            None => member.to_string(),
        });
    }
    // Fall back to the receiver's source text, which matches no entity and
    // lands on a ghost: an unresolved receiver stays visibly unresolved
    // rather than collapsing onto a same-named method somewhere else.
    let unresolved = format!("{}::{}", receiver, member);
    match env.resolve_receiver(receiver) {
        Some(ReceiverPath { base, rest }) if rest.is_empty() => {
            MemberCallee::plain(format!("{}::{}", base, member))
        }
        // Typed as far as this file reaches; the analyzer resumes from `base`.
        Some(ReceiverPath { base, rest }) => MemberCallee {
            callee: unresolved,
            deferred: Some((
                std::iter::once(base)
                    .chain(rest)
                    .collect::<Vec<_>>()
                    .join("."),
                member.to_string(),
            )),
        },
        None => MemberCallee::plain(unresolved),
    }
}

/// Record the 0-based (line, byte-column) of a call-site identifier onto a
/// `Calls` relationship, for the AN-004 rust-analyzer tracer to feed to
/// `textDocument/definition`. Kept in transient metadata (`lsp_line` /
/// `lsp_col`) — the tracer consumes and strips it before the graph is built,
/// so it never reaches renderers.
pub(super) fn record_call_site(rel: &mut Relationship, ident: &Node) {
    let pos = ident.start_position();
    rel.metadata
        .insert("lsp_line".to_string(), pos.row.to_string());
    rel.metadata
        .insert("lsp_col".to_string(), pos.column.to_string());
}

/// The identifier a language server should be asked to resolve for a callee
/// — the *final* path segment, not the whole path. Pointing rust-analyzer at
/// `foo` in `a::b::foo()` resolves the module `a`, not the function; it must
/// land on `foo`. Falls back to the node itself for simple identifiers.
fn lsp_ident_node<'a>(function_node: &Node<'a>) -> Node<'a> {
    match function_node.kind() {
        "scoped_identifier" => function_node
            .child_by_field_name("name")
            .unwrap_or(*function_node),
        "field_expression" => function_node
            .child_by_field_name("field")
            .unwrap_or(*function_node),
        "generic_function" => function_node
            .child_by_field_name("function")
            .map(|n| lsp_ident_node(&n))
            .unwrap_or(*function_node),
        _ => *function_node,
    }
}

/// Extract the callee form from a call expression.
/// Returns qualified `Type::method` when the call site provides the type,
/// with `Self::` substituted by the enclosing impl's type when known.
/// `local_types` is used to resolve variable receivers to their inferred types.
/// [`extract_callee_name`], minus the calls that name a closure bound in this
/// body (AN-029).
///
/// A wrapper rather than a branch inside either neighbour: the complexity gate
/// fails on any metric increase to a function that already exists (CI-001),
/// and both of those are walkers with no room left.
///
/// Dropping the edge loses nothing real. The closure's body sits inside this
/// one, so whatever it calls is already attributed to this caller by the same
/// walk; what goes is only the edge naming the local binding itself. It is the
/// argument [`member_callee`] already makes for an unresolved receiver —
/// staying visibly unresolved beats collapsing onto a same-named stranger —
/// carried one step further.
fn callee_of(call_node: &Node, source: &str, env: &TypeEnv<'_>) -> Option<MemberCallee> {
    let callee = extract_callee_name(call_node, source, env)?;
    let names_a_local_closure = callee.deferred.is_none() && env.closures.contains(&callee.callee);
    (!names_a_local_closure).then_some(callee)
}

fn extract_callee_name(call_node: &Node, source: &str, env: &TypeEnv<'_>) -> Option<MemberCallee> {
    let func_node = call_node.child_by_field_name("function")?;

    match func_node.kind() {
        "identifier" => Some(MemberCallee::plain(
            node_text(&func_node, source).to_string(),
        )),
        "field_expression" => {
            // `expr.field(...)` — qualify with the enclosing type when
            // the receiver is `self`, resolve through local_types when
            // the receiver is a known variable, otherwise use receiver
            // text to avoid bare-name collisions.
            let field = func_node.child_by_field_name("field")?;
            let field_name = node_text(&field, source).to_string();
            let receiver = func_node.child_by_field_name("value");
            let receiver_text = receiver.as_ref().map(|v| node_text(v, source).to_string());
            Some(member_callee(env, receiver_text.as_deref(), &field_name))
        }
        "scoped_identifier" => {
            // `path::to::Thing::func` — strip any generic args and collapse
            // to `Type::func` so the resolver can find it. `Self::func`
            // gets rewritten to the enclosing impl's type.
            let text = node_text(&func_node, source);
            let cleaned: String = text.chars().filter(|c| *c != ' ').collect();
            let without_turbofish = cleaned.replace("::<", "<");
            // Drop any generic args: `Foo<T>::bar` → `Foo::bar`
            let stripped = strip_generics(&without_turbofish);
            let mut segments: Vec<&str> = stripped.split("::").collect();
            let func_seg = segments.pop()?.to_string();
            let type_seg = segments.pop().map(|s| s.to_string());
            let callee = match (type_seg.as_deref(), env.self_type) {
                (Some("Self"), Some(ty)) => format!("{}::{}", ty, func_seg),
                (Some(ty), _) => format!("{}::{}", ty, func_seg),
                (None, _) => func_seg,
            };
            Some(MemberCallee::plain(callee))
        }
        _ => None,
    }
}
