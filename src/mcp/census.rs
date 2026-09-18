//! What a folder holds, beside what `map` can give a row to.
//!
//! `map`'s listing is a projection of the graph, so a file the analysis never
//! opened is indistinguishable in it from a file that is not there. Two field
//! reports from one session (2026-08-29) are the two halves of that: one read
//! `19 files` over an Astro `src/pages` holding thirteen `.astro` routes and
//! concluded the site had no HTML pages at all; the other read twelve modules
//! and no `index.ts` over a folder whose entire public surface is that barrel,
//! because a file of pure re-exports lists no entity and so produced no group
//! to print.
//!
//! The unresolved-import footer already accounts for the **edges** a hole in
//! the graph costs. This accounts for the **files**, in the same voice: say
//! what you could not see, not just what you saw.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::analyzer::{FileWalker, TestPaths};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind};
use crate::parser::detect_language;

use super::tools::{is_listed, metric_suffix, rel_path};
use super::McpServer;

/// How many extensions the unread note names before it stops counting.
/// Three fit the line, and a reader only has to recognise their own —
/// the same bargain [`super::answer::NAMED_HOLED_FOLDERS`] strikes for the
/// footer.
const NAMED_EXTENSIONS: usize = 3;

/// Why a file the folder holds never reached the graph.
///
/// Ordered as the note prints them, which is roughly least to most likely
/// to be somebody's own doing: an unsupported extension is mezz's gap, a
/// test exclusion is the reader's setting working, and a scope miss is
/// their settings file.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Unread {
    /// No parser claims the extension. `.astro`, `.vue`, `.erb` — the
    /// templates that in some stacks *are* the application — and also every
    /// asset beside them, which is why the note names the extensions.
    Unsupported,
    /// Test code, with `include_tests` off. Documented behaviour, and still
    /// worth counting: it is what stops `8 of 15` reading as a defect.
    Test,
    /// A language filter or an exclude pattern turned it away.
    OutOfScope,
}

/// The files under a mapped folder, split into the ones `map` shows a row
/// for and the ones it cannot, with the reason.
pub(super) struct FolderCensus<'a> {
    /// One entry per file the walk admitted, keyed by its path relative to
    /// the mapped folder, holding the entities `map` lists from it.
    ///
    /// Kept even when that vector is empty. A barrel of re-exports lists
    /// nothing and is the one file every consumer of the folder names; an
    /// absent row and an absent file look identical from the outside.
    files: BTreeMap<String, Vec<&'a CodeEntity>>,
    /// Every listed entity under the folder, by kind — including the nested
    /// ones, which get no row of their own at `depth 1` but are what the
    /// header's breakdown has always counted.
    kinds: BTreeMap<&'static str, usize>,
    /// The files under the folder no analysis read, by reason.
    unread: BTreeMap<Unread, usize>,
    /// The extensions behind [`Unread::Unsupported`]. "unsupported" alone
    /// sends a reader looking for a setting to change; `.astro` tells them
    /// it is a parser, and `.png` tells them to stop reading.
    extensions: BTreeSet<String>,
}

impl<'a> FolderCensus<'a> {
    /// Count `path` twice — once out of the graph, once off the disk — so
    /// the header can say how far apart the two answers are.
    pub(super) fn of(server: &McpServer, graph: &'a DependencyGraph, path: &Path) -> Self {
        let mut census = Self {
            files: BTreeMap::new(),
            kinds: BTreeMap::new(),
            unread: BTreeMap::new(),
            extensions: BTreeSet::new(),
        };
        census.take_entities(graph, path);
        census.take_folder(server, path);
        census
    }

    /// The graph's half: every listed entity under `path`, counted by kind,
    /// and the top-level ones grouped under the file they came from.
    fn take_entities(&mut self, graph: &'a DependencyGraph, path: &Path) {
        let by_id: HashMap<&str, &CodeEntity> =
            graph.entities().map(|e| (e.id.as_str(), e)).collect();
        let is_top_level = |e: &CodeEntity| match &e.parent_id {
            None => true,
            Some(pid) => by_id
                .get(pid.as_str())
                .map(|p| p.kind == EntityKind::File)
                .unwrap_or(true), // unresolvable parent → treat as top-level
        };

        let under = graph
            .entities()
            .filter(|e| is_listed(e))
            .filter(|e| Path::new(&e.file_path).starts_with(path));
        for e in under {
            *self.kinds.entry(e.kind.display_name()).or_default() += 1;
            if is_top_level(e) {
                self.files
                    .entry(rel_path(Path::new(&e.file_path), path))
                    .or_default()
                    .push(e);
            }
        }
        for entities in self.files.values_mut() {
            entities.sort_by_key(|e| e.span.start.line);
        }
    }

