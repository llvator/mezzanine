//! `const`/`let` lexical declarations — the entities created depend on the
//! initializer kind:
//! - arrow function or function expression → Function/Method entity (with
//!   call extraction from the body)
//! - object literal → Constant entity, plus one Method per function-valued
//!   member (see [`emit_object_members`])
//! - anything else → Constant entity

use super::super::language_parser::{node_text, node_to_span};
use super::calls::extract_body_calls;
use super::complexity::populate_body_metrics;
use super::helpers::{extract_type_text, parse_generics, parse_parameters};
use super::members;
use super::tsdoc::extract_tsdoc;
use super::ExtractCtx;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use tree_sitter::Node;

pub(super) fn handle_lexical_declaration(
    node: &Node,
    parent_id: Option<&str>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            handle_variable_declarator(node, &child, parent_id, self_type, ctx);
        }
    }
}

/// Route one declarator to the emitter its initializer calls for.
fn handle_variable_declarator(
    decl_node: &Node,
    node: &Node,
    parent_id: Option<&str>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let name_node = match node.child_by_field_name("name") {
        Some(n) if n.kind() == "identifier" => n,
        _ => return, // skip destructuring patterns
    };
    let name = node_text(&name_node, ctx.source).to_string();

    let Some(value_node) = node.child_by_field_name("value") else {
        emit_constant(decl_node, node, &name, parent_id, ctx);
        return;
    };

    match value_node.kind() {
        "arrow_function" | "function" => {
            emit_function(decl_node, node, &value_node, &name, parent_id, self_type, ctx);
        }
        "object" => {
            let owner_id = emit_constant(decl_node, node, &name, parent_id, ctx);
            emit_object_members(&value_node, &owner_id, &name, ctx);
        }
        _ => {
            emit_constant(decl_node, node, &name, parent_id, ctx);
        }
    }
}

/// Emit a Constant entity for a non-function binding and return its id, so an
/// object literal can parent its members to it.
fn emit_constant(
    decl_node: &Node,
    node: &Node,
    name: &str,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> String {
    let span = node_to_span(node);
    let mut entity = CodeEntity::new(name, EntityKind::Constant, ctx.path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);
    if let Some(type_ann) = node.child_by_field_name("type") {
        entity.return_type = Some(extract_type_text(&type_ann, ctx.source));
    }
    entity.documentation = extract_tsdoc(decl_node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    let id = entity.id.clone();
    ctx.result.add_entity(entity);
    id
}

/// Emit a Function/Method entity for an arrow function or function
/// expression, then walk its body for calls.
///
/// `span_node` owns the entity's span and the declared-type fallback (the
/// declarator for `const f: Handler = …`, the pair for an object member);
/// `doc_node` is where the TSDoc comment is looked for.
#[allow(clippy::too_many_arguments)]
fn emit_function(
    doc_node: &Node,
    span_node: &Node,
    value_node: &Node,
    name: &str,
    parent_id: Option<&str>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let span = node_to_span(span_node);
    let kind = if parent_id.is_some() {
        EntityKind::Method
    } else {
        EntityKind::Function
    };
    let mut entity = CodeEntity::new(name, kind, ctx.path, span);
    entity.visibility = Visibility::Public;
    entity.parent_id = parent_id.map(String::from);

    let mut ac = value_node.walk();
    for child in value_node.children(&mut ac) {
        if child.kind() == "async" {
            entity.tags.insert("async".to_string());
            entity.attributes.push("async".to_string());
            break;
        }
    }
    if value_node.kind() == "arrow_function" {
        entity.tags.insert("arrow".to_string());
    }

    if let Some(tp) = value_node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&tp, ctx.source);
    }
    entity.parameters = function_parameters(value_node, ctx);

    if let Some(ret) = value_node.child_by_field_name("return_type") {
        entity.return_type = Some(extract_type_text(&ret, ctx.source));
    } else if let Some(type_ann) = span_node.child_by_field_name("type") {
        // Type annotation on the variable: `const foo: () => string = …`
        entity.return_type = Some(extract_type_text(&type_ann, ctx.source));
    }

    // The span is the declarator, but the metrics describe the function it
    // holds — so the body measured here is the arrow's.
    populate_body_metrics(value_node.child_by_field_name("body"), &mut entity);

    entity.documentation = extract_tsdoc(doc_node, ctx.source);
    entity.source_code = Some(node_text(span_node, ctx.source).to_string());

    let caller_id = entity.id.clone();
    let caller_name = entity.name.clone();
    ctx.result.add_entity(entity);

    if let Some(body) = value_node.child_by_field_name("body") {
        extract_body_calls(
            &body,
            &caller_id,
            &caller_name,
            value_node.child_by_field_name("parameters"),
            self_type,
            ctx,
        );
    }
}

