//! Body metrics: cyclomatic + cognitive complexity, max nesting.
//!
//! Same convention as every other parser here — nesting structures score
//! `+1 + depth` for cognitive complexity, flat increments score `+1`, and
//! every branch adds `+1` to cyclomatic — with the tree-sitter-cpp node
//! names mapped onto it. Read [`crate::parser::java::complexity`] for the
//! reference; three things differ, and only three:
//!
//! * **`else` is a node.** Java hangs the else on the `if`'s `alternative`
//!   field; C++ gives it an `else_clause`. A chained `else if` is that
//!   clause wrapping another `if_statement`, so the else penalty is
//!   charged only when the clause holds something else — otherwise the
//!   nested `if` would be counted twice.
//! * **`case_statement` covers `default:`** as well as `case X:`, which is
//!   the same bargain Java's `switch_label` makes: one increment per label.
//! * **A lambda opens a nesting scope without branching.** Identical to
//!   Java, and worth restating because C++ bodies carry far more of them.

use crate::models::CodeEntity;
use tree_sitter::Node;

/// Kinds that add a path through the body.
const BRANCHES: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "for_range_loop",
    "case_statement",
    "catch_clause",
    "conditional_expression",
];

/// Kinds that cost a reader more the deeper they sit.
const NESTING: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "for_range_loop",
    "switch_statement",
    "conditional_expression",
    "catch_clause",
    "lambda_expression",
];

/// Kinds whose children are one level further in.
const SCOPES: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "for_range_loop",
    "switch_statement",
    "case_statement",
    "catch_clause",
    "conditional_expression",
    "lambda_expression",
];

/// Walk a body counting branches and depth. Returns
/// `(cyclomatic, max_nesting, cognitive)`.
pub(super) fn compute_complexity(body: &Node) -> (u32, u32, u32) {
    let mut cyclomatic: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        let kind = node.kind();
        if BRANCHES.contains(&kind) {
            cyclomatic = cyclomatic.saturating_add(1);
        }
        if NESTING.contains(&kind) {
            cognitive = cognitive.saturating_add(1 + depth);
        }
        if is_terminal_else(&node) {
            cyclomatic = cyclomatic.saturating_add(1);
            cognitive = cognitive.saturating_add(1);
        }
        let (short_circuits, _) = logical_operators(&node);
        cyclomatic = cyclomatic.saturating_add(short_circuits);
        cognitive = cognitive.saturating_add(short_circuits);

        let child_depth = if SCOPES.contains(&kind) { depth + 1 } else { depth };
        max_depth = max_depth.max(child_depth);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, child_depth));
        }
    }

    (cyclomatic, max_depth, cognitive)
}

/// Whether this node is an `else` that is not a chained `else if`. The
/// chained form is a nested `if_statement` the walk counts on its own
/// visit; charging the clause too would double it.
fn is_terminal_else(node: &Node) -> bool {
    if node.kind() != "else_clause" {
        return false;
    }
    let mut cursor = node.walk();
    let chained = node
        .children(&mut cursor)
        .filter(|c| c.is_named())
        .any(|c| c.kind() == "if_statement");
    !chained
}

/// How many `&&` / `||` operators this node spells, and nothing else.
/// Returned as a pair so the caller reads one number per metric rather
/// than adding the same count twice by hand.
fn logical_operators(node: &Node) -> (u32, u32) {
    if node.kind() != "binary_expression" {
        return (0, 0);
    }
    let mut cursor = node.walk();
    let count = node
        .children(&mut cursor)
        .filter(|c| matches!(c.kind(), "&&" | "||" | "and" | "or"))
        .count() as u32;
    (count, count)
}

/// Fill the three body metrics on a callable.
///
/// A declaration with no body — a prototype, a pure virtual, an
/// `= default` — scores `1 / 0 / 0` rather than `None`. The hotspot
/// ranking has to see "measured and trivial" rather than "not measured"
/// (§D2), and a signature does describe one straight-through path.
pub(super) fn score_body(entity: &mut CodeEntity, body: Option<&Node>) {
    let (cyclomatic, nesting, cognitive) = match body {
        Some(body) => compute_complexity(body),
        None => (1, 0, 0),
    };
    entity.metrics.cyclomatic = Some(cyclomatic);
    entity.metrics.max_nesting = Some(nesting);
    entity.metrics.cognitive_complexity = Some(cognitive);
}