    /// The folder's half: every file on disk under `path`, either given the
    /// row it was missing or counted against the reason it has none.
    ///
    /// Admission is asked of the walker rather than restated here.
    /// [`FileWalker::would_analyze`] is the walk's own answer, so a scope
    /// this module cannot see — a settings file, a `spec_dir` — cannot make
    /// the two disagree. Only the *reason* is worked out locally, and only
    /// for files the walk has already refused.
    fn take_folder(&mut self, server: &McpServer, path: &Path) {
        let config = crate::diff::build_analysis_config(
            &server.root,
            server.include_tests,
            &server.languages,
        );
        let walker = FileWalker::new(&config);
        for file in folder_files(path) {
            let name = rel_path(&file, path);
            if self.files.contains_key(&name) || walker.would_analyze(&file) {
                self.files.entry(name).or_default();
                continue;
            }
            let reason = unread_reason(&file, &config);
            *self.unread.entry(reason).or_default() += 1;
            if reason == Unread::Unsupported {
                self.extensions.insert(extension_of(&file));
            }
        }
    }

    /// The header: the title, what the listing shows against what the folder
    /// holds, and — only when they differ — what the difference is made of.
    pub(super) fn header(&self, path: &Path) -> Vec<String> {
        let mut lines = vec![format!("# Map of {}", path.display()), self.tally()];
        lines.extend(self.unread_note());
        lines.push(String::new());
        lines
    }

    /// `8 files — 37 function, …` when the listing is the folder, and
    /// `8 of 15 files listed — …` when it is not.
    ///
    /// The qualifier costs three words and appears only where it is
    /// load-bearing: `8 files` is true on its own terms, and is read as the
    /// folder by everyone who has no `ls` output to check it against.
    fn tally(&self) -> String {
        let held = self.files.len() + self.unread.values().sum::<usize>();
        let count = match held == self.files.len() {
            true => format!("{} files", self.files.len()),
            false => format!("{} of {held} files listed", self.files.len()),
        };
        match self.breakdown().as_str() {
            "" => count,
            breakdown => format!("{count} — {breakdown}"),
        }
    }

