//! Type parameters: PEP 695 bracket syntax and the `TypeVar` factory call
//! that predates it (PY-023).
//!
//! Both spellings declare the same thing — a name that stands for a type
//! rather than a value — so both land on the entity as a signal a reader can
//! act on. Shared by `classes.rs` and `functions.rs` because PEP 695 puts the
//! bracket group on classes and functions alike.

use super::super::language_parser::node_text;
use crate::models::CodeEntity;
use tree_sitter::Node;

/// The factory calls that mint a type parameter, bare or module-qualified.
const TYPE_PARAM_FACTORIES: &[&str] = &["TypeVar", "ParamSpec", "TypeVarTuple"];

/// Record a PEP 695 bracket group (`class Foo[T]:`, `def f[T](…)`) on the
/// entity's `generics`.
///
/// tree-sitter models the whole group as one `type_parameters` *field* whose
/// node kind is the singular `type_parameter`; each of its named children is
/// one parameter. Reading the field rather than the kind is what keeps this
/// from also matching subscript arguments elsewhere in the tree.
pub(super) fn collect_type_parameters(node: &Node, source: &str, entity: &mut CodeEntity) {
    let Some(params) = node.child_by_field_name("type_parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        entity.generics.push(node_text(&param, source).to_string());
    }
}

/// Is this assignment's right-hand side a `TypeVar("T")` — or a `ParamSpec` /
/// `TypeVarTuple` — call?
///
/// `T = TypeVar("T")` binds a name that only ever appears in annotations. It
/// stays an `EntityKind::Variable`, because that is what it is at runtime;
/// the tag is what says it participates in the type system.
pub(super) fn is_type_param_factory(assignment: &Node, source: &str) -> bool {
    let Some(right) = assignment.child_by_field_name("right") else {
        return false;
    };
    if right.kind() != "call" {
        return false;
    }
    let Some(function) = right.child_by_field_name("function") else {
        return false;
    };
    let called = node_text(&function, source);
    TYPE_PARAM_FACTORIES.contains(&called.rsplit('.').next().unwrap_or(called))
}
