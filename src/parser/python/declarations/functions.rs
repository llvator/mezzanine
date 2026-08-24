//! Python function and method parsing, including parameters and the
//! "fluent return self" detection.
//!
//! `handle_function` re-enters the dispatcher for nested `def`/`class`
//! definitions inside a function body so closures and inner classes
//! are still registered as their own entities.

use super::super::bodies::calls::extract_calls;
use super::super::bodies::complexity::{compute_complexity, count_return_tuple_elements};
use super::super::ctx::ExtractCtx;
use super::super::decorators::{emit_decorator_edges, extract_decorators};
use super::super::docstrings::extract_docstring;
use super::super::generics::collect_type_parameters;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

/// Register the function this node declares and return its id, so the
/// dispatcher can walk the body with the function as parent. Returning the
/// id rather than taking a callback keeps the dependency one-way: this
/// module knows about functions, the dispatcher knows about recursion.
pub(super) fn handle_function(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Option<String> {
    if let Some(mut entity) = parse_function(node, ctx.source, ctx.path, parent_id) {
        // `parse_function` defaults any function with a parent to
        // `Method`, which is wrong for a `def` nested inside another
        // `def` — that's still a Function (closures, decorators…).
        // Only keep the Method classification when the enclosing
        // entity is actually a class-like container.
        let parent_entity_kind = parent_id.and_then(|pid| {
            ctx.result
                .entities
                .iter()
                .find(|e| e.id == pid)
                .map(|e| e.kind)
        });
        let parent_is_class_like = matches!(
            parent_entity_kind,
            Some(EntityKind::Class)
                | Some(EntityKind::Dataclass)
                | Some(EntityKind::AbstractClass)
                | Some(EntityKind::Enum)
                | Some(EntityKind::Interface)
                | Some(EntityKind::Trait)
                | Some(EntityKind::Struct)
        );
        if matches!(entity.kind, EntityKind::Method) && !parent_is_class_like {
            entity.kind = EntityKind::Function;
        }
        let caller_id = entity.id.clone();
        let caller_name = entity.name.clone();
        let parent_class_name = parent_id.filter(|_| parent_is_class_like).and_then(|pid| {
            ctx.result
                .entities
                .iter()
                .find(|e| e.id == pid)
                .map(|e| e.name.clone())
        });
        // Fluent/iterator pattern: `return self` (no explicit return
        // annotation) implicitly yields an instance of the enclosing
        // class. Set `return_type` to the class name so the later
        // dependency resolver emits a Returns edge — otherwise the
        // method looks like a leaf in the tree view and the user has
        // no visual cue that `__iter__`/`__enter__`/builder setters
        // hand back `self`.
        if entity.return_type.is_none() {
            if let Some(body) = node.child_by_field_name("body") {
                if body_returns_self(&body, ctx.source) {
                    if let Some(cls) = &parent_class_name {
                        entity.return_type = Some(cls.clone());
                    }
                }
            }
        }
        ctx.result.add_entity(entity);
        emit_decorator_edges(node, ctx.source, &caller_id, ctx.result);
        if let Some(body) = node.child_by_field_name("body") {
            let mut call_order = 0u32;
            extract_calls(
                &body,
                ctx.source,
                ctx.path,
                &caller_id,
                &caller_name,
                parent_class_name.as_deref(),
                &mut call_order,
                None,
                ctx.result,
            );
        }
        // Nested `def`/`class` declarations inside the body are the
        // dispatcher's to register, once it has this id to parent them
        // onto. `extract_calls` deliberately skips them so the outer scope
        // doesn't claim their locals. Nested-function bodies come back
        // through here again, so arbitrary nesting works.
        return Some(caller_id);
    }
    None
}

fn parse_function(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let kind = if parent_id.is_some() {
        EntityKind::Method
    } else {
        EntityKind::Function
    };

    let mut entity = CodeEntity::new(&name, kind, path, span);
    entity.parent_id = parent_id.map(String::from);

    // Visibility from name convention.
    entity.visibility = if name.starts_with("__") && !name.ends_with("__") {
        Visibility::Private
    } else if name.starts_with('_') {
        Visibility::Protected
    } else {
        Visibility::Public
    };

    // Async detection — the node text starts with "async def" for async
    // functions (tree-sitter-python includes the keyword in the node).
    if node_text(node, source).starts_with("async ") {
        entity.tags.insert("async".to_string());
        entity.attributes.push("async".to_string());
    }

    tag_if_generator(node, &mut entity);
    collect_type_parameters(node, source, &mut entity);

    // Decorators (from decorated_definition parent).
    extract_decorators(node, source, &mut entity.attributes);

    apply_decorator_tags(&mut entity);

    // Tag __init__ as constructor.
    if name == "__init__" {
        entity.tags.insert("constructor".to_string());
    }

    // Tag dunder methods.
    if name.starts_with("__") && name.ends_with("__") {
        entity.tags.insert("dunder".to_string());
    }

    // Parameters.
    if let Some(params) = node.child_by_field_name("parameters") {
        entity.parameters = parse_parameters(&params, source);
    }

    // Return type annotation.
    if let Some(ret) = node.child_by_field_name("return_type") {
        let ret_text = node_text(&ret, source);
        // Strip the leading ` -> ` that tree-sitter may include.
        let cleaned = ret_text.trim_start_matches("->").trim().to_string();
        entity.return_type = Some(cleaned);
        entity.metrics.return_complexity = count_return_tuple_elements(&ret, source);
    }

    populate_body_metrics(node, &mut entity);

    // Docstring.
    entity.documentation = extract_docstring(node, source);

    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

/// Tag (and sometimes re-kind) a callable from the decorators already
/// collected onto its `attributes`.
///
/// `@property` is the one that changes the entity's kind rather than just
/// tagging it — a property reads as an attribute at every call site, so the
/// UI should not draw it as a method. `setter` / `getter` / `deleter` are
/// excluded: `@x.setter` decorates the *second* half of a property and is a
/// method on the descriptor, not a new property.
///
/// PY-020: an `@overload` signature is a typing-only declaration sharing its
/// name with the real implementation, so three overloads make one function
/// appear four times in the graph. The entity is kept — the detail panel
/// wants the signature — but tagged so a UI can fold the stubs under the
/// implementation. Its body is `...` by convention, which the call walk finds
/// nothing in anyway, so nothing else needs to change.
fn apply_decorator_tags(entity: &mut CodeEntity) {
    let has = |needle: &str| entity.attributes.iter().any(|a| a.contains(needle));

    if has("staticmethod") {
        entity.tags.insert("static".to_string());
    }
    if has("classmethod") {
        entity.tags.insert("classmethod".to_string());
    }
    if has("abstractmethod") {
        entity.tags.insert("abstract".to_string());
    }
    if has("overload") {
        entity.tags.insert("overload_stub".to_string());
    }

    let is_property = entity.attributes.iter().any(|a| {
        a.contains("property")
            && !a.contains("setter")
            && !a.contains("getter")
            && !a.contains("deleter")
    });
    if is_property {
        entity.tags.insert("property".to_string());
        entity.kind = EntityKind::Property;
    }
}

/// Populate per-callable metrics: LOC, parameter count, and the three
/// body-complexity numbers. Mirrors the Java parser's helper of the same
/// name so the two produce comparable scores.
///
/// Python's grammar always gives a `def` a body, so the `else` arm only
/// fires on a parse error. A stub body (`...`, `pass`, an `@overload`
/// signature, anything in a `.pyi`) scores `1 / 0 / 0` — one
/// straight-through path — rather than `None`, so the hotspot ranking sees
/// "measured and trivial" instead of "not measured".
fn populate_body_metrics(node: &Node, entity: &mut CodeEntity) {
    entity.metrics.loc = (entity.span.end.line - entity.span.start.line + 1) as u32;
    // `parse_parameters` already drops `self` / `cls`.
    entity.metrics.param_count = Some(entity.parameters.len() as u32);
    if let Some(body) = node.child_by_field_name("body") {
        let (cc, nesting, cog) = compute_complexity(&body);
        entity.metrics.cyclomatic = Some(cc);
        entity.metrics.max_nesting = Some(nesting);
        entity.metrics.cognitive_complexity = Some(cog);
    } else {
        entity.metrics.cyclomatic = Some(1);
        entity.metrics.max_nesting = Some(0);
        entity.metrics.cognitive_complexity = Some(0);
    }
}

/// Tag `entity` as a generator (and `async_generator` when also async)
/// if its body contains a `yield` / `yield from`. Pulled out of
/// `parse_function` so the dispatcher there stays at its grandfathered
/// complexity rather than growing each time we add a body-level
/// classifier.
fn tag_if_generator(node: &Node, entity: &mut CodeEntity) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    if !body_has_yield(&body) {
        return;
    }
    entity.tags.insert("generator".to_string());
    if entity.tags.contains("async") {
        entity.tags.insert("async_generator".to_string());
    }
}

/// Does this subtree contain a `yield` / `yield from` outside of any
/// nested function or class scope? Tree-sitter-python uses one
/// `yield` node-kind for both forms (the `from` keyword + expression
/// is a child). The scan stops at `function_definition`,
/// `class_definition`, and `lambda` so a yield inside a closure doesn't
/// promote the outer function to a generator — the inner callable is
/// the generator, not us.
fn body_has_yield(node: &Node) -> bool {
    if matches!(
        node.kind(),
        "function_definition" | "class_definition" | "lambda"
    ) {
        return false;
    }
    if node.kind() == "yield" {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if body_has_yield(&child) {
            return true;
        }
    }
    false
}

/// Does this body node contain a bare `return self` statement?
/// Matches only `return self` with no suffix — `return self.x`,
/// `return self.copy()`, and `return (self, other)` are excluded
/// since they don't hand back the enclosing instance directly.
/// Recurses through nested blocks so `return self` inside an
/// `if`/`for`/`try` still counts.
fn body_returns_self(node: &Node, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "return_statement" {
            let mut inner = child.walk();
            for rc in child.named_children(&mut inner) {
                // return_statement's only named child is the
                // returned expression. Accept identifier `self`.
                if rc.kind() == "identifier" && node_text(&rc, source) == "self" {
                    return true;
                }
            }
        }
        if body_returns_self(&child, source) {
            return true;
        }
    }
    false
}

