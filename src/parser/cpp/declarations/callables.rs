//! Functions, methods, constructors, destructors and operators — and the
//! wiring that hands each body to call extraction.
//!
//! The awkward part of C++ is that a member function can be declared in
//! one place and defined in another, and the definition is a *top-level*
//! declaration that names its class in the declarator:
//!
//! ```cpp
//! double Order::total() const { … }   // in order.cpp
//! ```
//!
//! So the class a method belongs to is read from one of two places
//! depending on where it was written — the enclosing class body, or the
//! `qualified_identifier` in front of the name — and [`owner_of`] is the
//! single place that decides. When the named class is declared in this
//! file the parent is its entity id; when it is not, which is the usual
//! case for a `.cpp`, the bare class name is used instead. That is not a
//! compromise: `graph.rs` accepts a `parent_id` that is a bare type name
//! and registers `Type::method` from it, which is how the Rust parser's
//! impl blocks have always resolved.
//!
//! A declaration with no body is scored, not skipped. A prototype, a pure
//! virtual and an `= default` all describe one straight-through path, and
//! reporting `None` would drop them out of `mezz quality` entirely — the
//! mistake TS-003 and KT-002 already recorded.

use super::super::bodies::calls::{extract_calls, CallCtx, Caller};
use super::super::bodies::inference::Locals;
use super::super::complexity::score_body;
use super::super::ctx::{ExtractCtx, Scope};
use super::super::doc_comments::extract_doc;
use super::super::helpers::{
    declared_name, declared_type, function_declarator, join_scope, line_count, parse_parameters,
    qualified_parts, specifier_tags,
};
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use crate::parser::working_set;
use tree_sitter::Node;

/// A callable's identity, resolved from wherever C++ happened to spell it.
struct Signature {
    name: String,
    /// The class this callable belongs to, or `None` for a free function.
    owner: Option<String>,
    /// The namespace path to qualify by — the enclosing one, plus any the
    /// declarator named itself.
    namespace: String,
}

/// `T f(…) { … }` — a definition, with or without a body of its own.
pub(super) fn handle_definition(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    place(node, scope, ctx);
}

/// A declaration whose declarator is a function — a prototype in a header,
/// a method declared in a class body, a constructor declared without one.
pub(super) fn handle_prototype(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    place(node, scope, ctx);
}

/// Build the entity, add it, emit the types its signature names, and walk
/// its body when it has one.
fn place(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return;
    };
    let Some(function) = function_declarator(&declarator) else {
        return;
    };
    let Some(signature) = signature_of(&declarator, scope, ctx.source) else {
        return;
    };
    let entity = build(node, &declarator, &function, &signature, scope, ctx);
    let caller_id = entity.id.clone();
    let caller_name = entity.name.clone();

    // The parameters are in the declarator and the return type is not, so
    // both roots are read: a type used only as a return would otherwise
    // gain no dependent at all.
    let roots: Vec<Node> = node
        .child_by_field_name("type")
        .into_iter()
        .chain(std::iter::once(function))
        .collect();
    super::emit_signature_types(&entity, &roots, &[], ctx);
    ctx.result.add_entity(entity);

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let caller = Caller {
        source: ctx.source,
        path: ctx.path,
        id: &caller_id,
        name: &caller_name,
        owner: signature.owner.as_deref(),
        types: ctx.types,
        locals: Locals::for_body(&declarator, &body, ctx.source),
    };
    let mut call_ctx = CallCtx::new(caller, ctx.result);
    // A constructor's member-initialiser list runs before its body and
    // builds what the body then uses, so its calls are the constructor's.
    if let Some(initialisers) = super::field_initializers(node) {
        extract_calls(&initialisers, &mut call_ctx, None);
    }
    extract_calls(&body, &mut call_ctx, None);
}

