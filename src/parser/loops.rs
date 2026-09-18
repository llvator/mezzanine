//! How deep the loops go — the one structural fact behind "does this fall
//! over at 10k rows" (MCP-046).
//!
//! Every other per-callable metric mezz computes measures how hard a body is
//! to *read*: `cyclomatic` counts decisions, `cognitive` weights them by
//! nesting, `working_set` counts names in view. None of them measures how the
//! body *scales*, and the nearest-looking one is actively misleading —
//! `max_nesting` counts every nesting-opening construct, so a flat function
//! with three nested `if`s scores 3 and reads, to an unwary caller, as O(n³).
//!
//! This counts loops and only loops, and counts them *through* whatever sits
//! between them: a `for` inside an `if` inside a `for` is 2, because that is
//! what the inner body costs per outer iteration.
//!
//! ## Why one table rather than nine walks
//!
//! Each language's `complexity.rs` already enumerates its loop node kinds —
//! it has to, to count them as branches. Extending nine separate walks to
//! carry a fourth number would spread one definition across nine files and
//! let them drift. The kinds are the only thing that varies, so the kinds are
//! the table and the walk is shared, in the shape
//! [`crate::parser::effects`] uses for the same reason.
//!
//! ## What it deliberately does not count
//!
//! * **Iteration written as a call.** `xs.iter().map(…)`, `forEach`,
//!   `stream()`, a recursive descent — none of them is a loop node, and none
//!   is counted here. The metric under-reports on idiomatic functional code,
//!   which is the safe direction for a number that drives refactoring, and
//!   [`crate::parser::costs`] is what recovers the library half of it.
//! * **Another entity's loops.** A nested named function, class or `impl` is
//!   its own entity with its own count; folding its loops into the parent
//!   would charge one caller for a body it may never run. Closures and
//!   lambdas *are* descended into — they run where you are reading.
//! * **How many times a loop actually runs.** `for i in 0..3` counts as a
//!   loop. This is a structural worst case, not a bound on iterations.

use crate::models::file_info::Language;
use crate::models::CodeEntity;
use tree_sitter::Node;

use super::working_set::NESTED_ENTITY_KINDS;

/// Python's comprehension forms. Each carries its `for` clauses as flat
/// siblings rather than nesting them, so the comprehension node is where
/// their count has to be read — see [`depth_added`].
const PY_COMPREHENSIONS: &[&str] = &[
    "list_comprehension",
    "set_comprehension",
    "dictionary_comprehension",
    "generator_expression",
];

/// The loop node kinds of one grammar, or `&[]` for a language mezz cannot
/// tell a loop from a branch in.
///
/// Harvested from each language's own `complexity.rs`, which already lists
/// them to count branches — so the two agree on what a loop is by
/// construction rather than by review.
fn loop_kinds(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &[
            "for_expression",
            "while_expression",
            "while_let_expression",
            "loop_expression",
        ],
        // Go spells all three loops `for`.
        Language::Go => &["for_statement"],
        // Comprehensions are loops too, but they are weighed at the
        // comprehension node; see [`depth_added`].
        Language::Python => &["for_statement", "while_statement"],
        Language::TypeScript | Language::JavaScript | Language::Svelte => &[
            "for_statement",
            "for_in_statement",
            "while_statement",
            "do_statement",
        ],
        Language::Java | Language::Groovy => &[
            "for_statement",
            "enhanced_for_statement",
            "while_statement",
            "do_statement",
        ],
        Language::Kotlin => &["for_statement", "while_statement", "do_while_statement"],
        Language::Dart => &["for_statement", "while_statement", "do_statement"],
        Language::Cpp | Language::C => &[
            "for_statement",
            "for_range_loop",
            "while_statement",
            "do_statement",
        ],
        _ => &[],
    }
}

/// Whether mezz can tell a loop from a branch in this language.
///
/// The question a report has to ask before it turns a depth into an
/// exponent: `Some(0)` and "not measured" are opposite facts, and a zero
/// looks like the first.
pub(crate) fn has_table(language: Language) -> bool {
    !loop_kinds(language).is_empty()
}

/// The languages that have one, in reading order — so a silent language can
/// name what *is* covered rather than leave the reader to guess.
pub(crate) const COVERED: &str =
    "Rust, Python, TypeScript/JavaScript (and Svelte), Go, Java, Kotlin, Groovy, Dart, and C/C++";