fn parse_parameters(node: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(&child, source).to_string();
                // Skip self / cls — not real parameters for the visualizer.
                if name == "self" || name == "cls" {
                    continue;
                }
                params.push(Parameter {
                    name,
                    type_name: None,
                    default_value: None,
                    visibility: None,
                });
            }
            "typed_parameter" => {
                if let Some(p) = parse_typed_param(&child, source) {
                    params.push(p);
                }
            }
            "typed_default_parameter" => {
                if let Some(p) = parse_typed_default_param(&child, source) {
                    params.push(p);
                }
            }
            "default_parameter" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if name == "self" || name == "cls" {
                    continue;
                }
                let default_value = child
                    .child_by_field_name("value")
                    .map(|n| node_text(&n, source).to_string());
                params.push(Parameter {
                    name,
                    type_name: None,
                    default_value,
                    visibility: None,
                });
            }
            "list_splat_pattern" => {
                // *args without type annotation.
                let inner = node_text(&child, source);
                let name = if inner.starts_with('*') {
                    inner.to_string()
                } else {
                    format!("*{}", inner)
                };
                params.push(Parameter {
                    name,
                    type_name: None,
                    default_value: None,
                    visibility: None,
                });
            }
            "dictionary_splat_pattern" => {
                // **kwargs without type annotation.
                let inner = node_text(&child, source);
                let name = if inner.starts_with("**") {
                    inner.to_string()
                } else {
                    format!("**{}", inner)
                };
                params.push(Parameter {
                    name,
                    type_name: None,
                    default_value: None,
                    visibility: None,
                });
            }
            _ => {}
        }
    }

    params
}