/// Everything the entity holds that is read off the declaration.
fn build(
    node: &Node,
    declarator: &Node,
    function: &Node,
    signature: &Signature,
    scope: &Scope<'_>,
    ctx: &ExtractCtx<'_>,
) -> CodeEntity {
    let kind = if signature.owner.is_some() {
        EntityKind::Method
    } else {
        EntityKind::Function
    };
    let mut entity = CodeEntity::new(&signature.name, kind, ctx.path, node_to_span(node));
    entity.qualified_name = qualified_name(signature);
    entity.parent_id = signature.owner.as_deref().map(|owner| owner_of(owner, ctx));
    entity.visibility = visibility(scope, signature);
    entity.attributes = specifier_tags(node, Some(declarator));
    for tag in role_tags(signature, &entity.attributes) {
        entity.tags.insert(tag);
    }
    for tag in &entity.attributes {
        entity.tags.insert(tag.clone());
    }
    if let Some(params) = function.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, ctx.source);
    }
    entity.return_type = declared_type(node, Some(declarator), ctx.source);
    entity.documentation = extract_doc(node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    entity.metrics.loc = line_count(&entity);
    entity.metrics.param_count = Some(entity.parameters.len() as u32);

    let body = node.child_by_field_name("body");
    score_body(&mut entity, body.as_ref());
    working_set::populate(&mut entity, body.as_ref(), ctx.source);
    crate::parser::loops::populate(&mut entity, body.as_ref());
    entity
}

/// Read the name, the owning class and the namespace out of a declarator,
/// wherever C++ put them.
fn signature_of(declarator: &Node, scope: &Scope<'_>, source: &str) -> Option<Signature> {
    let name_node = declared_name(declarator)?;
    let (declared_scope, name) = qualified_parts(&name_node, source);
    if name.is_empty() {
        return None;
    }
    let Some(declared_scope) = declared_scope else {
        return Some(Signature {
            name,
            owner: scope.owner.map(str::to_string),
            namespace: scope.namespace.to_string(),
        });
    };
    // `void Order::total()` at namespace scope: the last segment of the
    // declared scope is the class, and anything before it is namespace.
    let (namespace, owner) = match declared_scope.rsplit_once("::") {
        Some((outer, class)) => (join_scope(scope.namespace, outer), class.to_string()),
        None => (scope.namespace.to_string(), declared_scope),
    };
    Some(Signature {
        name,
        owner: Some(owner),
        namespace,
    })
}

/// `app::core::Order::total`, or `app::core::run` for a free function.
fn qualified_name(signature: &Signature) -> String {
    match &signature.owner {
        Some(owner) => join_scope(&join_scope(&signature.namespace, owner), &signature.name),
        None => join_scope(&signature.namespace, &signature.name),
    }
}

/// The parent a method hangs off: the class's entity when this file
/// declares it, and the bare class name when it does not.
fn owner_of(owner: &str, ctx: &ExtractCtx<'_>) -> String {
    ctx.result
        .entities
        .iter()
        .find(|e| {
            e.name == owner
                && matches!(
                    e.kind,
                    EntityKind::Class | EntityKind::AbstractClass | EntityKind::Struct
                )
        })
        .map(|e| e.id.clone())
        .unwrap_or_else(|| owner.to_string())
}

/// A member defined out of line carries no access keyword — the class
/// body said it once, in a file this parse may not even have seen. Public
/// is the honest default there, and the access section is authoritative
/// only where the declaration actually sits inside a class body.
fn visibility(scope: &Scope<'_>, signature: &Signature) -> Visibility {
    match scope.owner {
        Some(_) => scope.access,
        None if signature.owner.is_some() => Visibility::Public,
        None => scope.access,
    }
}

/// What kind of member this is, said in tags (§B5): a constructor, a
/// destructor, an operator, or an ordinary function.
fn role_tags(signature: &Signature, attributes: &[String]) -> Vec<String> {
    let mut tags = Vec::new();
    if signature.name.starts_with('~') {
        tags.push("destructor".to_string());
    } else if Some(&signature.name) == signature.owner.as_ref() {
        tags.push("constructor".to_string());
    } else if signature.name.starts_with("operator") {
        tags.push("operator".to_string());
    }
    if attributes.iter().any(|a| a == "pure_virtual") {
        tags.push("abstract".to_string());
    }
    tags
}
