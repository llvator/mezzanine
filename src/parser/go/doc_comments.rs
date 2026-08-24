//! Go doc comments: the run of `//` lines (or one `/* … */`) sitting
//! directly above a declaration, with no blank line between.
//!
//! Adjacency is the whole rule, and it is load-bearing. Go has no marker
//! distinguishing documentation from an ordinary comment — `// TODO: drop
//! this` two lines above a function is not its doc, and the only thing
//! separating the two cases is the blank line. So the walk backwards stops
//! at the first gap rather than collecting every comment it can reach.

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// The doc comment for a declaration node, or `None` when the lines above
/// it are blank, code, or a detached comment.
pub(super) fn extract_doc(node: &Node, source: &str) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut next_row = node.start_position().row;
    let mut current = node.prev_sibling();

    while let Some(sib) = current {
        if sib.kind() != "comment" || sib.end_position().row + 1 != next_row {
            break;
        }
        lines.push(strip_comment(node_text(&sib, source)));
        next_row = sib.start_position().row;
        current = sib.prev_sibling();
    }

    lines.reverse();
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The doc for a spec inside a declaration wrapper (`type_spec` under
/// `type_declaration`, `const_spec` under `const_declaration`, …).
///
/// A grouped declaration documents each spec in place, so the comment is
/// the spec's own previous sibling. An ungrouped one — `// Doc` then
/// `type Order struct{…}` — puts it above the wrapper, where the spec
/// cannot see it, because the spec's previous sibling is the `type`
/// keyword. Try the spec first, then the wrapper it sits in.
pub(super) fn extract_spec_doc(spec: &Node, source: &str) -> Option<String> {
    extract_doc(spec, source).or_else(|| extract_doc(&spec.parent()?, source))
}

/// The package comment: the block above `package x`, which documents the
/// package rather than any one declaration in it.
///
/// It travels as `ParseResult::file_documentation` for the same reason
/// Rust's `//!` header does — the package is declared across every file
/// that says `package x`, so there is no entity in *this* parse to hang
/// it on.
pub(super) fn package_doc(root: Node, source: &str) -> Option<String> {
    let mut cursor = root.walk();
    let clause = root
        .children(&mut cursor)
        .find(|c| c.kind() == "package_clause")?;
    extract_doc(&clause, source)
}

/// Strip the comment's own punctuation, leaving the prose. `//` loses the
/// slashes and one following space; `/* … */` loses the delimiters and the
/// leading `*` decoration some writers keep on each line.
fn strip_comment(text: &str) -> String {
    if let Some(rest) = text.strip_prefix("//") {
        return rest
            .strip_prefix(' ')
            .unwrap_or(rest)
            .trim_end()
            .to_string();
    }
    let inner = text
        .trim_start_matches("/*")
        .trim_end_matches("*/")
        .trim_matches('\n');
    inner
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            match trimmed.strip_prefix('*') {
                Some(rest) => rest.trim_start().to_string(),
                None => trimmed.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_comment;

    #[test]
    fn line_comments_lose_their_slashes() {
        assert_eq!(
            strip_comment("// Handle serves one request."),
            "Handle serves one request."
        );
        assert_eq!(strip_comment("//no space"), "no space");
    }

    #[test]
    fn block_comments_lose_their_delimiters_and_decoration() {
        assert_eq!(strip_comment("/* one line */"), "one line");
        assert_eq!(
            strip_comment("/*\n * first\n * second\n */"),
            "first\nsecond"
        );
    }
}
