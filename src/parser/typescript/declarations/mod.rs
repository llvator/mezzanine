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
    module_scope(root, &mut ctx);
}

/// Give the file's top-level statements somewhere to hang their calls
/// (TS-008).
///
/// `export const x = f(…)` has a binding to own what it calls (TS-007). A
/// statement written straight into the module has none:
///
/// ```ts
/// subject.subscribe((current) => { void ensureDetailsLoaded().then(…); });
/// ```
///
/// Nothing covers that call, so no edge was produced and the file read as
/// depending on nothing. In a store or a side-effecting entry module that is
/// most of the file.
///
/// The owner is a **`Module`**, deliberately not a `File`. A `File` entity
/// would be picked up by `file_path_to_id`, which is what decides whether
/// `resolve_imports` emits an `Imports` edge — so it would mint an edge per
/// resolvable import and take `import_coverage` to ~100% while the calls this
/// exists to capture were still missing. A signal that goes quiet without the
/// problem being fixed is worse than no signal (AN-030).
///
/// Named with its extension — `description.ts`, not `description` — because
/// every entity name enters the resolver's `name_to_id`, and a bare stem is a
/// call target a stranger can bind to. That is AN-029 exactly, and a name no
/// callee expression can spell cannot repeat it.
///
/// Created only when there is something to own, so a file of pure
/// declarations gains nothing.
fn module_scope(root: &Node, ctx: &mut ExtractCtx<'_>) {
    let statements: Vec<Node> = top_level_statements(root);
    if statements.is_empty() {
        return;
    }
    let name = ctx
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "module".to_string());
    let entity = CodeEntity::new(
        &name,
        crate::models::EntityKind::Module,
        ctx.path,
        crate::parser::language_parser::node_to_span(root),
    );
    let id = entity.id.clone();
    ctx.result.add_entity(entity);
    for statement in statements {
        super::bodies::calls::extract_body_calls(&statement, &id, &name, None, None, ctx);
    }
}

/// The file's own statements: everything at module scope that is not a
/// declaration, an import or an export.
///
/// Declarations are excluded because they already own what they contain —
/// walking them here would attribute a function's calls to the module as well
/// as to the function, and double every edge they hold.
fn top_level_statements<'a>(root: &Node<'a>) -> Vec<Node<'a>> {
    let mut cursor = root.walk();
    root.children(&mut cursor)
        .filter(|child| !is_declaration(child.kind()))
        .collect()
}

/// Whether a node at module scope already has an owner of its own.
fn is_declaration(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "type_alias_declaration"
            | "function_declaration"
            | "generator_function_declaration"
            | "lexical_declaration"
            | "variable_declaration"
            | "export_statement"
            | "import_statement"
            | "comment"
    )
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
