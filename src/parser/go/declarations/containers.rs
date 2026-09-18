//! The types a Go file declares: structs, interfaces, defined types and
//! aliases — everything spelled `type X …`.
//!
//! Go's four shapes come out of one grammar node, `type_spec`, separated
//! only by what the right-hand side is. What they become here:
//!
//! | Source | Entity |
//! |---|---|
//! | `type Order struct { … }` | `Struct`, with its fields |
//! | `type Store interface { … }` | `Interface`, with its methods |
//! | `type Celsius float64` | `TypeAlias`, tagged `defined_type` |
//! | `type Alias = Other` | `TypeAlias`, tagged `alias` |
//!
//! The third row is the one worth naming. `type Celsius float64` defines a
//! *new* type — it is not an alias, it has its own method set, and Go's own
//! docs are careful about the difference. There is no `DefinedType` kind
//! and inventing one would ripple through every renderer, so it shares
//! `TypeAlias` and carries a tag that says which it is.
//!
//! **Embedding lands in `extends`.** `type Server struct { *Logger }`
//! promotes the `Logger`'s methods onto `Server`, which is what a reader
//! of an inheritance edge expects to find. It is composition in the type
//! system and inheritance in the call graph, and the call graph is what
//! this drawing is of.

use super::super::doc_comments::extract_spec_doc;
use super::super::helpers::{
    base_type_name, parse_generics, parse_parameters, result_text, visibility_of,
};
use crate::models::entity::Parameter;
use crate::models::{CodeEntity, EntityKind, Visibility};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

/// Parse `type X <something>`. The entity's kind follows the right-hand
/// side; its members, when it has any, are added by the caller.
pub(super) fn parse_type_spec(
    node: &Node,
    source: &str,
    path: &Path,
    package: &str,
) -> Option<CodeEntity> {
    let name = node_text(&node.child_by_field_name("name")?, source).to_string();
    let underlying = node.child_by_field_name("type")?;

    let kind = match underlying.kind() {
        "struct_type" => EntityKind::Struct,
        "interface_type" => EntityKind::Interface,
        _ => EntityKind::TypeAlias,
    };
    let mut entity = base_entity(&name, kind, node, source, path, package);
    if kind == EntityKind::TypeAlias {
        entity.tags.insert("defined_type".to_string());
        entity.return_type = Some(node_text(&underlying, source).to_string());
    }

    if let Some(generics) = node.child_by_field_name("type_parameters") {
        entity.generics = parse_generics(&generics, source);
    }

    match underlying.kind() {
        "struct_type" => populate_struct(&mut entity, &underlying, source),
        "interface_type" => entity.extends = embedded_interfaces(&underlying, source),
        _ => {}
    }

    Some(entity)
}

/// Parse `type X = Y`, the true alias — a second name for one type, with
/// no method set of its own.
pub(super) fn parse_type_alias(
    node: &Node,
    source: &str,
    path: &Path,
    package: &str,
) -> Option<CodeEntity> {
    let name = node_text(&node.child_by_field_name("name")?, source).to_string();
    let mut entity = base_entity(&name, EntityKind::TypeAlias, node, source, path, package);
    entity.tags.insert("alias".to_string());
    entity.return_type = node
        .child_by_field_name("type")
        .map(|t| node_text(&t, source).to_string());
    Some(entity)
}

/// The methods an interface requires, as entities under it.
///
/// An interface method has a signature and no body, which is exactly what
/// an abstract method is elsewhere — so it is a `Method` with the metrics
/// a bodyless callable gets: one path through it, no nesting.
pub(super) fn interface_methods(
    spec: &Node,
    source: &str,
    path: &Path,
    parent_id: &str,
) -> Vec<CodeEntity> {
    let Some(underlying) = spec.child_by_field_name("type") else {
        return Vec::new();
    };
    let mut methods = Vec::new();
    let mut cursor = underlying.walk();
    for element in underlying.children(&mut cursor) {
        if element.kind() != "method_elem" {
            continue;
        }
        let Some(name_node) = element.child_by_field_name("name") else {
            continue;
        };
        let name = node_text(&name_node, source).to_string();
        let mut entity = CodeEntity::new(&name, EntityKind::Method, path, node_to_span(&element));
        entity.visibility = visibility_of(&name);
        entity.parent_id = Some(parent_id.to_string());
        entity.tags.insert("interface_method".to_string());
        if let Some(params) = element.child_by_field_name("parameters") {
            entity.parameters = parse_parameters(&params, source);
        }
        entity.return_type = result_text(&element, source);
        entity.documentation = extract_spec_doc(&element, source);
        entity.source_code = Some(node_text(&element, source).to_string());
        entity.metrics.param_count = Some(entity.parameters.len() as u32);
        entity.metrics.cyclomatic = Some(1);
        entity.metrics.max_nesting = Some(0);
        entity.metrics.cognitive_complexity = Some(0);
        crate::parser::working_set::populate(&mut entity, None, source);
        methods.push(entity);
    }
    methods
}

