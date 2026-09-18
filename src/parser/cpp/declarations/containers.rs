//! The things a C++ file declares that hold other things: namespaces,
//! classes, structs, unions, enums, and the two spellings of an alias.
//!
//! Two C++ facts shape what follows, and neither has an equivalent in the
//! languages parsed before it:
//!
//! * **A namespace is a container that is also a name.** `app::core` is
//!   where a class lives *and* half of how the class is written at every
//!   call site, so a namespace becomes a `Module` entity — giving the
//!   declarations inside it a parent — and its path is threaded down the
//!   walk to build every `qualified_name` beneath it.
//! * **Abstractness is not a keyword.** C++ has no `abstract`; a class is
//!   abstract exactly when one of its members is pure virtual. So
//!   [`is_abstract`] reads the body rather than a modifier list, and the
//!   entity kind follows from what the class contains.
//!
//! Base classes all land in `extends` (§B4), including the ones a Java
//! reader would call interfaces. C++ draws no line between inheriting an
//! implementation and inheriting a contract — a pure-virtual base is a
//! convention, not a construct — and inventing one here would put two
//! identical declarations in different fields depending on what their
//! bases happened to contain.

use super::super::ctx::{ExtractCtx, Scope};
use super::super::doc_comments::extract_doc;
use super::super::helpers::{access_visibility, join_scope, line_count};
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{find_child_by_kind, node_text, node_to_span};
use tree_sitter::Node;

/// `namespace app { … }`, `namespace app::core { … }`, and the anonymous
/// form, whose contents have internal linkage and are visible to no other
/// translation unit.
pub(super) fn handle_namespace(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, ctx.source).to_string());
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let Some(name) = name else {
        // Anonymous. It declares no name to qualify by, so the walk
        // continues in the enclosing namespace with the visibility its
        // contents actually have.
        let inner = Scope {
            access: Visibility::Internal,
            ..*scope
        };
        super::extract_entities(body, &inner, ctx);
        return;
    };

    let path = join_scope(scope.namespace, &name);
    let mut entity = CodeEntity::new(&name, EntityKind::Module, ctx.path, node_to_span(node));
    entity.qualified_name = path.clone();
    entity.parent_id = scope.parent_id.map(String::from);
    entity.documentation = extract_doc(node, ctx.source);
    entity.metrics.loc = line_count(&entity);
    let id = entity.id.clone();
    ctx.result.add_entity(entity);

    let inner = Scope {
        namespace: &path,
        parent_id: Some(&id),
        owner: None,
        access: Visibility::Public,
    };
    super::extract_entities(body, &inner, ctx);
}

