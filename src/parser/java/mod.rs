//! Java parser — entry point and entity-extraction dispatcher.
//!
//! Submodules group the parsing logic by concern:
//! - [`containers`] — class, interface, enum, annotation type, record
//! - [`callables`] — methods, constructors (and call-extraction wiring)
//! - [`fields`] — field declarations (one node may yield multiple entities)
//! - [`imports`] — `import` declarations
//! - [`calls`] — call-site relationship extraction
//! - [`javadoc`] — `/** ... */` extraction
//! - [`helpers`] — visibility, modifiers, parameters, generics, type lists
//! - [`stdlib`] — built-in name table for call filtering
//! - [`types`] — `UsesType` edge extraction from signature/field types

mod callables;
mod calls;
mod complexity;
mod containers;
mod fields;
mod flow;
mod helpers;
mod imports;
mod javadoc;
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

pub struct JavaParser {
    parser: Parser,
}

/// Shared context threaded through the entity-extraction walk so that each
/// recursive call doesn't have to plumb 5+ parameters. Only the dispatch
/// layer uses this; leaf `parse_*` helpers keep their simpler signatures.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub package: &'a str,
    pub result: &'a mut ParseResult,
}

impl JavaParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::language())
            .expect("Failed to set Java language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Java code"))
    }
}

/// Extract the package name from the root program node.
fn extract_package(root: Node, source: &str) -> String {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_declaration" {
            let mut inner = child.walk();
            for c in child.children(&mut inner) {
                if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                    return node_text(&c, source).to_string();
                }
            }
        }
    }
    String::new()
}

/// Walk a node's children and dispatch each to the appropriate extractor.
fn extract_entities(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" => add_container(&child, parent_id, ctx, containers::parse_class),
            "interface_declaration" => {
                add_container(&child, parent_id, ctx, containers::parse_interface)
            }
            "enum_declaration" => add_container(&child, parent_id, ctx, containers::parse_enum),
            "annotation_type_declaration" => {
                add_container(&child, parent_id, ctx, containers::parse_annotation_type)
            }
            "record_declaration" => add_container(&child, parent_id, ctx, containers::parse_record),
            "method_declaration" => {
                callables::handle_callable(&child, parent_id, ctx, callables::parse_method);
            }
            "constructor_declaration" => {
                callables::handle_callable(&child, parent_id, ctx, callables::parse_constructor);
            }
            "field_declaration" | "constant_declaration" => {
                for entity in fields::parse_field(&child, ctx.source, ctx.path, parent_id) {
                    ctx.result.add_entity(entity);
                }
            }
            "import_declaration" => {
                if let Some(import) = imports::parse_import(&child, ctx.source) {
                    ctx.result.add_import(import);
                }
            }
            "package_declaration" => {}
            _ => {
                extract_entities(child, parent_id, ctx);
            }
        }
    }
}

/// Parse a container (class/interface/enum/annotation/record), add it, and
/// recurse into its `body` field with the new entity id as parent.
fn add_container(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse: fn(&Node, &str, &Path, Option<&str>, &str) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse(node, ctx.source, ctx.path, parent_id, ctx.package) {
        let entity_id = entity.id.clone();
        ctx.result.add_entity(entity);
        if let Some(body) = node.child_by_field_name("body") {
            extract_entities(body, Some(&entity_id), ctx);
        }
    }
}

impl Default for JavaParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for JavaParser {
    fn language(&self) -> Language {
        Language::Java
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

        // Emit UsesType edges from signature/field types (JV-001).
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