/// The parts every `type X …` entity shares, whichever shape it turns out
/// to be: name, visibility, package qualification, doc and source.
fn base_entity(
    name: &str,
    kind: EntityKind,
    node: &Node,
    source: &str,
    path: &Path,
    package: &str,
) -> CodeEntity {
    let span = node_to_span(node);
    let mut entity = CodeEntity::new(name, kind, path, span);
    entity.visibility = visibility_of(name);
    if !package.is_empty() {
        entity.qualified_name = format!("{}.{}", package, name);
    }
    entity.documentation = extract_spec_doc(node, source);
    entity.source_code = Some(node_text(node, source).to_string());
    entity.metrics.loc = (span.end.line - span.start.line + 1) as u32;
    entity
}

/// Fill in a struct's fields, its embedded types, and the metrics that
/// read off them.
fn populate_struct(entity: &mut CodeEntity, struct_type: &Node, source: &str) {
    let mut cursor = struct_type.walk();
    for list in struct_type.children(&mut cursor) {
        if list.kind() != "field_declaration_list" {
            continue;
        }
        let mut fields = list.walk();
        for declaration in list.children(&mut fields) {
            if declaration.kind() == "field_declaration" {
                read_field(entity, &declaration, source);
            }
        }
    }

    entity.metrics.field_count = Some(entity.fields.len() as u32);
    if entity.fields.is_empty() {
        return;
    }
    let exported = entity
        .fields
        .iter()
        .filter(|f| matches!(f.visibility, Some(Visibility::Public)))
        .count();
    entity.metrics.public_field_ratio = Some(exported as f32 / entity.fields.len() as f32);
}

/// One `field_declaration`, which may name several fields of one type
/// (`x, y int`) or none at all — the nameless form is an embedded type.
fn read_field(entity: &mut CodeEntity, declaration: &Node, source: &str) {
    let type_name = declaration
        .child_by_field_name("type")
        .map(|t| node_text(&t, source).to_string());
    let mut cursor = declaration.walk();
    let names: Vec<String> = declaration
        .children_by_field_name("name", &mut cursor)
        .map(|n| node_text(&n, source).to_string())
        .collect();

    if names.is_empty() {
        // `type Server struct { *Logger }` — the type is the field name,
        // and its methods are promoted onto the embedding struct.
        let Some(written) = &type_name else { return };
        let embedded = base_type_name(written);
        entity.extends.push(embedded.clone());
        entity.tags.insert("embeds".to_string());
        entity.fields.push(Parameter {
            name: embedded.clone(),
            type_name: type_name.clone(),
            default_value: None,
            visibility: Some(visibility_of(&embedded)),
        });
        return;
    }

    for name in names {
        let visibility = Some(visibility_of(&name));
        entity.fields.push(Parameter {
            name,
            type_name: type_name.clone(),
            default_value: None,
            visibility,
        });
    }
}

/// The interfaces an interface embeds. Anything in its body that is not a
/// `method_elem` is a type element, and an embedded interface is the case
/// that means something to the graph: `interface { io.Reader; Close() }`
/// requires everything `io.Reader` does.
fn embedded_interfaces(interface_type: &Node, source: &str) -> Vec<String> {
    let mut cursor = interface_type.walk();
    interface_type
        .children(&mut cursor)
        .filter(|c| c.kind() == "type_elem")
        .map(|c| base_type_name(node_text(&c, source)))
        .collect()
}
