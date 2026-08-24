//! Function-body metrics: cyclomatic + cognitive complexity, max nesting,
//! and return-tuple cardinality.
//!
//! Read by [`super::declarations`] while it builds a function entity, and by
//! nothing in [`super::bodies`]. It grew there, but a body's metrics are not
//! something the body *yields* — they are a property of the node the
//! declaration already has in hand, which is what the files at this level are
//! for. Living here leaves `bodies` with a single entrance.

use tree_sitter::Node;

/// Walk a function body subtree counting branching constructs and tracking
/// the maximum nesting depth of control-flow containers. Returns
/// `(cyclomatic_complexity, max_nesting_depth, cognitive_complexity)`.
///
/// **Cyclomatic** uses the classic "branches + 1" formulation: every
/// `if`/`match-arm`/loop/short-circuit boolean/`?` contributes 1.
///
/// **Cognitive** (SonarSource-inspired) weights each nesting-increasing
/// structure by its current depth (`+1 + depth`), while flat structures
/// (`else`, `&&`/`||`, `?`) contribute only `+1`. Match arms don't
/// increment cognitive — the `match` itself is counted.
///
/// Keeps the traversal iterative to stay cheap on large bodies.
pub(in crate::parser::rust) fn compute_complexity(body: &Node) -> (u32, u32, u32) {
    let mut complexity: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    // (node, depth-at-this-node)
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        let kind = node.kind();

        // --- Cyclomatic complexity: +1 for every branch ---
        let increments_cc = matches!(
            kind,
            "if_expression"
                | "else_clause"
                | "while_expression"
                | "while_let_expression"
                | "for_expression"
                | "loop_expression"
                | "match_arm"
                | "try_expression"
        );
        if increments_cc {
            complexity = complexity.saturating_add(1);
        }

        // --- Cognitive complexity ---
        // Nesting structures: +1 base + current nesting depth.
        let nesting_increment = matches!(
            kind,
            "if_expression"
                | "while_expression"
                | "while_let_expression"
                | "for_expression"
                | "loop_expression"
                | "match_expression"
                | "closure_expression"
        );
        if nesting_increment {
            cognitive = cognitive.saturating_add(1 + depth);
        }
        // Flat structures: +1 only (no nesting bonus).
        // `else_clause` and `try_expression` add a decision but don't deepen nesting.
        let flat_increment = matches!(kind, "else_clause" | "try_expression");
        if flat_increment {
            cognitive = cognitive.saturating_add(1);
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
            "if_expression"
                | "while_expression"
                | "while_let_expression"
                | "for_expression"
                | "loop_expression"
                | "match_expression"
                | "match_arm"
                | "closure_expression"
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

/// Count the number of elements in a return type's outermost tuple. Walks the
/// tree-sitter `return_type` node looking for a `tuple_type` child. Returns
/// `Some(n)` where n is the number of comma-separated elements, or `None` if
/// the return type is not a tuple (or is a single-element type).
///
/// Handles `Result<(A, B, C), E>` by finding the tuple inside the generic args.
pub(in crate::parser::rust) fn count_return_tuple_elements(return_type_node: &Node) -> Option<u32> {
    // DFS to find the first tuple_type node.
    let mut stack = vec![*return_type_node];
    while let Some(node) = stack.pop() {
        if node.kind() == "tuple_type" {
            let count = node.named_child_count() as u32;
            return if count > 1 { Some(count) } else { None };
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}
