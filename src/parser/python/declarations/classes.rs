//! Python class parsing — including superclass detection, dataclass / enum /
//! abstract promotion, and class field collection.

use super::super::ctx::ExtractCtx;
use super::super::decorators::{emit_decorator_edges, extract_decorators};
use super::super::docstrings::extract_docstring;
use super::super::generics::collect_type_parameters;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::Node;

/// Register the class this node declares and return its id, so the
/// dispatcher can walk the body with the class as parent. Returning the id
/// rather than taking a callback keeps the dependency one-way: this module
/// knows about classes, and the dispatcher knows about recursion.
pub(super) fn handle_class(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Option<String> {
    let entity = parse_class(node, ctx.source, ctx.path, parent_id)?;
    let entity_id = entity.id.clone();
    ctx.result.add_entity(entity);
    emit_decorator_edges(node, ctx.source, &entity_id, ctx.result);
    Some(entity_id)
}

fn parse_class(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, EntityKind::Class, path, span);
    entity.parent_id = parent_id.map(String::from);
    entity.visibility = Visibility::Public;

    collect_bases(node, source, &mut entity);
    collect_type_parameters(node, source, &mut entity);

    // Decorators
    extract_decorators(node, source, &mut entity.attributes);

    promote_kind(&mut entity);
    tag_if_exception(&mut entity);

    // Docstring
    entity.documentation = extract_docstring(node, source);

    // Class fields (slots + class-level annotations + self.xxx assignments)
    if parse_class_fields(node, source, &mut entity.fields) {
        entity.tags.insert("has_slots".to_string());
    }

    populate_container_metrics(&mut entity);

    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

/// Walk the `superclasses` argument list, recording each base on
/// `entity.extends` and noting the ones that carry meaning of their own.
///
/// A subscripted base (`class Foo(Bag[int])`) is the case PY-016 fixes: the
/// base *is* `Bag`, and recording the literal text `Bag[int]` meant the
/// resolver looked for a class by that name and never found one. The
/// subscript arguments are kept on `entity.generics`, where they read as the
/// type parameters they are.
fn collect_bases(node: &Node, source: &str, entity: &mut CodeEntity) {
    let Some(superclasses) = node.child_by_field_name("superclasses") else {
        return;
    };
    let mut cursor = superclasses.walk();
    for child in superclasses.children(&mut cursor) {
        match child.kind() {
            "identifier" | "attribute" => push_base(node_text(&child, source), entity),
            "subscript" => collect_subscript_base(&child, source, entity),
            "keyword_argument" => {
                // `metaclass=ABCMeta` — not a base, but it does make the
                // class abstract.
                if node_text(&child, source).contains("ABCMeta") {
                    entity.tags.insert("abstract".to_string());
                    entity.attributes.push("abstract".to_string());
                }
            }
            _ => {}
        }
    }
}

/// A `Base[Arg, …]` base: the base name comes from the `value` field, the
/// arguments become generics.
fn collect_subscript_base(node: &Node, source: &str, entity: &mut CodeEntity) {
    let Some(value) = node.child_by_field_name("value") else {
        return;
    };
    push_base(node_text(&value, source), entity);
    let mut cursor = node.walk();
    for arg in node.children_by_field_name("subscript", &mut cursor) {
        entity.generics.push(node_text(&arg, source).to_string());
    }
}

/// Record one base name, tagging the two that change how the class is read.
fn push_base(base: &str, entity: &mut CodeEntity) {
    match base {
        "ABC" | "ABCMeta" => {
            entity.tags.insert("abstract".to_string());
            entity.attributes.push("abstract".to_string());
        }
        "Protocol" => {
            entity.tags.insert("protocol".to_string());
        }
        _ => {}
    }
    entity.extends.push(base.to_string());
}

/// Decide the class's `EntityKind` from its bases and decorators.
///
/// Precedence, highest first — the UI reads colour and code from the kind, so
/// only one can win:
///
/// 1. `AbstractClass` (extends `ABC`, or `metaclass=ABCMeta`). Abstract is the
///    most load-bearing signal; an abstract dataclass is exotic but legal, and
///    "abstract" is what a reader needs to see first.
/// 2. `Enum` (extends `Enum` / `IntEnum` / …).
/// 3. `Dataclass` — `@dataclass`, and also `NamedTuple` / `TypedDict`, which
///    are the same thing said differently (PY-014). Each keeps a tag naming
///    its flavour so the UI can differentiate later without re-parsing.
fn promote_kind(entity: &mut CodeEntity) {
    for (base, tag) in [("NamedTuple", "namedtuple"), ("TypedDict", "typeddict")] {
        if entity.extends.iter().any(|e| last_segment(e) == base) {
            entity.tags.insert(tag.to_string());
            entity.tags.insert("dataclass".to_string());
        }
    }
    if entity.attributes.iter().any(|a| a.contains("dataclass")) {
        entity.tags.insert("dataclass".to_string());
    }

    if entity.tags.contains("abstract") {
        entity.kind = EntityKind::AbstractClass;
        return;
    }
    if entity.extends.iter().any(|e| {
        matches!(
            last_segment(e),
            "Enum" | "IntEnum" | "StrEnum" | "Flag" | "IntFlag"
        )
    }) {
        entity.kind = EntityKind::Enum;
        return;
    }
    if entity.tags.contains("dataclass") {
        entity.kind = EntityKind::Dataclass;
    }
}

/// Tag classes that exist to be raised (PY-015).
///
/// Two signals, either of which is enough: a base from the builtin exception
/// hierarchy, or a name ending the way exception classes conventionally do.
/// The name rule is what catches the common `class ConfigError(AppError)`,
/// where the base is itself a project class this pass never sees.
///
/// Tag only — no new `EntityKind`. An exception class is still a class, and
/// the tag is what lets a UI colour it or a reader filter on it.
fn tag_if_exception(entity: &mut CodeEntity) {
    const EXCEPTION_BASES: &[&str] = &[
        "Exception",
        "BaseException",
        "Warning",
        "ArithmeticError",
        "AssertionError",
        "AttributeError",
        "BufferError",
        "EOFError",
        "ImportError",
        "IndexError",
        "KeyError",
        "LookupError",
        "MemoryError",
        "NameError",
        "NotImplementedError",
        "OSError",
        "IOError",
        "OverflowError",
        "RecursionError",
        "ReferenceError",
        "RuntimeError",
        "StopIteration",
        "StopAsyncIteration",
        "SyntaxError",
        "SystemError",
        "SystemExit",
        "TypeError",
        "ValueError",
        "ZeroDivisionError",
    ];
    let by_base = entity
        .extends
        .iter()
        .any(|e| EXCEPTION_BASES.contains(&last_segment(e)));
    let by_name = entity.name.ends_with("Error")
        || entity.name.ends_with("Exception")
        || entity.name.ends_with("Warning");
    if by_base || by_name {
        entity.tags.insert("exception".to_string());
    }
}

/// The trailing name of a dotted reference: `typing.NamedTuple` → `NamedTuple`.
/// Bases arrive either bare or module-qualified depending on how the file
/// imported them, and every rule above cares about the name, not the path.
fn last_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Size and encapsulation metrics for a class, mirroring the Rust parser's
/// struct pass. They feed the god-class / data-bag smell rules and the
/// composite score, which saw nothing but zeroes from Python before.
///
/// A class with no fields has no ratio — `None` rather than a NaN.
fn populate_container_metrics(entity: &mut CodeEntity) {
    entity.metrics.loc = (entity.span.end.line - entity.span.start.line + 1) as u32;
    entity.metrics.field_count = Some(entity.fields.len() as u32);
    if entity.fields.is_empty() {
        return;
    }
    let pub_count = entity
        .fields
        .iter()
        .filter(|f| matches!(f.visibility, Some(Visibility::Public)))
        .count();
    entity.metrics.public_field_ratio = Some(pub_count as f32 / entity.fields.len() as f32);
}

/// Python has no access modifiers, so the naming convention *is* the
/// declaration: a leading double underscore triggers name mangling
/// (private), a single leading underscore is the "internal, don't touch"
/// convention (protected), and a dunder (`__x__`) is neither — it's a
/// language protocol slot, which is public API.
///
/// Same rule `functions.rs` applies to method names, kept in sync so a
/// class's `public_field_ratio` and its methods' visibility agree.
fn field_visibility(name: &str) -> Visibility {
    if name.starts_with("__") && !name.ends_with("__") {
        Visibility::Private
    } else if name.starts_with('_') {
        Visibility::Protected
    } else {
        Visibility::Public
    }
}

/// Collect a class's fields into `fields`. Returns whether the class
/// declares `__slots__`.
fn parse_class_fields(class_node: &Node, source: &str, fields: &mut Vec<Parameter>) -> bool {
    let mut seen = HashSet::new();
    let Some(body) = class_node.child_by_field_name("body") else {
        return false;
    };
    // Slots first: the names it lists are field declarations, and seeding
    // `seen` with `__slots__` is what stops the annotation pass below from
    // also recording the tuple itself as a field named `__slots__`.
    let has_slots = collect_slots(&body, source, fields, &mut seen);
    // Class-level annotations (e.g., `name: str` in dataclasses).
    collect_class_annotations(&body, source, fields, &mut seen);
    // Instance attributes from self.xxx assignments.
    collect_self_assignments(&body, source, &HashMap::new(), fields, &mut seen);
    has_slots
}

/// `__slots__ = ("name", "age")` declares which attributes a class may have —
/// a structural declaration, not a field called `__slots__` (PY-022).
///
/// Every literal form is accepted: tuple, list, set, a `dict` (whose *keys*
/// are the slots and whose values are per-slot docstrings), and the bare
/// string that declares a single slot.
fn collect_slots(
    body: &Node,
    source: &str,
    fields: &mut Vec<Parameter>,
    seen: &mut HashSet<String>,
) -> bool {
    let Some(rhs) = find_slots_rhs(body, source) else {
        return false;
    };
    seen.insert("__slots__".to_string());
    for name in slot_names(&rhs, source) {
        if seen.insert(name.clone()) {
            let visibility = Some(field_visibility(&name));
            fields.push(Parameter {
                name,
                type_name: None,
                default_value: None,
                visibility,
            });
        }
    }
    true
}

/// The right-hand side of a class body's `__slots__ = …`, if it has one.
fn find_slots_rhs<'a>(body: &Node<'a>, source: &str) -> Option<Node<'a>> {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() != "expression_statement" {
            continue;
        }
        let Some(assign) = child.named_child(0) else {
            continue;
        };
        if assign.kind() != "assignment" {
            continue;
        }
        let Some(left) = assign.child_by_field_name("left") else {
            continue;
        };
        if node_text(&left, source) == "__slots__" {
            return assign.child_by_field_name("right");
        }
    }
    None
}

