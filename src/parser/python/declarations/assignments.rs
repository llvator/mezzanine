//! Module-level assignment parsing (constants, variables and type aliases).

use super::super::ctx::ExtractCtx;
use super::super::generics::is_type_param_factory;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

/// Parse a module-level `expression_statement` and register the binding it
/// declares, if any.
///
/// Module state is *declared* once. A later `X = …` or `X += …` rebinds the
/// same name rather than introducing a second one, so this is first-write-
/// wins — the same rule the local-variable pass in `calls.rs` applies inside
/// a function body. Without it, PY-019's augmented assignments would each
/// mint a duplicate entity.
pub(super) fn handle_module_assignment(node: &Node, ctx: &mut ExtractCtx<'_>) {
    let Some(entity) = parse_assignment(node, ctx.source, ctx.path, None) else {
        return;
    };
    let already_declared = ctx.result.entities.iter().any(|e| {
        e.name == entity.name
            && e.parent_id.is_none()
            && matches!(
                e.kind,
                EntityKind::Variable | EntityKind::Constant | EntityKind::TypeAlias
            )
    });
    if already_declared {
        return;
    }
    ctx.result.add_entity(entity);
}

pub(super) fn parse_assignment(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let mut inner_cursor = node.walk();
    for child in node.children(&mut inner_cursor) {
        // PY-019: `COUNTER += 1` declares module state exactly as `COUNTER =
        // 0` does. A module that only ever maintains its state with `+=` used
        // to look like it had none.
        if matches!(child.kind(), "assignment" | "augmented_assignment") {
            return parse_simple_assignment(&child, source, path, parent_id);
        }
    }
    None
}

/// PEP 695 `type Vec3 = tuple[float, float, float]`.
///
/// Its own statement kind, with no `assignment` inside, so it never reached
/// `parse_assignment`. The children are positional: the alias name, then the
/// aliased type.
pub(super) fn parse_type_alias_statement(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = node.named_child(0)?;
    let name = node_text(&name_node, source).to_string();
    let mut entity = CodeEntity::new(&name, EntityKind::TypeAlias, path, node_to_span(node));
    entity.parent_id = parent_id.map(String::from);
    entity.visibility = name_visibility(&name);
    if let Some(value) = node.named_child(1) {
        entity.return_type = Some(node_text(&value, source).to_string());
    }
    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

fn parse_simple_assignment(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Option<CodeEntity> {
    let left = node.child_by_field_name("left")?;
    let left_text = node_text(&left, source);

    // Skip self.x assignments (handled as class fields).
    if left_text.starts_with("self.") {
        return None;
    }

    // Only handle simple identifier assignments.
    if left.kind() != "identifier" {
        return None;
    }

    let name = left_text.to_string();
    let span = node_to_span(node);

    let mut entity = CodeEntity::new(&name, classify(node, &name, source), path, span);
    entity.parent_id = parent_id.map(String::from);
    entity.visibility = name_visibility(&name);
    if is_type_param_factory(node, source) {
        entity.tags.insert("type_param".to_string());
    }

    // Type annotation.
    if let Some(type_node) = node.child_by_field_name("type") {
        entity.return_type = Some(node_text(&type_node, source).to_string());
    }

    entity.source_code = Some(node_text(node, source).to_string());
    Some(entity)
}

/// What kind of binding is this?
///
/// A type alias wins over the UPPER_CASE rule: `JSON = Dict[str, Any]` is an
/// alias whatever its name looks like.
fn classify(node: &Node, name: &str, source: &str) -> EntityKind {
    if is_type_alias(node, source) {
        return EntityKind::TypeAlias;
    }
    if is_screaming_snake(name) {
        return EntityKind::Constant;
    }
    EntityKind::Variable
}

/// Is this assignment declaring a type alias (PY-013)?
///
/// Two signals. `X: TypeAlias = …` says so outright — PEP 613 exists for
/// exactly this ambiguity. Otherwise the right-hand side has to *look* like a
/// type expression: a subscript of a known type constructor, which is what
/// `Optional[User]`, `Dict[str, Any]` and `Callable[[int], None]` all are.
///
/// A bare `UserId = int` is deliberately *not* claimed. It is a type alias to
/// a human, but nothing in the syntax distinguishes it from binding the
/// `int` builtin to a name, and guessing wrong reclassifies real variables.
fn is_type_alias(node: &Node, source: &str) -> bool {
    const TYPE_CONSTRUCTORS: &[&str] = &[
        "Union",
        "Optional",
        "Callable",
        "Literal",
        "Annotated",
        "Final",
        "ClassVar",
        "Tuple",
        "List",
        "Dict",
        "Set",
        "FrozenSet",
        "Type",
        "Sequence",
        "Mapping",
        "MutableMapping",
        "Iterable",
        "Iterator",
        "Awaitable",
        "Coroutine",
        "Generator",
        "list",
        "dict",
        "set",
        "tuple",
        "frozenset",
        "type",
    ];
    if let Some(annotation) = node.child_by_field_name("type") {
        let text = node_text(&annotation, source);
        if text.rsplit('.').next().unwrap_or(text) == "TypeAlias" {
            return true;
        }
    }
    let Some(right) = node.child_by_field_name("right") else {
        return false;
    };
    if right.kind() != "subscript" {
        return false;
    }
    let Some(base) = right.child_by_field_name("value") else {
        return false;
    };
    let text = node_text(&base, source);
    TYPE_CONSTRUCTORS.contains(&text.rsplit('.').next().unwrap_or(text))
}

/// `MAX_RETRIES` — all caps, at least one letter.
fn is_screaming_snake(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
        && name.chars().any(|c| c.is_alphabetic())
}

/// Visibility from Python's underscore convention. Same rule as methods and
/// class fields.
fn name_visibility(name: &str) -> Visibility {
    if name.starts_with("__") && !name.ends_with("__") {
        Visibility::Private
    } else if name.starts_with('_') {
        Visibility::Protected
    } else {
        Visibility::Public
    }
}