    /// The kind histogram the header has always carried.
    fn breakdown(&self) -> String {
        self.kinds
            .iter()
            .map(|(kind, n)| format!("{n} {kind}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The line naming what the listing could not show, or nothing at all
    /// when it showed everything — a caveat printed on every call is one
    /// nobody reads, and this one has to mean something when it appears.
    fn unread_note(&self) -> Option<String> {
        if self.unread.is_empty() {
            return None;
        }
        let clauses: Vec<String> = self
            .unread
            .iter()
            .map(|(reason, n)| self.clause(*reason, *n))
            .collect();
        Some(format!("_Not listed: {}._", clauses.join(", ")))
    }

    /// One reason, counted and spelled so a reader can tell whose problem
    /// it is without opening anything.
    fn clause(&self, reason: Unread, n: usize) -> String {
        match reason {
            Unread::Unsupported => format!("{n} no parser reads ({})", self.named_extensions()),
            Unread::Test => format!("{n} test — `include_tests` is off"),
            Unread::OutOfScope => format!("{n} outside the configured scope"),
        }
    }

    /// The unsupported extensions, capped and never silently dropped.
    fn named_extensions(&self) -> String {
        let named: Vec<&str> = self
            .extensions
            .iter()
            .take(NAMED_EXTENSIONS)
            .map(String::as_str)
            .collect();
        match self.extensions.len() - named.len() {
            0 => named.join(", "),
            rest => format!("{}, and {rest} more", named.join(", ")),
        }
    }

    /// One row per file the analysis read, with its entities when `depth`
    /// asks for them.
    pub(super) fn rows(&self, graph: &DependencyGraph, depth: u64) -> Vec<String> {
        let mut body = Vec::new();
        for (file, entities) in &self.files {
            body.push(format!("{} ({} entities)", file, entities.len()));
            if depth >= 2 {
                body.extend(entities.iter().flat_map(|e| entity_rows(graph, e, depth)));
            }
        }
        body
    }
}

impl FolderCensus<'_> {
    /// The census as fields — the same three facts the prose carries, in
    /// the same shape (CLI-003).
    ///
    /// `listed`/`held` is [`Self::tally`]'s "8 of 15 files listed" as two
    /// numbers, and `unread` is the note under it: a consumer that reads
    /// `files` as the folder without them makes the exact mistake the two
    /// field reports in this module's header describe.
    pub(super) fn as_json(
        &self,
        graph: &DependencyGraph,
        path: &Path,
        root: &Path,
        depth: u64,
    ) -> serde_json::Value {
        let unread: usize = self.unread.values().sum();
        serde_json::json!({
            "path": super::answer::scope_path(path, root),
            "depth": depth,
            "listed": self.files.len(),
            "held": self.files.len() + unread,
            "kinds": self.kinds,
            "unread": {
                "total": unread,
                "unsupported": self.unread.get(&Unread::Unsupported).copied().unwrap_or(0),
                "test": self.unread.get(&Unread::Test).copied().unwrap_or(0),
                "out_of_scope": self.unread.get(&Unread::OutOfScope).copied().unwrap_or(0),
                "extensions": self.extensions,
            },
            "files": self
                .files
                .iter()
                .map(|(file, entities)| {
                    serde_json::json!({
                        "file": file,
                        "entities": entity_values(graph, entities, path, depth),
                    })
                })
                .collect::<Vec<_>>(),
        })
    }
}

/// The entities of one file, nested as `depth` asks — the counterpart of
/// [`entity_rows`], which indents the same tree.
///
/// `depth 1` lists no entities at all, exactly as the prose does: the row
/// is the file, and its count is `files[].entities` being empty rather
/// than a number that contradicts the listing beside it.
fn entity_values(
    graph: &DependencyGraph,
    entities: &[&CodeEntity],
    path: &Path,
    depth: u64,
) -> Vec<serde_json::Value> {
    if depth < 2 {
        return Vec::new();
    }
    entities
        .iter()
        .map(|e| {
            let mut row = super::answer::entity_json(e, path);
            if depth >= 3 {
                let mut members = graph.children(&e.id);
                members.retain(|c| is_listed(c));
                members.sort_by_key(|c| c.span.start.line);
                row["members"] = serde_json::Value::Array(
                    members
                        .iter()
                        .map(|m| super::answer::entity_json(m, path))
                        .collect(),
                );
            }
            row
        })
        .collect()
}

/// One entity's row, and its members when `depth` asks for them.
fn entity_rows(graph: &DependencyGraph, e: &CodeEntity, depth: u64) -> Vec<String> {
    let mut rows = vec![format!(
        "  {} {} ({})",
        e.kind.display_name(),
        e.name,
        metric_suffix(e)
    )];
    if depth < 3 {
        return rows;
    }
    let mut members = graph.children(&e.id);
    members.retain(|c| is_listed(c));
    members.sort_by_key(|c| c.span.start.line);
    rows.extend(members.iter().map(|m| {
        format!(
            "    {} {} ({})",
            m.kind.display_name(),
            m.name,
            metric_suffix(m)
        )
    }));
    rows
}

/// Every file the folder holds, as the walk would find them.
///
/// The same ignore sources the analysis uses, so a `.gitignore`d build
/// directory is never counted as something `map` failed to show — a census
/// that reported `8 of 4213 files listed` over a folder with a `node_modules`
/// in it would be worse than no census.
fn folder_files(path: &Path) -> Vec<PathBuf> {
    ignore::WalkBuilder::new(path)
        .follow_links(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .hidden(true)
        .parents(true)
        .threads(0)
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

/// Which of the walk's rules turned this file away.
///
/// A best reading rather than the walk's own record — [`FileWalker`] answers
/// yes or no and keeps no reason — so the rules are applied here in the order
/// the walk applies them. A `.astro` test file therefore reads as unsupported,
/// which is the fact that would still be true with `include_tests` on.
fn unread_reason(file: &Path, config: &Config) -> Unread {
    let language = detect_language(file);
    if language == Language::Unknown {
        return Unread::Unsupported;
    }
    if !config.analysis.accepts_language(language) {
        return Unread::OutOfScope;
    }
    if !config.analysis.include_tests && TestPaths::rooted_at(&config.root_path).matches(file) {
        return Unread::Test;
    }
    Unread::OutOfScope
}

/// The extension a reader would recognise, dotted. A file without one is
/// named by itself: `Makefile` says more than "no extension" does.
fn extension_of(file: &Path) -> String {
    match file.extension() {
        Some(ext) => format!(".{}", ext.to_string_lossy()),
        None => file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    }
}

#[cfg(test)]
mod tests;
