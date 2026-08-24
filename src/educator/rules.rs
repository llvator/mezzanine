//! Rule loading — parses `content/<lang>/rules/*.md` files into [`Rule`] structs.
//!
//! File format: YAML frontmatter delimited by `---` lines, followed by a
//! markdown body. Frontmatter declares `id`, `language`, `applies-to`, optional
//! `match:`, `severity`, `kind`. The body holds the user-visible content
//! (typically a `# Title`, then `## Good` / `## Bad` / `## Why` sections).
//!
//! Only the rule lives here. The frontmatter split is [`super::content_files`]'s
//! (both content kinds share it), the `match:` primitives are
//! [`super::predicate`]'s, and the complaint type is [`super::issues`]'s.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::content_files::{split_frontmatter, walk_markdown_files};
use super::issues::{LoadIssue, LoadIssueSeverity};
use super::predicate::MatchOp;

/// Frontmatter shape, deserialized from the YAML header of a rule file.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    id: String,
    language: String,
    #[serde(rename = "applies-to")]
    applies_to: Vec<String>,
    #[serde(default)]
    r#match: Option<HashMap<String, serde_yaml::Value>>,
    #[serde(default = "default_severity")]
    severity: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    sources: Vec<String>,
}

fn default_severity() -> String {
    "info".to_string()
}

fn default_kind() -> String {
    "gotcha".to_string()
}

/// A loaded Educator Rule. See `CONTEXT.md` "Educator Rule" for the conceptual definition.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub language: String,
    pub applies_to: Vec<String>,
    /// Optional match predicate. Empty `HashMap` is treated identically to `None`
    /// (predicate-less rules always fire on attachment → General bucket).
    pub match_predicate: Option<HashMap<String, MatchOp>>,
    pub severity: String,
    pub kind: String,
    pub sources: Vec<String>,
    pub body_markdown: String,
    pub source_path: PathBuf,
}

impl Rule {
    /// True when this rule has a `match:` predicate (→ Specific bucket on a hit).
    pub fn is_specific(&self) -> bool {
        self.match_predicate.as_ref().is_some_and(|p| !p.is_empty())
    }

    /// Extract the first `# Title` line from the body, falling back to `id` if absent.
    pub fn title(&self) -> &str {
        for line in self.body_markdown.lines() {
            let line = line.trim_start();
            if let Some(rest) = line.strip_prefix("# ") {
                return rest.trim();
            }
        }
        &self.id
    }
}

fn parse_one(path: &Path) -> Result<Rule> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (frontmatter_str, body) = split_frontmatter(&raw)
        .with_context(|| format!("splitting frontmatter in {}", path.display()))?;
    let fm: Frontmatter = serde_yaml::from_str(frontmatter_str)
        .with_context(|| format!("parsing frontmatter YAML in {}", path.display()))?;

    let match_predicate = if let Some(raw) = fm.r#match {
        let mut compiled = HashMap::new();
        for (k, v) in raw {
            let op = MatchOp::from_yaml(&v)
                .with_context(|| format!("compiling `match.{}` in {}", k, path.display()))?;
            compiled.insert(k, op);
        }
        if compiled.is_empty() {
            None
        } else {
            Some(compiled)
        }
    } else {
        None
    };

    Ok(Rule {
        id: fm.id,
        language: fm.language,
        applies_to: fm.applies_to,
        match_predicate,
        severity: fm.severity,
        kind: fm.kind,
        sources: fm.sources,
        body_markdown: body.to_string(),
        source_path: path.to_path_buf(),
    })
}

/// Load every `*.md` rule file under `content_root/<lang>/rules/`. Returns
/// both the parsed rules and the parse-time issues (one issue per file that
/// failed to parse). The validator runs the post-load checks separately.
///
/// The traversal recurses into any subdirectories of `rules/` — rule
/// authors can organise files into semantic folders
/// (`rules/control-flow/`, `rules/exceptions/`, …) without changing the
/// loader. File order within the returned `Vec` is stable across runs by
/// sorting the full path of every discovered file.
pub(super) fn load_all(content_root: &Path) -> Result<(Vec<Rule>, Vec<LoadIssue>)> {
    let mut rules = Vec::new();
    let mut issues = Vec::new();
    let mut langs = std::fs::read_dir(content_root)
        .with_context(|| format!("reading content root {}", content_root.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect::<Vec<_>>();
    langs.sort();
    for lang_dir in langs {
        let rules_dir = lang_dir.join("rules");
        if !rules_dir.is_dir() {
            continue;
        }
        let rule_files = walk_markdown_files(&rules_dir)
            .with_context(|| format!("walking rules dir {}", rules_dir.display()))?;
        for path in rule_files {
            match parse_one(&path) {
                Ok(rule) => rules.push(rule),
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
    Ok((rules, issues))
}
