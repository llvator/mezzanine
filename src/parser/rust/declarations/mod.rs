//! What a Rust file declares, and the walk that finds it.
//!
//! One module per item kind, plus the dispatcher that routes each grammar
//! node to the one that owns it:
//! - [`functions`] — fns, trait methods, parameters, generics
//! - [`structs`], [`enums`], [`traits`] — type-defining items
//! - [`impls`] — `impl` blocks (including methods, consts, type items inside)
//! - [`leaves`] — modules, constants, type aliases, macros
//! - [`imports`] — `use` declarations
//!
//! The context threaded through the walk is built here, so the parser entry
//! point above can ask for a file's declarations without knowing what the
//! extractors need to carry between them.

mod enums;
mod functions;
mod impls;
mod imports;
mod leaves;
mod structs;
mod traits;

use super::ctx::ExtractCtx;
use super::doc_comments;
use crate::models::CodeEntity;
use crate::parser::language_parser::ParseResult;
use std::path::Path;
use tree_sitter::Node;

/// Extract every declaration in one parsed file into `result`.
pub(super) fn extract_file(root: &Node, path: &Path, source: &str, result: &mut ParseResult) {
    // Structs are collected up front: an impl block may appear before the
    // struct it implements, and field types are needed while walking it.
    let struct_fields = structs::collect_struct_fields(root, source);
    let mut impl_sources: Vec<(String, String)> = Vec::new();
    {
        let mut ctx = ExtractCtx {
            source,
            path,
            result,
            impl_sources: &mut impl_sources,
            struct_fields: &struct_fields,
        };
        extract_entities(*root, None, &mut ctx);
    }

    // Attach impl block sources to their corresponding type entities
    for (type_name, impl_source) in impl_sources {
        for entity in &mut result.entities {
            if entity.name == type_name && entity.kind.is_container() {
                entity.impl_blocks.push(impl_source.clone());
            }
        }
    }

    // The file's own `//!` header. It documents no entity — a `.rs` file
    // is a module whose declaration lives in another file — so it travels
    // beside the entities and the analyzer lifts it onto `FileInfo`.
    result.file_documentation = doc_comments::extract_inner_doc(root, source);
}

/// Walk a node's children and dispatch each to the appropriate extractor.
fn extract_entities(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                functions::handle_function(&child, parent_id, None, ctx);
            }
            "struct_item" => add_container(&child, parent_id, ctx, structs::parse_struct),
            "trait_item" => add_container(&child, parent_id, ctx, traits::parse_trait),
            "mod_item" => add_container(&child, parent_id, ctx, leaves::parse_module),
            "enum_item" => add_leaf(&child, parent_id, ctx, enums::parse_enum),
            "const_item" | "static_item" => {
                add_leaf(&child, parent_id, ctx, leaves::parse_constant)
            }
            "function_signature_item" => {
                add_leaf(&child, parent_id, ctx, functions::parse_trait_method)
            }
            "type_item" => add_leaf(&child, parent_id, ctx, leaves::parse_type_alias),
            "macro_definition" => add_leaf(&child, parent_id, ctx, leaves::parse_macro),
            "impl_item" => {
                impls::parse_impl(&child, ctx);
            }
            "use_declaration" => {
                if let Some(import) = imports::parse_use(&child, ctx.source) {
                    ctx.result.add_import(import);
                }
            }
            _ => {
                extract_entities(child, parent_id, ctx);
            }
        }
    }
}

/// Parse a non-container entity and add it to the result.
fn add_leaf(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse_one: fn(&Node, &str, &Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse_one(node, ctx.source, ctx.path, parent_id) {
        ctx.result.add_entity(entity);
    }
}

/// Parse a container entity (struct/trait/mod), add it, and recurse into
/// its body with the new entity id as parent.
fn add_container(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse_one: fn(&Node, &str, &Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse_one(node, ctx.source, ctx.path, parent_id) {
        let entity_id = entity.id.clone();
        ctx.result.add_entity(entity);
        extract_entities(*node, Some(&entity_id), ctx);
    }
}
