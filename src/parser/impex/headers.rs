//! IM-001 header-line parsing.
//!
//! A header line starts with one of `INSERT` / `UPDATE` /
//! `INSERT_UPDATE` / `REMOVE`, followed by a target type that may
//! carry header-level modifiers in `[...]` brackets, and then a
//! `;`-separated list of column-header cells handled by
//! [`super::columns`]. Examples:
//!
//! ```text
//! INSERT_UPDATE UnitMapping; code[unique=true]; unit(code); country(isocode)[unique=true]
//! UPDATE ApplicationConfiguration[processor=com.example.SkipNotExistingItemImpexProcessor]; key[unique=true]; value
//! ```
//!
//! Output: an `ImpexHeader` carrying the operation, the target type,
//! header modifiers (with `processor=` / `translator=` /
//! `cellDecorator=` flagged as FQN class references for
//! cross-language `References` edges per IM-001's contract), and the
//! list of parsed columns.

use super::columns::{parse_column, parse_modifiers, Column};
use super::lexer::split_unquoted;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HeaderOp {
    Insert,
    Update,
    InsertUpdate,
    Remove,
}

impl HeaderOp {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "INSERT" => Some(Self::Insert),
            "UPDATE" => Some(Self::Update),
            "INSERT_UPDATE" => Some(Self::InsertUpdate),
            "REMOVE" => Some(Self::Remove),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ImpexHeader {
    pub op: HeaderOp,
    pub target_type: String,
    /// Header-level modifiers (from the `Type[...]` brackets), each
    /// already split into `(key, value)`. Class-reference values
    /// (`processor=...`, `translator=...`) are surfaced separately
    /// in `class_refs` rather than buried in this list.
    pub modifiers: Vec<(String, String)>,
    /// Fully-qualified class references called out from the header
    /// modifiers — these become cross-language `References` edges
    /// per IM-001's contract.
    pub class_refs: Vec<String>,
    pub columns: Vec<Column>,
}

/// Modifier keys whose value, when present, is a fully-qualified Java
/// class name worth emitting as a cross-language `References` edge.
/// The list is small and stable; SAP documents these in the Impex
/// reference under "header parameters".
const CLASS_REFERENCE_KEYS: &[&str] = &[
    "processor",
    "translator",
    "cellDecorator",
    "default",
    "modifierClass",
];

/// Parse a fully-substituted header line into an `ImpexHeader`. The
/// caller is responsible for macro substitution before reaching this
/// point so we never see literal `$<name>` references here.
pub(super) fn parse_header(line: &str) -> Option<ImpexHeader> {
    // Cells are `;`-separated. The first cell carries the operation +
    // target type + header modifiers; the rest are column headers.
    let cells = split_unquoted(line, ';');
    if cells.is_empty() {
        return None;
    }

    let (op, target_type, modifiers) = parse_op_target(cells[0].trim())?;
    let class_refs = collect_class_refs(&modifiers);

    let columns: Vec<Column> = cells[1..].iter().filter_map(|c| parse_column(c)).collect();

    Some(ImpexHeader {
        op,
        target_type,
        modifiers,
        class_refs,
        columns,
    })
}

/// Split the leading cell into (operation, target_type, modifiers).
/// `INSERT_UPDATE Type[mod=val, mod2=val2]` → `(InsertUpdate, "Type",
/// [(mod, val), (mod2, val2)])`. Returns `None` when the leading
/// token isn't a recognised operation — the dispatcher uses this as
/// the test for "is this a header line."
pub(super) fn parse_op_target(cell: &str) -> Option<(HeaderOp, String, Vec<(String, String)>)> {
    let mut it = cell.splitn(2, char::is_whitespace);
    let op_token = it.next()?.trim();
    let op = HeaderOp::parse(op_token)?;
    let rest = it.next().unwrap_or("").trim();
    if rest.is_empty() {
        return None;
    }

    // `Type[mod=val]` — split on the first `[` if any. The closing
    // `]` is at the *end* of `rest` because Impex header modifiers
    // cover the whole leading cell.
    let (target_raw, mod_body) = if let Some(open) = rest.find('[') {
        if rest.ends_with(']') {
            (&rest[..open], &rest[open + 1..rest.len() - 1])
        } else {
            // Malformed (open bracket without close on this line) —
            // treat the bracket as part of the type name and let the
            // resolver fail at link time. A dedicated warning isn't
            // worth the noise.
            (rest, "")
        }
    } else {
        (rest, "")
    };

    let target_type = target_raw.trim().to_string();
    let modifiers = parse_modifiers(mod_body);
    Some((op, target_type, modifiers))
}

fn collect_class_refs(modifiers: &[(String, String)]) -> Vec<String> {
    modifiers
        .iter()
        .filter_map(|(k, v)| {
            if CLASS_REFERENCE_KEYS.iter().any(|c| c == k) && looks_like_fqn(v) {
                Some(v.clone())
            } else {
                None
            }
        })
        .collect()
}

/// True for values that look like a fully-qualified Java class name —
/// dot-separated, all components alphanumeric or `_`, leading
/// component starts lowercase (package), trailing component starts
/// uppercase (class). Filters out non-class `default=` values like
/// numbers / quoted strings.
fn looks_like_fqn(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    let last = parts[parts.len() - 1];
    let first = parts[0];
    last.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && first.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_simple() {
        let h = parse_header("INSERT_UPDATE Foo; code[unique=true]; value").unwrap();
        assert_eq!(h.op, HeaderOp::InsertUpdate);
        assert_eq!(h.target_type, "Foo");
        assert_eq!(h.columns.len(), 2);
        assert_eq!(h.columns[0].name, "code");
        assert_eq!(h.columns[1].name, "value");
    }

    #[test]
    fn header_with_processor_modifier() {
        let h = parse_header(
            "UPDATE ApplicationConfiguration[processor=com.example.SkipNotExistingItemImpexProcessor]; key[unique=true]; value",
        )
        .unwrap();
        assert_eq!(h.target_type, "ApplicationConfiguration");
        assert_eq!(
            h.class_refs,
            vec!["com.example.SkipNotExistingItemImpexProcessor".to_string()]
        );
    }

    #[test]
    fn header_remove() {
        let h = parse_header("REMOVE Foo; code[unique=true]").unwrap();
        assert_eq!(h.op, HeaderOp::Remove);
    }

    #[test]
    fn fqn_detection_rejects_literal_values() {
        // `default=true` should not become a class reference.
        let h = parse_header("INSERT_UPDATE Foo[default=true]; code").unwrap();
        assert!(h.class_refs.is_empty());
    }

    #[test]
    fn fqn_detection_rejects_non_class_dotted() {
        // version-like strings shouldn't false-match.
        let h = parse_header("INSERT_UPDATE Foo[default=1.2.3]; code").unwrap();
        assert!(h.class_refs.is_empty());
    }

    #[test]
    fn non_header_returns_none() {
        assert!(parse_header("$catalog = foo").is_none());
        assert!(parse_header("# a comment").is_none());
    }
}