/// `class X : public Base { … }` and its `struct` / `union` siblings.
///
/// A declaration with no body is a forward declaration — it says a name
/// exists and nothing else — so it produces no entity, which is what
/// stops `class Order;` in twenty headers from becoming twenty classes.
pub(super) fn handle_class(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = type_name(&name_node, ctx.source);
    let is_struct = matches!(node.kind(), "struct_specifier" | "union_specifier");

    let kind = if is_abstract(&body) {
        EntityKind::AbstractClass
    } else if is_struct {
        EntityKind::Struct
    } else {
        EntityKind::Class
    };
    let mut entity = CodeEntity::new(&name, kind, ctx.path, node_to_span(node));
    entity.qualified_name = join_scope(scope.namespace, &name);
    entity.parent_id = scope.parent_id.map(String::from);
    entity.visibility = scope.access;
    entity.extends = base_classes(node, ctx.source);
    entity.fields = data_members(&body, ctx.source, is_struct);
    entity.documentation = extract_doc(node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    entity.metrics.loc = line_count(&entity);
    entity.metrics.field_count = Some(entity.fields.len() as u32);
    if is_abstract(&body) {
        entity.tags.insert("abstract".to_string());
    }
    if node.kind() == "union_specifier" {
        entity.tags.insert("union".to_string());
    }

    let id = entity.id.clone();
    let owner = entity.name.clone();
    super::emit_signature_types(&entity, &[*node], &["field_declaration_list"], ctx);
    ctx.result.add_entity(entity);

    let inner = Scope {
        namespace: scope.namespace,
        parent_id: Some(&id),
        owner: Some(&owner),
        // `class` members are private until an access specifier says
        // otherwise; `struct` and `union` members are public.
        access: if is_struct {
            Visibility::Public
        } else {
            Visibility::Private
        },
    };
    super::extract_entities(body, &inner, ctx);
}

/// `enum Color { … }` and `enum class Color { … }`. The enumerators
/// become the entity's fields, which is what `field_count` reads.
pub(super) fn handle_enum(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = type_name(&name_node, ctx.source);
    let mut entity = CodeEntity::new(&name, EntityKind::Enum, ctx.path, node_to_span(node));
    entity.qualified_name = join_scope(scope.namespace, &name);
    entity.parent_id = scope.parent_id.map(String::from);
    entity.visibility = scope.access;
    entity.documentation = extract_doc(node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    entity.metrics.loc = line_count(&entity);
    if find_child_by_kind(node, "class").is_some() || node_text(node, ctx.source).starts_with("enum class") {
        entity.tags.insert("scoped".to_string());
    }
    if let Some(body) = node.child_by_field_name("body") {
        entity.fields = enumerators(&body, ctx.source);
    }
    entity.metrics.field_count = Some(entity.fields.len() as u32);
    ctx.result.add_entity(entity);
}

/// `using Id = long;` and `typedef unsigned int Uint;` — one construct as
/// far as the graph is concerned, spelled two ways with the name and the
/// type at opposite ends of each other.
pub(super) fn handle_alias(node: &Node, scope: &Scope<'_>, ctx: &mut ExtractCtx<'_>) {
    let name_node = match node.kind() {
        "alias_declaration" => node.child_by_field_name("name"),
        _ => node.child_by_field_name("declarator"),
    };
    let Some(name_node) = name_node else { return };
    let name = node_text(&name_node, ctx.source).to_string();
    let mut entity = CodeEntity::new(&name, EntityKind::TypeAlias, ctx.path, node_to_span(node));
    entity.qualified_name = join_scope(scope.namespace, &name);
    entity.parent_id = scope.parent_id.map(String::from);
    entity.visibility = scope.access;
    entity.return_type = node
        .child_by_field_name("type")
        .map(|t| node_text(&t, ctx.source).to_string());
    entity.documentation = extract_doc(node, ctx.source);
    entity.source_code = Some(node_text(node, ctx.source).to_string());
    entity.metrics.loc = line_count(&entity);
    super::emit_signature_types(&entity, &[*node], &[], ctx);
    ctx.result.add_entity(entity);
}

/// Whether a class body declares a pure virtual member — the one thing
/// that makes a C++ class abstract.
fn is_abstract(body: &Node) -> bool {
    let mut stack = vec![*body];
    while let Some(node) = stack.pop() {
        if node.kind() == "pure_virtual_clause" {
            return true;
        }
        // A nested class's pure virtual makes the nested class abstract,
        // not this one.
        if node.id() != body.id() && matches!(node.kind(), "class_specifier" | "struct_specifier") {
            continue;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    false
}

/// The types after the `:` of a class head, without the `public` /
/// `private` / `virtual` keywords that decorate them.
fn base_classes(node: &Node, source: &str) -> Vec<String> {
    let Some(clause) = find_child_by_kind(node, "base_class_clause") else {
        return Vec::new();
    };
    let mut cursor = clause.walk();
    clause
        .children(&mut cursor)
        .filter(|c| matches!(c.kind(), "type_identifier" | "qualified_identifier" | "template_type"))
        .map(|c| node_text(&c, source).to_string())
        .collect()
}

/// The data members of a class body, with the access section each one
/// sits in — which is what `public_field_ratio` reads.
fn data_members(body: &Node, source: &str, is_struct: bool) -> Vec<Parameter> {
    let mut access = if is_struct {
        Visibility::Public
    } else {
        Visibility::Private
    };
    let mut fields = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "access_specifier" {
            access = access_visibility(&child).unwrap_or(access);
            continue;
        }
        if child.kind() != "field_declaration" {
            continue;
        }
        let Some(declarator) = child.child_by_field_name("declarator") else {
            continue;
        };
        if super::super::helpers::function_declarator(&declarator).is_some() {
            continue;
        }
        let Some(name) = super::super::helpers::declared_name(&declarator) else {
            continue;
        };
        fields.push(Parameter {
            name: node_text(&name, source).to_string(),
            type_name: super::super::helpers::declared_type(&child, Some(&declarator), source),
            default_value: child
                .child_by_field_name("default_value")
                .map(|v| node_text(&v, source).to_string()),
            visibility: Some(access),
        });
    }
    fields
}

fn enumerators(body: &Node, source: &str) -> Vec<Parameter> {
    let mut cursor = body.walk();
    body.children(&mut cursor)
        .filter(|c| c.kind() == "enumerator")
        .filter_map(|c| {
            let name = c.child_by_field_name("name")?;
            Some(Parameter {
                name: node_text(&name, source).to_string(),
                type_name: None,
                default_value: c
                    .child_by_field_name("value")
                    .map(|v| node_text(&v, source).to_string()),
                visibility: None,
            })
        })
        .collect()
}

/// The name a type declaration introduces. A template specialization
/// writes it as `Repository<int>`; the entity is named for the template,
/// because that is the name every other declaration in the file uses.
fn type_name(node: &Node, source: &str) -> String {
    if node.kind() == "template_type" {
        if let Some(name) = node.child_by_field_name("name") {
            return node_text(&name, source).to_string();
        }
    }
    node_text(node, source).to_string()
}
