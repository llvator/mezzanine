//! Inverted code-reference map for an Elevator (`.elv`) project.
//!
//! Walks every Elevator entity's `cr:` / `cr.<tag>:` attributes,
//! groups by code path, and emits a `path → entities` listing.
//! Paths referenced by two-or-more entities with the **same** `cr`
//! kind get a `★ DUPLICATE` marker — the strong signal that two
//! differently-named spec entities point at the same physical code,
//! which is almost always a deduplication candidate.
//!
//! Same path under *different* cr kinds (e.g. one entity says
//! `cr.fe` and another says `cr.be`) is **not** flagged: that's how
//! shared resources legitimately get referenced from multiple
//! layers.

use crate::analyzer::AnalysisResult;
use crate::models::{CodeEntity, EntityKind};
use std::collections::BTreeMap;
use std::fmt::Write;

/// Render the code-reference map. Output is sorted by path so diffs
/// across runs are stable.
pub fn render(result: &AnalysisResult) -> String {
    // (path, cr_kind) → [(entity_short_ref, entity_qualified_name_for_sort)]
    let mut by_path: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut total_refs = 0usize;

    for entity in result
        .entities
        .iter()
        .filter(|e| e.tags.contains("elevator"))
    {
        for attr in &entity.attributes {
            let Some((kind, path)) = parse_cr_attr(attr) else {
                continue;
            };
            total_refs += 1;
            by_path
                .entry(path)
                .or_default()
                .push((kind, short_ref(entity)));
        }
    }

    // Identify duplicates: a path is a duplicate if any cr-kind
    // appears under it more than once.
    let dup_paths: Vec<&String> = by_path
        .iter()
        .filter(|(_, refs)| has_duplicate_kind(refs))
        .map(|(p, _)| p)
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "# Code reference map (Elevator)");
    let _ = writeln!(
        out,
        "> {} path(s), {} reference(s), {} potential duplicate(s)",
        by_path.len(),
        total_refs,
        dup_paths.len()
    );
    let _ = writeln!(out);

    if by_path.is_empty() {
        let _ = writeln!(
            out,
            "(no `cr:` references in this project — add `cr: \"path\"` to entities to map them to code)"
        );
        return out;
    }

    for (path, refs) in &by_path {
        let is_dup = has_duplicate_kind(refs);
        let marker = if is_dup { "   ★ DUPLICATE" } else { "" };
        let _ = writeln!(out, "{}{}", path, marker);
        // Sort references for stable output: by cr-kind then entity name.
        let mut sorted = refs.clone();
        sorted.sort();
        for (kind, name) in sorted {
            let _ = writeln!(out, "  {} ← {}", kind, name);
        }
        let _ = writeln!(out);
    }

    if !dup_paths.is_empty() {
        let _ = writeln!(out, "# Potential duplicates");
        let _ = writeln!(
            out,
            "> Same `cr` kind references the same path from multiple entities — likely the same functionality named twice."
        );
        let _ = writeln!(out);
        for path in dup_paths {
            let _ = writeln!(out, "{}", path);
            let mut sorted = by_path[path].clone();
            sorted.sort();
            // Group same-kind duplicates so the user sees only the
            // pairs that actually conflict, not single-kind refs.
            let mut current_kind = String::new();
            for (kind, name) in &sorted {
                if kind != &current_kind {
                    let _ = writeln!(out, "  [{}]", kind);
                    current_kind = kind.clone();
                }
                let _ = writeln!(out, "    - {}", name);
            }
            let _ = writeln!(out);
        }
    }

    out
}

/// Parse `"cr:path"` or `"cr.<tag>:path"` into `(kind_label, path)`.
/// `kind_label` is the bracketed form `cr` or `cr.<tag>` so the
/// renderer can show it verbatim.
pub(crate) fn parse_cr_attr(attr: &str) -> Option<(String, String)> {
    if let Some(rest) = attr.strip_prefix("cr.") {
        let (tag, path) = rest.split_once(':')?;
        Some((format!("cr.{}", tag), path.to_string()))
    } else if let Some(path) = attr.strip_prefix("cr:") {
        Some(("cr".to_string(), path.to_string()))
    } else {
        None
    }
}

/// True if any (kind) appears two or more times among the refs to a
/// path. Same-kind same-path is the duplicate signal; mixed kinds
/// (e.g. `cr.fe` + `cr.be` on the same path) is intentional.
fn has_duplicate_kind(refs: &[(String, String)]) -> bool {
    let mut seen: std::collections::HashSet<&String> = std::collections::HashSet::new();
    for (kind, _) in refs {
        if !seen.insert(kind) {
            return true;
        }
    }
    false
}

pub(crate) fn short_ref(e: &CodeEntity) -> String {
    let prefix = match e.kind {
        EntityKind::Extension => "e",
        EntityKind::Category => "c",
        EntityKind::Feature => "f",
        EntityKind::Functionality => "fu",
        EntityKind::Concept => "@",
        EntityKind::UiPage => "ui",
        _ => "?",
    };
    format!("{} {}", prefix, e.qualified_name)
}