/// Set `loop_nesting` for one callable, from its body.
///
/// Mirrors [`crate::parser::working_set::populate`] and is called beside it,
/// so every parser that measures a body measures this too. A body-less
/// declaration — an interface method, an overload signature — is `Some(0)`:
/// it demonstrably has no loops, which is a measurement and not a gap.
pub fn populate(entity: &mut CodeEntity, body: Option<&Node>) {
    let language = Language::from_path(&entity.file_path);
    if !has_table(language) {
        return;
    }
    entity.metrics.loop_nesting = Some(body.map_or(0, |body| deepest(body, language)));
}

/// The deepest chain of loops nested inside one another anywhere in `body`.
///
/// Iterative, like every other body walk here, to stay cheap on large
/// functions. The root is never tested — it is the body, not a construct
/// inside it.
fn deepest(body: &Node, language: Language) -> u32 {
    let mut max = 0;
    let mut stack: Vec<(Node, u32)> = Vec::new();
    push_children(&mut stack, body, 0);

    while let Some((node, depth)) = stack.pop() {
        if NESTED_ENTITY_KINDS.contains(&node.kind()) {
            continue;
        }
        let child_depth = depth + depth_added(&node, language);
        max = max.max(child_depth);
        push_children(&mut stack, &node, child_depth);
    }
    max
}

fn push_children<'a>(stack: &mut Vec<(Node<'a>, u32)>, node: &Node<'a>, depth: u32) {
    let mut cursor = node.walk();
    stack.extend(node.children(&mut cursor).map(|child| (child, depth)));
}

/// How much deeper everything under `node` sits.
///
/// One for a loop, zero for anything else — except a Python comprehension,
/// which is the one construct in these grammars that nests loops without
/// nesting nodes. `[y for row in rows for y in row]` parses as a
/// comprehension with two sibling `for_in_clause` children, and reading
/// those as siblings would score a genuine O(n²) as 1.
fn depth_added(node: &Node, language: Language) -> u32 {
    if language == Language::Python && PY_COMPREHENSIONS.contains(&node.kind()) {
        let mut cursor = node.walk();
        return node
            .children(&mut cursor)
            .filter(|c| c.kind() == "for_in_clause")
            .count() as u32;
    }
    u32::from(loop_kinds(language).contains(&node.kind()))
}

#[cfg(test)]
mod tests {
    use crate::models::{CodeEntity, EntityKind};
    use crate::parser::language_parser::LanguageParser;
    use std::path::Path;

    /// `loop_nesting` for one callable, taken through the real parser rather
    /// than a hand-built tree — the node kinds in [`super::loop_kinds`] are a
    /// claim about nine grammars, and only a real parse can check it.
    fn depth(parser: &dyn LanguageParser, file: &str, src: &str, name: &str) -> Option<u32> {
        let result = parser.parse(Path::new(file), src).expect("parse");
        let e: &CodeEntity = result
            .entities
            .iter()
            .find(|e| {
                e.name == name && matches!(e.kind, EntityKind::Function | EntityKind::Method)
            })
            .unwrap_or_else(|| panic!("callable `{name}` not found in {file}"));
        e.metrics.loop_nesting
    }

    /// The defect the metric exists for: branches are not loops. Three
    /// nested `if`s are `max_nesting 3` and `loop_nesting 0`, and reporting
    /// the first as a scaling claim is what MCP-046 filed.
    #[test]
    fn nested_branches_are_not_loop_nesting() {
        let src = r#"
fn classify(a: bool, b: bool, c: bool) -> u32 {
    if a {
        if b {
            if c {
                return 3;
            }
        }
    }
    0
}
"#;
        let parser = crate::parser::RustParser::new();
        assert_eq!(depth(&parser, "a.rs", src, "classify"), Some(0));
    }

    /// …and loops nest *through* branches. The `if` between the two `for`s
    /// does not reset the chain: the inner body still runs once per outer
    /// iteration.
    #[test]
    fn a_loop_inside_a_branch_inside_a_loop_is_depth_two() {
        let src = r#"
fn scan(rows: &[Row]) {
    for row in rows {
        if row.live {
            for cell in &row.cells {
                touch(cell);
            }
        }
    }
}
"#;
        let parser = crate::parser::RustParser::new();
        assert_eq!(depth(&parser, "a.rs", src, "scan"), Some(2));
    }

