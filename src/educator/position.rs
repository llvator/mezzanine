//! Position query — re-parses the file on demand and walks the ancestor stack
//! at the cursor, returning the construct stack plus the rule hits partitioned
//! into Specific / General buckets.
//!
//! The server already keeps a cached graph elsewhere but does not retain
//! per-file ASTs, so the educator re-parses on each query. Tree-sitter on a
//! Java file is fast enough (<30ms typical) that this stays comfortably under
//! the hover latency budget.

use super::corpus::Educator;
use super::lessons::Lesson;
use super::predicate::{self, Attrs};
use super::rules::Rule;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use tree_sitter::Node;

/// One layer in the ancestor stack at the cursor — wire shape returned to the client.
///
/// `text` is a short single-line snippet of the source the construct covers,
/// truncated to ~80 characters with `…`. It exists so the UI can render
/// "method invocation `Arrays.asList(...)`" instead of just "method invocation"
/// — letting the reader connect the construct to the code they're looking at.
#[derive(Debug, Clone, Serialize)]
pub struct ConstructInstance {
    pub kind: String,
    pub span: SpanWire,
    pub attrs: Attrs,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpanWire {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// Whether a hit attaches to the cursor's innermost construct (`direct`) or to
/// an enclosing one (`ancestor`). Lets the UI distinguish "this lesson is
/// about the thing under your cursor" from "this lesson is about the method
/// containing the thing under your cursor."
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Attachment {
    Direct,
    Ancestor,
}

/// One rule hit returned to the client. Server-side evaluation means the client
/// renders without needing to know the rule schema or evaluate predicates.
///
/// `attached_to` names the construct-kind in the cursor stack that the rule
/// actually matched against; `attachment` says whether that kind is the
/// innermost-most-specific construct under the cursor (`direct`) or an
/// enclosing one (`ancestor`).
#[derive(Debug, Clone, Serialize)]
pub struct RuleHit {
    pub rule_id: String,
    pub title: String,
    pub kind: String,
    pub severity: String,
    pub body_markdown: String,
    pub source_path: String,
    pub attached_to: String,
    pub attachment: Attachment,
    /// Source-text snippet of the construct the rule matched against
    /// (same value as the `text` of the corresponding `ConstructInstance`).
    /// Lets the UI render "in: `method_invocation` `Arrays.asList(...)`".
    pub attached_text: String,
}

/// One lesson hit returned to the client — the teaching counterpart of
/// [`RuleHit`]. No severity / problem-kind because lessons aren't problems.
/// Same `attached_to` / `attachment` / `attached_text` story as [`RuleHit`].
#[derive(Debug, Clone, Serialize)]
pub struct LessonHit {
    pub lesson_id: String,
    pub title: String,
    pub level: String,
    pub body_markdown: String,
    pub source_path: String,
    pub attached_to: String,
    pub attachment: Attachment,
    pub attached_text: String,
}

/// Wire shape of `GET /api/educator/position`.
///
/// `cursor_kind` is the innermost construct-kind under the cursor (`stack[0]`
/// when the stack is non-empty). `cursor_text` is its source-text snippet —
/// together they power the "Cursor on: annotation `@Resource`" header the
/// sidebar always shows, even when no rule or lesson attaches.
#[derive(Debug, Clone, Serialize)]
pub struct PositionResponse {
    pub language: String,
    pub stack: Vec<ConstructInstance>,
    pub cursor_kind: Option<String>,
    pub cursor_text: Option<String>,
    pub specific: Vec<RuleHit>,
    pub general: Vec<RuleHit>,
    pub lessons: Vec<LessonHit>,
}

impl PositionResponse {
    pub fn empty(language: &str) -> Self {
        Self {
            language: language.to_string(),
            stack: Vec::new(),
            cursor_kind: None,
            cursor_text: None,
            specific: Vec::new(),
            general: Vec::new(),
            lessons: Vec::new(),
        }
    }
}

/// Resolve a `(line, col)` (both 0-based, char-column) to a byte offset inside
/// `source`. Returns `None` if the line does not exist. For ASCII text (the
/// common case for Java source), char column equals byte column equals UTF-16
/// code unit, so this agrees with all three coordinate systems.
fn byte_offset_at(source: &str, line: u32, col: u32) -> Option<usize> {
    let mut current_line = 0u32;
    let mut current_col = 0u32;
    for (byte_idx, ch) in source.char_indices() {
        if current_line == line && current_col == col {
            return Some(byte_idx);
        }
        if ch == '\n' {
            if current_line == line {
                return Some(byte_idx);
            }
            current_line += 1;
            current_col = 0;
        } else {
            current_col += 1;
        }
    }
    if current_line == line {
        Some(source.len())
    } else {
        None
    }
}

/// Run the position query.
pub fn query(educator: &Educator, file: &Path, line: u32, col: u32) -> Result<PositionResponse> {
    let language = match file.extension().and_then(|s| s.to_str()) {
        Some("java") => "java",
        _ => return Ok(PositionResponse::empty("unknown")),
    };

    let source =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let offset = match byte_offset_at(&source, line, col) {
        Some(off) => off,
        None => return Ok(PositionResponse::empty(language)),
    };

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::language())
        .map_err(|e| anyhow::anyhow!("failed to set java language: {}", e))?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned no tree"))?;

    let root = tree.root_node();
    let leaf = match root.descendant_for_byte_range(offset, offset) {
        Some(n) => n,
        None => return Ok(PositionResponse::empty(language)),
    };

    let stack = build_stack(leaf, &source);
    let cursor_kind = stack.first().map(|c| c.kind.clone());
    let cursor_text = stack.first().map(|c| c.text.clone());
    let (specific, general) = evaluate(educator, language, &stack);
    let lessons = evaluate_lessons(educator, language, &stack);

    Ok(PositionResponse {
        language: language.to_string(),
        stack,
        cursor_kind,
        cursor_text,
        specific,
        general,
        lessons,
    })
}

/// Walk from `leaf` up to root, calling the Java extractor on each node and
/// collecting the ones it recognizes. Returns innermost-first.
fn build_stack(leaf: Node, source: &str) -> Vec<ConstructInstance> {
    let mut out = Vec::new();
    let mut current = Some(leaf);
    while let Some(node) = current {
        if let Some(extracted) = super::java::extract(&node, source) {
            let sp = node.range();
            let text = snippet_for_node(&node, source);
            out.push(ConstructInstance {
                kind: extracted.kind.to_string(),
                span: SpanWire {
                    start_line: sp.start_point.row as u32,
                    start_col: sp.start_point.column as u32,
                    end_line: sp.end_point.row as u32,
                    end_col: sp.end_point.column as u32,
                },
                attrs: extracted.attrs,
                text,
            });
        }
        current = node.parent();
    }
    out
}

/// Build a single-line, length-bounded preview of the node's source. Used as
/// `ConstructInstance.text` so the UI can show "method declaration `void
/// transfer()`" instead of just "method declaration". For multi-line nodes
/// (whole methods, classes) only the first line is kept — enough to identify
/// the construct, short enough to fit on a chip.
fn snippet_for_node(node: &Node, source: &str) -> String {
    const MAX_CHARS: usize = 80;
    let raw = node.utf8_text(source.as_bytes()).unwrap_or("");
    let first_line = raw.lines().next().unwrap_or("").trim_end();
    if first_line.chars().count() <= MAX_CHARS && first_line.len() == raw.len() {
        return first_line.to_string();
    }
    // Truncate by char count to respect multi-byte boundaries; suffix with …
    // when we actually shortened (either by line break or by length).
    let truncated: String = first_line.chars().take(MAX_CHARS).collect();
    format!("{}…", truncated)
}

fn evaluate(
    educator: &Educator,
    language: &str,
    stack: &[ConstructInstance],
) -> (Vec<RuleHit>, Vec<RuleHit>) {
    let mut specific = Vec::new();
    let mut general = Vec::new();
    let mut seen_specific: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_general: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (depth, instance) in stack.iter().enumerate() {
        let attachment = if depth == 0 {
            Attachment::Direct
        } else {
            Attachment::Ancestor
        };
        let rules = educator.rules_for(language, &instance.kind);
        for rule in rules {
            if let Some(predicate) = &rule.match_predicate {
                if !predicate.is_empty() && predicate::matches(predicate, &instance.attrs) {
                    if seen_specific.insert(rule.id.clone()) {
                        specific.push(rule_hit(rule, instance, attachment));
                    }
                }
            } else if seen_general.insert(rule.id.clone()) {
                general.push(rule_hit(rule, instance, attachment));
            }
        }
    }

    (specific, general)
}

fn rule_hit(rule: &Rule, instance: &ConstructInstance, attachment: Attachment) -> RuleHit {
    RuleHit {
        rule_id: rule.id.clone(),
        title: rule.title().to_string(),
        kind: rule.kind.clone(),
        severity: rule.severity.clone(),
        body_markdown: rule.body_markdown.clone(),
        source_path: rule.source_path.display().to_string(),
        attached_to: instance.kind.clone(),
        attachment,
        attached_text: instance.text.clone(),
    }
}

/// Walk the ancestor stack and collect every lesson attached to a kind on it.
/// Lessons have no predicates — every lesson attached to a kind on the stack
/// fires. Deduplicate by lesson id so a lesson attached to multiple kinds
/// (e.g. one targeting both `method_declaration` and `class_declaration`)
/// renders once per hover even if the cursor sits inside both ancestors.
fn evaluate_lessons(
    educator: &Educator,
    language: &str,
    stack: &[ConstructInstance],
) -> Vec<LessonHit> {
    let mut hits = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (depth, instance) in stack.iter().enumerate() {
        let attachment = if depth == 0 {
            Attachment::Direct
        } else {
            Attachment::Ancestor
        };
        for lesson in educator.lessons_for(language, &instance.kind) {
            if seen.insert(lesson.id.clone()) {
                hits.push(lesson_hit(lesson, instance, attachment));
            }
        }
    }
    hits
}

fn lesson_hit(lesson: &Lesson, instance: &ConstructInstance, attachment: Attachment) -> LessonHit {
    LessonHit {
        lesson_id: lesson.id.clone(),
        title: lesson.title.clone(),
        level: lesson.level.clone(),
        body_markdown: lesson.body_markdown.clone(),
        source_path: lesson.source_path.display().to_string(),
        attached_to: instance.kind.clone(),
        attachment,
        attached_text: instance.text.clone(),
    }
}
