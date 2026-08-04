//! Python class parsing — including superclass detection, dataclass / enum /
//! abstract promotion, and class field collection.

use super::super::language_parser::{node_text, node_to_span};
use super::decorators::{emit_decorator_edges, extract_decorators};
use super::docstrings::extract_docstring;
use super::generics::collect_type_parameters;
use super::ExtractCtx;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use std::collections::HashSet;
use std::path::Path;
use tree_sitter::Node;

/// Recursion target: callback used by `handle_class` to walk back into the
/// generic dispatcher in `mod.rs` for the class body.
pub(super) type ExtractInto = fn(Node, Option<&str>, &mut ExtractCtx<'_>);

pub(super) fn handle_class(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    extract_entities: ExtractInto,
) {
    if let Some(entity) = parse_class(node, ctx.source, ctx.path, parent_id) {
        let entity_id = entity.id.clone();
        ctx.result.add_entity(entity);
        emit_decorator_edges(node, ctx.source, &entity_id, ctx.result);
        if let Some(body) = node.child_by_field_name("body") {
            extract_entities(body, Some(&entity_id), ctx);
        }
    }
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
    for (base, tag) in [
        ("NamedTuple", "namedtuple"),
        ("TypedDict", "typeddict"),
    ] {
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
    collect_self_assignments(&body, source, fields, &mut seen);
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

fn collect_class_annotations(
    node: &Node,
    source: &str,
    fields: &mut Vec<Parameter>,
    seen: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "expression_statement" {
            let mut inner = child.walk();
            for expr in child.children(&mut inner) {
                if expr.kind() == "assignment" {
                    if let Some(left) = expr.child_by_field_name("left") {
                        if left.kind() == "identifier" {
                            let field_name = node_text(&left, source).to_string();
                            if seen.insert(field_name.clone()) {
                                let type_name = expr
                                    .child_by_field_name("type")
                                    .map(|t| node_text(&t, source).to_string());
                                let default_value = expr
                                    .child_by_field_name("right")
                                    .map(|v| node_text(&v, source).to_string());
                                let visibility = Some(field_visibility(&field_name));
                                fields.push(Parameter {
                                    name: field_name,
                                    type_name,
                                    default_value,
                                    visibility,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

fn collect_self_assignments(
    node: &Node,
    source: &str,
    fields: &mut Vec<Parameter>,
    seen: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "expression_statement" {
            let mut inner = child.walk();
            for expr in child.children(&mut inner) {
                if expr.kind() == "assignment" {
                    if let Some(left) = expr.child_by_field_name("left") {
                        // Only plain `self.<identifier>` assignments are
                        // field declarations. `self.x[k] = …` is a
                        // subscript write on an existing dict/list and
                        // `self.a.b = …` mutates a nested object — neither
                        // declares a new field on this class. In
                        // tree-sitter-python both forms have `left.kind()`
                        // != "attribute", so we can gate on that.
                        if left.kind() != "attribute" {
                            continue;
                        }
                        let obj = left.child_by_field_name("object");
                        let attr = left.child_by_field_name("attribute");
                        let Some(obj) = obj else { continue };
                        let Some(attr) = attr else { continue };
                        if node_text(&obj, source) != "self" {
                            continue;
                        }
                        let field_name = node_text(&attr, source).to_string();
                        if !field_name.is_empty() && seen.insert(field_name.clone()) {
                            let type_name = expr
                                .child_by_field_name("type")
                                .map(|t| node_text(&t, source).to_string());
                            let visibility = Some(field_visibility(&field_name));
                            fields.push(Parameter {
                                name: field_name,
                                type_name,
                                default_value: None,
                                visibility,
                            });
                        }
                    }
                }
            }
        }
        // Recurse into function definitions and blocks.
        if child.kind() == "function_definition" || child.kind() == "block" {
            collect_self_assignments(&child, source, fields, seen);
        }
    }
}
