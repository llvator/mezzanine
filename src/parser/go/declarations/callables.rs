//! Go functions and methods, and the wiring that hands their bodies to
//! call extraction.
//!
//! The asymmetry with every other object language parsed here is the
//! receiver. `func (s *Server) Handle()` is a *top-level* declaration —
//! a sibling of `type Server struct`, not a child of it, and legal in any
//! file of the package. So the containment edge is built here rather than
//! by descending into a class body: the receiver's type is looked up among
//! the entities this file has already produced, and when it is not there
//! (the type lives in another file of the package, which is idiomatic) the
//! bare type name is used as the parent instead.
//!
//! That fallback is not a compromise — `graph.rs` accepts a `parent_id`
//! that is a bare type name and registers `Type.Method` from it, which is
//! how the Rust parser's impl blocks have always resolved.

use super::super::bodies::calls::{extract_calls, CallCtx, Caller};
use super::super::bodies::inference::Locals;
use super::super::complexity::compute_complexity;
use super::super::ctx::ExtractCtx;
use super::super::doc_comments::extract_doc;
use super::super::helpers::{
    parse_generics, parse_parameters, populate_signature_metrics, receiver, result_text,
    visibility_of,
};
use super::super::types::{self, TypeUse};
use crate::models::{CodeEntity, EntityKind};
use crate::parser::language_parser::{node_text, node_to_span};
use tree_sitter::Node;

/// Parse a `func Name(…)`, add it, and walk its body.
pub(super) fn handle_function(node: &Node, ctx: &mut ExtractCtx<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(&name_node, ctx.source).to_string();
    let mut entity = base_entity(&name, EntityKind::Function, node, ctx);
    if !ctx.package.is_empty() {
        entity.qualified_name = format!("{}.{}", ctx.package, name);
    }
    place(entity, node, None, ctx);
}

/// Parse a `func (r T) Name(…)`, attach it to `T`, and walk its body.
pub(super) fn handle_method(node: &Node, ctx: &mut ExtractCtx<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(recv) = receiver(node, ctx.source) else {
        return;
    };
    let name = node_text(&name_node, ctx.source).to_string();
    let mut entity = base_entity(&name, EntityKind::Method, node, ctx);
    entity.parent_id = Some(owner_id(&recv.type_name, ctx));
    entity.tags.insert(if recv.is_pointer {
        "pointer_receiver".to_string()
    } else {
        "value_receiver".to_string()
    });
    if !ctx.package.is_empty() {
        entity.qualified_name = format!("{}.{}.{}", ctx.package, recv.type_name, name);
    }
    let receiver_type = recv.type_name.clone();
    place(entity, node, Some(&receiver_type), ctx);
}

/// Everything a function and a method share: name, visibility, signature,
/// doc, source, and the metrics that read off a signature.
fn base_entity(name: &str, kind: EntityKind, node: &Node, ctx: &ExtractCtx<'_>) -> CodeEntity {
    let mut entity = CodeEntity::new(name, kind, ctx.path, node_to_span(node));
    entity.visibility = visibility_of(name);
    if let Some(generics) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&generics, ctx.source);
    }
    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, ctx.source);
    }
    entity.return_type = result_text(node, ctx.source);
    entity.documentation = extract_doc(node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    populate_signature_metrics(node, &mut entity);
    entity
}

/// Add the entity, then hand its body to call extraction.
///
/// A Go function without a body is not abstract — it is implemented in
/// assembly or by a compiler intrinsic (`func Sqrt(x float64) float64`
/// in `math`). It scores as one straight-through path, the same as an
/// interface method, because that is what a caller sees.
fn place(
    mut entity: CodeEntity,
    node: &Node,
    receiver_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let body = node.child_by_field_name("body");
    match body {
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

    let caller_id = entity.id.clone();
    let caller_name = entity.name.clone();
    ctx.result.add_entity(entity);

    // Every type the signature names is a dependency of this callable,
    // whether or not it ever calls anything.
    let signature: Vec<Node> = ["receiver", "parameters", "result"]
        .iter()
        .filter_map(|field| node.child_by_field_name(field))
        .collect();
    types::emit(
        &signature,
        &[],
        &TypeUse {
            entity_id: &caller_id,
            owner_name: receiver_type.unwrap_or(&caller_name),
            source: ctx.source,
            imports: ctx.imports,
        },
        ctx.result,
    );

    let Some(body) = body else { return };
    let caller = Caller {
        source: ctx.source,
        path: ctx.path,
        id: &caller_id,
        name: &caller_name,
        receiver_type,
        imports: ctx.imports,
        types: ctx.types,
        locals: Locals::for_body(node, &body, ctx.source),
    };
    let mut call_ctx = CallCtx::new(caller, ctx.result);
    extract_calls(&body, &mut call_ctx, None);
}

/// The parent a method hangs off: the receiver type's entity when this file
/// declares it, and the bare type name when it does not.
fn owner_id(type_name: &str, ctx: &ExtractCtx<'_>) -> String {
    ctx.result
        .entities
        .iter()
        .find(|e| {
            e.name == type_name
                && e.file_path == ctx.path
                && matches!(
                    e.kind,
                    EntityKind::Struct | EntityKind::Interface | EntityKind::TypeAlias
                )
        })
        .map(|e| e.id.clone())
        .unwrap_or_else(|| type_name.to_string())
}
