//! Rule validation. Runs after [`super::rules::load_all`] has produced its
//! `(rules, parse_issues)` pair and adds the post-load checks: known
//! construct-kinds, known attribute names, valid severity/kind enumerations,
//! `id` uniqueness, presence of a `## Why` body section.
//!
//! Errors drop the rule from the returned set; warnings keep it. The
//! HTTP `/api/educator/diagnostics` endpoint exposes the full issue list.

use super::catalog::{self, AttributeSpec, ConstructKindSpec};
use super::lessons::Lesson;
use super::rules::{LoadIssue, LoadIssueSeverity, MatchOp, Rule};
use std::collections::{HashMap, HashSet};

const ALLOWED_SEVERITIES: &[&str] = &["info", "warning", "error"];
const ALLOWED_KINDS: &[&str] = &["style", "gotcha", "naming", "structure"];
const ALLOWED_LESSON_LEVELS: &[&str] = &["beginner", "intermediate", "advanced"];

/// Run post-load validation. Returns the rules that survived plus the issue
/// list. The `parse_issues` argument is appended to the returned issues —
/// callers should not have to glue them back together themselves.
pub fn validate(
    candidates: Vec<Rule>,
    mut parse_issues: Vec<LoadIssue>,
) -> (Vec<Rule>, Vec<LoadIssue>) {
    let mut id_to_paths: HashMap<String, Vec<std::path::PathBuf>> = HashMap::new();
    for rule in &candidates {
        id_to_paths
            .entry(rule.id.clone())
            .or_default()
            .push(rule.source_path.clone());
    }

    let duplicate_ids: HashSet<String> = id_to_paths
        .iter()
        .filter_map(|(id, paths)| if paths.len() > 1 { Some(id.clone()) } else { None })
        .collect();

    let mut kept: Vec<Rule> = Vec::new();
    for rule in candidates {
        let mut errors: Vec<LoadIssue> = Vec::new();
        let mut warnings: Vec<LoadIssue> = Vec::new();

        if duplicate_ids.contains(&rule.id) {
            errors.push(LoadIssue {
                path: rule.source_path.clone(),
                rule_id: Some(rule.id.clone()),
                severity: LoadIssueSeverity::Error,
                field: Some("id".to_string()),
                message: format!("duplicate rule id `{}`", rule.id),
                suggestion: None,
            });
        }

        validate_applies_to(&rule, &mut errors);
        validate_match(&rule, &mut errors);
        validate_severity(&rule, &mut errors, &mut warnings);
        validate_kind(&rule, &mut warnings);
        validate_body(&rule, &mut warnings);

        let drop_rule = !errors.is_empty();
        parse_issues.extend(errors);
        parse_issues.extend(warnings);
        if !drop_rule {
            kept.push(rule);
        }
    }

    (kept, parse_issues)
}

fn validate_applies_to(rule: &Rule, errors: &mut Vec<LoadIssue>) {
    let specs = catalog::for_language(&rule.language);
    if specs.is_none() {
        errors.push(LoadIssue {
            path: rule.source_path.clone(),
            rule_id: Some(rule.id.clone()),
            severity: LoadIssueSeverity::Error,
            field: Some("language".to_string()),
            message: format!("no Educator catalog registered for language `{}`", rule.language),
            suggestion: None,
        });
        return;
    }
    let specs = specs.unwrap();
    let known: Vec<&str> = specs.iter().map(|s| s.kind).collect();
    for kind in &rule.applies_to {
        if !known.iter().any(|k| k == kind) {
            errors.push(LoadIssue {
                path: rule.source_path.clone(),
                rule_id: Some(rule.id.clone()),
                severity: LoadIssueSeverity::Error,
                field: Some("applies-to".to_string()),
                message: format!("unknown construct-kind `{}`", kind),
                suggestion: nearest(kind, &known),
            });
        }
    }
}

