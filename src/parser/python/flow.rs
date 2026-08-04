//! Control-flow entity emission: branch arms (if/elif/else, try arms) and
//! loops (for/while). Each emit_* function pushes one synthetic
//! `Branch`/`Loop` `CodeEntity` into the parse result so the UI can render
//! decision trees and loop structure as graph nodes.

use super::super::language_parser::{node_to_span, ParseResult};
use crate::models::{CodeEntity, EntityKind, Visibility};
use std::path::Path;
use tree_sitter::Node;

/// Mark a synthetic flow entity as the `async` form of its construct
/// (PY-021).
///
/// `async for` and `async with` share a node kind with their sync forms —
/// the `async` is an anonymous leading token — so without an explicit tag an
/// async-heavy codebase reads as synchronous. `specific` is the construct's
/// own tag (`async_loop` / `async_with`), carried alongside the generic
/// `async` one so a reader can filter either way.
fn tag_async(entity: &mut CodeEntity, is_async: bool, specific: &str) {
    if !is_async {
        return;
    }
    entity.tags.insert("async".to_string());
    entity.tags.insert(specific.to_string());
    entity.attributes.push("async".to_string());
}

/// Twin of `emit_branch_entity` for loop bodies. Same structural
/// conventions (id, parent_id, qualified_name, tag) — just a
/// different `EntityKind`. Kept separate so each flow-control
/// concept gets its own point of customisation later.
///
/// `is_async` distinguishes `async for` from `for` (PY-021); a `while`
/// loop is never async and always passes `false`.
pub(super) fn emit_loop_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    loop_path: &str,
    body_node: &Node,
    path: &Path,
    is_async: bool,
    result: &mut ParseResult,
) {
    let entity_id = format!("{}::branch::{}", caller_id, loop_path);
    let parent_id = match parent_branch {
        Some(p) => format!("{}::branch::{}", caller_id, p),
        None => caller_id.to_string(),
    };
    let span = node_to_span(body_node);
    let mut entity = CodeEntity::new(
        loop_path.to_string(),
        EntityKind::Loop,
        path,
        span,
    );
    entity.id = entity_id;
    entity.qualified_name = format!("{}::{}", caller_id, loop_path);
    entity.parent_id = Some(parent_id);
    entity.tags.insert("loop_node".to_string());
    tag_async(&mut entity, is_async, "async_loop");
    entity.visibility = Visibility::Private;
    result.add_entity(entity);
}

/// Emit a `Branch` entity into the parse result for a conditional
/// arm. Called once per arm entry, even when the arm contains no
/// analysable calls — that way an empty `else: pass` still renders
/// as a child node of its caller so the reader can see the decision
/// tree is intentionally empty rather than missing entirely.
///
/// The id/parent_id construction mirrors what the analyzer's
/// `create_branch_entities` would compute from tagged relationships,
/// so when both paths fire (an arm with calls) the analyzer's
/// `contains_key` guard deduplicates to this entity.
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
    let mut entity = CodeEntity::new(
        branch_path.to_string(),
        EntityKind::Branch,
        path,
        span,
    );
    entity.id = branch_id;
    entity.qualified_name = format!("{}::{}", caller_id, branch_path);
    entity.parent_id = Some(parent_id);
    entity.tags.insert("branch_node".to_string());
    entity.visibility = Visibility::Private;
    result.add_entity(entity);
}

/// Variant of `emit_branch_entity` for the arms of a `try` statement.
/// Adds two extra signals on top of the standard Branch entity:
///
/// * an `arm_kind` tag (`try_body_arm`, `except_arm`, `except_group_arm`,
///   `else_arm`, `finally_arm`) so the UI can colour or label the arm
///   distinctly without having to inspect the source span, and
/// * the caught exception type (when present) — surfaced as a
///   `caught:<type>` attribute and as the entity's documentation so
///   the detail panel can show "ValueError" or "(IOError, OSError)"
///   without re-parsing.
///
/// `try_arm` is added on every arm so the UI can also filter / group
/// the whole construct as one unit.
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
    emit_branch_entity(caller_id, parent_branch, branch_path, body_node, path, result);
    // Layer the try-specific metadata onto the entity we just pushed.
    // `add_entity` is a plain Vec push, so the last element is ours.
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

/// Variant of `emit_branch_entity` for the body of a `with` /
/// `async with` statement (PEP 343 context manager).
///
/// The body of the construct is treated as a synthetic Branch arm so
/// calls inside group visually under the resource — both the manager
/// expression itself (`open(path)`) and the body's calls re-source to
/// this entity, matching how a loop groups its iterator + body.
///
/// Adds a `with_node` tag plus, when applicable, an `async` tag and
/// attribute. The full `with_clause` text (e.g. `open(p) as f, lock`)
/// travels as a `manager:<text>` attribute and as the entity's
/// documentation so the detail panel can show what resource the block
/// scopes without re-parsing.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_with_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    with_path: &str,
    body_node: &Node,
    path: &Path,
    is_async: bool,
    manager_text: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(caller_id, parent_branch, with_path, body_node, path, result);
    if let Some(entity) = result.entities.last_mut() {
        entity.tags.insert("with_node".to_string());
        tag_async(entity, is_async, "async_with");
        if let Some(m) = manager_text {
            let trimmed = m.trim();
            if !trimmed.is_empty() {
                entity.attributes.push(format!("manager:{}", trimmed));
                entity.documentation = Some(format!("manager: {}", trimmed));
            }
        }
    }
}

/// Variant of `emit_branch_entity` for the arms of a `match` statement
/// (PEP 634 structural pattern matching).
///
/// Adds a `case_arm` tag (mirroring `except_arm` / `try_body_arm`) and,
/// when a pattern is provided, a `pattern:<text>` attribute plus a
/// human-readable `documentation` string. The pattern text is the same
/// `case ... :` form the user wrote, with the optional `if <guard>`
/// suffix appended — so the detail panel can show "Point(x=0, y=0)"
/// or "[1, *rest] if rest" without re-parsing.
pub(super) fn emit_case_arm_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    branch_path: &str,
    body_node: &Node,
    path: &Path,
    pattern: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(caller_id, parent_branch, branch_path, body_node, path, result);
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
