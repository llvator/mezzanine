//! Rust parser — entry point and entity-extraction dispatcher.
//!
//! Submodules group the parsing logic by concern:
//! - [`functions`] — fns, trait methods, parameters, generics
//! - [`structs`], [`enums`], [`traits`] — type-defining items
//! - [`impls`] — `impl` blocks (including methods, consts, type items inside)
//! - [`leaves`] — modules, constants, type aliases, macros
//! - [`imports`] — `use` declarations
//! - [`calls`] — call-site relationship extraction
//! - [`doc_comments`] — `///`, `//!`, `/** */` extraction
//! - [`helpers`] — shared visibility / type / generics utilities
//! - [`complexity`] — cyclomatic / cognitive metrics
//! - [`inference`] — local variable type inference
//! - [`stdlib`] — built-in name table for call filtering

mod calls;
mod complexity;
mod doc_comments;
mod enums;
mod functions;
mod helpers;
mod impls;
mod imports;
pub(super) mod inference;
mod leaves;
mod stdlib;
mod structs;
mod traits;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::CodeEntity;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

pub struct RustParser {
    parser: Parser,
}

/// Shared context threaded through the entity-extraction walk so that each
/// recursive call doesn't have to plumb 4+ parameters. Only the dispatch
/// layer uses this; leaf `parse_*` helpers keep their simpler signatures.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub result: &'a mut ParseResult,
    pub impl_sources: &'a mut Vec<(String, String)>,
    /// Struct name → (field name → type), collected before the walk so an
    /// impl block can type `self.<field>` receivers (AN-010).
    pub struct_fields: &'a std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

impl RustParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::language())
            .expect("Failed to set Rust language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Rust code"))
    }
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
            "const_item" | "static_item" => add_leaf(&child, parent_id, ctx, leaves::parse_constant),
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
    parse: fn(&Node, &str, &Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse(node, ctx.source, ctx.path, parent_id) {
        ctx.result.add_entity(entity);
    }
}

/// Parse a container entity (struct/trait/mod), add it, and recurse into
/// its body with the new entity id as parent.
fn add_container(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse: fn(&Node, &str, &Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse(node, ctx.source, ctx.path, parent_id) {
        let entity_id = entity.id.clone();
        ctx.result.add_entity(entity);
        extract_entities(*node, Some(&entity_id), ctx);
    }
}

impl Default for RustParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for RustParser {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        let mut impl_sources: Vec<(String, String)> = Vec::new();
        // Structs are collected up front: an impl block may appear before the
        // struct it implements, and field types are needed while walking it.
        let struct_fields = inference::collect_struct_fields(&tree.root_node(), content);
        let mut ctx = ExtractCtx {
            source: content,
            path,
            result: &mut result,
            impl_sources: &mut impl_sources,
            struct_fields: &struct_fields,
        };
        extract_entities(tree.root_node(), None, &mut ctx);

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
        result.file_documentation = doc_comments::extract_inner_doc(&tree.root_node(), content);

        // Emit UsesType edges from signature/field types (RS-001).
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