/// The names a `__slots__` right-hand side declares.
fn slot_names(rhs: &Node, source: &str) -> Vec<String> {
    // `__slots__ = "only"` is legal, and declares exactly one slot.
    if let Some(single) = string_literal_text(rhs, source) {
        return vec![single];
    }
    let mut cursor = rhs.walk();
    let names: Vec<String> = rhs
        .named_children(&mut cursor)
        .filter_map(|item| string_literal_text(&item, source))
        .collect();
    names
}

/// The text inside a string literal, or inside a dict entry's key. `None`
/// for anything that isn't one — a `__slots__` entry built by a comprehension
/// or a name lookup declares nothing we can read statically.
fn string_literal_text(node: &Node, source: &str) -> Option<String> {
    if node.kind() == "pair" {
        return string_literal_text(&node.child_by_field_name("key")?, source);
    }
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    let content = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "string_content")?;
    Some(node_text(&content, source).to_string())
}

/// Class-level annotated declarations: the `name: str` lines of a dataclass,
/// and any `LIMIT: int = 5` beside them.
fn collect_class_annotations(
    node: &Node,
    source: &str,
    fields: &mut Vec<Parameter>,
    seen: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "expression_statement" {
            continue;
        }
        let mut inner = child.walk();
        for expr in child.children(&mut inner) {
            record_annotation(&expr, source, fields, seen);
        }
    }
}

