//! Visibility and modifier-attribute parsing for Kotlin nodes.
//!
//! Kotlin groups modifiers into typed buckets (`class_modifier`,
//! `inheritance_modifier`, `member_modifier`, `function_modifier`,
//! `property_modifier`, `platform_modifier`, plus annotations), so the
//! attribute extractor walks each bucket in turn.

use super::super::language_parser::{find_child_by_kind, node_text};
use crate::models::Visibility;
use tree_sitter::Node;

pub(super) fn parse_visibility(node: &Node) -> Visibility {
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            if child.kind() == "visibility_modifier" {
                let mut inner = child.walk();
                for vc in child.children(&mut inner) {
                    match vc.kind() {
                        "public" => return Visibility::Public,
                        "private" => return Visibility::Private,
                        "protected" => return Visibility::Protected,
                        "internal" => return Visibility::Internal,
                        _ => {}
                    }
                }
            }
        }
    }
    // Kotlin default is public
    Visibility::Public
}

pub(super) fn parse_modifier_attributes(node: &Node, source: &str) -> Vec<String> {
    let mut attrs = Vec::new();
    let Some(modifiers) = find_child_by_kind(node, "modifiers") else {
        return attrs;
    };

    let mut cursor = modifiers.walk();
    for child in modifiers.children(&mut cursor) {
        match child.kind() {
            // Skip visibility — handled separately
            "visibility_modifier" => {}
            "class_modifier" => {
                // data, sealed, annotation, inner, enum, value
                let mut inner = child.walk();
                for mc in child.children(&mut inner) {
                    match mc.kind() {
                        "data" | "sealed" | "annotation" | "inner" | "enum" | "value" => {
                            attrs.push(mc.kind().to_string());
                        }
                        _ => {}
                    }
                }
            }
            "inheritance_modifier" => {
                // open, final, abstract
                let mut inner = child.walk();
                for mc in child.children(&mut inner) {
                    match mc.kind() {
                        "open" | "final" | "abstract" => {
                            attrs.push(mc.kind().to_string());
                        }
                        _ => {}
                    }
                }
            }
            "member_modifier" => {
                // override, lateinit
                let mut inner = child.walk();
                for mc in child.children(&mut inner) {
                    match mc.kind() {
                        "override" | "lateinit" => {
                            attrs.push(mc.kind().to_string());
                        }
                        _ => {}
                    }
                }
            }
            "function_modifier" => {
                // suspend, tailrec, inline, infix, operator, external
                let mut inner = child.walk();
                for mc in child.children(&mut inner) {
                    match mc.kind() {
                        "suspend" | "tailrec" | "inline" | "infix" | "operator" | "external" => {
                            attrs.push(mc.kind().to_string());
                        }
                        _ => {}
                    }
                }
            }
            "property_modifier" => {
                // const
                attrs.push("const".to_string());
            }
            "platform_modifier" => {
                // actual, expect
                let mut inner = child.walk();
                for mc in child.children(&mut inner) {
                    match mc.kind() {
                        "actual" | "expect" => {
                            attrs.push(mc.kind().to_string());
                        }
                        _ => {}
                    }
                }
            }
            "annotation" => {
                attrs.push(node_text(&child, source).to_string());
            }
            _ => {}
        }
    }

    attrs
}
