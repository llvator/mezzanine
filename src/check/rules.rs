//! The rules a project declares, and the file it declares them in.
//!
//! Everything here is read from `<repo>/.nao/rules.json` and nothing is
//! defaulted: a repo with no file has no rules, and `check` has nothing to
//! fail on ([ADR 0024](../../../docs/adr/0024-a-check-fails-on-the-projects-rules-not-naos.md),
//! decision 1). The bar in `max_entities_per_file: 7` is the project's — nao
//! supplies the measurement and never the number.
//!
//! The reading is deliberately stricter than [`crate::settings`]. A bad
//! settings file warns and is ignored, because it must never fail a command
//! that would otherwise have worked. A bad rules file is fatal, because the
//! whole output of `check` is a verdict, and a verdict computed from a file
//! nao misread is worse than no verdict (ADR 0024, decision 4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glob::Pattern;
use serde_json::Value;

use crate::settings;

/// The rules file, beside `settings.json` in the same repo-scope directory.
///
/// A separate file rather than a `rules` key in the settings file: settings
/// answer *what to analyze* and merge across scopes, rules *assert* and may
/// not, or one developer's machine would fail a build another's passes
/// (ADR 0024, decision 2).
pub const FILE_NAME: &str = "rules.json";

/// A rule this build can evaluate.
///
/// Sorted by the name a reader writes, so a report listing several rules
/// reads in the order they would look them up.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// Files outside a folder land on, ties included — the **Door** of
    /// [CONTEXT.md](../../../CONTEXT.md), counted rather than turned into
    /// the `entry_concentration` ratio.
    MaxDoorsPerFolder,
    /// Members of a class-like entity; parameters of a callable.
    MaxElementsPerEntity,
    /// Declarations a reader meets on opening the file — everything the MCP
    /// listings would show, minus what a class-like entity owns, since those
    /// are that entity's elements and counted by the rule above.
    MaxEntitiesPerFile,
    /// Other files depending on this one, counted against the file that
    /// *declares* what they use rather than the one they named.
    MaxImportersPerFile,
}

/// Every rule name this build understands, in report order. Named here
/// rather than derived, so the list a diagnostic prints is the list the
/// parser accepts.
pub(super) const KNOWN: &[Rule] = &[
    Rule::MaxDoorsPerFolder,
    Rule::MaxElementsPerEntity,
    Rule::MaxEntitiesPerFile,
    Rule::MaxImportersPerFile,
];

impl Rule {
    /// The key a project writes in the file. The one spelling of this rule
    /// anywhere: the parser, the human report and the JSON all use it.
    pub fn name(self) -> &'static str {
        match self {
            Rule::MaxDoorsPerFolder => "max_doors_per_folder",
            Rule::MaxElementsPerEntity => "max_elements_per_entity",
            Rule::MaxEntitiesPerFile => "max_entities_per_file",
            Rule::MaxImportersPerFile => "max_importers_per_file",
        }
    }

    /// How one breach of this rule reads. `subject` is the entity that broke
    /// it, absent when the subject is the path itself.
    ///
    /// The count only. What the count is *of* — which importers, which doors
    /// — is a list the report prints beside this, because a rule an author
    /// can act on has to name the places, and a phrase that carried them
    /// would be a sentence with a list inside it.
    pub fn phrase(self, measured: u32, subject: Option<&str>) -> String {
        match self {
            Rule::MaxDoorsPerFolder => super::tally(measured as usize, "door"),
            Rule::MaxElementsPerEntity => match subject {
                Some(name) => format!("{name} has {measured} elements"),
                None => format!("{measured} elements"),
            },
            Rule::MaxEntitiesPerFile => format!("{measured} declared"),
            Rule::MaxImportersPerFile => super::tally(measured as usize, "importer"),
        }
    }

    fn parse(name: &str) -> Option<Rule> {
        KNOWN.iter().copied().find(|rule| rule.name() == name)
    }
}

/// What one repo declared: a bar per rule, and the files none of them reach.
pub struct Rules {
    /// Bars by rule. A `BTreeMap` so the report is byte-stable whatever
    /// order the file listed them in.
    bars: BTreeMap<Rule, u32>,
    exempt: Vec<Pattern>,
    /// The file this came from. Every diagnostic names it, because "unknown
    /// rule" is only actionable next to the path holding the typo.
    pub path: PathBuf,
}

