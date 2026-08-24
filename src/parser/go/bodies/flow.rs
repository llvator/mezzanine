//! Control-flow entity emission for Go: branch arms (if / else-if / else,
//! switch cases, type-switch cases, select arms) and loop bodies.
//!
//! The shapes match [`crate::parser::java::bodies::flow`] one-for-one — the
//! same `branch_node` / `loop_node` tags, the same `case_arm` convention,
//! the same `pattern:` attribute — so the UI's existing decision-tree
//! rendering handles Go without a change.
//!
//! What Go has and the others do not is `select`. Its arms are cases over
//! channel readiness rather than over a value, and which one runs is
//! decided by the scheduler, not by the code above it. They are emitted as
//! case arms with an extra `select_arm` tag: a reader following the graph
//! should see that these arms are siblings, and also that nothing in the
//! function chose between them.

use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_to_span, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Where one synthetic flow entity sits: whose body it is in, which arm
/// encloses it, what its own path is, and which file to point at.
///
/// Grouped because these four always travel together and none of them
/// means anything alone — an arm path without the caller it counts from
/// is not an address.
pub(super) struct Arm<'a> {
    pub caller_id: &'a str,
    pub parent_branch: Option<&'a str>,
    pub path: &'a str,
    pub file: &'a Path,
}

/// Emit a Loop entity for the body of a `for`. Loop bodies use the `l<idx>`
/// path prefix while arms use `c<idx>`, so a sibling decision tree and loop
/// at the same scope read as `c1` / `l1` rather than colliding on one
/// shared sequence.
pub(super) fn emit_loop_entity(arm: &Arm<'_>, body: &Node, result: &mut ParseResult) {
    emit_flow_entity(arm, body, EntityKind::Loop, "loop_node", result);
}

/// Emit a Branch entity for one control-flow arm. Shared by the if/else and
/// case helpers so every kind of arm reaches the UI through one shape.
pub(super) fn emit_branch_entity(arm: &Arm<'_>, body: &Node, result: &mut ParseResult) {
    emit_flow_entity(arm, body, EntityKind::Branch, "branch_node", result);
}

/// Variant of [`emit_branch_entity`] for one arm of a `switch`, a type
/// switch, or a `select`. Adds the `case_arm` tag, the `select_arm` tag
/// when the arm belongs to a `select`, and — when the arm has a label to
/// show (`case 1`, `case *store.Row`, `case <-done`) — a `pattern:`
/// attribute and matching documentation. A `default` arm has no pattern;
/// it is still emitted, because choosing to have one is a decision.
pub(super) fn emit_case_arm_entity(
    arm: &Arm<'_>,
    body: &Node,
    pattern: Option<&str>,
    from_select: bool,
    result: &mut ParseResult,
) {
    emit_branch_entity(arm, body, result);
    let Some(entity) = result.entities.last_mut() else {
        return;
    };
    entity.tags.insert("case_arm".to_string());
    if from_select {
        entity.tags.insert("select_arm".to_string());
    }
    let Some(trimmed) = pattern.map(str::trim).filter(|p| !p.is_empty()) else {
        return;
    };
    entity.attributes.push(format!("pattern:{}", trimmed));
    entity.documentation = Some(format!("pattern: {}", trimmed));
}

/// The one shape a synthetic flow entity has, whichever kind it is: an id
/// derived from the caller and the arm path, a parent that is either the
/// enclosing arm or the caller itself, and private visibility because
/// nothing outside the function can name it.
fn emit_flow_entity(
    arm: &Arm<'_>,
    body: &Node,
    kind: EntityKind,
    tag: &str,
    result: &mut ParseResult,
) {
    let parent_id = match arm.parent_branch {
        Some(p) => format!("{}::branch::{}", arm.caller_id, p),
        None => arm.caller_id.to_string(),
    };
    let mut entity = CodeEntity::new(arm.path.to_string(), kind, arm.file, node_to_span(body));
    entity.id = format!("{}::branch::{}", arm.caller_id, arm.path);
    entity.qualified_name = format!("{}::{}", arm.caller_id, arm.path);
    entity.parent_id = Some(parent_id);
    entity.tags.insert(tag.to_string());
    entity.visibility = Visibility::Private;
    result.add_entity(entity);
}