    /// A nested named function is its own entity with its own count.
    /// Charging the parent for a body it may never run would turn every
    /// file of helpers into a false O(n²).
    #[test]
    fn a_nested_named_function_keeps_its_own_loops() {
        let rust = r#"
fn outer(xs: &[u32]) {
    fn inner(ys: &[u32]) {
        for y in ys {
            for z in ys {
                touch(y, z);
            }
        }
    }
    for x in xs {
        inner(&[*x]);
    }
}
"#;
        assert_eq!(
            depth(&crate::parser::RustParser::new(), "a.rs", rust, "outer"),
            Some(1),
            "the inner fn's two loops were charged to its parent"
        );

        // The same line in TypeScript, where the nested declaration is a
        // `function_declaration` rather than a `function_item` — one list,
        // nine grammars, so both have to hold.
        let ts = "function outer(xs: number[]) {\n  \
                  function inner(ys: number[]) {\n    \
                  for (const y of ys) { for (const z of ys) { touch(y, z); } }\n  }\n  \
                  for (const x of xs) { inner([x]); }\n}\n";
        assert_eq!(
            depth(&crate::parser::TypeScriptParser::new(), "a.ts", ts, "outer"),
            Some(1)
        );
    }

    /// Every grammar in the table, through its own loop spelling. This is
    /// the test that catches a renamed node kind.
    #[test]
    fn each_grammar_reads_its_own_nested_loops() {
        let cases: Vec<(Box<dyn LanguageParser>, &str, &str, &str)> = vec![
            (
                Box::new(crate::parser::PythonParser::new()),
                "a.py",
                "def scan(rows):\n    for row in rows:\n        while row:\n            row = row.next\n",
                "scan",
            ),
            (
                Box::new(crate::parser::TypeScriptParser::new()),
                "a.ts",
                "function scan(rows: Row[]) {\n  for (const r of rows) {\n    for (let i = 0; i < r.n; i++) { touch(i); }\n  }\n}\n",
                "scan",
            ),
            (
                Box::new(crate::parser::GoParser::new()),
                "a.go",
                "func scan(rows []Row) {\n\tfor _, r := range rows {\n\t\tfor i := 0; i < r.N; i++ {\n\t\t\ttouch(i)\n\t\t}\n\t}\n}\n",
                "scan",
            ),
            (
                Box::new(crate::parser::JavaParser::new()),
                "A.java",
                "class A { void scan(Row[] rows) { for (Row r : rows) { while (r.next != null) { touch(r); } } } }",
                "scan",
            ),
            (
                Box::new(crate::parser::KotlinParser::new()),
                "A.kt",
                "fun scan(rows: List<Row>) {\n    for (r in rows) {\n        do { touch(r) } while (r.more)\n    }\n}\n",
                "scan",
            ),
            (
                Box::new(crate::parser::DartParser::new()),
                "a.dart",
                "void scan(List<Row> rows) {\n  for (var r in rows) {\n    while (r.more) { touch(r); }\n  }\n}\n",
                "scan",
            ),
            (
                Box::new(crate::parser::CppParser::new()),
                "a.cpp",
                "void scan(std::vector<Row> rows) {\n  for (auto& r : rows) {\n    for (int i = 0; i < r.n; i++) { touch(i); }\n  }\n}\n",
                "scan",
            ),
            (
                Box::new(crate::parser::GroovyParser::new()),
                "a.groovy",
                "class A { void scan(rows) { for (r in rows) { while (r.more) { touch(r) } } } }",
                "scan",
            ),
        ];
        for (parser, file, src, name) in cases {
            assert_eq!(
                depth(parser.as_ref(), file, src, name),
                Some(2),
                "{file} did not read two nested loops"
            );
        }
    }

    /// Python writes its second-commonest nested loop as one expression.
    /// Two `for` clauses in one comprehension are siblings in the tree and a
    /// naive walk scores them 1.
    #[test]
    fn two_for_clauses_in_one_comprehension_are_depth_two() {
        let parser = crate::parser::PythonParser::new();
        let flat = "def f(rows):\n    return [y for row in rows for y in row]\n";
        assert_eq!(depth(&parser, "a.py", flat, "f"), Some(2));
        let one = "def g(rows):\n    return [r.n for r in rows]\n";
        assert_eq!(depth(&parser, "a.py", one, "g"), Some(1));
    }

    /// Silence has to be distinguishable from a finding of zero. A language
    /// with no table leaves the field unset rather than claiming no loops.
    #[test]
    fn a_language_without_a_table_reports_nothing_rather_than_zero() {
        use crate::models::file_info::Language;
        assert!(super::has_table(Language::Rust));
        assert!(super::has_table(Language::Kotlin));
        assert!(!super::has_table(Language::Ruby));
        assert!(!super::has_table(Language::Scala));
    }
}
