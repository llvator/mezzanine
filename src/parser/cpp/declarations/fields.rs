//! Data: class members, and the variables and constants a namespace
//! declares.
//!
//! One node can declare several of them — `int width, height;` is a
//! single `declaration` with two declarators — so this yields entities
//! rather than an entity, the same shape the Java field extractor has for
//! the same reason.
//!
//! The kind depends on where the declaration sits, not on what it says. A
//! class member is a `Property`, because it belongs to the class the way a
//! Java field does. A namespace-scope name is a `Constant` when `const` or
//! `constexpr` says its value cannot change and a `Variable` when nothing
//! does — a distinction worth keeping, because a header full of the first
//! is a shared vocabulary and a file full of the second is shared state.

use super::super::ctx::{ExtractCtx, Scope};
use super::super::doc_comments::extract_doc;
use super::super::helpers::{declared_name, declared_type, join_scope, line_count, specifier_tags};
use crate::models::{CodeEntity, EntityKind};
use crate::parser::language_parser::{node_text, node_to_span};
use tree_sitter::Node;

/// Add one entity per name a data declaration binds.
pub(super) fn handle(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let tags = specifier_tags(node, None);
    let is_constant = tags.iter().any(|t| t == "const" || t == "constexpr");
    let kind = match (scope.owner, is_constant) {
        (Some(_), _) => EntityKind::Property,
        (None, true) => EntityKind::Constant,
        (None, false) => EntityKind::Variable,
    };

    let mut cursor = node.walk();
    let declarators: Vec<Node> = node.children_by_field_name("declarator", &mut cursor).collect();
    for declarator in declarators {
        let Some(name_node) = declared_name(&declarator) else {
            continue;
        };
        let name = node_text(&name_node, ctx.source).to_string();
        let mut entity = CodeEntity::new(&name, kind, ctx.path, node_to_span(&declarator));
        entity.qualified_name = qualified(scope, &name);
        entity.parent_id = scope.parent_id.map(String::from);
        entity.visibility = scope.access;
        entity.return_type = declared_type(node, Some(&declarator), ctx.source);
        entity.attributes = tags.clone();
        entity.documentation = extract_doc(node, ctx.source);
        entity.source_code = Some(node_text(node, ctx.source).to_string());
        entity.metrics.loc = line_count(&entity);
        for tag in &tags {
            entity.tags.insert(tag.clone());
        }
        super::emit_signature_types(&entity, &[*node], &[], ctx);
        ctx.result.add_entity(entity);
    }
}

/// A member is qualified by its class, a namespace-scope name by its
/// namespace — and a class is itself inside a namespace, which is why the
/// owner is joined onto the namespace rather than replacing it.
fn qualified(scope: &Scope<'_>, name: &str) -> String {
    match scope.owner {
        Some(owner) => join_scope(&join_scope(scope.namespace, owner), name),
        None => join_scope(scope.namespace, name),
    }
}
