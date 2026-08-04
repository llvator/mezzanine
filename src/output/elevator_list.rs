//! Two small text artifacts derived from an Elevator analysis:
//!
//! - [`render_list`] — enumerate every entity, grouped by kind,
//!   sorted alphabetically. Designed for "what features exist?"
//!   tracking and for grepping (`elevator . --list | grep '^  f '`).
//! - [`render_stats`] — compact count table, including unresolved
//!   and duplicate signals. Designed for status checks and CI.
//!
//! Both deliberately stay text-only. The structure is simple enough
//! that a script can parse with `grep`/`awk`; if a future consumer
//! needs structured data, the existing `nao analyze -f json` already
//! provides the full graph.

use crate::analyzer::AnalysisResult;
use crate::models::{CodeEntity, EntityKind};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

/// Enumerate Elevator entities, grouped by kind, sorted by
/// qualified name so consecutive runs produce stable diffs.
///
/// `kind_filter` restricts the output:
///   - `"all"` (or empty) → every kind
///   - `"c"`/`"category"`/`"categories"` → Categories only
///   - `"f"`/`"feature"`/`"features"` → Features only
///   - `"fu"`/`"functionality"`/`"functionalities"` → Functionalities only
///   - `"concept"`/`"concepts"` → Concepts only
///   - `"ui"`/`"ui-page"`/`"ui-pages"`/`"pages"` → UI Pages only
///
/// Returns `Err` with a helpful message if the filter doesn't match
/// any of the accepted aliases — the caller turns this into a
/// non-zero exit so a typo in CI fails loudly.
pub fn render_list(
    result: &AnalysisResult,
    kind_filter: &str,
    grouped: bool,
) -> Result<String, String> {
    let kinds = resolve_kind_filter(kind_filter)?;

    let entities: Vec<&CodeEntity> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("elevator"))
        .collect();
    let by_id: HashMap<String, &CodeEntity> =
        entities.iter().map(|e| (e.id.clone(), *e)).collect();

    let mut out = String::new();
    let total = entities.len();
    let unresolved = entities.iter().filter(|e| e.tags.contains("unresolved")).count();

    let _ = writeln!(out, "# Elevator entity list");
    let header = if kind_filter == "all" || kind_filter.is_empty() {
        if unresolved > 0 {
            format!("{} entities ({} unresolved)", total, unresolved)
        } else {
            format!("{} entities", total)
        }
    } else {
        // Filtered view — count only the kinds shown, but keep the
        // grand total visible so a reader knows how big the slice is
        // relative to the whole spec.
        let shown: usize = entities.iter().filter(|e| kinds.contains(&e.kind)).count();
        let shown_unresolved: usize = entities
            .iter()
            .filter(|e| kinds.contains(&e.kind) && e.tags.contains("unresolved"))
            .count();
        let label = canonical_filter_label(kind_filter);
        if shown_unresolved > 0 {
            format!(
                "{} {} ({} unresolved; {} entities total in spec)",
                shown, label, shown_unresolved, total
            )
        } else {
            format!("{} {} ({} entities total in spec)", shown, label, total)
        }
    };
    let _ = writeln!(out, "> {}", header);
    let _ = writeln!(out);

    if entities.is_empty() {
        let _ = writeln!(out, "(no Elevator entities in this project)");
        return Ok(out);
    }

    // Render in the same kind order the legend uses: Category first
    // (top of hierarchy), UI Pages last (leaves). Concepts sit
    // between Functionalities and UI Pages so cross-cuts appear
    // adjacent to the structure they cut across.
    for (label, kind) in [
        ("Extensions", EntityKind::Extension),
        ("Categories", EntityKind::Category),
        ("Features", EntityKind::Feature),
        ("Functionalities", EntityKind::Functionality),
        ("Concepts", EntityKind::Concept),
        ("UI Pages", EntityKind::UiPage),
    ] {
        if !kinds.contains(&kind) {
            continue;
        }
        let group: Vec<&&CodeEntity> = entities.iter().filter(|e| e.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        // Grouping only matters for kinds with a parent chain.
        // Categories, Concepts, and UI Pages render the same way
        // either way; Features and Functionalities are where
        // `--grouped` adds value.
        let should_group = grouped
            && matches!(kind, EntityKind::Feature | EntityKind::Functionality);
        if should_group {
            render_grouped_section(label, kind, &group, &by_id, &mut out);
        } else {
            render_flat_section(label, kind, &group, &mut out);
        }
    }

    Ok(out)
}

