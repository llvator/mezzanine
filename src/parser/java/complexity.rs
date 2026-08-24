//! Method-body metrics: cyclomatic + cognitive complexity, max nesting.
//!
//! Mirrors [`crate::parser::rust::bodies::complexity`] so per-entity scoring lines
//! up across languages. The Rust pass is the reference: nesting structures
//! count `+1 + depth` for cognitive complexity, flat increments count `+1`,
//! and every branch adds `+1` to cyclomatic. The tree-sitter-java node
//! names diverge from Rust's (`if_statement` vs `if_expression`,
//! `switch_label` vs `match_arm`), so this file maps the equivalents.

use tree_sitter::Node;

/// Walk a method body subtree counting branching constructs and tracking
/// nesting depth. Returns `(cyclomatic, max_nesting, cognitive)`.
///
/// Tree-sitter-java specifics:
/// - `else` has no dedicated node — it sits in the parent `if_statement`'s
///   `alternative` field. A chained `else if` is a nested `if_statement`
///   that gets counted on its own visit, so we only add the "else penalty"
///   when the alternative is something *other* than another `if_statement`.
/// - `switch_label` covers both classic `case X:` and the arrow form (the
///   arrow's `switch_rule` wraps one). Each label increments cyclomatic by
///   one and contributes nothing to cognitive — same convention as Rust's
///   `match_arm`.
/// - `lambda_expression` opens a cognitive nesting scope but does not add
///   to cyclomatic complexity (lambdas don't branch the enclosing method's
///   control flow until they're invoked).
pub(super) fn compute_complexity(body: &Node) -> (u32, u32, u32) {
    let mut complexity: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        let kind = node.kind();

        // --- Cyclomatic complexity: +1 for every branch ---
        let increments_cc = matches!(
            kind,
            "if_statement"
                | "while_statement"
                | "do_statement"
                | "for_statement"
                | "enhanced_for_statement"
                | "switch_label"
                | "catch_clause"
                | "ternary_expression"
        );
        if increments_cc {
            complexity = complexity.saturating_add(1);
        }

        // `if/else` adds a second path. Mirror the Rust parser's `else_clause`
        // increment, but in Java the else lives on the parent `if`'s
        // `alternative` field. Skip chained `else if` — that branch is the
        // nested `if_statement` we'll visit anyway.
        if kind == "if_statement" {
            if let Some(alt) = node.child_by_field_name("alternative") {
                if alt.kind() != "if_statement" {
                    complexity = complexity.saturating_add(1);
                    cognitive = cognitive.saturating_add(1);
                }
            }
        }

        // --- Cognitive complexity ---
        // Nesting structures: +1 base + current nesting depth.
        let nesting_increment = matches!(
            kind,
            "if_statement"
                | "while_statement"
                | "do_statement"
                | "for_statement"
                | "enhanced_for_statement"
                | "switch_expression"
                | "ternary_expression"
                | "catch_clause"
                | "lambda_expression"
        );
        if nesting_increment {
            cognitive = cognitive.saturating_add(1 + depth);
        }

        // Short-circuit operators: both CC and cognitive get +1 each.
        if kind == "binary_expression" {
            let mut cursor = node.walk();
            for c in node.children(&mut cursor) {
                let t = c.kind();
                if t == "&&" || t == "||" {
                    complexity = complexity.saturating_add(1);
                    cognitive = cognitive.saturating_add(1);
                }
            }
        }

        // --- Nesting depth tracking (shared by max_nesting + cognitive) ---
        let opens_scope = matches!(
            kind,
            "if_statement"
                | "while_statement"
                | "do_statement"
                | "for_statement"
                | "enhanced_for_statement"
                | "switch_expression"
                | "switch_label"
                | "catch_clause"
                | "ternary_expression"
                | "lambda_expression"
        );
        let child_depth = if opens_scope { depth + 1 } else { depth };
        if child_depth > max_depth {
            max_depth = child_depth;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, child_depth));
        }
    }

    (complexity, max_depth, cognitive)
}