/// Parse a `typed_parameter` node (e.g. `x: int`, `*args: int`).
fn parse_typed_param(node: &Node, source: &str) -> Option<Parameter> {
    // The first named child is the name part (identifier / list_splat_pattern / dict_splat_pattern).
    let first = node.named_child(0)?;
    let raw_name = node_text(&first, source);

    // Derive a clean name, prefixing * / ** as needed.
    let (name, skip_check) = match first.kind() {
        "list_splat_pattern" => {
            let clean = raw_name.trim_start_matches('*');
            (format!("*{}", clean), clean.to_string())
        }
        "dictionary_splat_pattern" => {
            let clean = raw_name.trim_start_matches('*');
            (format!("**{}", clean), clean.to_string())
        }
        _ => (raw_name.to_string(), raw_name.to_string()),
    };

    if skip_check == "self" || skip_check == "cls" {
        return None;
    }

    let type_name = node
        .child_by_field_name("type")
        .map(|t| node_text(&t, source).to_string());

    Some(Parameter {
        name,
        type_name,
        default_value: None,
        visibility: None,
    })
}

/// Parse a `typed_default_parameter` node (e.g. `x: int = 5`).
fn parse_typed_default_param(node: &Node, source: &str) -> Option<Parameter> {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, source).to_string())
        .unwrap_or_default();

    if name == "self" || name == "cls" {
        return None;
    }

    let type_name = node
        .child_by_field_name("type")
        .map(|t| node_text(&t, source).to_string());
    let default_value = node
        .child_by_field_name("value")
        .map(|v| node_text(&v, source).to_string());

    Some(Parameter {
        name,
        type_name,
        default_value,
        visibility: None,
    })
}
