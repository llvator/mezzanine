//! Control-flow entity emission: branch arms (if/elif/else, try arms, case
//! arms, `with` bodies) and loops (for/while/comprehension). Each `emit_*`
//! function pushes one synthetic `Branch`/`Loop` `CodeEntity` into the parse
//! result so the UI can render decision trees and loop structure as graph
//! nodes.
//!
//! The shapes match [`crate::parser::go::bodies::flow`] one-for-one — the
//! same `branch_node` / `loop_node` tags, the same `case_arm` convention,
//! the same `pattern:` attribute — so one UI rendering handles both.

use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_to_span, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Where one synthetic flow entity sits: whose body it is in, which arm
/// encloses it, what its own path is, and which file to point at.
///
/// Grouped because these four always travel together and none of them means
/// anything alone — an arm path without the caller it counts from is not an
/// address.
pub(super) struct Arm<'a> {
    pub caller_id: &'a str,
    pub parent_branch: Option<&'a str>,
    pub path: &'a str,
    pub file: &'a Path,
}

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
/// conventions (id, parent_id, qualified_name, tag) — just a different
/// `EntityKind`. Kept separate so each flow-control concept gets its own
/// point of customisation later.
///
/// `is_async` distinguishes `async for` from `for` (PY-021); a `while` loop
/// is never async and always passes `false`.
pub(super) fn emit_loop_entity(
    arm: &Arm<'_>,
    body_node: &Node,
    is_async: bool,
    result: &mut ParseResult,
) {
    emit_flow_entity(arm, body_node, EntityKind::Loop, "loop_node", result);
    if let Some(entity) = result.entities.last_mut() {
        tag_async(entity, is_async, "async_loop");
    }
}

/// Emit a `Branch` entity into the parse result for a conditional arm.
/// Called once per arm entry, even when the arm contains no analysable calls
/// — that way an empty `else: pass` still renders as a child node of its
/// caller so the reader can see the decision tree is intentionally empty
/// rather than missing entirely.
///
/// The id/parent_id construction mirrors what the analyzer's
/// `create_branch_entities` would compute from tagged relationships, so when
/// both paths fire (an arm with calls) the analyzer's `contains_key` guard
/// deduplicates to this entity.
pub(super) fn emit_branch_entity(arm: &Arm<'_>, body_node: &Node, result: &mut ParseResult) {
    emit_flow_entity(arm, body_node, EntityKind::Branch, "branch_node", result);
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
/// `try_arm` is added on every arm so the UI can also filter / group the
/// whole construct as one unit.
pub(super) fn emit_try_arm_entity(
    arm: &Arm<'_>,
    body_node: &Node,
    arm_kind: &str,
    caught: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(arm, body_node, result);
    // Layer the try-specific metadata onto the entity we just pushed.
    // `add_entity` is a plain Vec push, so the last element is ours.
    let Some(entity) = result.entities.last_mut() else {
        return;
    };
    entity.tags.insert("try_arm".to_string());
    entity.tags.insert(format!("{}_arm", arm_kind));
    describe(entity, "caught", caught);
}

/// Variant of `emit_branch_entity` for the body of a `with` / `async with`
/// statement (PEP 343 context manager).
///
/// The body of the construct is treated as a synthetic Branch arm so calls
/// inside group visually under the resource — both the manager expression
/// itself (`open(path)`) and the body's calls re-source to this entity,
/// matching how a loop groups its iterator + body.
///
/// Adds a `with_node` tag plus, when applicable, an `async` tag and
/// attribute. The full `with_clause` text (e.g. `open(p) as f, lock`)
/// travels as a `manager:<text>` attribute and as the entity's documentation
/// so the detail panel can show what resource the block scopes without
/// re-parsing.
pub(super) fn emit_with_entity(
    arm: &Arm<'_>,
    body_node: &Node,
    is_async: bool,
    manager_text: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(arm, body_node, result);
    let Some(entity) = result.entities.last_mut() else {
        return;
    };
    entity.tags.insert("with_node".to_string());
    tag_async(entity, is_async, "async_with");
    describe(entity, "manager", manager_text);
}

/// Variant of `emit_branch_entity` for the arms of a `match` statement
/// (PEP 634 structural pattern matching).
///
/// Adds a `case_arm` tag (mirroring `except_arm` / `try_body_arm`) and, when
/// a pattern is provided, a `pattern:<text>` attribute plus a human-readable
/// `documentation` string. The pattern text is the same `case ... :` form
/// the user wrote, with the optional `if <guard>` suffix appended — so the
/// detail panel can show "Point(x=0, y=0)" or "[1, *rest] if rest" without
/// re-parsing.
pub(super) fn emit_case_arm_entity(
    arm: &Arm<'_>,
    body_node: &Node,
    pattern: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(arm, body_node, result);
    let Some(entity) = result.entities.last_mut() else {
        return;
    };
    entity.tags.insert("case_arm".to_string());
    describe(entity, "pattern", pattern);
}

/// The one detail a flow entity carries beyond its shape: the source text
/// that says which arm this is — the exception it caught, the resource it
/// scopes, the pattern it matched.
///
/// Written once as `<key>:<text>` in the attributes and `<key>: <text>` in
/// the documentation, because three callers wanting the same two lines is
/// three chances for the two spellings to drift apart.
fn describe(entity: &mut CodeEntity, key: &str, text: Option<&str>) {
    let Some(trimmed) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return;
    };
    entity.attributes.push(format!("{}:{}", key, trimmed));
    entity.documentation = Some(format!("{}: {}", key, trimmed));
}

/// The one shape a synthetic flow entity has, whichever kind it is: an id
/// derived from the caller and the arm path, a parent that is either the
/// enclosing arm or the caller itself, and private visibility because
/// nothing outside the callable can name it.
fn emit_flow_entity(
    arm: &Arm<'_>,
    body_node: &Node,
    kind: EntityKind,
    tag: &str,
    result: &mut ParseResult,
) {
    let entity_id = format!("{}::branch::{}", arm.caller_id, arm.path);
    let parent_id = match arm.parent_branch {
        Some(p) => format!("{}::branch::{}", arm.caller_id, p),
        None => arm.caller_id.to_string(),
    };
    let span = node_to_span(body_node);
    let mut entity = CodeEntity::new(arm.path.to_string(), kind, arm.file, span);
    entity.id = entity_id;
    entity.qualified_name = format!("{}::{}", arm.caller_id, arm.path);
    entity.parent_id = Some(parent_id);
    entity.tags.insert(tag.to_string());
    entity.visibility = Visibility::Private;
    result.add_entity(entity);
}
