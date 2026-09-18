//! Reading a C++ declaration: declarators, parameters, specifiers, names.
//!
//! Every other language parsed here spells a declaration's name where the
//! grammar can hand it over in one field. C++ wraps it instead. `Order*
//! Factory::make(int id) const` is a `type` of `Order` and a *declarator*
//! that reads outward-in — a pointer around a function around a qualified
//! name — and the parts a reader calls "the return type" and "the name"
//! are at opposite ends of it. So the two descents below,
//! [`function_declarator`] and [`declared_name`], are what the rest of the
//! parser uses in place of `child_by_field_name("name")`, and
//! [`pointer_decoration`] puts the `*` back on the return type where a
//! reader expects to see it.

use crate::models::entity::Parameter;
use crate::models::Visibility;
use crate::parser::language_parser::node_text;
use tree_sitter::Node;

/// Declarator nodes that wrap another declarator without naming anything.
/// The grammar gives most of them a `declarator` field and some — a
/// `reference_declarator` around a function, notably — no field at all,
/// which is why [`inner_declarator`] falls back to the first named child.
const WRAPPERS: &[&str] = &[
    "pointer_declarator",
    "reference_declarator",
    "array_declarator",
    "parenthesized_declarator",
    "init_declarator",
    "attributed_declarator",
];

/// Leaf nodes that spell a declared name.
const NAME_LEAVES: &[&str] = &[
    "identifier",
    "field_identifier",
    "type_identifier",
    "qualified_identifier",
    "destructor_name",
    "operator_name",
];

/// The declarator nested inside a wrapper, or `None` at a leaf.
fn inner_declarator<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    node.child_by_field_name("declarator")
        .or_else(|| node.named_child(0))
}

/// The `function_declarator` a declarator chain arrives at, or `None` when
/// the declaration declares data rather than a callable.
///
/// This is the one question that separates `Order* make(int)` from
/// `Order* cache_` — both are a `field_declaration` with a
/// `pointer_declarator`, and only the presence of a parameter list below
/// says which one the author wrote.
pub(super) fn function_declarator<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    if node.kind() == "function_declarator" {
        return Some(*node);
    }
    if !WRAPPERS.contains(&node.kind()) {
        return None;
    }
    function_declarator(&inner_declarator(node)?)
}

/// The name leaf a declarator chain arrives at — an `identifier`, a
/// `field_identifier`, a `qualified_identifier` (`Order::total`), a
/// `destructor_name` (`~Order`) or an `operator_name` (`operator+`).
pub(super) fn declared_name<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    if NAME_LEAVES.contains(&node.kind()) {
        return Some(*node);
    }
    if node.kind() != "function_declarator" && !WRAPPERS.contains(&node.kind()) {
        return None;
    }
    declared_name(&inner_declarator(node)?)
}

/// `Order::total` split into the scope it names and the bare name, with
/// nested scopes joined back up (`app::util::run` → `Some("app::util")`,
/// `"run"`). Anything that is not a `qualified_identifier` is all name and
/// no scope.
pub(super) fn qualified_parts(node: &Node, source: &str) -> (Option<String>, String) {
    if node.kind() != "qualified_identifier" {
        return (None, node_text(node, source).to_string());
    }
    let scope = node
        .child_by_field_name("scope")
        .map(|s| node_text(&s, source).to_string());
    let Some(name) = node.child_by_field_name("name") else {
        return (None, scope.unwrap_or_default());
    };
    let (inner_scope, base) = qualified_parts(&name, source);
    let joined = match (scope, inner_scope) {
        (Some(outer), Some(inner)) => Some(format!("{}::{}", outer, inner)),
        (Some(outer), None) => Some(outer),
        (None, inner) => inner,
    };
    (joined, base)
}

/// The `*` and `&` a declarator chain puts between the type and the name,
/// in source order, so `Order` + a pointer declarator reads back as
/// `Order*` on the entity rather than as a bare `Order`.
pub(super) fn pointer_decoration(declarator: &Node) -> String {
    let mut out = String::new();
    let mut current = Some(*declarator);
    while let Some(node) = current {
        match node.kind() {
            "pointer_declarator" | "abstract_pointer_declarator" => out.push('*'),
            "reference_declarator" | "abstract_reference_declarator" => out.push('&'),
            "function_declarator" => break,
            k if !WRAPPERS.contains(&k) => break,
            _ => {}
        }
        current = inner_declarator(&node);
    }
    out
}

/// The type a declaration's `type` field names, with the declarator's
/// pointer decoration appended.
pub(super) fn declared_type(node: &Node, declarator: Option<&Node>, source: &str) -> Option<String> {
    let base = node.child_by_field_name("type")?;
    let decoration = declarator.map(pointer_decoration).unwrap_or_default();
    Some(format!(
        "{}{}{}",
        cv_qualifiers(node, &base, source),
        node_text(&base, source),
        decoration
    ))
}

