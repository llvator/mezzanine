//! Where each key of a top-level YAML mapping starts and ends.
//!
//! `serde_yaml::Value` carries no source locations, so the parse that
//! reads a Compose file's *meaning* cannot say which lines any of it came
//! from — and an entity with no line range leaves the details pane blank
//! (DK-001). This is the second, dumber pass that supplies them.
//!
//! It reads indentation only, never structure: find the top-level key
//! (`services:` at column 0), take its children as the next indent level
//! down, and end each child where the indentation returns to its own
//! level or shallower. That is enough for the block-style mappings every
//! real Compose file is written in.
//!
//! Deliberately not a second YAML parser. Anything it cannot locate
//! reports `None`, and the caller falls back to a zero-width span rather
//! than to a wrong one — the meaning always comes from `serde_yaml`, and
//! this only ever adds line numbers to it.

use std::collections::BTreeMap;

/// Inclusive line range of one mapping key's block, 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Block {
    pub first: usize,
    pub last: usize,
}

/// Index the children of `top` — e.g. every service under `services:`.
///
/// Returns an empty map when the key is absent or written in flow style
/// (`services: {…}`), which is legal YAML and which this pass declines to
/// guess at.
pub(super) fn index(content: &str, top: &str) -> BTreeMap<String, Block> {
    let mut out = BTreeMap::new();
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = top_level_key(&lines, top) else {
        return out;
    };

    // The children's indent is set by the first one seen; anything deeper
    // belongs to it rather than being a sibling.
    let mut child_indent: Option<usize> = None;
    let mut open: Option<(String, usize)> = None;

    for (index, raw) in lines.iter().enumerate().skip(start + 1) {
        let Some(indent) = content_indent(raw) else {
            continue;
        };
        // Back at column 0 — a new top-level key ends the block.
        if indent == 0 {
            close(&mut open, index.saturating_sub(1), &lines, &mut out);
            break;
        }
        let depth = *child_indent.get_or_insert(indent);
        if indent > depth {
            continue; // inside the current child
        }
        close(&mut open, index.saturating_sub(1), &lines, &mut out);
        if let Some(name) = mapping_key(raw) {
            open = Some((name, index));
        }
    }
    close(&mut open, lines.len().saturating_sub(1), &lines, &mut out);
    out
}

/// Close the open block at `last`, first walking back over the blank
/// lines that separate it from whatever comes next — those belong to
/// neither block, and including them would make a service's span trail
/// into empty space.
fn close(
    open: &mut Option<(String, usize)>,
    last: usize,
    lines: &[&str],
    out: &mut BTreeMap<String, Block>,
) {
    let Some((name, first)) = open.take() else {
        return;
    };
    let mut last = last.max(first);
    while last > first && lines.get(last).is_some_and(|raw| raw.trim().is_empty()) {
        last -= 1;
    }
    out.insert(name, Block { first, last });
}

/// The line holding `<top>:` at column 0.
fn top_level_key(lines: &[&str], top: &str) -> Option<usize> {
    lines
        .iter()
        .position(|raw| content_indent(raw) == Some(0) && mapping_key(raw).as_deref() == Some(top))
}

/// Indentation of a line that carries content, or `None` for a blank line
/// or a comment — neither ends a block, so neither should be measured.
fn content_indent(raw: &str) -> Option<usize> {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    Some(raw.len() - trimmed.len())
}

/// The key a `name:` line declares, unquoted. `None` for a sequence item
/// or anything else that is not a mapping key.
fn mapping_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('-') {
        return None;
    }
    let key = trimmed.split_once(':')?.0.trim();
    if key.is_empty() || key.contains(['{', '[', '"', '\'', '#']) {
        return None;
    }
    Some(key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSE: &str = r#"services:
  api:
    image: api
    ports:
      - "80:80"

  db:
    image: postgres:16
volumes:
  pgdata:
"#;

    #[test]
    fn a_service_spans_its_whole_block() {
        let blocks = index(COMPOSE, "services");
        assert_eq!(blocks["api"], Block { first: 1, last: 4 });
    }

    /// The last child ends where the next top-level key begins, not at the
    /// end of the file.
    #[test]
    fn the_last_service_stops_at_the_next_top_level_key() {
        let blocks = index(COMPOSE, "services");
        assert_eq!(blocks["db"], Block { first: 6, last: 7 });
    }

    #[test]
    fn a_later_top_level_key_indexes_too() {
        let blocks = index(COMPOSE, "volumes");
        assert_eq!(blocks["pgdata"], Block { first: 9, last: 9 });
    }

    #[test]
    fn a_missing_key_is_empty_not_a_guess() {
        assert!(index(COMPOSE, "networks").is_empty());
    }

    /// Flow style is legal and rare; declining to locate it is better than
    /// inventing a range.
    #[test]
    fn flow_style_is_declined() {
        assert!(index("services: {api: {image: x}}\n", "services").is_empty());
    }

    #[test]
    fn comments_and_blank_lines_do_not_end_a_block() {
        let yaml = "services:\n  api:\n    image: x\n\n    # note\n    ports: []\n";
        assert_eq!(index(yaml, "services")["api"], Block { first: 1, last: 5 });
    }
}
