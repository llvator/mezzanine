//! What a Kotlin file declares, and the walk that finds it.
//!
//! One module per declaration kind, plus the dispatcher that routes each
//! grammar node to the one that owns it:
//! - [`containers`] — class / interface / enum / object / companion
//! - [`functions`] — function declarations, including extension functions
//! - [`leaves`] — properties and type aliases
//! - [`imports`] — import-header declarations
//!
//! Each extractor registers what it found and hands back what is left to walk;
//! descending into a body is the dispatcher's job alone, so no module here
//! calls back up into this one.
//!
//! The context threaded through the walk is built here, so the parser entry
//! point above can ask for a file's declarations without knowing what the
//! extractors need to carry between them.

mod containers;
mod functions;
mod imports;
mod leaves;

use super::ctx::{Descent, ExtractCtx};
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
                descend(
                    containers::handle_class_declaration(&child, parent_id, ctx),
                    ctx,
                );
            }
            "object_declaration" => {
                descend(
                    containers::handle_object_declaration(&child, parent_id, ctx),
                    ctx,
                );
            }
            "companion_object" => {
                descend(
                    containers::handle_companion_object(&child, parent_id, ctx),
                    ctx,
                );
            }
            "function_declaration" => {
                functions::handle_function(&child, parent_id, ctx.source, ctx.path, ctx.result);
            }
            "property_declaration" => {
                let leaf = leaves::parse_property(&child, ctx.source, ctx.path, parent_id);
                add_leaf(leaf, ctx);
            }
            "type_alias" => {
                let leaf = leaves::parse_type_alias(&child, ctx.source, ctx.path, parent_id);
                add_leaf(leaf, ctx);
            }
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

/// Walk whatever a container left behind. Registering the container is the
/// extractor's job, descending into it is this module's — which is what keeps
/// `containers` from depending back on the walk.
fn descend(next: Descent<'_>, ctx: &mut ExtractCtx<'_>) {
    if let Descent::Into { owner, body } = next {
        extract_entities(body, owner.as_deref(), ctx);
    }
}

/// Register a leaf entity — a property or a type alias — if it parsed. Like
/// [`descend`], the extractor is called at the match arm and this only acts on
/// what came back, so the walk names every module it reaches into.
fn add_leaf(parsed: Option<CodeEntity>, ctx: &mut ExtractCtx<'_>) {
    if let Some(entity) = parsed {
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
