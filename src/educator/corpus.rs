//! The loaded corpus — rules and lessons read off disk, indexed by
//! `(language, construct-kind)`, plus the issues the loaders and validators
//! raised on the way in.
//!
//! This file knows how to *build and hold* the corpus. It does not know how to
//! ask it questions about a cursor or a file — the query entry points live in
//! [`super`], which is what the rest of the codebase depends on.

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::issues::{LoadIssue, LoadIssueSeverity};
use super::lessons::{self, Lesson};
use super::rules::{self, Rule};
use super::validator;

/// Loaded Educator state — the rule corpus and the lesson corpus, each indexed
/// by `(language, construct-kind)`, plus the merged issue list from both
/// loaders + validators.
///
/// Loading happens once at server startup; the indexes are read-only afterwards.
pub struct Educator {
    rules: Vec<Rule>,
    /// Index: `(language, construct-kind) → rule indices in `rules`.
    by_kind: HashMap<(String, String), Vec<usize>>,
    lessons: Vec<Lesson>,
    /// Index: `(language, construct-kind) → lesson indices in `lessons`.
    lessons_by_kind: HashMap<(String, String), Vec<usize>>,
    content_root: Option<PathBuf>,
    issues: Vec<LoadIssue>,
}

impl Educator {
    /// Empty educator — used when no content root is configured.
    pub fn empty() -> Self {
        Self {
            rules: Vec::new(),
            by_kind: HashMap::new(),
            lessons: Vec::new(),
            lessons_by_kind: HashMap::new(),
            content_root: None,
            issues: Vec::new(),
        }
    }

    /// Load both rules (`content/<lang>/rules/`) and lessons
    /// (`content/<lang>/lessons/`) under `content_root`. Returns an empty
    /// educator (no error) when the directory does not exist — the educator is
    /// optional and a missing content tree should not break the server.
    /// Items with **errors** (per the validator) are dropped from the index;
    /// items with **warnings** are kept. The full issue list (rules + lessons)
    /// is exposed via [`Educator::issues`] and `/api/educator/diagnostics`.
    pub fn load(content_root: &Path) -> Result<Self> {
        if !content_root.exists() {
            return Ok(Self::empty());
        }
        let (rule_candidates, rule_parse_issues) = rules::load_all(content_root)?;
        let (rules, rule_issues) = validator::validate(rule_candidates, rule_parse_issues);

        let (lesson_candidates, lesson_parse_issues) = lessons::load_all(content_root)?;
        let (lessons, lesson_issues) =
            validator::validate_lessons(lesson_candidates, lesson_parse_issues);

        let mut issues = rule_issues;
        issues.extend(lesson_issues);

        // Surface the issue list to the operator's terminal at startup so a
        // broken rule or lesson is impossible to miss when running `nao watch`.
        log_issues(&issues);

        let mut by_kind: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (idx, rule) in rules.iter().enumerate() {
            for kind in &rule.applies_to {
                by_kind
                    .entry((rule.language.clone(), kind.clone()))
                    .or_default()
                    .push(idx);
            }
        }
        let mut lessons_by_kind: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (idx, lesson) in lessons.iter().enumerate() {
            for kind in &lesson.applies_to {
                lessons_by_kind
                    .entry((lesson.language.clone(), kind.clone()))
                    .or_default()
                    .push(idx);
            }
        }
        Ok(Self {
            rules,
            by_kind,
            lessons,
            lessons_by_kind,
            content_root: Some(content_root.to_path_buf()),
            issues,
        })
    }

    /// Resolve the content root in priority order:
    /// 1. `NAO_EDUCATOR_CONTENT` env var, if set.
    /// 2. `<workspace>/content/` — content checked in alongside the project being analyzed.
    ///
    /// Returns `None` if neither resolves to an existing directory.
    pub fn resolve_content_root(workspace: &Path) -> Option<PathBuf> {
        if let Ok(path) = std::env::var("NAO_EDUCATOR_CONTENT") {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        let p = workspace.join("content");
        if p.exists() {
            return Some(p);
        }
        None
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn lessons(&self) -> &[Lesson] {
        &self.lessons
    }

    pub fn issues(&self) -> &[LoadIssue] {
        &self.issues
    }

    pub fn content_root(&self) -> Option<&Path> {
        self.content_root.as_deref()
    }

    /// Look up the rules attached to a given `(language, construct-kind)`.
    pub fn rules_for(&self, language: &str, kind: &str) -> Vec<&Rule> {
        self.by_kind
            .get(&(language.to_string(), kind.to_string()))
            .map(|indices| indices.iter().map(|&i| &self.rules[i]).collect())
            .unwrap_or_default()
    }

    /// Look up the lessons attached to a given `(language, construct-kind)`.
    pub fn lessons_for(&self, language: &str, kind: &str) -> Vec<&Lesson> {
        self.lessons_by_kind
            .get(&(language.to_string(), kind.to_string()))
            .map(|indices| indices.iter().map(|&i| &self.lessons[i]).collect())
            .unwrap_or_default()
    }
}

fn log_issues(issues: &[LoadIssue]) {
    if issues.is_empty() {
        return;
    }
    let errors = issues
        .iter()
        .filter(|i| i.severity == LoadIssueSeverity::Error)
        .count();
    let warnings = issues.len() - errors;
    eprintln!(
        "⚠ Educator: {} error(s), {} warning(s) during rule load:",
        errors, warnings
    );
    for issue in issues {
        let prefix = match issue.severity {
            LoadIssueSeverity::Error => "  ✗",
            LoadIssueSeverity::Warning => "  ⚠",
        };
        let field = issue
            .field
            .as_deref()
            .map(|f| format!(" [{}]", f))
            .unwrap_or_default();
        eprintln!(
            "{} {}{}: {}",
            prefix,
            issue.path.display(),
            field,
            issue.message
        );
        if let Some(s) = &issue.suggestion {
            eprintln!("    {}", s);
        }
    }
}
