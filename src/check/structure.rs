//! What the two rules over the tree count: importers, and doors.
//!
//! These are the two rules a fractal refactor works to — *every file gets
//! its dependencies from its parent* and *every folder has one entry file*
//! — and both are asked of the whole repo rather than of one folder's
//! drawing. That is the difference from the shape scores, and it is the
//! reason this counts rather than reads them (ADR 0024, decision 5):
//! `arborescence` is a ratio over a folder's *immediate children*, with
//! each subfolder collapsed to a node, so two importers of a file inside
//! one subfolder are a single merge on that subfolder — the right answer
//! for the drawing and the wrong one for the rule.
//!
//! What it does read from the shape pass is the **Door** itself, because
//! there must be one definition of it. `entry_concentration` is the ratio
//! this rounds off; the doors behind it are
//! [`folder_shape::doors_by_folder`], ties included, and the tie is not
//! broken here — inventing a tie-breaker would give `Door` a second meaning
//! in the one place it is enforced.
//!
//! Both count over the file-level dependency edges, which is where the
//! capability the hand-written test lacked lives: an edge lands on the file
//! that **declares** what the importer uses, so `export { x } from './y'`
//! is counted against `y`, and a re-export shim cannot launder a second
//! importer into looking like one.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use crate::analyzer::folder_shape;
use crate::graph::{DependencyGraph, FileGraph};

use super::rules::{Rule, Rules};
use super::{repo_relative, Cited, Violation};

/// Every breach of the two rules counted over the tree, in no particular
/// order — the parent sorts one list over all four rules.
pub(super) fn violations(
    graph: &DependencyGraph,
    rules: &Rules,
    repo_root: &Path,
) -> Vec<Violation> {
    let bars = (
        rules.bar(Rule::MaxImportersPerFile),
        rules.bar(Rule::MaxDoorsPerFolder),
    );
    if bars == (None, None) {
        // The file-level graph is a second walk over every edge, and a repo
        // declaring neither rule should not pay for it.
        return Vec::new();
    }
    let tree = graph.file_graph();
    let pairs = graded(&tree, rules, repo_root);
    let mut found = Vec::new();
    if let Some(bar) = bars.0 {
        found.extend(importers(
            &pairs,
            &Cites::of(graph, repo_root),
            repo_root,
            bar,
        ));
    }
    if let Some(bar) = bars.1 {
        found.extend(doors(&pairs, &tree.folders, rules, repo_root, bar));
    }
    found
}

/// The edges between files this repo asked to be graded on.
///
/// An exempt file leaves the graph on both sides, not just as a subject. A
/// file nobody grades cannot be the reason a graded file fails — that
/// violation names a fix inside a file the author has declared out of
/// scope, which is the one thing an exemption is for.
///
/// Kept in the graph's own path spelling, which is what `folder_shape`
/// splits on; a path is translated to the one `check` reports when a
/// violation is built.
fn graded(tree: &FileGraph, rules: &Rules, repo_root: &Path) -> Vec<(String, String)> {
    let exempt: HashSet<&str> = tree
        .files
        .iter()
        .filter(|file| rules.is_exempt(&repo_relative(repo_root, Path::new(file))))
        .map(String::as_str)
        .collect();
    tree.pairs
        .iter()
        .filter(|(from, to)| !exempt.contains(from.as_str()) && !exempt.contains(to.as_str()))
        .cloned()
        .collect()
}

/// `max_importers_per_file`, reported against line 1 of the file being
/// depended on: the subject is the file, and the fix — give it one parent —
/// is not a change to any one line of it.
///
/// Distinct importers, so a file naming another on four lines is one
/// importer. The count is of files that depend on this one, which is the
/// question the rule asks; how many times each says so is a different one.
fn importers(
    pairs: &[(String, String)],
    cites: &Cites,
    repo_root: &Path,
    bar: u32,
) -> Vec<Violation> {
    let mut by_target: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (from, to) in pairs {
        by_target.entry(to).or_default().insert(from);
    }
    by_target
        .into_iter()
        .filter(|(_, importers)| importers.len() as u32 > bar)
        .map(|(target, importers)| {
            let file = repo_relative(repo_root, Path::new(target));
            let names = importers
                .iter()
                .map(|from| cites.cite(repo_relative(repo_root, Path::new(from)), &file))
                .collect();
            Violation {
                rule: Rule::MaxImportersPerFile,
                path: file,
                line: Some(1),
                subject: None,
                measured: importers.len() as u32,
                names,
                bar,
            }
        })
        .collect()
}