/// One `name: T [= default]` declaration, if that is what this expression is.
fn record_annotation(
    expr: &Node,
    source: &str,
    fields: &mut Vec<Parameter>,
    seen: &mut HashSet<String>,
) {
    if expr.kind() != "assignment" {
        return;
    }
    let Some(left) = expr.child_by_field_name("left") else {
        return;
    };
    if left.kind() != "identifier" {
        return;
    }
    let name = node_text(&left, source).to_string();
    if !seen.insert(name.clone()) {
        return;
    }
    fields.push(Parameter {
        visibility: Some(field_visibility(&name)),
        name,
        type_name: field_text(expr, "type", source),
        default_value: field_text(expr, "right", source),
    });
}

/// The source text of one of an assignment's fields, when it has it.
fn field_text(node: &Node, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| node_text(&n, source).to_string())
}

/// Instance attributes assigned onto `self` in a method body, and the types
/// those assignments give away.
///
/// A type reaches a field two ways, and Python spells both:
/// - `self.limit: int = 0` — the annotation is on the assignment itself.
/// - `def __init__(self, store: Store): self.store = store` — the annotation
///   is on the parameter the field is copied from. This is the shape most
///   Python classes use to say what they hold, and reading it is what lets a
///   call on `self.store` resolve to `Store.save` rather than to a ghost
///   named `store.save` (PY-031).
///
/// `params` is the enclosing `def`'s annotated parameters, refreshed on the
/// way into each one; at class-body level it is empty.
fn collect_self_assignments(
    node: &Node,
    source: &str,
    params: &HashMap<String, String>,
    fields: &mut Vec<Parameter>,
    seen: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "expression_statement" => record_self_assignment(&child, source, params, fields, seen),
            "function_definition" => {
                let inner = annotated_parameters(&child, source);
                collect_self_assignments(&child, source, &inner, fields, seen);
            }
            "block" => collect_self_assignments(&child, source, params, fields, seen),
            _ => {}
        }
    }
}

