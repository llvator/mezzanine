//! IM-001 column-header parsing + IM-002 reference-column extraction.
//!
//! A header cell looks like `name[mod1=val1, mod2=val2]` for a plain
//! attribute, or `name(targetAttr1, targetAttr2)[mod1=val1]` for a
//! reference column. The parser splits the cell into:
//!
//! * **name** — the attribute name on the row's target type.
//! * **target_attrs** — `Some(vec)` when the cell was a reference
//!   column (the parens block was present); `None` otherwise.
//! * **modifiers** — the bracketed `key=value` pairs.
//!
//! Reference columns produce a type→type relationship at the dispatch
//! layer (see `mod.rs::handle_header`); this module just exposes the
//! parsed shape.

use super::lexer::{split_unquoted, unquote};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Column {
    pub name: String,
    /// `None` for a plain attribute column. `Some(vec)` for a
    /// reference column (the parentheses block was present); the vec
    /// holds the comma-separated target-attribute names. Stripped of
    /// surrounding whitespace.
    pub target_attrs: Option<Vec<String>>,
    pub modifiers: Vec<(String, String)>,
}

/// Parse a single column-header cell. Whitespace at the boundaries is
/// tolerated; the lexer respects double-quoted segments inside
/// modifier values.
pub(super) fn parse_column(cell: &str) -> Option<Column> {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (head, modifiers_raw) = split_brackets(trimmed);
    let (name, target_attrs) = split_reference(head);
    let modifiers = parse_modifiers(modifiers_raw);

    if name.is_empty() {
        return None;
    }
    Some(Column {
        name,
        target_attrs,
        modifiers,
    })
}

/// Split off the bracketed modifier block. `name[mod=val]` →
/// `("name", "mod=val")`; `name` → `("name", "")`. Brackets must be
/// matched at the outer level; inner brackets are preserved verbatim
/// so a future Spring-bean style `[default=foo[bar]]` round-trips.
fn split_brackets(cell: &str) -> (&str, &str) {
    let Some(open) = cell.find('[') else {
        return (cell, "");
    };
    if !cell.ends_with(']') {
        return (cell, "");
    }
    let head = cell[..open].trim_end();
    let body = &cell[open + 1..cell.len() - 1];
    (head, body)
}

/// Split off the reference-column parentheses. `unit(code)` →
/// `("unit", Some(["code"]))`; `unit(code, system)` →
/// `("unit", Some(["code", "system"]))`; `code` → `("code", None)`.
/// Whitespace inside the parens is tolerated.
fn split_reference(head: &str) -> (String, Option<Vec<String>>) {
    let Some(open) = head.find('(') else {
        return (head.trim().to_string(), None);
    };
    if !head.ends_with(')') {
        return (head.trim().to_string(), None);
    }
    let name = head[..open].trim().to_string();
    let body = &head[open + 1..head.len() - 1];
    let attrs: Vec<String> = split_unquoted(body, ',')
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (name, Some(attrs))
}

/// Parse a modifier expression body (`mod1=val1, mod2=val2`) into a
/// list of `(key, value)` pairs. Values are quote-stripped.
pub(super) fn parse_modifiers(body: &str) -> Vec<(String, String)> {
    if body.trim().is_empty() {
        return Vec::new();
    }
    split_unquoted(body, ',')
        .into_iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            let eq = raw.find('=')?;
            let key = raw[..eq].trim().to_string();
            let value = unquote(raw[eq + 1..].trim()).to_string();
            Some((key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_column() {
        let c = parse_column("code").unwrap();
        assert_eq!(c.name, "code");
        assert!(c.target_attrs.is_none());
        assert!(c.modifiers.is_empty());
    }

    #[test]
    fn column_with_modifier() {
        let c = parse_column("code[unique=true]").unwrap();
        assert_eq!(c.name, "code");
        assert_eq!(c.modifiers, vec![("unique".to_string(), "true".to_string())]);
    }

    #[test]
    fn reference_column_single_attr() {
        let c = parse_column("unit(code)").unwrap();
        assert_eq!(c.name, "unit");
        assert_eq!(c.target_attrs.as_deref(), Some(&["code".to_string()][..]));
    }

    #[test]
    fn reference_column_multi_attr() {
        let c = parse_column("unit(code, system)").unwrap();
        assert_eq!(
            c.target_attrs.as_deref(),
            Some(&["code".to_string(), "system".to_string()][..])
        );
    }

    #[test]
    fn reference_column_with_modifier() {
        let c = parse_column("country(isocode)[unique=true]").unwrap();
        assert_eq!(c.name, "country");
        assert_eq!(c.target_attrs.as_deref(), Some(&["isocode".to_string()][..]));
        assert_eq!(c.modifiers, vec![("unique".to_string(), "true".to_string())]);
    }

    #[test]
    fn whitespace_tolerated() {
        let a = parse_column("unit ( code )").unwrap();
        let b = parse_column("unit(code)").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn modifier_value_with_quoted_comma() {
        let c = parse_column("code[default=\"a,b\"]").unwrap();
        assert_eq!(c.modifiers, vec![("default".to_string(), "a,b".to_string())]);
    }
}
