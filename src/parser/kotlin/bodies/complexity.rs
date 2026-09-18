//! Function-body metrics: cyclomatic + cognitive complexity, max nesting
//! (KT-002).
//!
//! Fourth translation of the same convention. [`crate::parser::rust::bodies::complexity`]
//! is the reference — nesting structures score `+1 + depth` cognitive, flat
//! increments score `+1`, every branch scores `+1` cyclomatic — and
//! [`crate::parser::java::complexity`] is the closest analogue, because the
//! two languages compile to the same platform and are the pair users most
//! often put side by side.
//!
//! Grammar mapping against `tree-sitter-kotlin 0.3`, and the decisions
//! behind it:
//!
//! - **`if` is an expression here**, so the node is `if_expression` rather
//!   than `if_statement`. `val x = if (a) b else c` branches exactly like a
//!   statement `if` and is counted the same way. There is no separate
//!   ternary node to count — this *is* Kotlin's ternary.
//! - **`else` is an anonymous token** inside the `if_expression`, and a
//!   chained `else if` arrives as `control_structure_body → if_expression`.
//!   The else penalty (`+1` to both scores) is therefore charged only when
//!   the alternative does not wrap another `if_expression`; the chained case
//!   is scored by the nested `if_expression` we visit anyway.
//! - **`when` is the `switch`/`match` analogue.** `when_entry` adds one to
//!   cyclomatic and nothing to cognitive, matching Java's `switch_label` and
//!   Rust's `match_arm`; the enclosing `when_expression` is the nesting
//!   structure. The `else ->` entry counts like Java's `default:` — not
//!   strictly a decision, but comparability across languages is the point of
//!   this file.
//! - **`&&` and `||` are their own node kinds** (`conjunction_expression` /
//!   `disjunction_expression`) rather than operators inside a shared binary
//!   node, so no child scan is needed: one node is one short-circuit, `+1`
//!   to both scores.
//! - **Elvis (`?:`) is a branch** — `a ?: b` takes one of two paths — and it
//!   is a nesting structure, filling the slot Java's `ternary_expression`
//!   occupies.
//! - **Lambdas and the scope functions** (`let` / `run` / `apply` / `also` /
//!   `with`) open a cognitive nesting scope and add nothing to cyclomatic —
//!   the rule Java applies to `lambda_expression`. Nothing special is needed
//!   for the scope functions themselves: their block argument is a
//!   `lambda_literal`, which is what is counted. The same names appear in
//!   [`super::stdlib`], but that list is about call noise and is unrelated.
//! - **Safe-call (`?.`) is not counted.** It is arguably a branch, but it is
//!   pervasive in idiomatic Kotlin in positions where Java and Rust write
//!   nothing at all, and counting it would inflate every Kotlin score
//!   relative to the languages these numbers are meant to be compared
//!   against. This is the decision TS-003 took for `?.` in TypeScript, which
//!   named Kotlin explicitly; staying consistent with it is deliberate.
//!
//! Verified against the audit's gnarly fixture: the Kotlin port scores
//! exactly Java's 12 / 31 / 6 (see `tests.rs`).

use crate::models::CodeEntity;
use tree_sitter::Node;
use crate::parser::working_set;

/// Populate the per-callable metrics every entity kind shares: LOC,
/// parameter count, and the three body-complexity numbers.
///
/// A bodyless declaration — an `interface` member, an `abstract fun`, an
/// `expect` declaration — gets `Some(1)` / `Some(0)` / `Some(0)` rather than
/// `None`. It has one straight-through path by virtue of existing as a
/// signature, and leaving the fields unset would make `mezz quality` skip the
/// entity entirely: the hotspot ranking filters on `composite_score > 0.0`,
/// which is the whole reason KT-002 exists.
pub(in crate::parser::kotlin) fn populate_body_metrics(
    body: Option<Node>,
    source: &str,
    entity: &mut CodeEntity,
) {
    entity.metrics.loc = (entity.span.end.line - entity.span.start.line + 1) as u32;
    entity.metrics.param_count = Some(entity.parameters.len() as u32);
    match body {
        Some(body) => {
            let (cyclomatic, nesting, cognitive) = compute_complexity(&body);
            entity.metrics.cyclomatic = Some(cyclomatic);
            entity.metrics.max_nesting = Some(nesting);
            entity.metrics.cognitive_complexity = Some(cognitive);
        }
        None => {
            entity.metrics.cyclomatic = Some(1);
            entity.metrics.max_nesting = Some(0);
            entity.metrics.cognitive_complexity = Some(0);
        }
    }
    working_set::populate(entity, body.as_ref(), source);
    crate::parser::loops::populate(entity, body.as_ref());
}

