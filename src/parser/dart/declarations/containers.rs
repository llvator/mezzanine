//! Container declarations: classes, mixins, extensions, extension types,
//! enums and type aliases.
//!
//! Each extractor registers what it found and hands the dispatcher the body
//! left to walk; descending is the dispatcher's job alone, which is what
//! keeps this module from calling back up into it.
//!
//! Three mappings are decisions rather than mechanics:
//!
//! * A **mixin is an `Interface`**, tagged `mixin`. It declares members for
//!   other types to take on and is never instantiated, which is what the
//!   Interface kind means everywhere else in the graph. Its `on` clause is a
//!   constraint on who may mix it in, so it lands in `extends`.
//! * An **extension is a `Class`**, tagged `extension`, and the type it
//!   extends is recorded as an `on:<Type>` attribute with a matching
//!   `UsesType` edge. Not `extends`: an extension does not inherit from its
//!   target, and rendering an `Inherits` edge there would claim it does. An
//!   extension *type* is the same shape over its representation type.
//! * **`with` mixins join `implements`**, not `extends`. A Dart class has
//!   one superclass; a mixin contributes members the way an interface
//!   contributes a contract, and neither is the type's parent.

use super::super::ctx::{Descent, ExtractCtx};
use super::super::doc_comments;
use super::super::helpers::{
    child_of_kind, children_of_kind, declared_type, parse_generics, span_over, visibility_of,
};
use super::attributes::declared_attributes;
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, node_to_span};
use tree_sitter::Node;

/// `class C extends B with M implements I { … }`, and every Dart 3 modifier
/// in front of it.
pub(super) fn handle_class<'t>(
    node: &Node<'t>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let Some(name_node) = child_of_kind(node, "identifier") else {
        return Descent::Stop;
    };
    let is_abstract = child_of_kind(node, "abstract").is_some();
    let kind = if is_abstract {
        EntityKind::AbstractClass
    } else {
        EntityKind::Class
    };

    let mut entity = new_container(node, &name_node, kind, parent_id, ctx);
    tag_modifiers(&mut entity);
    read_supertypes(node, ctx.source, &mut entity);
    if let Some(generics) = child_of_kind(node, "type_parameters") {
        entity.generics = parse_generics(&generics, ctx.source);
    }

    register(entity, node, "class_body", ctx)
}

/// `mixin M on A implements B { … }`.
pub(super) fn handle_mixin<'t>(
    node: &Node<'t>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let Some(name_node) = child_of_kind(node, "identifier") else {
        return Descent::Stop;
    };
    let mut entity = new_container(node, &name_node, EntityKind::Interface, parent_id, ctx);
    entity.tags.insert("mixin".to_string());
    tag_modifiers(&mut entity);
    read_supertypes(node, ctx.source, &mut entity);

    register(entity, node, "class_body", ctx)
}

/// `extension OrderX on Order { … }` and `extension type Meters(int v) { … }`.
pub(super) fn handle_extension<'t>(
    node: &Node<'t>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let is_extension_type = node.kind() == "extension_type_declaration";
    let Some(name_node) = extension_name(node) else {
        return Descent::Stop;
    };

    let mut entity = new_container(node, &name_node, EntityKind::Class, parent_id, ctx);
    entity.tags.insert("extension".to_string());
    if is_extension_type {
        entity.tags.insert("extension_type".to_string());
    }

    // The extended (or represented) type is a real dependency; it is just
    // not a parent, so it travels as an attribute plus a type edge.
    if let Some(target) = extension_target(node, ctx.source) {
        entity.attributes.push(format!("on:{}", target));
        ctx.result.add_relationship(Relationship::new(
            entity.id.clone(),
            target,
            RelationshipKind::UsesType,
        ));
    }

    let body_kind = if is_extension_type {
        "class_body"
    } else {
        "extension_body"
    };
    register(entity, node, body_kind, ctx)
}

/// `enum Status { pending, shipped }`. Constants land in `fields`, which is
/// where every other parser puts enum members.
pub(super) fn handle_enum<'t>(
    node: &Node<'t>,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let Some(name_node) = child_of_kind(node, "identifier") else {
        return Descent::Stop;
    };
    let mut entity = new_container(node, &name_node, EntityKind::Enum, parent_id, ctx);
    if let Some(body) = child_of_kind(node, "enum_body") {
        entity.fields = enum_constants(&body, ctx.source);
    }
    read_supertypes(node, ctx.source, &mut entity);

    register(entity, node, "enum_body", ctx)
}

