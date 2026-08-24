//! Callable-body metrics: cyclomatic + cognitive complexity, max nesting.
//!
//! Ports [`crate::parser::java::complexity`] — the scoring convention has to
//! hold across languages for the scores to be comparable, so the shape is
//! deliberately identical: a nesting structure costs `+1 + depth` cognitive,
//! a flat increment costs `+1`, and every branch adds `+1` cyclomatic.
//!
//! Five Dart-specific decisions:
//!
//! * **`??` is a branch.** `a ?? b()` decides at runtime whether `b` runs,
//!   which is one more path through the callable — the same call the
//!   TypeScript pass makes for the operator Dart borrowed it from. It
//!   arrives as its own `if_null_expression` node rather than as an
//!   operator inside a binary expression.
//! * **`?.` costs nothing.** It guards one dereference rather than opening
//!   a second path: the same call sequence either runs or does not. This
//!   matches SonarSource, and the Groovy pass's treatment of `?.`.
//! * **`&&` / `||` are their own node kinds** (`logical_and_expression`,
//!   `logical_or_expression`) rather than operators inside a shared binary
//!   node, so no child scan is needed to find them. Each adds `+1` to both
//!   scores, as everywhere else.
//! * **Closures nest, local functions nest.** A `function_expression` opens
//!   a cognitive nesting scope and adds no cyclomatic complexity — Java's
//!   rule for `lambda_expression`. A function *declared* inside a body is
//!   not skipped either: this parser registers no entity for one, so its
//!   branching belongs to the callable that contains it.
//!
//! * **A pattern `switch` counts like any other.** Dart 3's expression form
//!   (`switch (o) { int i when i > 0 => … }`) is a different node kind from
//!   the statement form, and its arms are `switch_expression_case`s — so
//!   both spellings are listed, and a `when` guard adds a path of its own on
//!   top of the arm it guards (the grammar leaves the guard as a bare
//!   keyword, so it is counted where the arm is). `default` counts as an arm
//!   like any other, which is what Java's `switch_label` does. An `if (o case P)` needs nothing extra: the
//!   `if` is already counted, and the pattern decides the same one branch.
//!
//! `await` and `yield` are not branches. They suspend one path; they do
//! not create a second.

use tree_sitter::Node;

/// Node kinds that add one path through the callable.
const BRANCHING_KINDS: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "switch_statement_case",
    "switch_statement_default",
    "switch_expression_case",
    "catch_clause",
    "conditional_expression",
    "if_null_expression",
    "logical_and_expression",
    "logical_or_expression",
];

/// Node kinds that open a cognitive nesting scope: `+1 + depth` on entry,
/// and one more level of depth for everything inside.
const NESTING_KINDS: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "switch_statement",
    "switch_expression",
    "conditional_expression",
    "catch_clause",
    "function_expression",
];

/// Node kinds that add `+1` cognitive with no nesting weight — the flat
/// increments. Short-circuit operators are the whole list: they add a path
/// without adding a level for what follows.
const FLAT_COGNITIVE_KINDS: &[&str] = &[
    "if_null_expression",
    "logical_and_expression",
    "logical_or_expression",
];

/// Node kinds that add a level of nesting depth. A superset of
/// [`NESTING_KINDS`]: a `case`'s statements really are one level deeper than
/// the `switch`, but the case itself scores no cognitive increment (Java's
/// convention for `switch_label`).
const DEPTH_KINDS: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "switch_statement",
    "switch_expression",
    "switch_statement_case",
    "switch_statement_default",
    "switch_expression_case",
    "conditional_expression",
    "catch_clause",
    "function_expression",
];

/// Walk a callable body counting branching constructs and tracking nesting
/// depth. Returns `(cyclomatic, max_nesting, cognitive)`.
pub(super) fn compute_complexity(body: &Node) -> (u32, u32, u32) {
    let mut cyclomatic: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        let kind = node.kind();

        if BRANCHING_KINDS.contains(&kind) {
            cyclomatic = cyclomatic.saturating_add(1);
        }
        if NESTING_KINDS.contains(&kind) {
            cognitive = cognitive.saturating_add(1 + depth);
        }
        if FLAT_COGNITIVE_KINDS.contains(&kind) {
            cognitive = cognitive.saturating_add(1);
        }
        // A terminal `else` is a second path the `if` node alone doesn't
        // account for. A chained `else if` is not: the grammar nests
        // another `if_statement` there, and that one is counted on its
        // own visit.
        if kind == "if_statement" && has_plain_else(&node) {
            cyclomatic = cyclomatic.saturating_add(1);
            cognitive = cognitive.saturating_add(1);
        }
        // A `when` guard is a second decision on top of the arm it guards:
        // the pattern can match and the arm still not run. The grammar
        // leaves it as a bare keyword rather than wrapping the guarded
        // pattern, so it is found the same way a terminal `else` is.
        if kind == "switch_expression_case" && has_child_token(&node, "when") {
            cyclomatic = cyclomatic.saturating_add(1);
            cognitive = cognitive.saturating_add(1);
        }

        let child_depth = if DEPTH_KINDS.contains(&kind) {
            depth + 1
        } else {
            depth
        };
        if child_depth > max_depth {
            max_depth = child_depth;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, child_depth));
        }
    }

    (cyclomatic, max_depth, cognitive)
}

/// True when `node` has a direct child token of the given kind.
fn has_child_token(node: &Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|child| child.kind() == kind);
    found
}

/// True when this `if_statement` ends in an `else` that is not another
/// `if`. The grammar names no fields, so the alternative is found as the
/// first named node after the `else` token.
fn has_plain_else(node: &Node) -> bool {
    let mut cursor = node.walk();
    let mut seen_else = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "else" {
            seen_else = true;
        } else if seen_else && child.is_named() {
            return child.kind() != "if_statement";
        }
    }
    false
}
