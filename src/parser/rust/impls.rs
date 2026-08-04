//! `impl` block parsing — emits implements-edges, attaches impl source to
//! the implemented type, and walks the body to extract methods, constants,
//! and type aliases with trait awareness.

use super::super::language_parser::node_text;
use super::calls::extract_calls;
use super::functions::parse_function;
use super::helpers::base_type_name;
use super::inference::{infer_local_types, infer_param_types, TypeEnv};
use super::leaves::{parse_constant, parse_type_alias};
use super::ExtractCtx;
use crate::models::{Relationship, RelationshipKind};
use tree_sitter::Node;

pub(super) fn parse_impl(node: &Node, ctx: &mut ExtractCtx<'_>) {
    // Get the type being implemented (strip generic/lifetime args so
    // `impl<'a> FileWalker<'a>` yields "FileWalker", matching the struct entity).
    let type_node = node.child_by_field_name("type");
    let type_name = type_node.map(|n| base_type_name(&n, ctx.source));

    // Get the trait being implemented (if any)
    let trait_node = node.child_by_field_name("trait");
    let trait_name = trait_node.map(|n| base_type_name(&n, ctx.source));

    // Resolve the bare type name to the actual entity ID from THIS file.
    // Without this, structs with the same name in different files (e.g.,
    // `Db` in coupling_good.rs and coupling_bad.rs) collapse onto one
    // node because the graph resolver's `name_to_id` is first-come-
    // first-served globally.
    let resolved_type_id = type_name.as_ref().and_then(|tn| {
        ctx.result
            .entities
            .iter()
            .find(|e| e.name == *tn && e.kind.is_container() && e.file_path == ctx.path)
            .map(|e| e.id.clone())
    });

    // Create implements relationship using full entity IDs when possible.
    if let (Some(ref type_ref), Some(ref trait_name)) = (&type_name, &trait_name) {
        let source = resolved_type_id
            .as_deref()
            .unwrap_or(type_ref.as_str());
        let rel = Relationship::new(
            source.to_string(),
            trait_name.clone(),
            RelationshipKind::Implements,
        );
        ctx.result.add_relationship(rel);
    }

    // Record impl block source code for the type
    if let Some(ref type_name) = type_name {
        ctx.impl_sources.push((type_name.clone(), node_text(node, ctx.source).to_string()));
    }

    // Parse methods in the impl block with trait context. Use the
    // resolved entity ID as parent so methods are correctly scoped to
    // the struct in THIS file, not an identically-named one elsewhere.
    // `self_type_name` is the bare type name (e.g. "DependencyGraph")
    // used to resolve `Self::method()` calls — separate from the full
    // entity ID used for containment edges.
    let parent_id = resolved_type_id
        .as_deref()
        .or(type_name.as_deref());
    let self_type_name = type_name.as_deref();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "declaration_list" {
            extract_impl_items(&child, parent_id, self_type_name, trait_name.as_deref(), ctx);
        }
    }
}

/// Extract items from an impl block with trait awareness.
fn extract_impl_items(
    node: &Node,
    parent_id: Option<&str>,
    self_type_name: Option<&str>,
    trait_name: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if let Some(mut entity) = parse_function(&child, ctx.source, ctx.path, parent_id) {
                    // Mark if this method implements a trait
                    if let Some(trait_name) = trait_name {
                        entity.implements.push(trait_name.to_string());
                        entity.tags.insert("trait_impl".to_string());
                    } else {
                        entity.tags.insert("inherent".to_string());
                    }
                    let caller_id = entity.id.clone();
                    let caller_name = entity.name.clone();
                    ctx.result.add_entity(entity);
                    // Extract function calls from the body. `self_type_name`
                    // is the bare type name (e.g. "DependencyGraph") used to
                    // resolve `Self::foo()` / `self.foo()` to `Type::foo` so
                    // the graph resolver can match them via typed_method_to_id.
                    if let Some(body) = child.child_by_field_name("body") {
                        // Parameters first, then locals — a `let` rebinding
                        // shadows the parameter of the same name, so locals win.
                        // Parameters first, then locals — a `let` rebinding
                        // shadows the parameter of the same name.
                        let mut locals = infer_param_types(&child, ctx.source);
                        locals.extend(infer_local_types(&body, ctx.source));
                        let env = TypeEnv {
                            locals,
                            fields: ctx.struct_fields,
                            self_type: self_type_name,
                        };
                        let mut call_order = 0u32;
                        extract_calls(
                            &body,
                            ctx.source,
                            &caller_id,
                            &caller_name,
                            &env,
                            &mut call_order,
                            ctx.result,
                        );
                    }
                }
            }
            "const_item" | "static_item" => {
                if let Some(entity) = parse_constant(&child, ctx.source, ctx.path, parent_id) {
                    ctx.result.add_entity(entity);
                }
            }
            "type_item" => {
                if let Some(entity) = parse_type_alias(&child, ctx.source, ctx.path, parent_id) {
                    ctx.result.add_entity(entity);
                }
            }
            _ => {}
        }
    }
}

