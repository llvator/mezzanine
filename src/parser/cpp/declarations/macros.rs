//! `#define` — the declaration kind no other language parsed here has.
//!
//! A macro is not a constant and not a function, and calling it either
//! would be wrong in a way that matters: an object-like `#define LIMIT 10`
//! has no type and no storage, and a function-like `#define MAX(a,b) …`
//! has parameters but no signature, no return type and no body the
//! complexity metrics could honestly score. So both become
//! [`EntityKind::Macro`], tagged with which of the two they are, and
//! neither is given metrics it does not have.
//!
//! They are worth an entity all the same. A `#define` is how a C++ header
//! states a limit, and [`super::super::bodies::calls`] emits a `UsesValue`
//! edge for every `SCREAMING_CASE` name a body reads — which lands on
//! these entities and nowhere else.

use super::super::ctx::{ExtractCtx, Scope};
use super::super::doc_comments::extract_doc;
use super::super::helpers::{join_scope, line_count};
use crate::models::{CodeEntity, EntityKind};
use crate::parser::language_parser::{node_text, node_to_span};
use tree_sitter::Node;

/// Add the entity a `#define` declares.
pub(super) fn handle(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(&name_node, ctx.source).to_string();
    if is_header_guard(node, &name, ctx.source) {
        return;
    }
    let mut entity = CodeEntity::new(&name, EntityKind::Macro, ctx.path, node_to_span(node));
    entity.qualified_name = join_scope(scope.namespace, &name);
    entity.parent_id = scope.parent_id.map(String::from);
    entity.documentation = extract_doc(node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    entity.metrics.loc = line_count(&entity);

    match node.child_by_field_name("parameters") {
        Some(params) => {
            entity.tags.insert("function_like".to_string());
            entity.parameters = macro_parameters(&params, ctx.source);
            entity.metrics.param_count = Some(entity.parameters.len() as u32);
        }
        None => {
            entity.tags.insert("object_like".to_string());
            entity.return_type = node
                .child_by_field_name("value")
                .map(|v| node_text(&v, ctx.source).trim().to_string());
        }
    }
    ctx.result.add_entity(entity);
}

/// Whether this `#define` is the header guard it sits inside.
///
/// `#ifndef ORDER_H / #define ORDER_H` declares nothing a reader
/// navigates to — it names the file it is already in. The test is the
/// precise one rather than "an object-like macro with no value", because
/// a valueless `#define FEATURE_X` used as a build flag is a real
/// declaration and would fail that looser rule.
fn is_header_guard(node: &Node, name: &str, source: &str) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "preproc_ifdef" {
            let guard = parent
                .child_by_field_name("name")
                .map(|n| node_text(&n, source));
            if guard == Some(name) {
                return true;
            }
        }
        current = parent.parent();
    }
    false
}

/// The names between the parentheses of a function-like macro. They have
/// no types — the preprocessor substitutes tokens — so each carries a name
/// and nothing else.
fn macro_parameters(list: &Node, source: &str) -> Vec<crate::models::entity::Parameter> {
    let mut cursor = list.walk();
    list.children(&mut cursor)
        .filter(|c| c.is_named())
        .map(|c| crate::models::entity::Parameter {
            name: node_text(&c, source).to_string(),
            type_name: None,
            default_value: None,
            visibility: None,
        })
        .collect()
}