fn validate_match(rule: &Rule, errors: &mut Vec<LoadIssue>) {
    let Some(predicate) = rule.match_predicate.as_ref() else { return };
    if predicate.is_empty() {
        return;
    }
    let Some(specs) = catalog::for_language(&rule.language) else { return };

    // Union of attributes across every applies-to kind the rule names.
    // A `match:` key is OK if any of the attached kinds declares it.
    let known_attrs: HashSet<&str> = rule
        .applies_to
        .iter()
        .filter_map(|kind| catalog::find(specs, kind))
        .flat_map(|spec: &ConstructKindSpec| spec.attributes.iter().map(|a: &AttributeSpec| a.name))
        .collect();

    for (key, _op) in predicate {
        if !known_attrs.contains(key.as_str()) {
            let known_list: Vec<&str> = known_attrs.iter().copied().collect();
            errors.push(LoadIssue {
                path: rule.source_path.clone(),
                rule_id: Some(rule.id.clone()),
                severity: LoadIssueSeverity::Error,
                field: Some(format!("match.{}", key)),
                message: format!(
                    "attribute `{}` is not declared on any of the rule's applies-to kinds ({})",
                    key,
                    rule.applies_to.join(", ")
                ),
                suggestion: nearest(key, &known_list),
            });
        }
        // Note: the primitive (eq/in/absent/present) was validated at parse
        // time by `MatchOp::from_yaml` — unknown primitives produce a parse
        // error and the rule never reaches here.
        let _ = _op;
    }
}

fn validate_severity(rule: &Rule, errors: &mut Vec<LoadIssue>, warnings: &mut Vec<LoadIssue>) {
    if !ALLOWED_SEVERITIES.contains(&rule.severity.as_str()) {
        warnings.push(LoadIssue {
            path: rule.source_path.clone(),
            rule_id: Some(rule.id.clone()),
            severity: LoadIssueSeverity::Warning,
            field: Some("severity".to_string()),
            message: format!(
                "unknown severity `{}` (allowed: info, warning, error) — keeping rule but the hover renderer may not style it as you expect",
                rule.severity
            ),
            suggestion: nearest(&rule.severity, ALLOWED_SEVERITIES),
        });
    }
    let _ = errors;
}

fn validate_kind(rule: &Rule, warnings: &mut Vec<LoadIssue>) {
    if !ALLOWED_KINDS.contains(&rule.kind.as_str()) {
        warnings.push(LoadIssue {
            path: rule.source_path.clone(),
            rule_id: Some(rule.id.clone()),
            severity: LoadIssueSeverity::Warning,
            field: Some("kind".to_string()),
            message: format!(
                "unfamiliar kind `{}` (typical: style, gotcha, naming, structure) — keeping rule, but please reconsider the taxonomy bucket",
                rule.kind
            ),
            suggestion: nearest(&rule.kind, ALLOWED_KINDS),
        });
    }
}

fn validate_body(rule: &Rule, warnings: &mut Vec<LoadIssue>) {
    let has_why = rule
        .body_markdown
        .lines()
        .any(|l| l.trim_start().starts_with("## Why"));
    if !has_why {
        warnings.push(LoadIssue {
            path: rule.source_path.clone(),
            rule_id: Some(rule.id.clone()),
            severity: LoadIssueSeverity::Warning,
            field: Some("body".to_string()),
            message: "rule body is missing a `## Why` section — users learn from the rationale, not the example".to_string(),
            suggestion: None,
        });
    }
}

