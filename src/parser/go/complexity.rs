//! Function-body metrics: cyclomatic + cognitive complexity, max nesting.
//!
//! Mirrors [`crate::parser::java::complexity`], which mirrors the Rust pass,
//! so a Go function and a Java method scoring 12 mean the same thing. The
//! node names differ; the counting rules do not — nesting structures score
//! `+1 + depth` for cognitive, flat increments score `+1`, and every branch
//! adds `+1` to cyclomatic.
//!
//! What Go changes is which nodes are which:
//! - There is one loop keyword. `for` covers the counted loop, the
//!   condition-only loop, the range loop and the infinite loop, so
//!   `for_statement` is the single loop node rather than one of four.
//! - There is no ternary and no `catch`. The `if err != nil { return … }`
//!   guard is the idiom that replaces both, and it counts as the plain `if`
//!   it is — which is why idiomatic Go scores lower here than the same
//!   logic in a language with exceptions, and should.
//! - `select` is a switch over channel readiness. Its arms count like a
//!   switch's, because a reader has to follow each one the same way.

use tree_sitter::Node;

/// Walk a function body counting branching constructs and tracking nesting
/// depth. Returns `(cyclomatic, max_nesting, cognitive)`.
pub(super) fn compute_complexity(body: &Node) -> (u32, u32, u32) {
    let mut cyclomatic: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        let score = score_node(&node, depth);
        cyclomatic = cyclomatic.saturating_add(score.cyclomatic);
        cognitive = cognitive.saturating_add(score.cognitive);

        let child_depth = if opens_scope(node.kind()) {
            depth + 1
        } else {
            depth
        };
        max_depth = max_depth.max(child_depth);

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, child_depth));
        }
    }

    (cyclomatic, max_depth, cognitive)
}

/// What one node adds to each score. Separated from the walk so the two
/// questions — "what does this node cost" and "how deep is it" — are each
/// answerable on their own.
#[derive(Default)]
struct Score {
    cyclomatic: u32,
    cognitive: u32,
}

fn score_node(node: &Node, depth: u32) -> Score {
    let kind = node.kind();
    let mut score = Score::default();

    if increments_cyclomatic(kind) {
        score.cyclomatic += 1;
    }
    // Nesting structures cost `+1 + depth`: the same construct is harder
    // to follow the deeper it sits.
    if opens_cognitive_nesting(kind) {
        score.cognitive += 1 + depth;
    }
    match kind {
        "if_statement" if has_plain_else(node) => {
            // An `else` is a second path through the same decision. A
            // chained `else if` is not — the grammar nests it in the
            // `alternative` field as another `if_statement`, and it is
            // counted when the walk reaches it on its own.
            score.cyclomatic += 1;
            score.cognitive += 1;
        }
        // `&&` / `||` each add a path and a thing to hold in mind.
        "binary_expression" if is_short_circuit(node) => {
            score.cyclomatic += 1;
            score.cognitive += 1;
        }
        _ => {}
    }
    score
}

/// Whether an `if` ends in an `else` block rather than another `if`.
fn has_plain_else(node: &Node) -> bool {
    node.child_by_field_name("alternative")
        .is_some_and(|alt| alt.kind() != "if_statement")
}

fn is_short_circuit(node: &Node) -> bool {
    node.child_by_field_name("operator")
        .is_some_and(|op| matches!(op.kind(), "&&" | "||"))
}

/// Branch points: each one is a path through the function.
///
/// `default_case` is counted, which over-counts by the strict definition —
/// a default arm introduces no new predicate. It is counted anyway because
/// the Java pass counts `default:` (its grammar folds it into the same
/// `switch_label` node), and a Go `switch` and a Java `switch` scoring
/// differently for the same shape would make the two languages'
/// hotspot lists incomparable, which is the thing these metrics are for.
fn increments_cyclomatic(kind: &str) -> bool {
    matches!(
        kind,
        "if_statement"
            | "for_statement"
            | "expression_case"
            | "type_case"
            | "communication_case"
            | "default_case"
    )
}

/// Structures that make what is inside them harder to follow, and so score
/// by how deep they already are.
fn opens_cognitive_nesting(kind: &str) -> bool {
    matches!(
        kind,
        "if_statement"
            | "for_statement"
            | "expression_switch_statement"
            | "type_switch_statement"
            | "select_statement"
            | "func_literal"
    )
}

/// Structures whose children are one level deeper. A switch arm nests
/// inside its switch, so a call in a case body sits at depth two — the
/// same as the Java pass, where a `switch_label` nests inside a
/// `switch_expression`.
fn opens_scope(kind: &str) -> bool {
    matches!(
        kind,
        "if_statement"
            | "for_statement"
            | "expression_switch_statement"
            | "type_switch_statement"
            | "select_statement"
            | "expression_case"
            | "type_case"
            | "communication_case"
            | "default_case"
            | "func_literal"
    )
}
