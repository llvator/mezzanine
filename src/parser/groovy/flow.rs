//! Control-flow entity emission for Groovy: branch arms (try/catch/finally).
//!
//! Mirrors Python's [`super::super::python::flow`] helpers and the shapes
//! established by PY-001 (try arm tags / caught-type metadata) so the UI's
//! existing branch rendering handles Groovy's arms without changes.

use super::super::language_parser::{node_to_span, ParseResult};
use crate::models::{CodeEntity, EntityKind, Visibility};
use std::path::Path;
use tree_sitter::Node;

/// Emit a Loop entity for the body of a `for` / `while` / `do-while`.
/// Mirrors `emit_branch_entity`'s shape with a `loop_node` tag and
/// `EntityKind::Loop`. Loop bodies use the `l<idx>` path prefix while
/// arms use `c<idx>`, so a sibling decision tree, loop, and try chain
/// at the same scope read as `c1` / `l1` / `c2.…` rather than
/// colliding on one shared sequence.
pub(super) fn emit_loop_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    loop_path: &str,
    body_node: &Node,
    path: &Path,
    result: &mut ParseResult,
) {
    let entity_id = format!("{}::branch::{}", caller_id, loop_path);
    let parent_id = match parent_branch {
        Some(p) => format!("{}::branch::{}", caller_id, p),
        None => caller_id.to_string(),
    };
    let span = node_to_span(body_node);
    let mut entity = CodeEntity::new(loop_path.to_string(), EntityKind::Loop, path, span);
    entity.id = entity_id;
    entity.qualified_name = format!("{}::{}", caller_id, loop_path);
    entity.parent_id = Some(parent_id);
    entity.tags.insert("loop_node".to_string());
    entity.visibility = Visibility::Private;
    result.add_entity(entity);
}

/// Variant of `emit_branch_entity` for one arm of a Groovy `switch`
/// statement. Adds a `case_arm` tag plus, when the label carries a
/// pattern (`case 1`, `case String`, `case 1..10`, `case [1, 2, 3]`),
/// a `pattern:<text>` attribute and matching `documentation`. The
/// `default:` label has no pattern; the entity is still emitted so the
/// arm appears in the graph as an intentional branch.
pub(super) fn emit_case_arm_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    branch_path: &str,
    body_node: &Node,
    path: &Path,
    pattern: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(
        caller_id,
        parent_branch,
        branch_path,
        body_node,
        path,
        result,
    );
    if let Some(entity) = result.entities.last_mut() {
        entity.tags.insert("case_arm".to_string());
        if let Some(p) = pattern {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                entity.attributes.push(format!("pattern:{}", trimmed));
                entity.documentation = Some(format!("pattern: {}", trimmed));
            }
        }
    }
}

/// Emit a generic Branch entity for a control-flow arm body. Called by
/// the try-arm helpers below. Kept as a free function so a future
/// if/while/loop addition (out of scope for GR-002) can reuse it.
pub(super) fn emit_branch_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    branch_path: &str,
    body_node: &Node,
    path: &Path,
    result: &mut ParseResult,
) {
    let branch_id = format!("{}::branch::{}", caller_id, branch_path);
    let parent_id = match parent_branch {
        Some(p) => format!("{}::branch::{}", caller_id, p),
        None => caller_id.to_string(),
    };
    let span = node_to_span(body_node);
    let mut entity = CodeEntity::new(branch_path.to_string(), EntityKind::Branch, path, span);
    entity.id = branch_id;
    entity.qualified_name = format!("{}::{}", caller_id, branch_path);
    entity.parent_id = Some(parent_id);
    entity.tags.insert("branch_node".to_string());
    entity.visibility = Visibility::Private;
    result.add_entity(entity);
}

/// Emit a Branch entity for one arm of a `try` statement (the try body
/// itself, a `catch` clause, or a `finally` clause). Mirrors PY-001's
/// metadata shape so the same UI grouping fires without changes:
///
/// * a `try_arm` tag on every arm,
/// * a `<kind>_arm` tag (`try_body_arm`, `catch_arm`, `finally_arm`),
/// * for `catch` arms with one or more declared exception types, the
///   types travel as a `caught:<type>` attribute and the entity's
///   documentation. Multi-catch (`catch (Foo | Bar e)`) joins types
///   with ` | `, matching how the Python side renders `except (A, B)`.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_try_arm_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    branch_path: &str,
    body_node: &Node,
    path: &Path,
    arm_kind: &str,
    caught: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(
        caller_id,
        parent_branch,
        branch_path,
        body_node,
        path,
        result,
    );
    if let Some(entity) = result.entities.last_mut() {
        entity.tags.insert("try_arm".to_string());
        entity.tags.insert(format!("{}_arm", arm_kind));
        if let Some(c) = caught {
            let trimmed = c.trim();
            if !trimmed.is_empty() {
                entity.attributes.push(format!("caught:{}", trimmed));
                entity.documentation = Some(format!("caught: {}", trimmed));
            }
        }
    }
}