/// Validate lessons after [`super::lessons::load_all`]. Same shape as
/// [`validate`]: returns the lessons that survived plus a merged issue list
/// (parse-time errors + validator errors/warnings). Lessons have a simpler
/// schema than rules (no predicate, no severity, no kind), so the validation
/// is correspondingly smaller — known construct kinds, valid `level` value,
/// `id` uniqueness, non-empty body.
pub fn validate_lessons(
    candidates: Vec<Lesson>,
    mut parse_issues: Vec<LoadIssue>,
) -> (Vec<Lesson>, Vec<LoadIssue>) {
    let mut id_to_paths: HashMap<String, Vec<std::path::PathBuf>> = HashMap::new();
    for lesson in &candidates {
        id_to_paths
            .entry(lesson.id.clone())
            .or_default()
            .push(lesson.source_path.clone());
    }
    let duplicate_ids: HashSet<String> = id_to_paths
        .iter()
        .filter_map(|(id, paths)| if paths.len() > 1 { Some(id.clone()) } else { None })
        .collect();

    let mut kept: Vec<Lesson> = Vec::new();
    for lesson in candidates {
        let mut errors: Vec<LoadIssue> = Vec::new();
        let mut warnings: Vec<LoadIssue> = Vec::new();

        if duplicate_ids.contains(&lesson.id) {
            errors.push(LoadIssue {
                path: lesson.source_path.clone(),
                rule_id: Some(lesson.id.clone()),
                severity: LoadIssueSeverity::Error,
                field: Some("id".to_string()),
                message: format!("duplicate lesson id `{}`", lesson.id),
                suggestion: None,
            });
        }

        validate_lesson_applies_to(&lesson, &mut errors);
        validate_lesson_level(&lesson, &mut warnings);
        validate_lesson_body(&lesson, &mut warnings);

        let drop_lesson = !errors.is_empty();
        parse_issues.extend(errors);
        parse_issues.extend(warnings);
        if !drop_lesson {
            kept.push(lesson);
        }
    }

    (kept, parse_issues)
}

fn validate_lesson_applies_to(lesson: &Lesson, errors: &mut Vec<LoadIssue>) {
    let specs = catalog::for_language(&lesson.language);
    if specs.is_none() {
        errors.push(LoadIssue {
            path: lesson.source_path.clone(),
            rule_id: Some(lesson.id.clone()),
            severity: LoadIssueSeverity::Error,
            field: Some("language".to_string()),
            message: format!("no Educator catalog registered for language `{}`", lesson.language),
            suggestion: None,
        });
        return;
    }
    let specs = specs.unwrap();
    let known: Vec<&str> = specs.iter().map(|s| s.kind).collect();
    for kind in &lesson.applies_to {
        if !known.iter().any(|k| k == kind) {
            errors.push(LoadIssue {
                path: lesson.source_path.clone(),
                rule_id: Some(lesson.id.clone()),
                severity: LoadIssueSeverity::Error,
                field: Some("applies-to".to_string()),
                message: format!("unknown construct-kind `{}`", kind),
                suggestion: nearest(kind, &known),
            });
        }
    }
}

fn validate_lesson_level(lesson: &Lesson, warnings: &mut Vec<LoadIssue>) {
    if !ALLOWED_LESSON_LEVELS.contains(&lesson.level.as_str()) {
        warnings.push(LoadIssue {
            path: lesson.source_path.clone(),
            rule_id: Some(lesson.id.clone()),
            severity: LoadIssueSeverity::Warning,
            field: Some("level".to_string()),
            message: format!(
                "unknown level `{}` (allowed: beginner, intermediate, advanced) — keeping lesson but the future level-filter setting will not match it",
                lesson.level
            ),
            suggestion: nearest(&lesson.level, ALLOWED_LESSON_LEVELS),
        });
    }
}

fn validate_lesson_body(lesson: &Lesson, warnings: &mut Vec<LoadIssue>) {
    if lesson.body_markdown.trim().is_empty() {
        warnings.push(LoadIssue {
            path: lesson.source_path.clone(),
            rule_id: Some(lesson.id.clone()),
            severity: LoadIssueSeverity::Warning,
            field: Some("body".to_string()),
            message: "lesson body is empty — learners need at least a short explanation".to_string(),
            suggestion: None,
        });
    }
}

/// Find the closest match in `candidates` by Levenshtein distance (≤ 3). Used
/// for "did you mean…" suggestions on typo'd kind/attribute names.
fn nearest(input: &str, candidates: &[&str]) -> Option<String> {
    let mut best: Option<(&str, usize)> = None;
    for cand in candidates {
        let d = levenshtein(input, cand);
        if d <= 3 && best.map(|(_, b)| d < b).unwrap_or(true) {
            best = Some((cand, d));
        }
    }
    best.map(|(s, _)| format!("did you mean `{}`?", s))
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1)
                .min(curr[j - 1] + 1)
                .min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}
