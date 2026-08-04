//! Function-body metrics: cyclomatic + cognitive complexity, max nesting,
//! and return-tuple cardinality.
//!
//! Mirrors [`crate::parser::rust::complexity`] and
//! [`crate::parser::java::complexity`] so per-entity scoring lines up across
//! languages. The convention those two established, and this one keeps:
//! nesting structures count `+1 + depth` cognitive, flat increments count
//! `+1`, and every branch adds `+1` cyclomatic. The cross-language `gnarly`
//! fixture scores 12 / 31 / 6 in Java and, ported to Python, scores the same
//! here — that equality is asserted by
//! `tests::gnarly_fixture_scores_the_same_as_java` and is the point of this
//! file.
//!
//! The tree-sitter-python node names diverge from both siblings, so the
//! mapping is spelled out in the three classifier functions below rather
//! than inline in the walker. Two Python-specific shapes drive most of it:
//!
//! - `elif` and `else` are their own nodes (`elif_clause` / `else_clause`)
//!   hanging off the parent `if_statement`'s `alternative` field, not a
//!   nested if the way Java models a chained `else if`. An `elif_clause`
//!   therefore takes the *nesting* increment Java's chained if takes, and
//!   `else_clause` takes the flat one.
//! - Comprehensions are expressions, so the loop lives in a `for_in_clause`
//!   and the filter in an `if_clause`. Those carry the branch counts; the
//!   comprehension node itself only opens the nesting scope.
//!
//! Nested `def`s and `lambda`s are *not* skipped: they contribute to the
//! enclosing callable exactly as a Java lambda or a Rust nested `fn` does.
//! They also get their own entity and their own metrics, so a closure's
//! complexity is visible both on its own node and in its container's score.

use tree_sitter::Node;

/// Walk a function body subtree counting branching constructs and tracking
/// nesting depth. Returns `(cyclomatic, max_nesting, cognitive)`.
///
/// Kept iterative, and kept thin: every node-kind decision lives in one of
/// the three classifiers so this stays a loop rather than a match tree.
pub(super) fn compute_complexity(body: &Node) -> (u32, u32, u32) {
    let mut cyclomatic: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    // (node, depth-at-this-node)
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        // Anonymous tokens are leaves, and one of them is a trap: the
        // `lambda` *keyword* has node kind `"lambda"`, exactly like the
        // expression that contains it, so counting unnamed nodes scores
        // every lambda twice. Skipping them costs nothing — a token has no
        // children — and closes the whole class of collision.
        if !node.is_named() {
            continue;
        }
        let kind = node.kind();

        cyclomatic = cyclomatic.saturating_add(cyclomatic_increment(kind));
        cognitive = cognitive.saturating_add(cognitive_increment(kind, depth));

        let child_depth = depth + u32::from(opens_scope(kind));
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

/// Cyclomatic: `+1` for every construct that adds an independent path.
///
/// `async for` / `async with` are the same node kinds as their sync forms
/// (the `async` keyword is an anonymous token inside), so they score
/// identically without a separate arm.
///
/// Deliberately absent: `with_statement` and `finally_clause` (no decision
/// is taken), `match_statement` (the `case_clause`s carry the count, as
/// `switch_label` does in Java), the comprehension nodes (their
/// `for_in_clause` / `if_clause` children carry it), `lambda` (it doesn't
/// branch the enclosing flow until invoked), and `assert_statement`.
fn cyclomatic_increment(kind: &str) -> u32 {
    let branches = matches!(
        kind,
        "if_statement"
            | "elif_clause"
            | "else_clause"
            | "for_statement"
            | "while_statement"
            | "except_clause"
            | "except_group_clause"
            | "case_clause"
            | "conditional_expression"
            | "boolean_operator"
            | "for_in_clause"
            | "if_clause"
    );
    u32::from(branches)
}

/// Cognitive: nesting structures score `+1 + depth`, flat ones `+1`.
///
/// `else_clause` is flat — Java's equivalent is the "else penalty" it adds
/// on the parent `if_statement`, which carries no depth bonus either.
/// `boolean_operator` is one node per `and`/`or`, mirroring Java's per-token
/// `&&`/`||` increment. `case_clause` scores nothing, matching Java's
/// `switch_label` and Rust's `match_arm`.
fn cognitive_increment(kind: &str, depth: u32) -> u32 {
    let nests = matches!(
        kind,
        "if_statement"
            | "elif_clause"
            | "for_statement"
            | "while_statement"
            | "except_clause"
            | "except_group_clause"
            | "match_statement"
            | "conditional_expression"
            | "lambda"
            | "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression"
    );
    if nests {
        return 1 + depth;
    }
    let flat = matches!(kind, "else_clause" | "boolean_operator" | "if_clause");
    u32::from(flat)
}

/// Does this node deepen the nesting level of its children?
///
/// Shared by `max_nesting` and by the cognitive depth bonus. `try_statement`
/// and `with_statement` are excluded on purpose: Java's `try` doesn't open a
/// scope either, and the two metrics only stay comparable if the exclusion
/// matches. The `except_clause` is what nests, not the `try`.
fn opens_scope(kind: &str) -> bool {
    matches!(
        kind,
        "if_statement"
            | "elif_clause"
            | "else_clause"
            | "for_statement"
            | "while_statement"
            | "except_clause"
            | "except_group_clause"
            | "match_statement"
            | "case_clause"
            | "conditional_expression"
            | "lambda"
            | "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression"
    )
}

/// Count the elements of a tuple return annotation, so `-> tuple[int, str]`
/// scores 2. Mirrors the Rust parser's `count_return_tuple_elements`, which
/// reads a `tuple_type` node.
///
/// In annotation position tree-sitter-python wraps the whole thing in a
/// `type` node and models the subscript as `generic_type` + `type_parameter`
/// — *not* as the `subscript` node the same syntax produces in an
/// expression. Both the PEP 585 `tuple[…]` and the `typing.Tuple[…]`
/// spelling land on that shape.
///
/// Returns `None` for anything that isn't a tuple, and for a one-element
/// tuple — a single value is not "return complexity".
pub(super) fn count_return_tuple_elements(return_type_node: &Node, source: &str) -> Option<u32> {
    let node = if return_type_node.kind() == "type" {
        return_type_node.named_child(0)?
    } else {
        *return_type_node
    };
    if node.kind() != "generic_type" {
        return None;
    }
    let base = node.named_child(0)?;
    let base_text = &source[base.start_byte()..base.end_byte()];
    // `typing.Tuple` / `t.Tuple` arrive dotted; compare the final segment.
    if !matches!(base_text.rsplit('.').next(), Some("tuple") | Some("Tuple")) {
        return None;
    }
    let params = node
        .named_children(&mut node.walk())
        .find(|c| c.kind() == "type_parameter")?;
    let mut count = 0u32;
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        // `tuple[int, ...]` is a homogeneous variable-length tuple, not a
        // 2-element one — the ellipsis isn't a returned value.
        if source[child.start_byte()..child.end_byte()].trim() == "..." {
            continue;
        }
        count += 1;
    }
    if count > 1 {
        Some(count)
    } else {
        None
    }
}
