//! What a TypeScript file declares, and the walk that finds it.
//!
//! One module per declaration kind, plus the dispatcher that routes each
//! grammar node to the one that owns it:
//! - [`containers`] — class, interface, enum (plus heritage extraction)
//! - [`functions`] — top-level function declarations
//! - [`members`] — class/interface body members (methods, fields, signatures)
//! - [`lexical`] — `const`/`let` declarations (arrow fns, function exprs, vars)
//! - [`leaves`] — type aliases
//! - [`imports`] — `import` statements
//!
//! The context threaded through the walk is built here, so the parser entry
//! point above can ask for a file's declarations without knowing what the
//! extractors need to carry between them.

mod containers;
mod functions;
mod imports;
mod leaves;
mod lexical;
mod members;

use super::bodies::inference;
use super::ctx::ExtractCtx;
use crate::models::CodeEntity;
use crate::parser::language_parser::ParseResult;
use std::path::Path;
use tree_sitter::Node;

/// Extract every declaration in one parsed file into `result`.
pub(super) fn extract_file(root: &Node, path: &Path, source: &str, result: &mut ParseResult) {
    // Collected before the walk so a method body can type a receiver
    // against a class declared later in the file.
    let class_members = inference::collect_class_members(root, source);
    let mut ctx = ExtractCtx {
        source,
        path,
        members: &class_members,
        result,
    };
    extract_entities(*root, None, None, &mut ctx);
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
            "export_statement" => handle_export(&child, parent_id, self_type, ctx),
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
            "method_signature" => add_leaf(&child, parent_id, ctx, members::parse_method_signature),
            _ => {
                extract_entities(child, parent_id, self_type, ctx);
            }
        }
    }
}

/// An `export` is two unrelated statements sharing a keyword. `export
/// { X } from './y'` names another module and declares nothing; every
/// other `export` declares and names nothing. Both arrive at the same
/// dispatcher arm, so both are asked.
///
/// A function rather than a branch in that arm: `extract_entities` is
/// already over the repo's complexity ceiling and grandfathered, and a
/// grandfathered function may not get worse.
fn handle_export(
    node: &Node,
    parent_id: Option<&str>,
    self_type: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) {
    if let Some(import) = imports::parse_reexport(node, ctx.source) {
        ctx.result.add_import(import);
    }
    extract_entities(*node, parent_id, self_type, ctx);
}

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

fn add_container(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse_one: fn(&Node, &str, &Path, Option<&str>) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse_one(node, ctx.source, ctx.path, parent_id) {
        let entity_id = entity.id.clone();
        let entity_name = entity.name.clone();
        ctx.result.add_entity(entity);
        if let Some(body) = node.child_by_field_name("body") {
            extract_entities(body, Some(&entity_id), Some(&entity_name), ctx);
        }
    }
}