/// Parameters of an arrow function or function expression. A single-parameter
/// arrow written without parentheses (`x => …`) has no `parameters` list —
/// the grammar exposes the lone binding as `parameter` instead.
fn function_parameters(value_node: &Node, ctx: &ExtractCtx<'_>) -> Vec<Parameter> {
    if let Some(params) = value_node.child_by_field_name("parameters") {
        return parse_parameters(&params, ctx.source);
    }
    match value_node.child_by_field_name("parameter") {
        Some(param) => vec![Parameter {
            name: node_text(&param, ctx.source).to_string(),
            type_name: None,
            default_value: None,
            visibility: None,
        }],
        None => Vec::new(),
    }
}

/// Emit one Method per function-valued member of an object literal.
///
/// `export const api = { load() {…}, save: (x) => {…} }` is one of the most
/// common ways TypeScript and JavaScript group behaviour, and it used to be
/// invisible: the whole literal collapsed to a single Constant, its members
/// produced no entities, and — worse — the calls in their bodies produced no
/// edges at all, because nothing walked them. A silent zero, which is the
/// failure mode this parser's whole readiness batch was about.
///
/// Members are parented to the constant, so the graph registers them under
/// `<constant>.<member>` and a call to `api.load()` resolves. The constant's
/// name is also passed as the `self_type`, because `this` inside an object
/// method is the object itself.
///
/// Not covered: nested object literals (`{ handlers: { onClick() {…} } }`),
/// whose inner members have no entity to hang from, and `export default
/// { … }`, which is not a lexical declaration at all.
fn emit_object_members(
    object: &Node,
    owner_id: &str,
    owner_name: &str,
    ctx: &mut ExtractCtx<'_>,
) {
    let mut cursor = object.walk();
    for member in object.children(&mut cursor) {
        match member.kind() {
            // `{ load() {…} }` — shares its shape with a class method, so it
            // shares the parser too, metrics and call extraction included.
            "method_definition" => {
                members::handle_method(&member, Some(owner_id), Some(owner_name), ctx);
            }
            // `{ load: (id) => {…} }`
            "pair" => emit_object_pair(&member, owner_id, owner_name, ctx),
            _ => {}
        }
    }
}

/// Emit a `key: <function>` object member. Non-function values are left as
/// plain data — they carry no behaviour to attribute calls to.
fn emit_object_pair(pair: &Node, owner_id: &str, owner_name: &str, ctx: &mut ExtractCtx<'_>) {
    let (Some(key), Some(value)) = (
        pair.child_by_field_name("key"),
        pair.child_by_field_name("value"),
    ) else {
        return;
    };
    if !matches!(value.kind(), "arrow_function" | "function") {
        return;
    }
    // A quoted key (`{ 'on-click': () => … }`) names the member just as a
    // bare one does; the quotes are syntax, not part of the name.
    let name = node_text(&key, ctx.source)
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();
    emit_function(
        pair,
        pair,
        &value,
        &name,
        Some(owner_id),
        Some(owner_name),
        ctx,
    );
}