/// `max_doors_per_folder`, reported against the folder and nothing in it.
///
/// A folder is not a place in a file, so the violation carries no line: the
/// subject is the directory, and the doors named beside it are where a
/// reader looks. A folder nothing outside it depends on has no doors and
/// cannot break this rule — it has never been entered, which is a different
/// state from being entered twice.
fn doors(
    pairs: &[(String, String)],
    folders: &HashSet<String>,
    rules: &Rules,
    repo_root: &Path,
    bar: u32,
) -> Vec<Violation> {
    folder_shape::doors_by_folder(pairs, folders)
        .into_iter()
        .filter(|(_, doors)| doors.len() as u32 > bar)
        .map(|(folder, doors)| {
            let path = repo_relative(repo_root, Path::new(&folder));
            let names = doors
                .iter()
                .map(|door| Cited {
                    path: repo_relative(repo_root, Path::new(door)),
                    line: None,
                    via: None,
                })
                .collect();
            Violation {
                rule: Rule::MaxDoorsPerFolder,
                path,
                line: None,
                subject: None,
                measured: doors.len() as u32,
                names,
                bar,
            }
        })
        .filter(|v| !rules.is_exempt(&v.path))
        .collect()
}

/// Where each importer said it. Built from the import sites AN-024 records,
/// which is the only place in the graph that knows what line an edge was
/// written on.
struct Cites {
    /// `(importer, imported)` → the first line the importer names it on.
    direct: HashMap<(String, String), usize>,
    /// Importer → every file it names outright, and where. The way in to
    /// the shim question: an importer with no direct statement about a file
    /// it depends on reached it through one of these.
    named: HashMap<String, Vec<(String, usize)>>,
    /// A file that re-exports → everything it forwards, through any number
    /// of further shims.
    forwards: HashMap<String, HashSet<String>>,
}

impl Cites {
    fn of(graph: &DependencyGraph, repo_root: &Path) -> Self {
        let mut cites = Cites {
            direct: HashMap::new(),
            named: HashMap::new(),
            forwards: HashMap::new(),
        };
        let mut reexports: HashMap<String, Vec<String>> = HashMap::new();
        for site in graph.import_sites() {
            let from = repo_relative(repo_root, &site.from);
            let to = repo_relative(repo_root, &site.to);
            // Several statements between one pair are all real; the first is
            // the one a reader is sent to.
            let line = cites
                .direct
                .entry((from.clone(), to.clone()))
                .or_insert(site.line);
            *line = (*line).min(site.line);
            cites
                .named
                .entry(from.clone())
                .or_default()
                .push((to.clone(), site.line));
            if site.is_reexport {
                reexports.entry(from).or_default().push(to);
            }
        }
        for sites in cites.named.values_mut() {
            sites.sort();
        }
        cites.forwards = forwarding(&reexports);
        cites
    }

    /// One importer, with whatever the graph knows about where it said so.
    ///
    /// Three answers, and the difference between them is the teaching. A
    /// direct statement cites its line. A file reached *through* a shim
    /// cites the line the importer actually typed and names the shim, since
    /// the importer never wrote the path it is being charged with — which is
    /// the case this rule exists to catch. An edge no statement created — a
    /// call, a type reference, a specifier naming a package — cites no line,
    /// because one that did not create the edge is worse than none.
    fn cite(&self, importer: String, target: &str) -> Cited {
        if let Some(line) = self.direct.get(&(importer.clone(), target.to_string())) {
            return Cited {
                path: importer,
                line: Some(*line as u32 + 1),
                via: None,
            };
        }
        let through = self.through(&importer, target);
        Cited {
            path: importer,
            line: through.map(|(_, line)| line as u32 + 1),
            via: through.map(|(shim, _)| shim.to_string()),
        }
    }

    /// The re-export the importer went through, if one of the files it does
    /// name forwards the target.
    fn through(&self, importer: &str, target: &str) -> Option<(&str, usize)> {
        self.named.get(importer)?.iter().find_map(|(named, line)| {
            self.forwards
                .get(named)
                .is_some_and(|forwarded| forwarded.contains(target))
                .then_some((named.as_str(), *line))
        })
    }
}

/// What each shim forwards, following a chain of them to the end.
///
/// `a` re-exporting `b` which re-exports `c` means `a` forwards both, and a
/// reader of `a` is looking at `c`'s declarations. Visited-guarded, because
/// two files re-exporting each other is a shape a repo can be in and is not
/// this rule's business to refuse.
fn forwarding(reexports: &HashMap<String, Vec<String>>) -> HashMap<String, HashSet<String>> {
    let mut out = HashMap::with_capacity(reexports.len());
    for shim in reexports.keys() {
        let mut reached: HashSet<String> = HashSet::new();
        let mut frontier = vec![shim.clone()];
        while let Some(here) = frontier.pop() {
            for next in reexports.get(&here).into_iter().flatten() {
                if reached.insert(next.clone()) {
                    frontier.push(next.clone());
                }
            }
        }
        out.insert(shim.clone(), reached);
    }
    out
}
