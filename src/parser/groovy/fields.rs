//! Field-style entity parsing for Groovy:
//!
//! * class-body `field_declaration` — emitted as `Property` entities,
//!   matching the Java parser's shape.
//! * script-scope `local_variable_declaration` annotated `@Field`
//!   (or `@groovy.transform.Field`) — emitted as `Variable` entities
//!   with a `module_state` tag, plus a `WritesTo` edge from the
//!   enclosing script container so they show up as the script's
//!   module-level state. Mirrors how Python module-level assignments
//!   become graph nodes (see PY-019).
//!
//! A single declaration node may carry multiple `variable_declarator`
//! children (`int a, b;`), so both helpers return a `Vec`.

use super::super::language_parser::{node_text, node_to_span};
use super::helpers::{declared_type, parse_modifier_attributes, parse_visibility};
use super::javadoc::extract_javadoc;
use crate::models::{CodeEntity, EntityKind, Visibility};
use std::path::Path;
use tree_sitter::Node;

/// Parse a class-body `field_declaration`. One node may yield multiple
/// entities (one per `variable_declarator`).
pub(super) fn parse_class_field(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Vec<CodeEntity> {
    let mut entities = Vec::new();
    let visibility = parse_visibility(node);
    let attributes = parse_modifier_attributes(node, source);
    let type_name = node
        .child_by_field_name("type")
        .and_then(|t| declared_type(&t, source));

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source).to_string();
                let span = node_to_span(&child);
                let mut entity = CodeEntity::new(&name, EntityKind::Property, path, span);
                entity.visibility = visibility;
                entity.attributes = attributes.clone();
                entity.parent_id = parent_id.map(String::from);
                entity.return_type = type_name.clone();
                entity.documentation = extract_javadoc(node, source);
                entity.source_code = Some(node_text(node, source).to_string());
                entities.push(entity);
            }
        }
    }
    entities
}

/// Parse a `@Field`-annotated `local_variable_declaration` at script
/// scope. Returns one Variable per `variable_declarator`, tagged
/// `module_state`. Each entity also carries:
///
/// * declared `return_type` — the Groovy type before the variable name
///   (`HybrisJdbcTemplate t = …`),
/// * modifiers — `final`, `private`, etc., plus the `@Field` annotation
///   itself (kept verbatim in attributes for traceability),
/// * a `bean:<name>` attribute when the initializer is a
///   `Registry.applicationContext.getBean('<name>')` call — cheap to
///   detect, and the natural seed for a future Spring-bean-resolution
///   ticket.
pub(super) fn parse_field_decl(
    node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
) -> Vec<CodeEntity> {
    let mut entities = Vec::new();
    let attributes = parse_modifier_attributes(node, source);
    let type_name = node
        .child_by_field_name("type")
        .and_then(|t| declared_type(&t, source));

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        if let Some(entity) = build_field_entity(
            &child,
            node,
            source,
            path,
            parent_id,
            &attributes,
            type_name.as_deref(),
        ) {
            entities.push(entity);
        }
    }
    entities
}

/// Build the Variable entity for one `@Field` declarator. Pulled out of
/// the per-declarator loop so the loop's nesting stays under the
/// project's max-depth ceiling, and so the per-declarator metadata
/// (visibility, bean detection, source_code) lives in one block.
fn build_field_entity(
    declarator: &Node,
    decl_node: &Node,
    source: &str,
    path: &Path,
    parent_id: Option<&str>,
    attributes: &[String],
    type_name: Option<&str>,
) -> Option<CodeEntity> {
    let name_node = declarator.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let span = node_to_span(declarator);
    let mut entity = CodeEntity::new(&name, EntityKind::Variable, path, span);
    entity.visibility = if attributes.iter().any(|a| a == "private") {
        Visibility::Private
    } else {
        Visibility::Public
    };
    entity.attributes = attributes.to_vec();
    entity.parent_id = parent_id.map(String::from);
    entity.return_type = type_name.map(String::from);
    entity.tags.insert("module_state".to_string());
    entity.tags.insert("field_annotation".to_string());

    if let Some(value) = declarator.child_by_field_name("value") {
        if let Some(bean) = extract_get_bean_target(&value, source) {
            entity.attributes.push(format!("bean:{}", bean));
        }
    }

    entity.source_code = Some(node_text(decl_node, source).to_string());
    Some(entity)
}

/// If the initializer expression matches the
/// `<chain>.applicationContext.getBean('<name>')` pattern (or the
/// `applicationContext.getBean(...)` short form), return the bean name
/// literal stripped of its quotes. Anything else returns `None` —
/// callers treat that as "not a bean lookup".
fn extract_get_bean_target(value: &Node, source: &str) -> Option<String> {
    if value.kind() != "method_invocation" {
        return None;
    }
    let name = value.child_by_field_name("name")?;
    if node_text(&name, source) != "getBean" {
        return None;
    }
    let args = value.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    for arg in args.children(&mut cursor) {
        // Take the first string literal argument. Groovy's grammar
        // exposes both single- and double-quoted strings as
        // `string_literal` / `character_literal`; strip the outer
        // quotes whichever form they used.
        match arg.kind() {
            "string_literal" | "character_literal" => {
                let raw = node_text(&arg, source);
                let trimmed = raw.trim_matches(|c| c == '\'' || c == '"');
                return Some(trimmed.to_string());
            }
            _ => {}
        }
    }
    None
}