/// Flat alphabetic listing — the original behaviour. Used for
/// Categories, Concepts, UI Pages, and for everything when
/// `--grouped` is off.
fn render_flat_section(
    label: &str,
    kind: EntityKind,
    group: &[&&CodeEntity],
    out: &mut String,
) {
    let mut sorted: Vec<&&CodeEntity> = group.iter().copied().collect();
    sorted.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    let _ = writeln!(out, "## {} ({})", label, sorted.len());
    for e in sorted {
        let _ = writeln!(out, "  {} {}{}", kind_marker(kind), e.qualified_name, unresolved_tag(e));
    }
    let _ = writeln!(out);
}

/// Hierarchical listing: bucket entities by their parent chain
/// (root-most first), render each bucket as a `c X / f Y` header
/// followed by the entities underneath. Orphans (entities whose
/// chain is empty) land in a `(orphan / unparented)` bucket at the
/// bottom — usually a spec bug worth flagging.
fn render_grouped_section(
    label: &str,
    kind: EntityKind,
    group: &[&&CodeEntity],
    by_id: &HashMap<String, &CodeEntity>,
    out: &mut String,
) {
    // BTreeMap keyed by the chain string keeps groups sorted
    // alphabetically by the path (Categories grouped first, then
    // Features inside, etc. — matches the visual hierarchy).
    let mut buckets: BTreeMap<String, Vec<&&CodeEntity>> = BTreeMap::new();
    let mut orphans: Vec<&&CodeEntity> = Vec::new();
    for e in group {
        let chain = parent_chain(e, by_id);
        if chain.is_empty() {
            orphans.push(*e);
        } else {
            let header = chain
                .iter()
                .map(|p| format!("{} {}", kind_marker(p.kind), p.qualified_name))
                .collect::<Vec<_>>()
                .join(" / ");
            buckets.entry(header).or_default().push(*e);
        }
    }

    let by_what = match kind {
        EntityKind::Feature => "Category",
        EntityKind::Functionality => "parent path",
        _ => "parent",
    };
    let _ = writeln!(out, "## {} grouped by {} ({})", label, by_what, group.len());

    for (header, mut entries) in buckets {
        let _ = writeln!(out, "{}", header);
        entries.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        for e in entries {
            let _ = writeln!(out, "  {} {}{}", kind_marker(kind), e.qualified_name, unresolved_tag(e));
        }
    }
    if !orphans.is_empty() {
        let _ = writeln!(out, "(orphan / unparented)");
        let mut sorted_orphans: Vec<&&CodeEntity> = orphans.iter().copied().collect();
        sorted_orphans.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        for e in sorted_orphans {
            let _ = writeln!(out, "  {} {}{}", kind_marker(kind), e.qualified_name, unresolved_tag(e));
        }
    }
    let _ = writeln!(out);
}

/// Walk parent_id chain root-most first. Used by `--grouped` to
/// bucket each entity by its ancestor path.
fn parent_chain<'a>(
    entity: &'a CodeEntity,
    by_id: &HashMap<String, &'a CodeEntity>,
) -> Vec<&'a CodeEntity> {
    let mut chain: Vec<&CodeEntity> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor = entity.parent_id.as_ref().and_then(|p| by_id.get(p).copied());
    while let Some(p) = cursor {
        if !seen.insert(p.id.clone()) {
            break; // cycle guard — shouldn't happen but be safe
        }
        chain.push(p);
        cursor = p.parent_id.as_ref().and_then(|pp| by_id.get(pp).copied());
    }
    chain.reverse();
    chain
}

