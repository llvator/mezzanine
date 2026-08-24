//! Dart callables: functions, methods, getters, setters, operators,
//! constructors and factories — and the wiring that hands each body to the
//! call extractor.
//!
//! Every one of them reaches this module as the same three things: the node
//! that carries the span and the docs, the `*_signature` node that names it,
//! and the body that may or may not follow. [`Callable`] is that triple, and
//! it is what lets one `handle` serve a top-level `int score(x) => x`, a
//! class's `Future<Order?> find(id) async { … }` and an abstract `void
//! doIt();` without a branch per shape.
//!
//! The one thing the grammar scatters is modifiers: `static` sits on
//! `method_signature`, `factory` on `factory_constructor_signature`,
//! `external` and `const` on the enclosing `declaration`. [`spine_modifiers`]
//! collects them by walking the declaration's spine, deliberately stopping
//! before the parameter list so a `final` parameter never reads as a `final`
//! member.

use super::super::bodies::calls::{extract_calls, CallCtx, Caller};
use super::super::complexity::compute_complexity;
use super::super::ctx::ExtractCtx;
use super::super::doc_comments;
use super::super::helpers::{
    child_of_any, child_of_kind, children_of_kind, declared_type, parse_generics, parse_parameters,
    span_over, visibility_of,
};
use super::attributes::is_modifier;
use crate::models::{CodeEntity, EntityKind, Span};
use crate::parser::language_parser::{node_text, node_to_span};
use tree_sitter::Node;

/// The `*_signature` kinds that name a callable.
const SIGNATURE_KINDS: &[&str] = &[
    "function_signature",
    "getter_signature",
    "setter_signature",
    "operator_signature",
    "constructor_signature",
    "constant_constructor_signature",
    "factory_constructor_signature",
    "redirecting_factory_constructor_signature",
];

/// The declaration wrappers a class member's callable can arrive in.
const MEMBER_HOLDERS: &[&str] = &["method_declaration", "declaration"];

/// Parts of a declaration that are not its spine. The modifier walk stops at
/// each: a `final` inside a parameter list belongs to the parameter.
const OFF_SPINE: &[&str] = &[
    "formal_parameter_list",
    "function_body",
    "type",
    "initializers",
    "arguments",
];

/// One callable as the grammar hands it over.
pub(super) struct Callable<'t> {
    /// Carries the entity's span and its documentation.
    pub anchor: Node<'t>,
    /// The `*_signature` node that names it.
    pub signature: Node<'t>,
    /// The body, absent for an abstract or external declaration.
    pub body: Option<Node<'t>>,
}

/// Parse a callable, register it, and extract the relationships in its body.
pub(super) fn handle(callable: &Callable<'_>, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let Some(entity) = parse(callable, parent_id, ctx) else {
        return;
    };
    let caller_id = entity.id.clone();
    let caller_name = entity.name.clone();
    // Resolve the owning type's name so receiver-less calls to siblings
    // qualify as `Class.method`, the way the resolver expects.
    let parent_class = parent_id.and_then(|pid| {
        ctx.result
            .entities
            .iter()
            .find(|e| e.id == pid)
            .map(|e| e.name.clone())
    });
    ctx.result.add_entity(entity);

    let Some(body) = callable.body else {
        return;
    };
    let caller = Caller {
        source: ctx.source,
        path: ctx.path,
        id: &caller_id,
        name: &caller_name,
        parent_class: parent_class.as_deref(),
    };
    let mut call_ctx = CallCtx::new(caller, &mut *ctx.result);
    extract_calls(&body, &mut call_ctx, None);
}

/// The declared name of a callable, and the tags that say what kind it is.
struct Named {
    name: String,
    tags: Vec<&'static str>,
}

fn parse(
    callable: &Callable<'_>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Option<CodeEntity> {
    let source = ctx.source;
    let signature = callable.signature;
    let named = name_of(&signature, source)?;
    let span = declaration_span(callable, source);

    let kind = if parent_id.is_some() {
        EntityKind::Method
    } else {
        EntityKind::Function
    };
    let mut entity = CodeEntity::new(&named.name, kind, ctx.path, span);
    entity.visibility = visibility_of(&named.name);
    entity.parent_id = parent_id.map(String::from);
    for tag in named.tags {
        entity.tags.insert(tag.to_string());
    }
    if parent_id.is_none() && !ctx.library.is_empty() {
        entity.qualified_name = format!("{}.{}", ctx.library, named.name);
    }

    entity.attributes = spine_modifiers(&callable.anchor);
    entity
        .attributes
        .extend(annotations(&callable.anchor, source));

    if let Some(generics) = child_of_kind(&signature, "type_parameters") {
        entity.generics = parse_generics(&generics, source);
    }
    // A setter and a constructor declare no return type, and the grammar
    // gives them no `type` child — so this is right for all of them.
    entity.return_type = declared_type(&signature, source);
    if let Some(params) = child_of_kind(&signature, "formal_parameter_list") {
        entity.parameters = parse_parameters(&params, source);
    }

    populate_body_metrics(callable, &mut entity);
    entity.documentation = doc_comments::extract(&callable.anchor, source);
    entity.source_code = Some(source[span.start.offset..span.end.offset].to_string());
    Some(entity)
}

/// The span a reader would call the declaration: the grammar's node, widened
/// over the doc comment written in front of it.
fn declaration_span(callable: &Callable<'_>, source: &str) -> Span {
    match doc_comments::preceding_doc(&callable.anchor, source) {
        Some(comment) => span_over(&comment, &callable.anchor),
        None => node_to_span(&callable.anchor),
    }
}

