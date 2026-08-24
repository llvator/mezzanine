//! Call-site extraction for Kotlin function bodies.
//!
//! In Kotlin's tree-sitter grammar, a `call_expression` is either
//! `simple_identifier + call_suffix` (plain `foo(...)`) or
//! `navigation_expression + call_suffix` (member `obj.foo(...)`).

use super::stdlib::is_stdlib_method;
use crate::models::{Relationship, RelationshipKind};
use crate::parser::language_parser::{node_text, ParseResult};
use tree_sitter::Node;

#[allow(clippy::too_many_arguments)]
pub(in crate::parser::kotlin) fn extract_calls(
    node: &Node,
    source: &str,
    caller_id: &str,
    caller_name: &str,
    parent_class: Option<&str>,
    call_order: &mut u32,
    result: &mut ParseResult,
) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            handle_call_expression(
                &child,
                source,
                caller_id,
                caller_name,
                parent_class,
                call_order,
                result,
            );
            // Continue recursing into the call_expression for nested calls
            extract_calls(
                &child,
                source,
                caller_id,
                caller_name,
                parent_class,
                call_order,
                result,
            );
        } else {
            extract_calls(
                &child,
                source,
                caller_id,
                caller_name,
                parent_class,
                call_order,
                result,
            );
        }
    }
}

fn handle_call_expression(
    node: &Node,
    source: &str,
    caller_id: &str,
    caller_name: &str,
    parent_class: Option<&str>,
    call_order: &mut u32,
    result: &mut ParseResult,
) {
    let mut cursor = node.walk();
    let first_child = node.children(&mut cursor).next();

    let Some(target) = first_child else { return };

    match target.kind() {
        "simple_identifier" => {
            let func_name = node_text(&target, source).to_string();
            *call_order += 1;
            if func_name == caller_name || is_stdlib_method(&func_name) {
                return;
            }

            // Check if the name starts with uppercase → likely a constructor call
            if func_name.starts_with(|c: char| c.is_uppercase()) {
                let mut rel = Relationship::new(
                    caller_id.to_string(),
                    func_name,
                    RelationshipKind::Instantiates,
                );
                rel.metadata
                    .insert("order".to_string(), call_order.to_string());
                result.add_relationship(rel);
            } else {
                // Qualify with parent class if available
                let callee = if let Some(cls) = parent_class {
                    format!("{}.{}", cls, func_name)
                } else {
                    func_name
                };
                let mut rel =
                    Relationship::new(caller_id.to_string(), callee, RelationshipKind::Calls);
                rel.metadata
                    .insert("order".to_string(), call_order.to_string());
                result.add_relationship(rel);
            }
        }
        "navigation_expression" => {
            // Extract receiver and method name from navigation_expression
            if let Some((receiver, method_name)) = extract_navigation_parts(&target, source) {
                *call_order += 1;
                if method_name == caller_name || is_stdlib_method(&method_name) {
                    return;
                }

                let callee = match receiver.as_str() {
                    "this" | "super" => {
                        if let Some(cls) = parent_class {
                            format!("{}.{}", cls, method_name)
                        } else {
                            method_name
                        }
                    }
                    _ => format!("{}.{}", receiver, method_name),
                };

                let mut rel =
                    Relationship::new(caller_id.to_string(), callee, RelationshipKind::Calls);
                rel.metadata
                    .insert("order".to_string(), call_order.to_string());
                result.add_relationship(rel);
            }
        }
        _ => {}
    }
}

/// Extract (receiver, method_name) from a `navigation_expression` node.
fn extract_navigation_parts(node: &Node, source: &str) -> Option<(String, String)> {
    // navigation_expression has: child[0] = receiver, then navigation_suffix containing `.` + simple_identifier
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    let receiver_node = children.first()?;
    let nav_suffix = children.iter().find(|c| c.kind() == "navigation_suffix")?;

    let receiver = match receiver_node.kind() {
        "simple_identifier" => node_text(receiver_node, source).to_string(),
        "this_expression" => "this".to_string(),
        "super_expression" => "super".to_string(),
        // For chained calls like `a.b.c()`, take the full text as receiver
        _ => node_text(receiver_node, source).to_string(),
    };

    // navigation_suffix contains `.` and `simple_identifier`
    let method_name = {
        let mut sc = nav_suffix.walk();
        let found = nav_suffix
            .children(&mut sc)
            .find(|c| c.kind() == "simple_identifier")
            .map(|c| node_text(&c, source).to_string());
        match found {
            Some(n) => n,
            None => return None,
        }
    };

    Some((receiver, method_name))
}
