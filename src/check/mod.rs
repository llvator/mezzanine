//! `nao check`: grade a tree against the rules its project declared.
//!
//! Nao ships **no rules of its own**. Every failure this command can produce
//! traces to a line somebody in the checked repo wrote down, in
//! `<repo>/.nao/rules.json`, and a repo with no such file passes and says so
//! ([ADR 0024](../../docs/adr/0024-a-check-fails-on-the-projects-rules-not-naos.md)).
//!
//! That line is the whole design, and it holds because of *who chose the
//! bar*. Rules are expressed over nao's measurements, but the threshold is
//! the project's — arbitrary, arguable and theirs to argue over. When nao
//! calls a folder `tangled`, the tier is nao's, and ADRs 0014, 0017 and 0023
//! have each conceded that its gates can be wrong about a specific folder. A
//! tool may hand you a diagnosis you are free to disagree with; it may not
//! fail your build with one. So `min_shape: fractal` is not a rule anybody
//! can declare, and its absence is a decision rather than an omission.
//!
//! # Exit codes
//!
//! | code | meaning |
//! |---|---|
//! | 0 | every declared rule holds — or none was declared |
//! | 1 | at least one declared rule does not hold |
//! | 2 | no verdict could be computed |
//!
//! 2 covers the rules file that could not be used *and* the analysis that
//! could not be run, because both leave the caller without the one thing the
//! command exists to produce. A gate that passed because it did not
//! understand its own configuration is the failure this area exists to stop.

mod count;
mod report;
pub mod rules;
mod structure;
#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub use report::Format;
pub use rules::{Rule, Rules};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::FileInfo;
use crate::settings;

/// Every declared rule holds, or the repo declared none.
pub const EXIT_PASS: i32 = 0;
/// A declared rule does not hold.
pub const EXIT_VIOLATIONS: i32 = 1;
/// No verdict: the rules file, or the analysis, could not be used.
pub const EXIT_NO_VERDICT: i32 = 2;

/// One rule broken in one place.
///
/// Every field is what the report prints; nothing is recomputed downstream,
/// so the human line and the JSON object can never disagree about what was
/// measured.
pub struct Violation {
    /// The rule that does not hold here.
    pub rule: Rule,
    /// Where it broke: repo-relative, forward slashes, and a *folder* for a
    /// rule whose subject is one — `max_doors_per_folder` grades the folder,
    /// and calling the field `file` would have made three rules right and
    /// one a lie. See [`repo_relative`].
    pub path: String,
    /// 1-based, so it pairs with the path as `file:line`. `None` when the
    /// subject has no line: a folder is not a place in a file, and `:1`
    /// after one is a location no editor can open.
    pub line: Option<u32>,
    /// The entity that broke the rule; absent when the subject is the path.
    pub subject: Option<String>,
    /// What nao counted.
    pub measured: u32,
    /// The places behind the count, in path order — every importer, every
    /// door. Empty for the rules whose measurement is already a property of
    /// the subject.
    ///
    /// Named exhaustively rather than counted, because a count tells an
    /// author a file is entered twice and this tells them from where; the
    /// second is the one they can act on without re-deriving the first.
    pub names: Vec<Cited>,
    /// What the project declared. Printed beside the measurement, because a
    /// number without the bar it missed is not something a reader can act on.
    pub bar: u32,
}

/// One place behind a count: an importer of the file, a door of the folder.
///
/// Three fields rather than the line the report prints, for the reason the
/// edge site is a field on `Relationship` and not a `metadata` key: a
/// location a consumer has to re-parse out of a sentence is a location most
/// consumers will not use. The human line is assembled from these.
pub struct Cited {
    /// Repo-relative, forward slashes — the same spelling as
    /// [`Violation::path`]. A door is trimmed to the folder holding it only
    /// where it is printed, and only because the folder is on the same line.
    pub path: String,
    /// The line the importer wrote its statement on, 1-based. `None` when no
    /// statement created the edge — a call, a type reference, a specifier
    /// naming a package rather than a path — because a line that did not
    /// create it is worse than no line at all (AN-024).
    pub line: Option<u32>,
    /// The re-export this arrived through, when the importer never named the
    /// file it is being counted against. The `why` of a second importer, and
    /// the shape both these rules exist to catch.
    pub via: Option<String>,
}

/// What one run of `check` concluded.
pub enum Outcome {
    /// No rules file, or a file declaring nothing. Said out loud rather than
    /// passed in silence, which would read as a graded clean tree.
    NoRules { path: String },
    /// The tree was graded. Passing is `violations.is_empty()`.
    Checked {
        /// The file the rules came from, as a reader would type it.
        rules_file: String,
        /// What the project declared, in report order.
        declared: Vec<(Rule, u32)>,
        /// Every breach, in path order.
        violations: Vec<Violation>,
        /// Files graded — the walk minus the exempt ones.
        files_checked: usize,
        /// Files no rule was applied to.
        files_exempt: usize,
    },
    /// Nothing could be graded, and the message says why.
    Unusable { message: String },
}