impl Rules {
    /// The bar declared for one rule, or `None` when the project did not
    /// declare it. An undeclared rule is not a rule with a default — it is a
    /// rule that does not exist here.
    pub fn bar(&self, rule: Rule) -> Option<u32> {
        self.bars.get(&rule).copied()
    }

    /// Every declared rule, in report order.
    pub fn declared(&self) -> impl Iterator<Item = (Rule, u32)> + '_ {
        self.bars.iter().map(|(rule, bar)| (*rule, *bar))
    }

    /// True when the file parsed but asserted nothing. Reported as "no rules
    /// declared" rather than as a pass: an empty `rules` object is a page
    /// somebody left blank, and silence would read as a clean tree.
    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    /// Whether `path` — repo-relative, forward slashes, one spelling
    /// whatever directory `check` was pointed at — is exempt from every
    /// rule.
    pub fn is_exempt(&self, path: &str) -> bool {
        self.exempt.iter().any(|glob| glob.matches(path))
    }
}

/// Where the rules file for an analyzed path lives, whether or not it
/// exists. The same repo-scope directory the settings file uses, so
/// `nao check src` and `nao check .` read one file (CFG-012).
pub fn path_for(root: &Path) -> PathBuf {
    settings::repo_dir(root).join(FILE_NAME)
}

/// Read the rules file for an analyzed path.
///
/// `Ok(None)` is the repo that declared nothing, which is the common case
/// and passes. `Err` is a file that exists and could not be used — every
/// message names the file and the key, because the reader's next action is
/// to open it at that key.
pub fn load(root: &Path) -> Result<Option<Rules>, String> {
    let path = path_for(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(at(&path, format!("{e}"))),
    };
    parse(&path, &text).map(Some)
}

/// Parse one rules file. Split from [`load`] so the tests can hand it a
/// body without a filesystem.
pub(super) fn parse(path: &Path, text: &str) -> Result<Rules, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| at(path, format!("{e}")))?;
    let Value::Object(body) = value else {
        return Err(at(path, "the file is not a JSON object".to_string()));
    };
    // An unknown *top-level* key is fatal for the same reason an unknown
    // rule is: `exempts` is a plausible typo, and a gate that silently
    // ignored it would grade files the author believed were exempt.
    if let Some(key) = body
        .keys()
        .find(|k| !matches!(k.as_str(), "rules" | "exempt"))
    {
        return Err(at(
            path,
            format!("unknown key `{key}` — a rules file holds `rules` and `exempt`"),
        ));
    }
    Ok(Rules {
        bars: parse_bars(path, body.get("rules"))?,
        exempt: parse_exempt(path, body.get("exempt"))?,
        path: path.to_path_buf(),
    })
}

/// The `rules` object: a bar per rule name, every name known and every value
/// a whole number of things.
fn parse_bars(path: &Path, value: Option<&Value>) -> Result<BTreeMap<Rule, u32>, String> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let Some(declared) = value.as_object() else {
        return Err(at(
            path,
            "`rules` must be an object of rule name to number".to_string(),
        ));
    };
    let mut bars = BTreeMap::new();
    for (name, bar) in declared {
        let rule = Rule::parse(name).ok_or_else(|| {
            at(
                path,
                format!("unknown rule `{name}` — this build knows {}", known_names()),
            )
        })?;
        let bar = bar
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| at(path, format!("`{name}`: {bar} is not a whole number")))?;
        bars.insert(rule, bar);
    }
    Ok(bars)
}

/// The `exempt` list: globs, compiled here so an uncompilable one is an
/// error at the file rather than a pattern that quietly matches nothing.
fn parse_exempt(path: &Path, value: Option<&Value>) -> Result<Vec<Pattern>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(entries) = value.as_array() else {
        return Err(at(path, "`exempt` must be an array of globs".to_string()));
    };
    entries
        .iter()
        .map(|entry| {
            let spelling = entry
                .as_str()
                .ok_or_else(|| at(path, format!("`exempt`: {entry} is not a glob string")))?;
            Pattern::new(spelling).map_err(|e| {
                at(
                    path,
                    format!("`exempt`: `{spelling}` is not a valid glob ({e})"),
                )
            })
        })
        .collect()
}

/// Every diagnostic from this module names the file first: the reader's next
/// action is to open it.
fn at(path: &Path, message: String) -> String {
    format!("{}: {message}", super::tidy(path))
}

fn known_names() -> String {
    KNOWN
        .iter()
        .map(|rule| rule.name())
        .collect::<Vec<_>>()
        .join(", ")
}
