//! What a C++ file declares, and the walk that finds it.
//!
//! One module per declaration kind, plus the dispatcher that routes each
//! grammar node to the one that owns it:
//! - [`containers`] — namespaces, classes, structs, unions, enums, aliases
//! - [`callables`] — functions, methods, constructors, operators
//! - [`fields`] — data members, and namespace-scope variables and constants
//! - [`imports`] — `#include`, `using`, namespace aliases
//! - [`macros`] — `#define`
//!
//! Descending into a body is the dispatcher's job alone, so no module here
//! calls back up into this one — except through [`extract_entities`],
//! which a container calls to walk its own body and which exists for
//! exactly that.
//!
//! Two things about the walk are C++-specific.
//!
//! **Access is positional.** There is no modifier per member: an
//! `access_specifier` opens a section and every declaration after it
//! inherits that access until the next one. So the dispatcher carries the
//! current access in [`Scope`] and updates it as it passes each specifier,
//! rather than reading a modifier list per declaration.
//!
//! **`declaration` and `field_declaration` are ambiguous.** Both spell a
//! data member and a function declaration with the same node kind — the
//! difference is a `function_declarator` somewhere in the declarator
//! chain, which is what [`route_declaration`] looks for. This is also why
//! the preprocessor kinds are dispatched here rather than filtered
//! upstream: `#ifdef` blocks wrap ordinary declarations, and the default
//! arm walks straight through them.

mod callables;
mod containers;
mod fields;
mod imports;
mod macros;

use super::bodies::inference::TypeIndex;
use super::ctx::{ExtractCtx, Scope};
use super::doc_comments;
use super::helpers::{access_visibility, function_declarator, parse_template_parameters};
use super::types::{self, TypeUse};
use crate::models::CodeEntity;
use crate::parser::language_parser::ParseResult;
use std::path::Path;
use tree_sitter::Node;

/// Extract every declaration in one parsed file into `result`.
pub(super) fn extract_file(root: Node, path: &Path, source: &str, result: &mut ParseResult) {
    imports::collect(root, source, result);
    let types = TypeIndex::build(root, source);
    let mut ctx = ExtractCtx {
        source,
        path,
        types: &types,
        result,
    };
    extract_entities(root, &Scope::file(), &mut ctx);
    result.file_documentation = doc_comments::file_doc(root, source);
}

/// Walk a node's children, tracking the access section, and dispatch each
/// child to the extractor that owns it.
pub(super) fn extract_entities(node: Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    let mut access = scope.access;
    for child in children {
        if child.kind() == "access_specifier" {
            access = access_visibility(&child).unwrap_or(access);
            continue;
        }
        let here = Scope { access, ..*scope };
        dispatch(&child, &here, ctx);
    }
}

/// Route one grammar node to the extractor that owns it.
fn dispatch(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    match node.kind() {
        "namespace_definition" => containers::handle_namespace(node, scope, ctx),
        "class_specifier" | "struct_specifier" | "union_specifier" => {
            containers::handle_class(node, scope, ctx)
        }
        "enum_specifier" => containers::handle_enum(node, scope, ctx),
        "alias_declaration" | "type_definition" => containers::handle_alias(node, scope, ctx),
        "template_declaration" => handle_template(node, scope, ctx),
        "function_definition" => callables::handle_definition(node, scope, ctx),
        "field_declaration" | "declaration" => route_declaration(node, scope, ctx),
        "preproc_def" | "preproc_function_def" => macros::handle(node, scope, ctx),
        // Read by the import pre-pass, and by nothing here.
        "preproc_include" | "using_declaration" | "namespace_alias_definition" => {}
        // `friend class Painter;` names a class without declaring one.
        "comment" | "friend_declaration" => {}
        _ => extract_entities(*node, scope, ctx),
    }
}

/// A `declaration` or `field_declaration` is a callable when its
/// declarator chain reaches a parameter list, and data when it does not.
/// Nothing else in the node says which — `Order* make(int)` and
/// `Order* cache_` are the same kind with the same fields.
fn route_declaration(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let Some(declarator) = node.child_by_field_name("declarator") else {
        // A forward declaration (`class Order;`) or a bare specifier. It
        // introduces a name and nothing to hang on it.
        return;
    };
    if function_declarator(&declarator).is_some() {
        callables::handle_prototype(node, scope, ctx);
    } else {
        fields::handle(node, scope, ctx);
    }
}

/// `template <typename T> …` wraps the declaration it parameterises.
///
/// The generics are read here and attached to whatever the inner
/// declaration produced — its own entity, which is the first one added,
/// since a container adds itself before descending into its members.
fn handle_template(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let generics = node
        .child_by_field_name("parameters")
        .map(|p| parse_template_parameters(&p, ctx.source))
        .unwrap_or_default();
    let mut cursor = node.walk();
    let inner: Vec<Node> = node
        .children(&mut cursor)
        .filter(|c| c.is_named() && c.kind() != "template_parameter_list")
        .collect();
    for child in inner {
        let first_new = ctx.result.entities.len();
        dispatch(&child, scope, ctx);
        let Some(entity) = ctx.result.entities.get_mut(first_new) else {
            continue;
        };
        entity.generics = generics.clone();
        entity.tags.insert("template".to_string());
    }
}

/// Emit the `UsesType` edges an entity's own declaration names.
///
/// Called before the entity is added, because the caller still owns it —
/// which also keeps the edges in declaration order rather than in
/// whatever order a post-pass over the entity list would produce.
fn emit_signature_types(
    entity: &CodeEntity,
    roots: &[Node],
    skip: &[&str],
    ctx: &mut ExtractCtx<'_>,
) {
    types::emit(
        roots,
        skip,
        &TypeUse {
            entity_id: &entity.id,
            owner_name: &entity.name,
            source: ctx.source,
        },
        ctx.result,
    );
}

/// A constructor's `: id_(id), repo_(new Repository())` list, which runs
/// before the body and is part of what the constructor does.
fn field_initializers<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|c| c.kind() == "field_initializer_list");
    found
}
