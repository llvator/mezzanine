//! Function/method-body metrics: cyclomatic + cognitive complexity, max
//! nesting (TS-003).
//!
//! Third translation of the same convention. [`crate::parser::rust::bodies::complexity`]
//! is the reference — nesting structures score `+1 + depth` cognitive, flat
//! increments score `+1`, every branch scores `+1` cyclomatic — and
//! [`crate::parser::java::complexity`] is the closest analogue, because
//! tree-sitter-typescript shares Java's statement-shaped grammar rather than
//! Rust's expression-shaped one.
//!
//! Grammar mapping, and the decisions behind it:
//!
//! - `else_clause` **is** a node in this grammar (unlike Java, where the else
//!   lives in the parent `if_statement`'s `alternative` field), so it can be
//!   counted directly. It increments both scores by one, but only when it
//!   does not wrap another `if_statement`: a chained `else if` is counted by
//!   the nested `if_statement` we visit anyway, and double-counting it would
//!   put TypeScript two points above Java on the same code.
//! - `else_clause` deliberately does **not** open a nesting scope. Java's
//!   chained `else if` is a direct child of the outer `if_statement`, so
//!   letting the intervening `else_clause` deepen the nesting would score the
//!   same source one level deeper in TypeScript than in Java.
//! - `switch_case` and `switch_default` each add one to cyclomatic and
//!   nothing to cognitive, matching Java's `switch_label` and Rust's
//!   `match_arm`. `default:` is not strictly a decision, but Java counts it
//!   and comparability across languages is the point of this file.
//! - `arrow_function` and `function` (a function *expression*) open a
//!   cognitive nesting scope and add nothing to cyclomatic — the same rule
//!   Java applies to `lambda_expression`. A callback does not branch its
//!   enclosing function's control flow until it is invoked.
//! - `&&`, `||` and `??` each add one to both scores. `??` is a
//!   short-circuit in exactly the way the other two are.
//! - **Optional chaining (`?.`) is not counted.** It is arguably a branch,
//!   but it is pervasive in idiomatic TypeScript in positions where Java and
//!   Rust write nothing at all, and counting it would inflate every
//!   TypeScript score relative to the languages these numbers are meant to be
//!   compared against. The same argument applies to Kotlin's `?.` (KT-002).
//!
//! Verified against the audit's gnarly fixture: the TypeScript port scores
//! exactly Java's 12 / 31 / 6 (see `tests.rs`).

use crate::models::CodeEntity;
use tree_sitter::Node;

/// Populate the per-callable metrics every entity kind shares: LOC, parameter
/// count, and the three body-complexity numbers.
///
/// A bodyless declaration — an interface method signature, an `abstract`
/// member, an overload signature, a `declare` ambient — gets `Some(1)` /
/// `Some(0)` / `Some(0)` rather than `None`. It has one straight-through path
/// by virtue of existing as a signature, and leaving the fields unset would
/// make `mezz quality` skip the entity entirely (the hotspot ranking filters
/// on `composite_score > 0.0`, which is the whole reason TS-003 exists).
pub(in crate::parser::typescript) fn populate_body_metrics(
    body: Option<Node>,
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
}

/// Node kinds that add one to cyclomatic complexity on sight.
const CYCLOMATIC_KINDS: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "for_in_statement",
    "switch_case",
    "switch_default",
    "catch_clause",
    "ternary_expression",
];

/// Node kinds that add `1 + depth` to cognitive complexity and deepen the
/// nesting level for everything inside them.
const NESTING_KINDS: &[&str] = &[
    "if_statement",
    "while_statement",
    "do_statement",
    "for_statement",
    "for_in_statement",
    "switch_statement",
    "ternary_expression",
    "catch_clause",
    "arrow_function",
    "function",
    "function_expression",
];

/// Node kinds that deepen the nesting level without contributing a cognitive
/// increment of their own. `switch_case` / `switch_default` are the arms of a
/// `switch_statement` that already scored; the arms only carry depth.
const EXTRA_SCOPE_KINDS: &[&str] = &["switch_case", "switch_default"];

/// Short-circuit operators. Each occurrence adds one to both scores.
const SHORT_CIRCUIT_OPERATORS: &[&str] = &["&&", "||", "??"];

/// Walk a function body subtree counting branching constructs and tracking
/// nesting depth. Returns `(cyclomatic, max_nesting, cognitive)`.
///
/// Iterative, like its Rust and Java siblings, so a 2000-line generated
/// module can't blow the stack.
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
        // A terminal `else { … }` is one more path with no nesting bonus.
        // A chained `else if` is skipped — its `if_statement` scores itself.
        if kind == "else_clause" && !wraps_if(&node) {
            complexity = complexity.saturating_add(1);
            cognitive = cognitive.saturating_add(1);
        }
        let short_circuits = count_short_circuits(&node);
        complexity = complexity.saturating_add(short_circuits);
        cognitive = cognitive.saturating_add(short_circuits);

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

/// Does this `else_clause` hold a chained `else if` rather than a real else
/// body? The grammar wraps the chained case as `else_clause → if_statement`.
fn wraps_if(else_clause: &Node) -> bool {
    let mut cursor = else_clause.walk();
    let found = else_clause
        .children(&mut cursor)
        .any(|c| c.kind() == "if_statement");
    found
}

/// Number of short-circuit operators directly under `node`. Only
/// `binary_expression` carries them; every other kind scores zero without a
/// walk.
fn count_short_circuits(node: &Node) -> u32 {
    if node.kind() != "binary_expression" {
        return 0;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| SHORT_CIRCUIT_OPERATORS.contains(&c.kind()))
        .count() as u32
}