impl Outcome {
    /// The process exit code this verdict deserves.
    pub fn exit_code(&self) -> i32 {
        match self {
            Outcome::NoRules { .. } => EXIT_PASS,
            Outcome::Checked { violations, .. } if violations.is_empty() => EXIT_PASS,
            Outcome::Checked { .. } => EXIT_VIOLATIONS,
            Outcome::Unusable { .. } => EXIT_NO_VERDICT,
        }
    }
}

/// Run the check over `path` and print the verdict. Returns the exit code
/// the caller should exit with.
pub fn run(path: &Path, format: Format) -> i32 {
    let outcome = evaluate(path);
    report::print(&outcome, format);
    // The caller exits the process on this code, which runs no destructors.
    let _ = std::io::stdout().flush();
    outcome.exit_code()
}

/// The verdict, with the tree configured the way every other command
/// configures it: the repo's settings file decides what is walked, and the
/// rules file decides only what is asserted about what was walked.
fn evaluate(path: &Path) -> Outcome {
    let mut config = Config::for_path(path);
    settings::load(path).apply_to_config(&mut config);
    verdict(path, config)
}

/// [`evaluate`] with the analysis configuration handed in, so a test can
/// grade a fixture without the developer's own settings file reaching it.
fn verdict(path: &Path, config: Config) -> Outcome {
    // Before anything else, because every other answer this command can give
    // about a path that is not there is a lie: an analysis of nothing has no
    // violations, and "0 violations" is how a mistyped path in a CI job comes
    // to read as a graded tree.
    if !path.exists() {
        return Outcome::Unusable {
            message: format!("{}: no such path", tidy(path)),
        };
    }
    let rules = match rules::load(path) {
        Ok(Some(rules)) => rules,
        Ok(None) => {
            return Outcome::NoRules {
                path: tidy(&rules::path_for(path)),
            }
        }
        Err(message) => return Outcome::Unusable { message },
    };
    if rules.is_empty() {
        return Outcome::NoRules {
            path: tidy(&rules.path),
        };
    }

    let result = match Analyzer::new(config).analyze() {
        Ok(result) => result,
        Err(e) => {
            return Outcome::Unusable {
                message: format!("{}: {e}", path.display()),
            }
        }
    };
    let repo_root = settings::repo_root(path);
    let (files_checked, files_exempt) = tally_files(&result.files, &rules, &repo_root);
    let graph = DependencyGraph::from_analysis(&result);
    Outcome::Checked {
        rules_file: tidy(&rules.path),
        declared: rules.declared().collect(),
        violations: violations(&graph, &rules, &repo_root),
        files_checked,
        files_exempt,
    }
}

/// Every breach of every declared rule, in path order (ADR 0024, decision 3).
///
/// Exhaustive on purpose, and the opposite of `reshape`'s one-blocker
/// contract: this answers "is the tree finished", and reporting one
/// violation per folder would make an author fix, re-run and discover the
/// next one N times.
///
/// Two counters, joined and sorted here rather than each sorting its own
/// list. They measure different things — [`count`] reads entities, and
/// [`structure`] reads the edges between files — but an author reads one
/// list and works down it, so the order is a property of the report and not
/// of either counter.
fn violations(graph: &DependencyGraph, rules: &Rules, repo_root: &Path) -> Vec<Violation> {
    let mut found = count::violations(graph, rules, repo_root);
    found.extend(structure::violations(graph, rules, repo_root));
    found.sort_by(|a, b| (&a.path, a.line, a.rule.name()).cmp(&(&b.path, b.line, b.rule.name())));
    found
}

/// How many files were graded and how many were let off.
///
/// An exempt file is counted apart rather than among the passes: a summary
/// that folded the two together would let a `**` in the exempt list read as
/// a clean tree.
fn tally_files(files: &[FileInfo], rules: &Rules, repo_root: &Path) -> (usize, usize) {
    let exempt = files
        .iter()
        .filter(|file| rules.is_exempt(&repo_relative(repo_root, &file.path)))
        .count();
    (files.len() - exempt, exempt)
}

/// The one spelling of a path this command uses: relative to the repo root,
/// forward slashes, no leading `./`.
///
/// Relative to the *repo* rather than to the analyzed path, deliberately.
/// The rules file sits at the repo root and is written once, so
/// `nao check src` and `nao check .` have to agree about whether
/// `src/vendor/**` is exempt — an exemption that held or lapsed depending on
/// which directory the author was standing in would be worse than none
/// (CFG-013).
pub(crate) fn repo_relative(repo_root: &Path, path: &Path) -> String {
    let root = plain(repo_root);
    let path = plain(path);
    let rest = path.strip_prefix(&root).unwrap_or(&path);
    rest.to_string_lossy().replace('\\', "/")
}

/// A path with its `./` components dropped, which is what makes
/// `nao check .` and `nao check /abs/repo` print the same lines.
fn plain(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// `1 file` / `3 files`, for every noun any surface of this command counts.
/// Here rather than in either child, so a rule's phrase and the summary
/// below it pluralise the same way.
pub(super) fn tally(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

/// A path as a reader would type it back: no `./`, forward slashes. Every
/// surface names the rules file this way, so the path in an error is the
/// path in a summary.
pub(super) fn tidy(path: &Path) -> String {
    plain(path).to_string_lossy().replace('\\', "/")
}
