//! Package-level `const` and `var` declarations.
//!
//! One spec can name several values against one type (`var a, b int`), and
//! a grouped `const ( … )` holds many specs, so a declaration yields a list
//! rather than an entity.
//!
//! Constants declared without a value inherit the previous spec's
//! expression — the `iota` idiom that gives Go its enums:
//!
//! ```go
//! const (
//!     StateIdle State = iota
//!     StateRunning
//!     StateDone
//! )
//! ```
//!
//! `StateRunning` states neither type nor value, and reading it as
//! untyped would lose exactly the fact that makes the block an enum. Each
//! spec without a type therefore takes the type of the last one that had
//! it, which is the rule the compiler applies.

use super::super::ctx::ExtractCtx;
use super::super::doc_comments::extract_spec_doc;
use super::super::helpers::visibility_of;
use super::super::types::{self, TypeUse};
use crate::models::{CodeEntity, EntityKind};
use crate::parser::language_parser::{node_text, node_to_span};
use std::path::Path;
use tree_sitter::Node;

/// Add every entity one `const` or `var` declaration introduces, with the
/// `UsesType` edges its declared type names.
pub(super) fn handle_declaration(node: &Node, ctx: &mut ExtractCtx<'_>) {
    for (entity, spec) in parse_declaration(node, ctx.source, ctx.path, ctx.package) {
        let entity_id = entity.id.clone();
        let owner_name = entity.name.clone();
        ctx.result.add_entity(entity);
        let Some(declared) = spec.child_by_field_name("type") else {
            continue;
        };
        types::emit(
            &[declared],
            &[],
            &TypeUse {
                entity_id: &entity_id,
                owner_name: &owner_name,
                source: ctx.source,
                imports: ctx.imports,
            },
            ctx.result,
        );
    }
}

/// Parse one `const` or `var` declaration into an entity per name, each
/// paired with the spec it came from.
fn parse_declaration<'a>(
    node: &Node<'a>,
    source: &str,
    path: &Path,
    package: &str,
) -> Vec<(CodeEntity, Node<'a>)> {
    let kind = match node.kind() {
        "const_declaration" => EntityKind::Constant,
        _ => EntityKind::Variable,
    };
    let mut entities = Vec::new();
    // Carried across specs so an `iota` run keeps the type its first line
    // declared.
    let mut inherited_type: Option<String> = None;

    for spec in specs(node) {
        let declared = spec
            .child_by_field_name("type")
            .map(|t| node_text(&t, source).to_string());
        let states_a_value = spec.child_by_field_name("value").is_some();
        let type_name = match (&declared, states_a_value) {
            // A spec that declares a type starts the run.
            (Some(_), _) => {
                inherited_type = declared.clone();
                declared
            }
            // A spec with neither type nor value repeats the one above it,
            // type included. This is `iota` counting down a const block.
            (None, false) if kind == EntityKind::Constant => inherited_type.clone(),
            // A spec with a value but no type starts an untyped run, and
            // anything repeating it is untyped too.
            _ => {
                inherited_type = None;
                None
            }
        };
        entities.extend(
            parse_spec(&spec, source, path, package, kind, type_name)
                .into_iter()
                .map(|entity| (entity, spec)),
        );
    }
    entities
}

/// The specs of one declaration — the single-line form holds one, the
/// parenthesised form wraps them in a list.
fn specs<'a>(declaration: &Node<'a>) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    let mut cursor = declaration.walk();
    for child in declaration.children(&mut cursor) {
        match child.kind() {
            "const_spec" | "var_spec" => out.push(child),
            "var_spec_list" => {
                let mut inner = child.walk();
                out.extend(
                    child
                        .children(&mut inner)
                        .filter(|c| c.kind() == "var_spec"),
                );
            }
            _ => {}
        }
    }
    out
}

fn parse_spec(
    spec: &Node,
    source: &str,
    path: &Path,
    package: &str,
    kind: EntityKind,
    type_name: Option<String>,
) -> Vec<CodeEntity> {
    let documentation = extract_spec_doc(spec, source);

    let mut cursor = spec.walk();
    let names: Vec<String> = spec
        .children_by_field_name("name", &mut cursor)
        .map(|n| node_text(&n, source).to_string())
        .collect();

    names
        .into_iter()
        .map(|name| {
            let mut entity = CodeEntity::new(&name, kind, path, node_to_span(spec));
            entity.visibility = visibility_of(&name);
            if !package.is_empty() {
                entity.qualified_name = format!("{}.{}", package, name);
            }
            entity.return_type = type_name.clone();
            entity.documentation = documentation.clone();
            entity.source_code = Some(node_text(spec, source).to_string());
            entity
        })
        .collect()
}