/// The name a callable is known by, plus what it is.
///
/// A constructor takes the class's own name, and a named one keeps the
/// suffix (`Cart.empty`) — that is how Dart itself refers to it, and it is
/// what keeps two constructors on one class from collapsing onto one entity.
/// An operator is named for the token it overloads (`operator+`).
fn name_of(signature: &Node, source: &str) -> Option<Named> {
    let identifiers = children_of_kind(signature, "identifier");
    let joined = |tags: Vec<&'static str>| {
        let name = identifiers
            .iter()
            .map(|n| node_text(n, source))
            .collect::<Vec<_>>()
            .join(".");
        (!name.is_empty()).then_some(Named { name, tags })
    };

    match signature.kind() {
        "constructor_signature" | "constant_constructor_signature" => joined(vec!["constructor"]),
        "factory_constructor_signature" | "redirecting_factory_constructor_signature" => {
            joined(vec!["constructor", "factory"])
        }
        "getter_signature" => joined(vec!["getter", "accessor"]),
        "setter_signature" => joined(vec!["setter", "accessor"]),
        "operator_signature" => {
            let operator = child_of_any(signature, &["binary_operator", "unary_operator"])
                .map(|n| node_text(&n, source).to_string())
                .unwrap_or_else(|| "?".to_string());
            Some(Named {
                name: format!("operator{}", operator),
                tags: vec!["operator"],
            })
        }
        _ => identifiers.first().map(|n| Named {
            name: node_text(n, source).to_string(),
            tags: Vec::new(),
        }),
    }
}

/// Every modifier written along a declaration's spine.
///
/// Recursive because Dart puts them at three different depths — see the
/// module header. [`OFF_SPINE`] is what keeps the recursion from wandering
/// into a parameter list or a body and collecting a keyword that belongs to
/// something else.
pub(super) fn spine_modifiers(node: &Node) -> Vec<String> {
    let mut found = Vec::new();
    collect_spine_modifiers(node, &mut found);
    found
}

fn collect_spine_modifiers(node: &Node, found: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_modifier(child.kind()) {
            found.push(child.kind().to_string());
        } else if child.is_named() && !OFF_SPINE.contains(&child.kind()) {
            collect_spine_modifiers(&child, found);
        }
    }
}

/// The annotations attached to a declaration. The grammar nests them one
/// level in for a method, so both levels are checked.
pub(super) fn annotations(anchor: &Node, source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut cursor = anchor.walk();
    for child in anchor.children(&mut cursor) {
        match child.kind() {
            "annotation" | "marker_annotation" => found.push(node_text(&child, source).to_string()),
            "method_declaration" | "declaration" => found.extend(annotations(&child, source)),
            _ => {}
        }
    }
    found
}

/// Populate per-callable metrics: LOC, parameter count, and the three body
/// complexity numbers.
///
/// A bodyless declaration — abstract, external, or an interface member —
/// scores `cyclomatic = 1` and zero nesting: it has one straight-through
/// path by virtue of existing as a signature. Leaving the metrics unset
/// instead would drop the entity out of `nao quality` entirely.
fn populate_body_metrics(callable: &Callable<'_>, entity: &mut CodeEntity) {
    entity.metrics.loc = (entity.span.end.line - entity.span.start.line + 1) as u32;
    entity.metrics.param_count = Some(entity.parameters.len() as u32);
    match callable.body {
        Some(body) => {
            let (cyclomatic, nesting, cognitive) = compute_complexity(&body);
            entity.metrics.cyclomatic = Some(cyclomatic);
            entity.metrics.max_nesting = Some(nesting);
            entity.metrics.cognitive_complexity = Some(cognitive);
        }
        None => {
            entity.metrics.cyclomatic = Some(1);
            entity.metrics.max_nesting = Some(0);
            entity.metrics.cognitive_complexity = Some(0);
        }
    }
}

/// Read a `class_member` as a callable, if it declares one.
///
/// A member is either a `method_declaration` (signature plus body) or a
/// bodyless `declaration`. Both wrap the same `*_signature` kinds, so the
/// only difference is whether a body follows — and a `declaration` that
/// wraps no signature at all is a field, which is what tells the dispatcher
/// to hand it to `fields` instead.
pub(super) fn from_member<'t>(member: &Node<'t>) -> Option<Callable<'t>> {
    let holder = child_of_any(member, MEMBER_HOLDERS)?;
    let signature = signature_within(&holder)?;
    Some(Callable {
        anchor: *member,
        signature,
        body: child_of_kind(&holder, "function_body"),
    })
}

/// The `*_signature` a holder wraps, whether directly or under the
/// `method_signature` the grammar inserts when a body follows.
fn signature_within<'t>(holder: &Node<'t>) -> Option<Node<'t>> {
    if let Some(signature) = child_of_any(holder, SIGNATURE_KINDS) {
        return Some(signature);
    }
    let method_signature = child_of_kind(holder, "method_signature")?;
    child_of_any(&method_signature, SIGNATURE_KINDS)
}

/// Read a top-level `function_declaration`, `getter_declaration` or
/// `setter_declaration` as a callable.
pub(super) fn from_top_level<'t>(node: &Node<'t>) -> Option<Callable<'t>> {
    Some(Callable {
        anchor: *node,
        signature: child_of_any(node, SIGNATURE_KINDS)?,
        body: child_of_kind(node, "function_body"),
    })
}