/// Node kinds that add one to cyclomatic complexity on sight.
const CYCLOMATIC_KINDS: &[&str] = &[
    "if_expression",
    "while_statement",
    "do_while_statement",
    "for_statement",
    "when_entry",
    "catch_block",
    "elvis_expression",
    "conjunction_expression",
    "disjunction_expression",
];

/// Node kinds that add `1 + depth` to cognitive complexity and deepen the
/// nesting level for everything inside them.
const NESTING_KINDS: &[&str] = &[
    "if_expression",
    "while_statement",
    "do_while_statement",
    "for_statement",
    "when_expression",
    "catch_block",
    "elvis_expression",
    "lambda_literal",
    "anonymous_function",
];

/// Node kinds that add a flat `+1` to cognitive complexity without deepening
/// the nesting level — the short-circuit operators, which in this grammar are
/// node kinds rather than operators inside a shared binary node.
const FLAT_COGNITIVE_KINDS: &[&str] = &["conjunction_expression", "disjunction_expression"];

/// Node kinds that deepen the nesting level without contributing a cognitive
/// increment of their own. A `when_entry` is an arm of a `when_expression`
/// that already scored; the arm only carries depth.
const EXTRA_SCOPE_KINDS: &[&str] = &["when_entry"];

/// Walk a function body subtree counting branching constructs and tracking
/// nesting depth. Returns `(cyclomatic, max_nesting, cognitive)`.
///
/// Iterative, like its Rust, Java and TypeScript siblings, so a 2000-line
/// generated file can't blow the stack.
pub(super) fn compute_complexity(body: &Node) -> (u32, u32, u32) {
    let mut complexity: u32 = 1;
    let mut cognitive: u32 = 0;
    let mut max_depth: u32 = 0;
    let mut stack: Vec<(Node, u32)> = vec![(*body, 0)];

    while let Some((node, depth)) = stack.pop() {
        let kind = node.kind();

        if CYCLOMATIC_KINDS.contains(&kind) {
            complexity = complexity.saturating_add(1);
        }
        if NESTING_KINDS.contains(&kind) {
            cognitive = cognitive.saturating_add(1 + depth);
        }
        if FLAT_COGNITIVE_KINDS.contains(&kind) {
            cognitive = cognitive.saturating_add(1);
        }
        // A terminal `else { … }` is one more path with no nesting bonus.
        // A chained `else if` is skipped — its `if_expression` scores itself.
        if kind == "if_expression" && has_terminal_else(&node) {
            complexity = complexity.saturating_add(1);
            cognitive = cognitive.saturating_add(1);
        }

        let opens_scope = NESTING_KINDS.contains(&kind) || EXTRA_SCOPE_KINDS.contains(&kind);
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

/// Does this `if_expression` end in a real `else`, rather than in a chained
/// `else if` or in nothing at all?
///
/// The grammar gives no field names, so the alternative is found positionally:
/// the `control_structure_body` that follows the anonymous `else` token. A
/// chained `else if` wraps an `if_expression` in that body and is left to
/// score itself.
fn has_terminal_else(node: &Node) -> bool {
    let mut cursor = node.walk();
    let mut seen_else = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "else" {
            seen_else = true;
        } else if seen_else && child.is_named() {
            return !wraps_if(&child);
        }
    }
    false
}

/// Does this alternative body hold a chained `else if` rather than a real
/// else body? The grammar wraps the chained case as
/// `control_structure_body → if_expression`.
fn wraps_if(alternative: &Node) -> bool {
    if alternative.kind() == "if_expression" {
        return true;
    }
    let mut cursor = alternative.walk();
    let chained = alternative
        .children(&mut cursor)
        .any(|c| c.kind() == "if_expression");
    chained
}
