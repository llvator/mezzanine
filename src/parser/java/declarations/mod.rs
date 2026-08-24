//! What a Java file declares, and the walk that finds it.
//!
//! One module per declaration kind, plus the dispatcher that routes each
//! grammar node to the one that owns it:
//! - [`containers`] — class, interface, enum, annotation type, record
//! - [`callables`] — methods and constructors, and their body wiring
//! - [`fields`] — field declarations (one node may yield multiple entities)
//! - [`imports`] — `import` declarations
//!
//! Descending into a container body is the dispatcher's job alone, so no
//! module here calls back up into this one.
//!
//! The context threaded through the walk is built here, so the parser entry
//! point above can ask for a file's declarations without knowing what the
//! extractors need to carry between them.

mod callables;
mod containers;
mod fields;
mod imports;

use super::ctx::ExtractCtx;
use crate::models::CodeEntity;
use crate::parser::language_parser::{node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Extract every declaration in one parsed file into `result`.
pub(super) fn extract_file(root: Node, path: &Path, source: &str, result: &mut ParseResult) {
    let package = extract_package(root, source);
    let mut ctx = ExtractCtx {
        source,
        path,
        package: &package,
        result,
    };
    extract_entities(root, None, &mut ctx);
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
    parse_container: fn(&Node, &str, &Path, Option<&str>, &str) -> Option<CodeEntity>,
) {
    if let Some(entity) = parse_container(node, ctx.source, ctx.path, parent_id, ctx.package) {
        let entity_id = entity.id.clone();
        ctx.result.add_entity(entity);
        if let Some(body) = node.child_by_field_name("body") {
            extract_entities(body, Some(&entity_id), ctx);
        }
    }
}
