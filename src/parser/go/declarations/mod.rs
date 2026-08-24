//! What a Go file declares, and the walk that finds it.
//!
//! One module per declaration kind, plus the dispatcher that routes each
//! grammar node to the one that owns it:
//! - [`containers`] — structs, interfaces, defined types and aliases
//! - [`callables`] — functions and methods, and their body wiring
//! - [`values`] — package-level `const` and `var`
//! - [`imports`] — `import` declarations, and the package table they build
//!
//! Two passes, not one. Go's declarations are order-independent — a method
//! may be written above the type it hangs off, and a call may go through a
//! struct field declared at the bottom of the file — so what the file says
//! about types is read first, and the entity walk runs against a complete
//! picture. Doing it in one pass would make an entity's edges depend on
//! where in the file it happened to be written.

mod callables;
mod containers;
mod imports;
mod values;

use super::bodies::inference::TypeIndex;
use super::ctx::ExtractCtx;
use super::doc_comments;
use super::types::{self, TypeUse};
use crate::models::CodeEntity;
use crate::parser::language_parser::{node_text, ParseResult};
use std::path::Path;
use tree_sitter::Node;

/// Extract every declaration in one parsed file into `result`.
pub(super) fn extract_file(root: Node, path: &Path, source: &str, result: &mut ParseResult) {
    let package = package_name(root, source);
    let import_table = imports::collect(root, source, result);
    let types = TypeIndex::build(root, source);

    let mut ctx = ExtractCtx {
        source,
        path,
        package: &package,
        imports: &import_table,
        types: &types,
        result,
    };
    extract_entities(root, &mut ctx);

    // The package comment documents the package, which is declared across
    // every file that joins it — so it belongs to no entity in this parse.
    result.file_documentation = doc_comments::package_doc(root, source);
}

/// The file's own `package x` name, which qualifies everything it declares.
fn package_name(root: Node, source: &str) -> String {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "package_clause" {
            continue;
        }
        let mut inner = child.walk();
        let name = child.children(&mut inner).find(|c| c.is_named());
        if let Some(name) = name {
            return node_text(&name, source).to_string();
        }
    }
    String::new()
}

/// Walk a node's children and dispatch each to the extractor that owns it.
fn extract_entities(node: Node, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        match child.kind() {
            "function_declaration" => callables::handle_function(&child, ctx),
            "method_declaration" => callables::handle_method(&child, ctx),
            "type_declaration" => add_types(&child, ctx),
            "const_declaration" | "var_declaration" => values::handle_declaration(&child, ctx),
            // Handled by the pre-passes above, and by nothing here.
            "package_clause" | "import_declaration" | "comment" => {}
            _ => extract_entities(child, ctx),
        }
    }
}

/// Add every type a `type` declaration introduces, grouped or not, plus
/// the members of the ones that have members.
fn add_types(declaration: &Node, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = declaration.walk();
    let specs: Vec<Node> = declaration
        .children(&mut cursor)
        .filter(|c| matches!(c.kind(), "type_spec" | "type_alias"))
        .collect();

    for spec in specs {
        let parsed = match spec.kind() {
            "type_alias" => containers::parse_type_alias(&spec, ctx.source, ctx.path, ctx.package),
            _ => containers::parse_type_spec(&spec, ctx.source, ctx.path, ctx.package),
        };
        let Some(entity) = parsed else { continue };
        add_with_members(entity, &spec, ctx);
    }
}

/// Add a type entity, the interface methods that belong under it, and the
/// `UsesType` edges each of them names.
///
/// An interface method's signature types are the *method's* dependency,
/// not the interface's — so the interface's own pass skips `method_elem`
/// subtrees and each method emits its own.
fn add_with_members(entity: CodeEntity, spec: &Node, ctx: &mut ExtractCtx<'_>) {
    let owner_id = entity.id.clone();
    let owner_name = entity.name.clone();
    ctx.result.add_entity(entity);

    types::emit(
        &[*spec],
        &["method_elem"],
        &TypeUse {
            entity_id: &owner_id,
            owner_name: &owner_name,
            source: ctx.source,
            imports: ctx.imports,
        },
        ctx.result,
    );

    let methods = containers::interface_methods(spec, ctx.source, ctx.path, &owner_id);
    let elements = method_elements(spec);
    for (method, element) in methods.into_iter().zip(elements) {
        let method_id = method.id.clone();
        let method_name = method.name.clone();
        ctx.result.add_entity(method);
        types::emit(
            &[element],
            &[],
            &TypeUse {
                entity_id: &method_id,
                owner_name: &method_name,
                source: ctx.source,
                imports: ctx.imports,
            },
            ctx.result,
        );
    }
}

/// The `method_elem` nodes of an interface, in the order
/// [`containers::interface_methods`] reads them, so the two zip.
fn method_elements<'a>(spec: &Node<'a>) -> Vec<Node<'a>> {
    let Some(underlying) = spec.child_by_field_name("type") else {
        return Vec::new();
    };
    let mut cursor = underlying.walk();
    underlying
        .children(&mut cursor)
        .filter(|c| c.kind() == "method_elem" && c.child_by_field_name("name").is_some())
        .collect()
}
