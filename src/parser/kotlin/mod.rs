//! Kotlin parser — entry point and entity-extraction dispatcher.
//!
//! Submodules group the parsing logic by concern:
//! - [`containers`] — class / interface / enum / object / companion
//! - [`functions`] — function declarations, including extension functions
//! - [`leaves`] — properties and type aliases
//! - [`imports`] — import-header declarations
//! - [`calls`] — call-site relationship extraction
//! - [`kdoc`] — `/** ... */` documentation extraction
//! - [`modifiers`] — visibility + Kotlin's many modifier buckets
//! - [`helpers`] — parameters, generics, identifier/type extraction
//! - [`stdlib`] — built-in name table for call filtering
//! - [`types`] — `UsesType` edge post-pass over captured type strings

mod calls;
mod containers;
mod functions;
mod helpers;
mod imports;
mod kdoc;
mod leaves;
mod modifiers;
mod stdlib;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{node_text, LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::CodeEntity;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

pub struct KotlinParser {
    parser: Parser,
}

/// Shared context threaded through the entity-extraction walk.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub package: &'a str,
    pub result: &'a mut ParseResult,
}

impl KotlinParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_kotlin::language())
            .expect("Failed to set Kotlin language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Kotlin code"))
    }
}

/// Extract the package name from the `package_header` node.
fn extract_package(root: Node, source: &str) -> String {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_header" {
            let mut inner = child.walk();
            for c in child.children(&mut inner) {
                if c.kind() == "identifier" {
                    return node_text(&c, source).to_string();
                }
            }
        }
    }
    String::new()
}

fn extract_entities(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" => {
                containers::handle_class_declaration(&child, parent_id, ctx, extract_entities);
            }
            "object_declaration" => {
                containers::handle_object_declaration(&child, parent_id, ctx, extract_entities);
            }
            "companion_object" => {
                containers::handle_companion_object(&child, parent_id, ctx, extract_entities);
            }
            "function_declaration" => {
                functions::handle_function(&child, parent_id, ctx.source, ctx.path, ctx.result);
            }
            "property_declaration" => add_leaf(&child, parent_id, ctx, leaves::parse_property),
            "type_alias" => add_leaf(&child, parent_id, ctx, leaves::parse_type_alias),
            "import_list" => collect_imports(&child, ctx),
            "import_header" => {
                if let Some(import) = imports::parse_import(&child, ctx.source) {
                    ctx.result.add_import(import);
                }
            }
            "package_header" => {}
            _ => extract_entities(child, parent_id, ctx),
        }
    }
}

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

fn collect_imports(import_list: &Node, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = import_list.walk();
    for child in import_list.children(&mut cursor) {
        if child.kind() == "import_header" {
            if let Some(import) = imports::parse_import(&child, ctx.source) {
                ctx.result.add_import(import);
            }
        }
    }
}

impl Default for KotlinParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for KotlinParser {
    fn language(&self) -> Language {
        Language::Kotlin
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        let package = extract_package(tree.root_node(), content);
        let mut ctx = ExtractCtx {
            source: content,
            path,
            package: &package,
            result: &mut result,
        };
        extract_entities(tree.root_node(), None, &mut ctx);

        // Emit UsesType edges from signature/property types (KT-001).
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
