//! Synthetic `Branch` and `Loop` entities for the arms of a C++ body.
//!
//! The shapes are the ones the Java and Python emitters already produce —
//! the `branch_node` / `loop_node` / `try_arm` / `case_arm` tags, the
//! `caught:` and `pattern:` attribute conventions, the `c1` / `c1.l2`
//! paths — so the UI's decision-tree rendering treats C++ like every other
//! language without a change. Only the dispatch in [`super::calls`]
//! differs, because C++ spells `else` as a node of its own.
//!
//! An [`Arm`] carries the four values that say where a flow entity sits.
//! They always travel together and mean nothing apart, which is what makes
//! them a struct rather than four more parameters on five functions.

use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_to_span, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Where one synthetic flow entity sits: whose body it is in, which arm
/// encloses it, its own path, and the file it was read from.
pub(super) struct Arm<'a> {
    pub caller_id: &'a str,
    pub parent_branch: Option<&'a str>,
    pub path: &'a str,
    pub file: &'a Path,
}

impl Arm<'_> {
    /// The entity id of whatever this arm hangs off — the enclosing arm
    /// when there is one, the callable otherwise.
    fn parent_id(&self) -> String {
        match self.parent_branch {
            Some(branch) => format!("{}::branch::{}", self.caller_id, branch),
            None => self.caller_id.to_string(),
        }
    }
}

/// Emit a `Loop` entity for the body of a `for` / `while` / `do-while` /
/// range-for. Loop paths use the `l` prefix so a sibling decision tree and
/// loop at the same scope read as `c1` / `l1` rather than colliding.
pub(super) fn emit_loop_entity(arm: &Arm<'_>, body: &Node, result: &mut ParseResult) {
    emit(arm, body, EntityKind::Loop, "loop_node", result);
}

/// Emit a `Branch` entity for one control-flow arm. Every other emitter
/// here goes through it, so all arms reach the UI in one shape.
pub(super) fn emit_branch_entity(arm: &Arm<'_>, body: &Node, result: &mut ParseResult) {
    emit(arm, body, EntityKind::Branch, "branch_node", result);
}

/// One arm of a `switch`. `pattern` is the label's own text — `None` for
/// `default:`, which is still emitted so the arm shows as intentional.
pub(super) fn emit_case_arm_entity(
    arm: &Arm<'_>,
    body: &Node,
    pattern: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(arm, body, result);
    let Some(entity) = result.entities.last_mut() else {
        return;
    };
    entity.tags.insert("case_arm".to_string());
    annotate(entity, "pattern", pattern);
}

/// One arm of a `try` — the try body, a `catch`, or the trailing block of
/// a function-try. `caught` carries the declared exception type.
pub(super) fn emit_try_arm_entity(
    arm: &Arm<'_>,
    body: &Node,
    arm_kind: &str,
    caught: Option<&str>,
    result: &mut ParseResult,
) {
    emit_branch_entity(arm, body, result);
    let Some(entity) = result.entities.last_mut() else {
        return;
    };
    entity.tags.insert("try_arm".to_string());
    entity.tags.insert(format!("{}_arm", arm_kind));
    annotate(entity, "caught", caught);
}

/// Build and add the entity every emitter above shares.
fn emit(arm: &Arm<'_>, body: &Node, kind: EntityKind, tag: &str, result: &mut ParseResult) {
    let mut entity = CodeEntity::new(arm.path, kind, arm.file, node_to_span(body));
    entity.id = format!("{}::branch::{}", arm.caller_id, arm.path);
    entity.qualified_name = format!("{}::{}", arm.caller_id, arm.path);
    entity.parent_id = Some(arm.parent_id());
    entity.tags.insert(tag.to_string());
    entity.visibility = Visibility::Private;
    result.add_entity(entity);
}

/// Record what an arm matched or caught, on the attribute the UI reads and
/// in the documentation a reader sees.
fn annotate(entity: &mut CodeEntity, key: &str, value: Option<&str>) {
    let Some(text) = value.map(str::trim).filter(|t| !t.is_empty()) else {
        return;
    };
    entity.attributes.push(format!("{}:{}", key, text));
    entity.documentation = Some(format!("{}: {}", key, text));
}
