//! Rule loading — parses `content/<lang>/rules/*.md` files into [`Rule`] structs.
//!
//! File format: YAML frontmatter delimited by `---` lines, followed by a
//! markdown body. Frontmatter declares `id`, `language`, `applies-to`, optional
//! `match:`, `severity`, `kind`. The body holds the user-visible content
//! (typically a `# Title`, then `## Good` / `## Bad` / `## Why` sections).

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// A single predicate primitive applied to one attribute. Per ADR-0002, this set
/// is intentionally small — extend by adding parser-emitted attributes, not by
/// adding code-handler escape hatches.
#[derive(Debug, Clone)]
pub enum MatchOp {
    Eq(String),
    In(Vec<String>),
    Absent,
    Present,
}

/// A single load-time issue surfaced by the loader or validator. Errors drop
/// the rule from the index; warnings keep the rule but log a diagnostic.
#[derive(Debug, Clone, Serialize)]
pub struct LoadIssue {
    pub path: PathBuf,
    pub rule_id: Option<String>,
    pub severity: LoadIssueSeverity,
    pub field: Option<String>,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoadIssueSeverity {
    Error,
    Warning,
}

impl MatchOp {
    fn from_yaml(v: &serde_yaml::Value) -> Result<Self> {
        match v {
            serde_yaml::Value::String(s) => Ok(MatchOp::Eq(s.clone())),
            serde_yaml::Value::Mapping(m) => {
                let mut keys: Vec<String> = m
                    .keys()
                    .filter_map(|k| k.as_str().map(|s| s.to_string()))
                    .collect();
                if keys.len() != 1 {
                    return Err(anyhow!(
                        "match operator must be a single-key mapping, got keys: {:?}",
                        keys
                    ));
                }
                let key = keys.remove(0);
                let val = m
                    .get(serde_yaml::Value::String(key.clone()))
                    .ok_or_else(|| anyhow!("missing value for operator {}", key))?;
                match key.as_str() {
                    "eq" => val
                        .as_str()
                        .map(|s| MatchOp::Eq(s.to_string()))
                        .ok_or_else(|| anyhow!("`eq` value must be a string")),
                    "in" => val
                        .as_sequence()
                        .map(|seq| {
                            MatchOp::In(
                                seq.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect(),
                            )
                        })
                        .ok_or_else(|| anyhow!("`in` value must be a sequence")),
                    "absent" => val
                        .as_bool()
                        .filter(|b| *b)
                        .map(|_| MatchOp::Absent)
                        .ok_or_else(|| anyhow!("`absent: true` is the only accepted form")),
                    "present" => val
                        .as_bool()
                        .filter(|b| *b)
                        .map(|_| MatchOp::Present)
                        .ok_or_else(|| anyhow!("`present: true` is the only accepted form")),
                    other => Err(anyhow!(
                        "unknown predicate primitive `{}` (ADR-0002: no Rust-handler escape hatch; \
                         allowed: eq, in, absent, present)",
                        other
                    )),
                }
            }
            _ => Err(anyhow!(
                "match value must be a string (shorthand for `eq`) or a single-key mapping"
            )),
        }
    }
}

/// Split a `---`-delimited frontmatter from the body. Visible to sibling
/// modules (lessons) so the shared file format stays one implementation.
pub(super) fn split_frontmatter(raw: &str) -> Result<(&str, &str)> {
    let raw = raw.trim_start_matches('\u{feff}');
    let raw = raw.trim_start_matches('\n');
    let rest = raw
        .strip_prefix("---")
        .ok_or_else(|| anyhow!("file must start with `---` frontmatter delimiter"))?;
    let rest = rest.trim_start_matches('\n');
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow!("frontmatter must end with `---` on its own line"))?;
    let frontmatter = &rest[..end];
    let after = &rest[end + 4..];
    let body = after.trim_start_matches('\n');
    Ok((frontmatter, body))
}

fn parse_one(path: &Path) -> Result<Rule> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let (frontmatter_str, body) = split_frontmatter(&raw)
        .with_context(|| format!("splitting frontmatter in {}", path.display()))?;
    let fm: Frontmatter = serde_yaml::from_str(frontmatter_str)
        .with_context(|| format!("parsing frontmatter YAML in {}", path.display()))?;

    let match_predicate = if let Some(raw) = fm.r#match {
        let mut compiled = HashMap::new();
        for (k, v) in raw {
            let op = MatchOp::from_yaml(&v).with_context(|| {
                format!("compiling `match.{}` in {}", k, path.display())
            })?;
            compiled.insert(k, op);
        }
        if compiled.is_empty() { None } else { Some(compiled) }
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
        let rule_files = super::scan::walk_markdown_files(&rules_dir)
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
