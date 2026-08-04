//! TypeScript / TSX parser — entry point and entity-extraction dispatcher.
//!
//! Submodules group the parsing logic by concern:
//! - [`containers`] — class, interface, enum (plus heritage extraction)
//! - [`functions`] — top-level function declarations
//! - [`members`] — class/interface body members (methods, fields, signatures)
//! - [`lexical`] — `const`/`let` declarations (arrow fns, function exprs, vars)
//! - [`leaves`] — type aliases
//! - [`imports`] — `import` statements
//! - [`calls`] — call-site relationships and control-flow arm dispatch
//! - [`flow`] — synthetic `Branch` / `Loop` entities (TS-002)
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics (TS-003)
//! - [`inference`] — parameter, local and class-member type inference
//! - [`types`] — `UsesType` edges from signature/member types (post-pass)
//! - [`tsdoc`] — `/** ... */` extraction
//! - [`decorators`] — child + sibling decorator collection
//! - [`helpers`] — accessibility, parameters, generics, type-text trimming
//! - [`stdlib`] — built-in name table for call filtering

mod calls;
mod complexity;
mod containers;
mod decorators;
mod flow;
mod functions;
mod helpers;
mod imports;
mod inference;
mod leaves;
mod lexical;
mod members;
mod stdlib;
mod tsdoc;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::CodeEntity;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

pub struct TypeScriptParser {
    parser: Parser,
}

/// Shared context threaded through the entity-extraction walk.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    /// Class/interface name → (member → declared type), collected once per
    /// file so every method body can type `this.<field>` receivers.
    pub members: &'a HashMap<String, HashMap<String, String>>,
    pub result: &'a mut ParseResult,
}

impl TypeScriptParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::language_typescript())
            .expect("Failed to set TypeScript language");
        Self { parser }
    }

    /// Pick the TSX or TS grammar based on file extension.
    fn new_for_path(path: &Path) -> Self {
        let mut parser = Parser::new();
        let is_tsx = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e == "tsx")
            .unwrap_or(false);
        let language = if is_tsx {
            tree_sitter_typescript::language_tsx()
        } else {
            tree_sitter_typescript::language_typescript()
        };
        parser
            .set_language(&language)
            .expect("Failed to set TypeScript language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse TypeScript code"))
    }
}

/// Walk a node's children and dispatch each to the appropriate extractor.
/// `self_type` is the enclosing class/interface name, used to qualify
/// `this.foo()` call targets inside its methods.
fn extract_entities(
    node: Node,
    parent_id: Option<&str>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "abstract_class_declaration" => {
                add_container(&child, parent_id, ctx, containers::parse_class);
            }
            "interface_declaration" => {
                add_container(&child, parent_id, ctx, containers::parse_interface);
            }
            "enum_declaration" => add_leaf(&child, parent_id, ctx, containers::parse_enum),
            "type_alias_declaration" => add_leaf(&child, parent_id, ctx, leaves::parse_type_alias),
            "function_declaration" | "generator_function_declaration" => {
                functions::handle_function(&child, parent_id, self_type, ctx);
            }
            "lexical_declaration" | "variable_declaration" => {
                lexical::handle_lexical_declaration(&child, parent_id, self_type, ctx);
            }
            "export_statement" => {
                extract_entities(child, parent_id, self_type, ctx);
            }
            "import_statement" => {
                if let Some(import) = imports::parse_import(&child, ctx.source) {
                    ctx.result.add_import(import);
                }
            }
            "method_definition" => {
                members::handle_method(&child, parent_id, self_type, ctx);
            }
            "abstract_method_signature" => {
                add_leaf(&child, parent_id, ctx, members::parse_abstract_method)
            }
            "public_field_definition" => add_leaf(&child, parent_id, ctx, members::parse_field),
            "property_signature" => {
                add_leaf(&child, parent_id, ctx, members::parse_property_signature)
            }
            "method_signature" => {
                add_leaf(&child, parent_id, ctx, members::parse_method_signature)
            }
            _ => {
                extract_entities(child, parent_id, self_type, ctx);
            }
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

fn add_container(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse: fn(&Node, &str, &Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse(node, ctx.source, ctx.path, parent_id) {
        let entity_id = entity.id.clone();
        let entity_name = entity.name.clone();
        ctx.result.add_entity(entity);
        if let Some(body) = node.child_by_field_name("body") {
            extract_entities(body, Some(&entity_id), Some(&entity_name), ctx);
        }
    }
}

impl Default for TypeScriptParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for TypeScriptParser {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new_for_path(path);
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        // Collected before the walk so a method body can type a receiver
        // against a class declared later in the file.
        let class_members = inference::collect_class_members(&tree.root_node(), content);
        let mut ctx = ExtractCtx {
            source: content,
            path,
            members: &class_members,
            result: &mut result,
        };
        extract_entities(tree.root_node(), None, None, &mut ctx);

        // Emit UsesType edges from signature/member types (TS-001).
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