/// One `self.<name> = …` statement, if that is what this is.
fn record_self_assignment(
    stmt: &Node,
    source: &str,
    params: &HashMap<String, String>,
    fields: &mut Vec<Parameter>,
    seen: &mut HashSet<String>,
) {
    let mut inner = stmt.walk();
    for expr in stmt.children(&mut inner) {
        let Some(name) = self_field_name(&expr, source) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        fields.push(Parameter {
            visibility: Some(field_visibility(&name)),
            name,
            type_name: assigned_type(&expr, source, params),
            default_value: None,
        });
    }
}

/// The field a `self.<name> = …` declares, or `None` when it declares none.
///
/// Only plain `self.<identifier>` assignments are field declarations.
/// `self.x[k] = …` is a subscript write on an existing dict/list and
/// `self.a.b = …` mutates a nested object — neither declares a new field on
/// this class. In tree-sitter-python both forms have `left.kind()` !=
/// "attribute", so we can gate on that.
fn self_field_name(expr: &Node, source: &str) -> Option<String> {
    if expr.kind() != "assignment" {
        return None;
    }
    let left = expr.child_by_field_name("left")?;
    if left.kind() != "attribute" {
        return None;
    }
    let object = left.child_by_field_name("object")?;
    if node_text(&object, source) != "self" {
        return None;
    }
    let name = node_text(&left.child_by_field_name("attribute")?, source);
    (!name.is_empty()).then(|| name.to_string())
}

/// The type a `self.<field> = …` gives away, in the order the evidence gets
/// weaker: its own annotation, the annotation of the parameter it copies,
/// then the constructor it calls.
fn assigned_type(expr: &Node, source: &str, params: &HashMap<String, String>) -> Option<String> {
    if let Some(annotated) = field_text(expr, "type", source) {
        return Some(annotated);
    }
    let right = expr.child_by_field_name("right")?;
    match right.kind() {
        "identifier" => params.get(node_text(&right, source)).cloned(),
        "call" => constructed_type(&right, source),
        _ => None,
    }
}

/// The type `self.<field> = Renderer()` builds, if the call is a constructor.
///
/// "Is a constructor" is the same test call extraction already applies when
/// it decides between `Calls` and `Instantiates`: a capitalised callee names
/// a class. `self.x = json.loads(text)` is lowercase and declines, which is
/// right — the type of what `loads` returns is not written anywhere here.
/// A dotted callee keeps its last segment, so `self.x = models.Store()` is a
/// `Store`, matching how `receiver_name` reduces a receiver.
fn constructed_type(call: &Node, source: &str) -> Option<String> {
    let function = call.child_by_field_name("function")?;
    if !matches!(function.kind(), "identifier" | "attribute") {
        return None;
    }
    let name = node_text(&function, source).rsplit('.').next()?.trim();
    let constructs = name
        .chars()
        .next()
        .is_some_and(|c| c.is_uppercase() && c.is_alphabetic());
    constructs.then(|| name.to_string())
}

/// One `def`'s annotated parameters, as name → annotation text.
///
/// Only the annotated forms are collected: an unannotated parameter says
/// nothing about the type of the field it is copied into, and recording it
/// as `None` would be indistinguishable from not being a parameter at all.
fn annotated_parameters(function: &Node, source: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(parameters) = function.child_by_field_name("parameters") else {
        return out;
    };
    let mut cursor = parameters.walk();
    for param in parameters.children(&mut cursor) {
        if let Some((name, annotation)) = annotated_parameter(&param, source) {
            out.insert(name, annotation);
        }
    }
    out
}

/// One annotated parameter, as `(name, annotation)`.
///
/// A `typed_parameter` holds its name as a bare first child rather than
/// under a field; a `typed_default_parameter` names it properly. Everything
/// else — a bare `x`, an `*args`, a `**kwargs` — carries no annotation and
/// answers `None`.
fn annotated_parameter(param: &Node, source: &str) -> Option<(String, String)> {
    if !matches!(param.kind(), "typed_parameter" | "typed_default_parameter") {
        return None;
    }
    let annotation = param.child_by_field_name("type")?;
    let name = param
        .child_by_field_name("name")
        .or_else(|| param.named_child(0))
        .filter(|n| n.kind() == "identifier")?;
    Some((
        node_text(&name, source).to_string(),
        node_text(&annotation, source).to_string(),
    ))
}
