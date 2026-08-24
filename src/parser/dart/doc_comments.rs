//! Dartdoc extraction — `///` line comments and `/** … */` blocks.
//!
//! The grammar folds a run of consecutive `///` lines into one `comment`
//! node, so there is nothing to stitch back together. It also puts
//! annotations *inside* the declaration they annotate, which leaves the
//! comment as the declaration's immediate previous sibling and makes this
//! module simpler than its Java and Kotlin counterparts — there is nothing
//! to walk back over.
//!
//! A `comment` node covers every comment, so the `///` and `/**` prefixes
//! are what separate documentation from an ordinary aside.

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// The Dartdoc attached to `node`, if it has any.
pub(super) fn extract(node: &Node, source: &str) -> Option<String> {
    let comment = preceding_doc(node, source)?;
    clean(node_text(&comment, source))
}

/// The documentation comment written directly in front of `node`.
///
/// Visible to `containers` because a declaration's span is widened over it:
/// the grammar's node starts at `class`, and a reader would say the
/// declaration starts at the prose explaining it. An ordinary `//` aside is
/// not that prose, so it does not count here either.
pub(super) fn preceding_doc<'t>(node: &Node<'t>, source: &str) -> Option<Node<'t>> {
    let sibling = node.prev_sibling()?;
    if sibling.kind() != "comment" {
        return None;
    }
    let text = node_text(&sibling, source).trim_start();
    (text.starts_with("///") || text.starts_with("/**")).then_some(sibling)
}

/// Strip the comment markers a reader would not want in a doc string.
///
/// Both Dartdoc forms are handled: `///` on every line, and `/** … */` with
/// a leading `*` per line. An ordinary `//` aside survives none of that with
/// content intact — it is reported as absent, which is what keeps a
/// `// TODO` above a class out of its documentation.
fn clean(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.starts_with("///") && !trimmed.starts_with("/**") {
        return None;
    }

    let body = trimmed
        .trim_start_matches("/**")
        .trim_end_matches("*/")
        .trim();

    let lines: Vec<String> = body
        .lines()
        .map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("///").unwrap_or(line);
            let line = line.strip_prefix('*').unwrap_or(line);
            line.trim().to_string()
        })
        .collect();

    let cleaned = lines.join("\n").trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// The library-level Dartdoc: the comment written above a `library`
/// directive.
///
/// Only that position counts. A `///` block at the top of a file with no
/// `library` directive documents whatever declaration follows it, and
/// claiming it for the file would steal the first function's docs.
pub(super) fn library_doc(root: Node, source: &str) -> Option<String> {
    let mut cursor = root.walk();
    let library = root
        .children(&mut cursor)
        .find(|c| c.kind() == "library_name")?;
    extract(&library, source)
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn strips_the_line_comment_form() {
        assert_eq!(
            clean("/// A shopping cart.\n/// Holds orders."),
            Some("A shopping cart.\nHolds orders.".to_string())
        );
    }

    #[test]
    fn strips_the_block_comment_form() {
        assert_eq!(
            clean("/**\n * A shopping cart.\n */"),
            Some("A shopping cart.".to_string())
        );
    }

    #[test]
    fn an_ordinary_aside_is_not_documentation() {
        assert_eq!(clean("// just a note"), None);
        assert_eq!(clean("/* a block aside */"), None);
    }

    #[test]
    fn a_comment_with_no_prose_is_absent() {
        assert_eq!(clean("///"), None);
        assert_eq!(clean("/** */"), None);
    }
}
