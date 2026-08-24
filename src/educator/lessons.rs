//! Lesson loading — parses `content/<lang>/lessons/*.md` into [`Lesson`] structs.
//!
//! Lessons are the teaching counterpart to rules: neutral explanations of
//! language syntax that fire unconditionally when the cursor is inside one of
//! the construct kinds the lesson declares. Unlike rules they have no
//! `match:` predicate, no severity, and no problem-taxonomy kind — they're
//! always-firing educational content.
//!
//! Wire format mirrors rules (YAML frontmatter + markdown body) so authors who
//! know the rule format can write lessons without learning a second schema.
//! Differences from a rule's frontmatter:
//!   - `match:` is forbidden (lessons always fire).
//!   - `severity` / `kind` are absent (lessons aren't problems).
//!   - `title` is required (lessons make the heading explicit so the loader
//!     doesn't have to parse the body to find a sidebar label).
//!   - `level` is required: `beginner | intermediate | advanced`.

use super::content_files::{split_frontmatter, walk_markdown_files};
use super::issues::{LoadIssue, LoadIssueSeverity};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Frontmatter {
    id: String,
    language: String,
    #[serde(rename = "applies-to")]
    applies_to: Vec<String>,
    title: String,
    level: String,
    #[serde(default)]
    sources: Vec<String>,
}

/// A loaded Educator Lesson.
#[derive(Debug, Clone)]
pub struct Lesson {
    pub id: String,
    pub language: String,
    pub applies_to: Vec<String>,
    pub title: String,
    pub level: String,
    pub sources: Vec<String>,
    pub body_markdown: String,
    pub source_path: PathBuf,
}

fn parse_one(path: &Path) -> Result<Lesson> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (frontmatter_str, body) = split_frontmatter(&raw)
        .with_context(|| format!("splitting frontmatter in {}", path.display()))?;
    let fm: Frontmatter = serde_yaml::from_str(frontmatter_str)
        .with_context(|| format!("parsing frontmatter YAML in {}", path.display()))?;

    Ok(Lesson {
        id: fm.id,
        language: fm.language,
        applies_to: fm.applies_to,
        title: fm.title,
        level: fm.level,
        sources: fm.sources,
        body_markdown: body.to_string(),
        source_path: path.to_path_buf(),
    })
}

/// Load every `*.md` lesson file under `content_root/<lang>/lessons/`.
/// Mirrors [`super::rules::load_all`] — same return shape, same error policy.
pub(super) fn load_all(content_root: &Path) -> Result<(Vec<Lesson>, Vec<LoadIssue>)> {
    let mut lessons = Vec::new();
    let mut issues = Vec::new();
    let mut langs = std::fs::read_dir(content_root)
        .with_context(|| format!("reading content root {}", content_root.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect::<Vec<_>>();
    langs.sort();
    for lang_dir in langs {
        let lessons_dir = lang_dir.join("lessons");
        if !lessons_dir.is_dir() {
            continue;
        }
        // Recurses into subdirectories so lessons can be organised into
        // semantic folders (`lessons/control-flow/`, `lessons/oop/`, …).
        let lesson_files = walk_markdown_files(&lessons_dir)
            .with_context(|| format!("walking lessons dir {}", lessons_dir.display()))?;
        for path in lesson_files {
            match parse_one(&path) {
                Ok(lesson) => lessons.push(lesson),
                Err(e) => {
                    issues.push(LoadIssue {
                        path: path.clone(),
                        rule_id: None,
                        severity: LoadIssueSeverity::Error,
                        field: None,
                        message: format!("{:#}", e),
                        suggestion: None,
                    });
                }
            }
        }
    }
    Ok((lessons, issues))
}
