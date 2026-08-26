//! File-level Educator scan — walks every node in a Java file and reports
//! every rule that fires, regardless of cursor position. Powers
//! `mezz educate <file>` (a linter-style report) and the future
//! `/api/educator/scan` endpoint.
//!
//! Differs from [`super::position::query`]:
//! - Position query: cursor in, ancestor stack + matched rules out.
//! - Scan: file in, every (span, rule_id) pair out.
//!
//! Both reuse the same per-language extractor and the same predicate
//! evaluator — the only difference is the AST traversal.

use super::corpus::Educator;
use super::predicate;
use super::rules::Rule;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use tree_sitter::Node;

#[derive(Debug, Clone, Serialize)]
pub struct ScanHit {
    /// Start line (0-based).
    pub line: u32,
    /// Start column (0-based).
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    /// Construct-kind that the rule matched on (e.g. `synchronized_statement`).
    pub kind: String,
    /// Bucket the rule fell into: `"specific"` (predicate matched) or
    /// `"general"` (no predicate, fires on every instance of the kind).
    pub bucket: String,
    pub rule_id: String,
    pub title: String,
    pub severity: String,
    pub rule_kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanResponse {
    pub language: String,
    pub hits: Vec<ScanHit>,
}

impl ScanResponse {
    pub fn empty(language: &str) -> Self {
        Self {
            language: language.to_string(),
            hits: Vec::new(),
        }
    }
}

/// Scan one file and return every rule hit, sorted by `(line, col)`.
pub fn scan_file(educator: &Educator, file: &Path) -> Result<ScanResponse> {
    let language = match file.extension().and_then(|s| s.to_str()) {
        Some("java") => "java",
        _ => return Ok(ScanResponse::empty("unknown")),
    };

    let source =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::language())
        .map_err(|e| anyhow::anyhow!("failed to set java language: {}", e))?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned no tree"))?;

    let mut hits = Vec::new();
    walk(tree.root_node(), educator, language, &source, &mut hits);
    hits.sort_by_key(|h| (h.line, h.col, h.rule_id.clone()));

    Ok(ScanResponse {
        language: language.to_string(),
        hits,
    })
}

/// Recursive walk: at every node, ask the extractor whether the kind is one
/// rules can attach to; if so, evaluate each attached rule's predicate and
/// emit a hit. Always descends into children regardless of the extract result.
fn walk(node: Node, educator: &Educator, language: &str, source: &str, hits: &mut Vec<ScanHit>) {
    if let Some(extracted) = super::java::extract(&node, source) {
        let range = node.range();
        for rule in educator.rules_for(language, extracted.kind) {
            if let Some(hit) = evaluate(rule, &extracted.attrs, &range, extracted.kind) {
                hits.push(hit);
            }
        }
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk(child, educator, language, source, hits);
        }
    }
}

fn evaluate(
    rule: &Rule,
    attrs: &super::predicate::Attrs,
    range: &tree_sitter::Range,
    kind: &str,
) -> Option<ScanHit> {
    let bucket = match &rule.match_predicate {
        Some(pred) if !pred.is_empty() => {
            if predicate::matches(pred, attrs) {
                "specific"
            } else {
                return None;
            }
        }
        _ => "general",
    };
    Some(ScanHit {
        line: range.start_point.row as u32,
        col: range.start_point.column as u32,
        end_line: range.end_point.row as u32,
        end_col: range.end_point.column as u32,
        kind: kind.to_string(),
        bucket: bucket.to_string(),
        rule_id: rule.id.clone(),
        title: rule.title().to_string(),
        severity: rule.severity.clone(),
        rule_kind: rule.kind.clone(),
    })
}
