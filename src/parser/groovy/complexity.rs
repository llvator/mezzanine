//! Method-body metrics for Groovy: cyclomatic + cognitive complexity and
//! max nesting (GR-014).
//!
//! Ports [`crate::parser::java::complexity`] — the scoring convention has
//! to hold across languages for scores to be comparable, so the shape is
//! deliberately identical: nesting structures count `+1 + depth` for
//! cognitive complexity, flat increments count `+1`, and every branch adds
//! `+1` to cyclomatic. Fed the same logic, Groovy and Java land on the
//! same triple.
//!
//! Four Groovy-specific decisions, all forced by how `tree-sitter-groovy`
//! models the language:
//!
//! * **A braced block is a `closure` node.** The grammar has no
//!   `block`-vs-closure distinction in statement position: the body of
//!   `if (a) { … }` parses as `closure`, exactly like the trailing
//!   `list.each { … }`. Counting every `closure` as a nesting scope would
//!   double-count every `if` arm in the language; counting none would
//!   flatten closure-heavy Gradle DSL to nothing. We split on *context*
//!   instead — see [`CLOSURE_VALUE_PARENTS`]. A closure in expression
//!   position is a real closure and opens a nesting scope (adding nothing
//!   to cyclomatic, matching Java's `lambda_expression`); a closure in
//!   statement position is a block and opens nothing on its own.
//! * **Elvis `?:` is a branch**, and needs no special handling: the
//!   grammar parses `a ?: b` as a `ternary_expression` with a missing
//!   consequence (see [`super::calls`]'s Elvis note), so it is already
//!   counted as one.
//! * **Safe navigation `?.` counts for nothing.** It guards one
//!   dereference rather than opening a second path through the method —
//!   the same call sequence runs or does not run. This matches SonarSource's
//!   treatment and is the convention `TS-003` / `KT-002` should follow for
//!   the same operator. (The grammar exposes `?.` as an `ERROR` sibling
//!   anyway, so there is nothing to count even if we wanted to.)
//! * **`switch` follows Java**: each `switch_label` is `+1` cyclomatic and
//!   nothing cognitive, whether the case matches on a value, a type, a
//!   regex or a range.
//!
//! Implicit returns don't branch, and neither does `def` — dynamic typing
//! changes nothing about control flow.

use super::helpers::flow_keyword_invocation;
use tree_sitter::Node;

/// Nested definitions own their own metrics, so the walk stops at them
/// rather than folding a class's methods into the script that declares
/// it. Mirrors the call-walk's attribution rule in [`super::calls`].
const NESTED_DEFINITIONS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "method_declaration",
    "function_definition",
    "constructor_declaration",
];

/// Node kinds that add one path through the method.
const BRANCHING_KINDS: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "enhanced_for_statement",
    "switch_label",
    "catch_clause",
    "ternary_expression",
];

/// Node kinds that open a cognitive nesting scope: `+1 + depth` when
/// entered, and `+1` depth for everything inside. Closures are handled
/// separately because their kind alone doesn't say whether they are one.
const NESTING_KINDS: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "enhanced_for_statement",
    "switch_expression",
    "switch_statement",
    "ternary_expression",
    "catch_clause",
];

/// Parent kinds in which a `closure` node is a real closure literal
/// rather than a braced statement block. Everything else — the arms of
/// an `if`, a loop body, a `try` block, a method body — is a block that
/// the grammar happens to spell `closure`.
const CLOSURE_VALUE_PARENTS: &[&str] = &[
    "method_invocation",
    "juxt_function_call",
    "argument_list",
    "variable_declarator",
    "assignment_expression",
    "map_item",
    "array_literal",
];

/// Walk a callable body — or a script's top-level statements — counting
/// branching constructs and tracking nesting depth. Returns
/// `(cyclomatic, max_nesting, cognitive)`.
pub(super) fn compute_complexity(body: &Node, source: &str) -> (u32, u32, u32) {
    let mut complexity: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        let (cc, cog) = flat_increments(&node, source);
        complexity = complexity.saturating_add(cc);
        cognitive = cognitive.saturating_add(cog);

        let nests = opens_nesting(&node, source);
        if nests {
            cognitive = cognitive.saturating_add(1 + depth);
        }

        let child_depth = if nests || node.kind() == "switch_label" {
            depth + 1
        } else {
            depth
        };
        if child_depth > max_depth {
            max_depth = child_depth;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !NESTED_DEFINITIONS.contains(&child.kind()) {
                stack.push((child, child_depth));
            }
        }
    }

    (complexity, max_depth, cognitive)
}

/// Increments that don't depend on nesting depth, as
/// `(cyclomatic, cognitive)`:
///
/// * every branching construct is `+1` cyclomatic,
/// * a terminal `else` is a second path — `+1` to both. A chained
///   `else if` is skipped here because the nested `if_statement` is
///   counted on its own visit,
/// * each short-circuit `&&` / `||` is `+1` to both,
/// * a control-flow statement the grammar degraded into a
///   `method_invocation` branches exactly like the real one it stands
///   for, so it is counted as such.
fn flat_increments(node: &Node, source: &str) -> (u32, u32) {
    let kind = node.kind();
    let mut cyclomatic = u32::from(
        BRANCHING_KINDS.contains(&kind) || flow_keyword_invocation(node, source).is_some(),
    );
    let mut cognitive = 0;

    if kind == "if_statement" {
        if let Some(alt) = node.child_by_field_name("alternative") {
            if alt.kind() != "if_statement" {
                cyclomatic += 1;
                cognitive += 1;
            }
        }
    }

    if kind == "binary_expression" {
        let operators = short_circuit_operators(node);
        cyclomatic += operators;
        cognitive += operators;
    }

    (cyclomatic, cognitive)
}

/// Count the `&&` / `||` tokens directly under a `binary_expression`.
/// The grammar left-nests chains (`a && b || c` is a `binary_expression`
/// over a `binary_expression`), so each visit sees exactly one operator
/// and the walk sums them.
fn short_circuit_operators(node: &Node) -> u32 {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| matches!(c.kind(), "&&" | "||"))
        .count() as u32
}

/// True when the node opens a cognitive nesting scope. Real closure
/// literals do; braced blocks spelled `closure` do not. A
/// `lambda_expression` is the parameter list of a closure the grammar
/// already reported (`{ x -> … }` is `closure(lambda_expression)`), so it
/// only counts when it stands on its own.
fn opens_nesting(node: &Node, source: &str) -> bool {
    match node.kind() {
        "closure" => is_closure_literal(node, source),
        "lambda_expression" => node.parent().is_none_or(|p| p.kind() != "closure"),
        "method_invocation" => flow_keyword_invocation(node, source).is_some(),
        kind => NESTING_KINDS.contains(&kind),
    }
}

/// A `closure` node is a closure literal — rather than a braced statement
/// block — when it sits in expression position. The `body` of a degraded
/// `if (…) { … }` is the exception: its parent is a `method_invocation`,
/// but the braces are a block and the invocation already counted.
fn is_closure_literal(node: &Node, source: &str) -> bool {
    let Some(parent) = node.parent() else { return false };
    CLOSURE_VALUE_PARENTS.contains(&parent.kind())
        && flow_keyword_invocation(&parent, source).is_none()
}