/// `typedef Reducer = Receipt Function(Order);`
pub(super) fn parse_type_alias(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &ExtractCtx<'_>,
) -> Option<CodeEntity> {
    let name_node = child_of_kind(node, "type_identifier")?;
    let name = node_text(&name_node, ctx.source).to_string();
    let mut entity = CodeEntity::new(&name, EntityKind::TypeAlias, ctx.path, node_to_span(node));
    entity.visibility = visibility_of(&name);
    entity.parent_id = parent_id.map(String::from);
    if !ctx.library.is_empty() {
        entity.qualified_name = format!("{}.{}", ctx.library, name);
    }
    // Recording the aliased type as the return type is what lets `types`
    // emit an edge to whatever it names.
    entity.return_type = declared_type(node, ctx.source);
    entity.documentation = doc_comments::extract(node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    Some(entity)
}

/// Every modifier a container declared is also a tag, so `sealed` and
/// friends are filterable rather than only readable in the detail panel.
fn tag_modifiers(entity: &mut CodeEntity) {
    for attribute in entity.attributes.clone() {
        if super::attributes::is_modifier(&attribute) {
            entity.tags.insert(attribute);
        }
    }
}

/// The shared skeleton every container starts from.
fn new_container(
    node: &Node,
    name_node: &Node,
    kind: EntityKind,
    parent_id: Option<&str>,
    ctx: &ExtractCtx<'_>,
) -> CodeEntity {
    let name = node_text(name_node, ctx.source).to_string();
    // A doc comment sits in front of the declaration rather than inside it,
    // so the span is widened over it. Otherwise `source_code` would start at
    // `class` and drop the prose that explains what the class is for.
    let span = match doc_comments::preceding_doc(node, ctx.source) {
        Some(comment) => span_over(&comment, node),
        None => node_to_span(node),
    };
    let mut entity = CodeEntity::new(&name, kind, ctx.path, span);
    entity.visibility = visibility_of(&name);
    entity.parent_id = parent_id.map(String::from);
    entity.attributes = declared_attributes(node, ctx.source);
    if !ctx.library.is_empty() {
        entity.qualified_name = format!("{}.{}", ctx.library, name);
    }
    entity.documentation = doc_comments::extract(node, ctx.source);
    entity.source_code = Some(ctx.source[span.start.offset..span.end.offset].to_string());
    entity
}

/// Register a container and hand its body back to the dispatcher, which
/// owns the descent.
fn register<'t>(
    entity: CodeEntity,
    node: &Node<'t>,
    body_kind: &str,
    ctx: &mut ExtractCtx<'_>,
) -> Descent<'t> {
    let owner = entity.id.clone();
    ctx.result.add_entity(entity);
    match child_of_kind(node, body_kind) {
        Some(body) => Descent::Into { owner, body },
        None => Descent::Stop,
    }
}

/// The name node of an extension or extension type. A plain extension names
/// itself with a bare `identifier`; an extension type wraps the name and its
/// type parameters in an `extension_type_name`.
fn extension_name<'t>(node: &Node<'t>) -> Option<Node<'t>> {
    if let Some(name) = child_of_kind(node, "identifier") {
        return Some(name);
    }
    let wrapper = child_of_kind(node, "extension_type_name")?;
    child_of_kind(&wrapper, "identifier")
}

/// The type an extension extends, or an extension type represents.
fn extension_target(node: &Node, source: &str) -> Option<String> {
    if let Some(representation) = child_of_kind(node, "extension_type_representation") {
        return declared_type(&representation, source);
    }
    declared_type(node, source)
}

/// Read `extends` / `with` / `implements` / `on` into the entity.
///
/// `extends` and a mixin's `on` are single-parent relations and share the
/// field. The mixins in a `with` clause join `implements` — see the module
/// header for why.
fn read_supertypes(node: &Node, source: &str, entity: &mut CodeEntity) {
    if let Some(superclass) = child_of_kind(node, "superclass") {
        if let Some(base) = declared_type(&superclass, source) {
            entity.extends.push(base);
        }
        if let Some(mixins) = child_of_kind(&superclass, "mixins") {
            entity.implements.extend(type_names(&mixins, source));
        }
    }
    if let Some(interfaces) = child_of_kind(node, "interfaces") {
        entity.implements.extend(type_names(&interfaces, source));
    }
    // A mixin's `on` clause names the types it may be applied to. The
    // grammar leaves them as bare `type` children of the mixin.
    if node.kind() == "mixin_declaration" {
        entity.extends.extend(type_names(node, source));
    }
}

/// The type names a clause lists, keeping each as the author spelled it.
fn type_names(node: &Node, source: &str) -> Vec<String> {
    children_of_kind(node, "type")
        .iter()
        .map(|n| node_text(n, source).to_string())
        .collect()
}

/// Enum constants, in declaration order.
fn enum_constants(body: &Node, source: &str) -> Vec<Parameter> {
    children_of_kind(body, "enum_constant")
        .iter()
        .filter_map(|constant| {
            let name = child_of_kind(constant, "identifier")?;
            Some(Parameter {
                name: node_text(&name, source).to_string(),
                type_name: None,
                default_value: None,
                visibility: None,
            })
        })
        .collect()
}