fn unresolved_tag(e: &CodeEntity) -> &'static str {
    if e.tags.contains("unresolved") {
        " [UNRESOLVED]"
    } else {
        ""
    }
}

/// Parse a kind-filter string (CLI value for `--list`) into the set
/// of EntityKinds to include. Free-form input with multiple aliases
/// per kind so a user can write `f`, `feature`, or `features`
/// interchangeably.
fn resolve_kind_filter(s: &str) -> Result<Vec<EntityKind>, String> {
    use EntityKind::*;
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "all" => Ok(vec![Extension, Category, Feature, Functionality, Concept, UiPage]),
        "e" | "extension" | "extensions" => Ok(vec![Extension]),
        "c" | "category" | "categories" => Ok(vec![Category]),
        "f" | "feature" | "features" => Ok(vec![Feature]),
        "fu" | "functionality" | "functionalities" => Ok(vec![Functionality]),
        "concept" | "concepts" => Ok(vec![Concept]),
        "ui" | "ui-page" | "ui-pages" | "uipage" | "uipages" | "pages" => Ok(vec![UiPage]),
        other => Err(format!(
            "unknown --list filter `{}`. Try one of: all, e|extensions, c|categories, f|features, fu|functionalities, concept|concepts, ui|pages.",
            other
        )),
    }
}

fn canonical_filter_label(s: &str) -> &'static str {
    match s.trim().to_ascii_lowercase().as_str() {
        "e" | "extension" | "extensions" => "Extensions",
        "c" | "category" | "categories" => "Categories",
        "f" | "feature" | "features" => "Features",
        "fu" | "functionality" | "functionalities" => "Functionalities",
        "concept" | "concepts" => "Concepts",
        "ui" | "ui-page" | "ui-pages" | "uipage" | "uipages" | "pages" => "UI Pages",
        _ => "all kinds",
    }
}

/// Compact count table per entity kind, with a top-line total plus
/// signals (unresolved entities, relationship count) so the artifact
/// works as a one-glance project-size and project-health summary.
pub fn render_stats(result: &AnalysisResult) -> String {
    let entities: Vec<&CodeEntity> = result
        .entities
        .iter()
        .filter(|e| e.tags.contains("elevator"))
        .collect();

    let mut out = String::new();
    let total = entities.len();
    let unresolved = entities.iter().filter(|e| e.tags.contains("unresolved")).count();
    let rel_count = result.relationships.len();

    let _ = writeln!(out, "# Elevator stats");
    let _ = writeln!(
        out,
        "> {} entities, {} relationships{}",
        total,
        rel_count,
        if unresolved > 0 {
            format!(" ({} unresolved)", unresolved)
        } else {
            String::new()
        }
    );
    let _ = writeln!(out);

    // Per-kind counts. Right-aligned numbers in a fixed-width column
    // so the column lines up visually when scanned. Extensions are
    // included regardless of whether the spec uses them — keeps the
    // table shape stable across projects with and without them.
    for (marker, label, kind) in [
        ("e ", "Extensions     ", EntityKind::Extension),
        ("c ", "Categories     ", EntityKind::Category),
        ("f ", "Features       ", EntityKind::Feature),
        ("fu", "Functionalities", EntityKind::Functionality),
        ("@ ", "Concepts       ", EntityKind::Concept),
        ("ui", "UI Pages       ", EntityKind::UiPage),
    ] {
        let n = entities.iter().filter(|e| e.kind == kind).count();
        let u = entities
            .iter()
            .filter(|e| e.kind == kind && e.tags.contains("unresolved"))
            .count();
        let suffix = if u > 0 {
            format!("  ({} unresolved)", u)
        } else {
            String::new()
        };
        let _ = writeln!(out, "{}  {} {:>4}{}", marker, label, n, suffix);
    }

    out
}

fn kind_marker(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Extension => "e",
        EntityKind::Category => "c",
        EntityKind::Feature => "f",
        EntityKind::Functionality => "fu",
        EntityKind::Concept => "@",
        EntityKind::UiPage => "ui",
        _ => "?",
    }
}
