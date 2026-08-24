//! What a Dart file declares, and the walk that finds it.
//!
//! One module per declaration kind, plus the dispatcher that routes each
//! grammar node to the one that owns it:
//! - [`containers`] — class, mixin, extension, extension type, enum, typedef
//! - [`callables`] — functions, methods, accessors, constructors
//! - [`fields`] — class fields and top-level variables
//! - [`imports`] — `import` / `export` / `part` directives
//! - [`attributes`] — modifiers and annotations, shared by all of them
//!
//! Each extractor registers what it found and hands back what is left to
//! walk; descending into a body is the dispatcher's job alone, so no module
//! here calls back up into this one.
//!
//! The class-member dispatch is the part worth knowing: Dart wraps every
//! member in a `class_member`, and what kind of member it is only becomes
//! visible one or two levels down. So a member is offered to
//! [`callables::from_member`] first, and is a field exactly when that
//! declines it.

mod attributes;
mod callables;
mod containers;
mod fields;
mod imports;

use super::ctx::{Descent, ExtractCtx};
use super::helpers::{add_parsed, child_of_kind};
use crate::parser::language_parser::{node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Extract every declaration in one parsed file into `result`.
pub(super) fn extract_file(root: Node, path: &Path, source: &str, result: &mut ParseResult) {
    let library = library_name(root, source);
    let mut ctx = ExtractCtx {
        source,
        path,
        library: &library,
        result,
    };
    extract_entities(root, None, &mut ctx);
}

/// The name a `library shop.cart;` directive declares, or the empty string.
/// It prefixes qualified names the way a Java package does — Dart files
/// usually declare none, and then there is nothing to prefix with.
fn library_name(root: Node, source: &str) -> String {
    let Some(directive) = child_of_kind(&root, "library_name") else {
        return String::new();
    };
    child_of_kind(&directive, "dotted_identifier_list")
        .map(|n| node_text(&n, source).to_string())
        .unwrap_or_default()
}

fn extract_entities(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch(&child, parent_id, ctx);
    }
}

fn dispatch(node: &Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    if dispatch_directive(node, ctx) {
        return;
    }
    match node.kind() {
        "class_declaration" => descend(containers::handle_class(node, parent_id, ctx), ctx),
        "mixin_declaration" => descend(containers::handle_mixin(node, parent_id, ctx), ctx),
        "extension_declaration" | "extension_type_declaration" => {
            descend(containers::handle_extension(node, parent_id, ctx), ctx)
        }
        "enum_declaration" => descend(containers::handle_enum(node, parent_id, ctx), ctx),
        "type_alias" => {
            let parsed = containers::parse_type_alias(node, parent_id, ctx);
            add_parsed(parsed, ctx.result);
        }
        "class_member" => handle_member(node, parent_id, ctx),
        "function_declaration" | "getter_declaration" | "setter_declaration" => {
            handle_top_level_callable(node, parent_id, ctx)
        }
        "top_level_variable_declaration" => handle_top_level_variable(node, parent_id, ctx),
        _ => extract_entities(*node, parent_id, ctx),
    }
}

/// The nodes that declare nothing: the directives, and the decoration a
/// declaration carries. Returns `true` when the node is fully dealt with, so
/// [`dispatch`] can go on being about declarations alone.
fn dispatch_directive(node: &Node, ctx: &mut ExtractCtx<'_>) -> bool {
    let import = match node.kind() {
        "import_or_export" => imports::parse_import_or_export(node, ctx.source),
        "part_directive" | "part_of_directive" => imports::parse_part(node, ctx.source),
        "library_name" | "comment" | "annotation" | "marker_annotation" => return true,
        _ => return false,
    };
    if let Some(import) = import {
        ctx.result.add_import(import);
    }
    true
}

/// Walk whatever a container left behind. Registering the container is the
/// extractor's job, descending into it is this module's — which is what
/// keeps `containers` from depending back on the walk.
fn descend(next: Descent<'_>, ctx: &mut ExtractCtx<'_>) {
    if let Descent::Into { owner, body } = next {
        extract_entities(body, Some(&owner), ctx);
    }
}

/// A class member is a callable if it wraps a signature, and a field
/// otherwise. Asking `callables` first is what keeps the two apart without
/// this module having to know the eight signature kinds.
fn handle_member(node: &Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    if let Some(callable) = callables::from_member(node) {
        callables::handle(&callable, parent_id, ctx);
        return;
    }
    let Some(declaration) = child_of_kind(node, "declaration") else {
        return;
    };
    for entity in fields::parse_field(node, &declaration, ctx.source, ctx.path, parent_id) {
        ctx.result.add_entity(entity);
    }
}

fn handle_top_level_callable(node: &Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    if let Some(callable) = callables::from_top_level(node) {
        callables::handle(&callable, parent_id, ctx);
    }
}

/// A `top_level_variable_declaration` is only ever at file scope; the guard
/// is there so a future grammar change cannot quietly reparent one.
fn handle_top_level_variable(node: &Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    if parent_id.is_some() {
        return;
    }
    for entity in fields::parse_top_level_variable(node, ctx.source, ctx.path, ctx.library) {
        ctx.result.add_entity(entity);
    }
}