/// The `const` / `volatile` written before the type. The grammar keeps
/// them as siblings of the `type` field rather than inside it, so
/// `const std::string&` would otherwise read back as `std::string&` —
/// which is a different parameter.
fn cv_qualifiers(node: &Node, type_node: &Node, source: &str) -> String {
    let mut out = String::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() >= type_node.start_byte() {
            break;
        }
        if child.kind() == "type_qualifier" {
            out.push_str(node_text(&child, source));
            out.push(' ');
        }
    }
    out
}

/// Every parameter of a `parameter_list`, including the defaulted and
/// variadic forms. An unnamed parameter — legal and common in C++ for an
/// argument the body ignores — keeps its type and an empty name rather
/// than being dropped, because the arity is part of the signature.
pub(super) fn parse_parameters(list: &Node, source: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    let mut cursor = list.walk();
    for child in list.children(&mut cursor) {
        match child.kind() {
            "parameter_declaration"
            | "optional_parameter_declaration"
            | "variadic_parameter_declaration" => params.push(parameter(&child, source)),
            // `f(...)` — a C variadic ellipsis, which names no type.
            "variadic_declarator" => params.push(Parameter {
                name: "...".to_string(),
                type_name: None,
                default_value: None,
                visibility: None,
            }),
            _ => {}
        }
    }
    params
}

fn parameter(node: &Node, source: &str) -> Parameter {
    let declarator = node.child_by_field_name("declarator");
    let name = declarator
        .as_ref()
        .and_then(declared_name)
        .map(|n| node_text(&n, source).to_string())
        .unwrap_or_default();
    Parameter {
        name,
        type_name: declared_type(node, declarator.as_ref(), source),
        default_value: node
            .child_by_field_name("default_value")
            .map(|v| node_text(&v, source).to_string()),
        visibility: None,
    }
}

/// The names a `template_parameter_list` introduces — `typename T`,
/// `class U`, `int N` — kept whole so the detail panel reads as written.
pub(super) fn parse_template_parameters(list: &Node, source: &str) -> Vec<String> {
    let mut cursor = list.walk();
    list.children(&mut cursor)
        .filter(|c| c.is_named())
        .map(|c| node_text(&c, source).to_string())
        .collect()
}

/// Keyword specifiers a declaration carries, as tags (§B5). Read from the
/// declaration node's own children and from the `function_declarator`,
/// which is where `const`, `override` and `final` end up.
pub(super) fn specifier_tags(node: &Node, declarator: Option<&Node>) -> Vec<String> {
    let mut tags = Vec::new();
    collect_specifiers(node, &mut tags);
    if let Some(declarator) = declarator.and_then(function_declarator) {
        collect_specifiers(&declarator, &mut tags);
    }
    tags
}

fn collect_specifiers(node: &Node, out: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let tag = match child.kind() {
            "virtual" => "virtual",
            "explicit_function_specifier" => "explicit",
            "noexcept" => "noexcept",
            "pure_virtual_clause" => "pure_virtual",
            "default_method_clause" => "defaulted",
            "delete_method_clause" => "deleted",
            "storage_class_specifier" | "type_qualifier" | "virtual_specifier" => {
                // `static`, `inline`, `const`, `constexpr`, `override`,
                // `final` — one keyword each, spelled by the child token.
                let mut inner = child.walk();
                let keyword = child
                    .children(&mut inner)
                    .next()
                    .map(|k| k.kind().to_string())
                    .unwrap_or_else(|| child.kind().to_string());
                push_unique(out, keyword);
                continue;
            }
            _ => continue,
        };
        push_unique(out, tag.to_string());
    }
}

fn push_unique(out: &mut Vec<String>, tag: String) {
    if !out.contains(&tag) {
        out.push(tag);
    }
}

/// `app::core` + `Order` → `app::core::Order`; an empty scope leaves the
/// name alone.
pub(super) fn join_scope(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        return name.to_string();
    }
    format!("{}::{}", scope, name)
}

/// The access section an `access_specifier` opens.
pub(super) fn access_visibility(node: &Node) -> Option<Visibility> {
    let mut cursor = node.walk();
    let access = node.children(&mut cursor).find_map(|c| match c.kind() {
        "public" => Some(Visibility::Public),
        "private" => Some(Visibility::Private),
        "protected" => Some(Visibility::Protected),
        _ => None,
    });
    access
}

/// Lines spanned by an entity, inclusive — the `loc` metric (§D1).
pub(super) fn line_count(entity: &crate::models::CodeEntity) -> u32 {
    (entity.span.end.line - entity.span.start.line + 1) as u32
}
