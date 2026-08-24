//! Control-flow entity emission for Dart: branch arms (if / else-if / else,
//! switch cases, try / on / catch / finally) and loop bodies (for, while,
//! do-while).
//!
//! Ports [`crate::parser::java::bodies::flow`] one-for-one — the tags
//! (`branch_node`, `loop_node`, `try_arm`, `case_arm`) and the `caught:` /
//! `pattern:` attribute conventions are what the analyzer's synthetic-entity
//! pass and the UI's decision-tree rendering key off, so Dart's arms reach
//! them through exactly the shape Java's do. Only the dispatch in
//! [`super::calls`] differs, because the Dart grammar hangs a `catch`
//! clause's body off the `try_statement` as a sibling rather than putting
//! it inside the clause.

use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_to_span, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Emit a Loop entity for the body of a `for` / `while` / `do-while`.
/// Loop bodies use the `l<idx>` path prefix while arms use `c<idx>`, so a
/// sibling decision tree and loop at the same scope read as `c1` / `l1`
/// rather than colliding on one shared sequence.
pub(super) fn emit_loop_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    loop_path: &str,
    body_node: &Node,
    path: &Path,
    result: &mut ParseResult,
) {
    let mut entity = arm_entity(caller_id, parent_branch, loop_path, body_node, path);
    entity.kind = EntityKind::Loop;
    entity.tags.insert("loop_node".to_string());
    result.add_entity(entity);
}

/// Emit a generic Branch entity for a control-flow arm body. Shared by the
/// if/else, switch-case and try-arm helpers so every kind of arm reaches
/// the UI through the same shape.
pub(super) fn emit_branch_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    branch_path: &str,
    body_node: &Node,
    path: &Path,
    result: &mut ParseResult,
) {
    let mut entity = arm_entity(caller_id, parent_branch, branch_path, body_node, path);
    entity.tags.insert("branch_node".to_string());
    result.add_entity(entity);
}

/// Variant of [`emit_branch_entity`] for one arm of a `switch`. Adds a
/// `case_arm` tag plus, when the label carries a pattern (`case 1:`,
/// `case Status.shipped:`), a `pattern:<text>` attribute and matching
/// documentation. The `default:` label has no pattern; the entity is still
/// emitted so the arm appears in the graph as an intentional branch.
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
        annotate(entity, "pattern", pattern);
    }
}

/// Emit a Branch entity for one arm of a `try` statement: the try body
/// itself, an `on` / `catch` handler, or the `finally` block. Mirrors the
/// Java and Python metadata so the same UI grouping fires without changes:
///
/// * a `try_arm` tag on every arm,
/// * a `<kind>_arm` tag (`try_body_arm`, `catch_arm`, `finally_arm`),
/// * for a handler with a declared exception type, the type travels as a
///   `caught:<type>` attribute and as the entity's documentation. Dart
///   writes that type in an `on` clause (`on FormatException catch (e)`);
///   a bare `catch (e)` catches everything and records nothing.
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
        annotate(entity, "caught", caught);
    }
}

/// The Branch entity every arm starts as, before its kind-specific tags.
fn arm_entity(
    caller_id: &str,
    parent_branch: Option<&str>,
    arm_path: &str,
    body_node: &Node,
    path: &Path,
) -> CodeEntity {
    let parent_id = match parent_branch {
        Some(parent) => format!("{}::branch::{}", caller_id, parent),
        None => caller_id.to_string(),
    };
    let mut entity = CodeEntity::new(
        arm_path.to_string(),
        EntityKind::Branch,
        path,
        node_to_span(body_node),
    );
    entity.id = format!("{}::branch::{}", caller_id, arm_path);
    entity.qualified_name = format!("{}::{}", caller_id, arm_path);
    entity.parent_id = Some(parent_id);
    entity.visibility = Visibility::Private;
    entity
}

/// Record `key:value` on an arm, as both an attribute and the documentation
/// the detail panel shows. A blank value is no value.
fn annotate(entity: &mut CodeEntity, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };
    entity.attributes.push(format!("{}:{}", key, value));
    entity.documentation = Some(format!("{}: {}", key, value));
}
