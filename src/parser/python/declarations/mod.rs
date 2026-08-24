//! What a Python file declares, and the walk that finds it.
//!
//! One module per declaration kind, plus the dispatcher that routes each
//! grammar node to the one that owns it:
//! - [`classes`] — class definitions and field collection
//! - [`functions`] — functions, methods, parameters, fluent-self detection
//! - [`assignments`] — module-level constants, variables and type aliases
//! - [`imports`] — import statements
//!
//! Each extractor registers what it found and hands back the entity id;
//! descending into a body is the dispatcher's job alone, so no module here
//! calls back up into this one.
//!
//! The context threaded through the walk is built here, so the parser entry
//! point above can ask for a file's declarations without knowing what the
//! extractors need to carry between them.

mod assignments;
mod classes;
mod functions;
mod imports;

use super::ctx::ExtractCtx;
use crate::parser::language_parser::{ImportCondition, ImportInfo, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Extract every declaration in one parsed file into `result`.
pub(super) fn extract_file(root: Node, path: &Path, source: &str, result: &mut ParseResult) {
    let mut ctx = ExtractCtx {
        source,
        path,
        result,
        import_condition: None,
        in_type_checking: false,
    };
    extract_entities(root, None, &mut ctx);
}

/// Walk a node's children and dispatch to the appropriate extractor for each
/// kind. Recurses into containers that may hold nested definitions.
fn extract_entities(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                declare_then_descend(&child, parent_id, ctx, functions::handle_function);
            }
            "class_definition" => {
                declare_then_descend(&child, parent_id, ctx, classes::handle_class);
            }
            "import_statement" | "import_from_statement" => handle_imports(&child, ctx),
            // PY-024: an import inside an `if` or a `try` is a weaker claim
            // than one at the top of the file. Recording *which* wrapper it
            // sits in is what separates `if TYPE_CHECKING:` from the
            // `try: import ujson / except ImportError: import json` shape.
            "if_statement" | "try_statement" => descend_conditional(child, parent_id, ctx),
            "decorated_definition" => {
                // Recurse — the inner function/class will be matched next.
                extract_entities(child, parent_id, ctx);
            }
            "expression_statement" => {
                // Only extract module-level assignments as entities.
                // Class-level annotations are captured in class.fields instead.
                if parent_id.is_none() {
                    assignments::handle_module_assignment(&child, ctx);
                }
            }
            "type_alias_statement" => handle_type_alias(&child, parent_id, ctx),
            _ => {
                extract_entities(child, parent_id, ctx);
            }
        }
    }
}

/// What a `def` or a `class` costs the dispatcher: `register` places the
/// declaration and hands back its id, then the body is walked with that id
/// as the parent. Both halves live here rather than in the extractor, which
/// is what keeps `functions` and `classes` from calling back up into this
/// module — and writing it once keeps the two match arms one line each.
fn declare_then_descend(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    register: fn(&Node, Option<&str>, &mut ExtractCtx<'_>) -> Option<String>,
) {
    let Some(entity_id) = register(node, parent_id, ctx) else {
        return;
    };
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    extract_entities(body, Some(&entity_id), ctx);
}

/// Record a file's imports, stamping whichever conditional wrapper the walk
/// is currently inside onto each one.
fn handle_imports(node: &Node, ctx: &mut ExtractCtx<'_>) {
    // Read before the loop borrows `ctx.result` mutably, and stamped by a
    // helper so this dispatcher does not grow a branch per fact the walk
    // learns to carry.
    let (condition, type_only) = (ctx.import_condition, ctx.in_type_checking);
    for import in imports::parse_import(node, ctx.source) {
        ctx.result.add_import(stamped(import, condition, type_only));
    }
}

/// What the walk knows about an import that the statement itself does not:
/// the wrapper it sits in (PY-024), and whether that wrapper is the
/// `TYPE_CHECKING` gate the interpreter never enters (AN-022).
fn stamped(
    mut import: ImportInfo,
    condition: Option<ImportCondition>,
    type_only: bool,
) -> ImportInfo {
    if let Some(condition) = condition {
        import = import.conditional(condition);
    }
    if type_only {
        import = import.type_only();
    }
    import
}

/// Walk into an `if` / `try` with `condition` in force, then put back
/// whatever was in force before — a `try` nested in an `if` must not leave
/// the walk thinking the rest of the `if` is a fallback.
fn descend_conditional(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    // A `try` says "one of these arms is an acceptable dependency"; an `if`
    // says "this one, when the gate opens".
    let condition = match node.kind() {
        "try_statement" => ImportCondition::Fallback,
        _ => ImportCondition::Guarded,
    };
    let outer = ctx.import_condition.replace(condition);
    // Sticky rather than replaced: a `try` written inside an
    // `if TYPE_CHECKING:` is still inside it, and its imports are still
    // erased (AN-022).
    let outer_type_checking = ctx.in_type_checking;
    ctx.in_type_checking |= is_type_checking_gate(&node, ctx.source);
    extract_entities(node, parent_id, ctx);
    ctx.import_condition = outer;
    ctx.in_type_checking = outer_type_checking;
}

/// Whether this `if` is the `TYPE_CHECKING` gate — the one guard whose body
/// the interpreter never runs (AN-022).
///
/// Matched on the name rather than on the import it comes from, so
/// `if TYPE_CHECKING:` and `if typing.TYPE_CHECKING:` both answer yes and a
/// local alias for the module does not have to be tracked. Anything else in
/// the condition — `and`, `or`, a version check beside it — leaves the gate
/// recognised, because the body is unreachable at runtime either way.
fn is_type_checking_gate(node: &Node, source: &str) -> bool {
    if node.kind() != "if_statement" {
        return false;
    }
    let Some(condition) = node.child_by_field_name("condition") else {
        return false;
    };
    crate::parser::language_parser::node_text(&condition, source)
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| word == "TYPE_CHECKING")
}

/// PEP 695 `type X = …`.
fn handle_type_alias(node: &Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    if let Some(entity) =
        assignments::parse_type_alias_statement(node, ctx.source, ctx.path, parent_id)
    {
        ctx.result.add_entity(entity);
    }
}
