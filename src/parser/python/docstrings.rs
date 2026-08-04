//! Python docstring extraction and cleaning.

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// Extract a Python docstring from the body of a function, class, or module.
///
/// Docstrings are string literals appearing as the first expression-statement
/// inside the body block. We skip leading comments and extract the string
/// content, stripping quotes and dedenting multiline text.
pub(super) fn extract_docstring(body_owner: &Node, source: &str) -> Option<String> {
    let body = body_owner.child_by_field_name("body")?;
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "expression_statement" {
            let mut inner = child.walk();
            for expr in child.children(&mut inner) {
                if expr.kind() == "string" || expr.kind() == "concatenated_string" {
                    return clean_docstring(node_text(&expr, source));
                }
            }
            // First non-comment statement isn't a string — no docstring.
            return None;
        }
        // Skip comments at the top of the body.
        if child.kind() != "comment" {
            return None;
        }
    }
    None
}

/// Strip triple/single quotes, dedent, and trim a raw docstring.
fn clean_docstring(raw: &str) -> Option<String> {
    let inner = if raw.starts_with("\"\"\"") || raw.starts_with("'''") {
        raw.get(3..raw.len().saturating_sub(3))?
    } else if raw.starts_with('"') || raw.starts_with('\'') {
        raw.get(1..raw.len().saturating_sub(1))?
    } else {
        return None;
    };

    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() <= 1 {
        return Some(trimmed.to_string());
    }

    // Minimum indentation of non-empty continuation lines.
    let min_indent = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    let dedented: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.to_string()
            } else if line.len() > min_indent {
                line[min_indent..].to_string()
            } else {
                line.trim().to_string()
            }
        })
        .collect();

    let result = dedented.join("\n").trim().to_string();
    if result.is_empty() { None } else { Some(result) }
}
