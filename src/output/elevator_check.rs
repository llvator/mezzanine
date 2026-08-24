//! Spec-level checker for Elevator (`.elv`) projects.
//!
//! Surfaces problems an author wants to know about *while writing*
//! the spec — without forcing them to add detail per node. The two
//! categories of finding are:
//!
//! - **Errors** — the spec is broken. Lex / parse failures, dangling
//!   references that prevent connections from rendering. Exit code 1.
//! - **Hints** — looks intentional during draft work but is probably
//!   a typo or a forgotten link in published output. Exit code 0.
//!
//! Deliberately *not* checked (would drag toward documentation):
//! required `d:`, required `where:`, required `cr:`, prescriptive
//! tag vocabularies. The checker catches mistakes; it doesn't
//! enforce style.

use crate::analyzer::AnalysisResult;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Hint,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

/// Run every check against the merged analysis. Returns the findings
/// sorted by severity (errors first), each with a printable message
/// already prefixed by the rule name.
pub fn check(result: &AnalysisResult) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();

    surface_parse_warnings(result, &mut findings);
    detect_unresolved_entities(result, &mut findings);
    detect_orphan_features(result, &mut findings);
    detect_empty_categories(result, &mut findings);
    detect_unused_concepts(result, &mut findings);

    // Errors first; hints in original order within their bucket.
    findings.sort_by_key(|f| match f.severity {
        Severity::Error => 0,
        Severity::Hint => 1,
    });
    findings
}

/// Pass through every analyzer-emitted warning. Today these are:
///   - Per-file parse / lex errors propagated from the parsers.
///   - Out-of-scope cross-file references (missing imports).
/// Both are real errors — the spec doesn't behave the way the
/// author wrote it — so they get `Severity::Error`.
fn surface_parse_warnings(result: &AnalysisResult, out: &mut Vec<Finding>) {
    for w in &result.warnings {
        out.push(Finding {
            severity: Severity::Error,
            message: format!("parse: {}", w),
        });
    }
}

/// Entities referenced by name but never defined in any parsed file.
/// The post-merge stub pass tags them `unresolved` so we can find
/// them here. Reported as errors because the link won't render in
/// the graph the way the author wrote it.
fn detect_unresolved_entities(result: &AnalysisResult, out: &mut Vec<Finding>) {
    for e in elevator_entities(result) {
        if e.tags.contains("unresolved") {
            // Skip auto-created UI page stubs: those are conventional
            // (UI pages are rarely defined explicitly) so flagging
            // every one of them is noise.
            if e.kind == EntityKind::UiPage && e.tags.contains("auto_created") {
                continue;
            }
            out.push(Finding {
                severity: Severity::Error,
                message: format!(
                    "unresolved: {} {} is referenced but never defined",
                    kind_marker(e.kind),
                    e.qualified_name
                ),
            });
        }
    }
}

/// Features defined but not contained by any Category. Probably
/// work-in-progress, but worth surfacing so the author can either
/// place them under a Category or remove them.
fn detect_orphan_features(result: &AnalysisResult, out: &mut Vec<Finding>) {
    for e in elevator_entities(result) {
        if e.kind != EntityKind::Feature {
            continue;
        }
        if e.tags.contains("unresolved") {
            continue; // already flagged separately
        }
        if e.parent_id.is_none() {
            out.push(Finding {
                severity: Severity::Hint,
                message: format!(
                    "orphan: f {} is defined but not listed under any Category",
                    e.qualified_name
                ),
            });
        }
    }
}

/// Categories with no children. Either intentional (placeholder for
/// future work) or forgotten — author's call.
fn detect_empty_categories(result: &AnalysisResult, out: &mut Vec<Finding>) {
    let parents: HashSet<&str> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Contains)
        .map(|r| r.source_id.as_str())
        .collect();
    for e in elevator_entities(result) {
        if e.kind == EntityKind::Category && !parents.contains(e.id.as_str()) {
            out.push(Finding {
                severity: Severity::Hint,
                message: format!("empty: c {} has no Features", e.qualified_name),
            });
        }
    }
}

/// Concepts with no `used_by:` consumers. A Concept exists *because*
/// multiple Features cross-cut it; one with no users is dead code.
fn detect_unused_concepts(result: &AnalysisResult, out: &mut Vec<Finding>) {
    let used: HashSet<&str> = result
        .relationships
        .iter()
        .filter(|r| r.metadata.get("link").map(String::as_str) == Some("used_by"))
        .map(|r| r.target_id.as_str())
        .collect();
    for e in elevator_entities(result) {
        if e.kind == EntityKind::Concept && !used.contains(e.id.as_str()) {
            out.push(Finding {
                severity: Severity::Hint,
                message: format!("unused: @ {} has no `used_by:` consumers", e.qualified_name),
            });
        }
    }
}

fn elevator_entities(result: &AnalysisResult) -> impl Iterator<Item = &CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("elevator"))
}

fn kind_marker(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Extension => "e",
        EntityKind::Category => "c",
        EntityKind::Feature => "f",
        EntityKind::Functionality => "fu",
        EntityKind::UiPage => "ui",
        EntityKind::Concept => "@",
        _ => "?",
    }
}
