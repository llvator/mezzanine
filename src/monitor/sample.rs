//! One measurement of the tree, and the per-scope rows a later measurement
//! is compared against.
//!
//! Everything here is a pure function of a [`DependencyGraph`]: no terminal,
//! no clock, no watcher. That is what lets the tests build a graph by hand
//! and assert on the numbers, and it is why this file — not [`super::draw`] —
//! is where the meaning of every figure on the dashboard is decided.
//!
//! ## Nothing here is a new measurement
//!
//! Every field below is read off metrics the analyzer already computed, or
//! counted with the *same* helper the corresponding pull-mode surface uses:
//! smells through [`production_smells`], rules through [`check::violations`],
//! shape off [`FolderShape::pattern`]. A dashboard that measured smells its
//! own way would eventually disagree with `mezz quality` about the same tree,
//! and the reader would have no way to tell which one was lying.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::check;
use crate::graph::DependencyGraph;
use crate::mcp::tools::{is_listed, production_smells, rel_path, TestCode};
use crate::models::{CodeEntity, EntityMetrics, ShapePattern, Thresholds};

/// Composite score above which an entity is drawn red, matching the
/// `quality_bad` band [`crate::models::ScopeMetrics`] rolls up.
const RED: f32 = 1.0;

/// How many changed paths a sample carries. The list is provenance for one
/// tick — "these files are why this number moved" — not a changelog, and a
/// swarm's burst can name hundreds.
const CHANGED_CAP: usize = 12;

/// The scalar reading. `Copy` on purpose: [`super::series::Series`] keeps one
/// of these per tick for the sparklines, so it has to stay small enough that
/// five hundred of them are not worth thinking about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Metrics {
    // --- The whole tree in one number ---
    /// The repository's overall quality score: the LOC-weighted mean
    /// composite the browser UI's repo tile shows. Lower is better, and the
    /// bands are the per-entity ones — 0.5 amber, 1.0 red — because it is a
    /// mean of exactly those scores.
    ///
    /// Read off the rollup the analyzer already computed for the top folder
    /// rather than averaged here, for the reason this module opens with: a
    /// dashboard that weighted the mean its own way would drift from the panel
    /// the operator checks it against, and neither would say which was wrong.
    /// [`Metrics::mean_composite`] beside it is the *unweighted* mean, and the
    /// two are different questions — a tree of small clean getters around one
    /// enormous tangle scores well on one and badly on the other.
    pub score: f32,

    // --- Smells and refactor pressure ---
    /// Smelly production entities. Test code is counted apart, below, for
    /// the reasons [`crate::mcp::tools`] gives at `smells_aside`.
    pub smells: u32,
    pub smells_in_tests: u32,
    /// Entities whose composite score is past [`RED`].
    pub red_entities: u32,
    pub mean_composite: f32,

    // --- Size and complexity ---
    pub entities: u32,
    pub loc: u32,
    pub mean_cyclomatic: f32,
    pub max_cyclomatic: u32,
    /// Entities past the `bad` band on cyclomatic, cognitive or nesting.
    pub over_ceiling: u32,

    // --- Cycles and coupling ---
    pub cycles: u32,
    pub folders_in_cycle: u32,
    pub mean_cohesion: f32,
    pub mean_instability: f32,

    // --- Shape and rules ---
    pub shape: ShapeCounts,
    pub rules: Verdict,
}

/// What the repo's own rules say about the tree.
///
/// Three states rather than a number, and the two that are not a number are
/// the point. A dashboard printing a confident green `0` for a tree nobody
/// ever set a bar for is quiet-when-blind wearing quiet-when-clean's
/// clothes; printing the same `0` for a rules file mezz could not parse is
/// worse, because there the operator *did* set a bar and is being told it
/// holds. `mezz check` spends a whole exit code (2) on that distinction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// No `.mezz/rules.json`, or one that declares nothing.
    #[default]
    Undeclared,
    /// A rules file that exists and could not be used.
    Unreadable,
    /// Breaches of the rules the repo declared.
    Broken(u32),
}

impl Verdict {
    /// The breach count, for the sparkline. `None` wherever there is no
    /// number to plot.
    pub fn count(self) -> Option<u32> {
        match self {
            Verdict::Broken(n) => Some(n),
            _ => None,
        }
    }
}

/// How many folders sit in each shape tier, worst first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct ShapeCounts {
    pub cyclic: u32,
    pub tangled: u32,
    pub hierarchical: u32,
    pub fractal: u32,
}

impl ShapeCounts {
    fn count(&mut self, pattern: ShapePattern) {
        match pattern {
            ShapePattern::Cyclic => self.cyclic += 1,
            ShapePattern::Tangled => self.tangled += 1,
            ShapePattern::Hierarchical => self.hierarchical += 1,
            ShapePattern::Fractal => self.fractal += 1,
        }
    }

    /// Folders that do not reach `Hierarchical` — the ones a reader cannot
    /// follow. Drawn as one number because two would not fit and because
    /// they are the same complaint at two depths.
    pub fn unreadable(&self) -> u32 {
        self.cyclic + self.tangled
    }

    /// Every folder measured — the denominator [`Self::unreadable`] is a
    /// share of.
    ///
    /// The count exists because a bare `12` cannot be read: twelve
    /// unreadable folders out of twenty and out of four hundred are
    /// different trees, and no amount of watching the number distinguishes
    /// them. The tile draws both halves for that reason alone.
    ///
    /// A sum over the tiers rather than a folder count taken beside them,
    /// because the two halves have to be counted over one population. Every
    /// folder in the tree lands in exactly one tier — `folder_shape::compute`
    /// scores precisely the paths `enumerate_folder_paths` hands it, and
    /// `graph.rs` then walks that same set — so a separately-measured total
    /// could only ever agree with this one or be wrong, and the drift would
    /// show up as a ratio over a population the numerator was never taken
    /// from.
    pub fn total(&self) -> u32 {
        self.cyclic + self.tangled + self.hierarchical + self.fractal
    }
}

/// One file's contribution, kept so the next tick can say what moved.
///
/// Both fields are counted by the rule the corresponding *tile* uses, not by a
/// second rule of this module's own — see [`tally_entities`]. A row that
/// disagreed with the figure above it would send a reader to a file the
/// headline says did nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FileRow {
    pub max_cyclomatic: u32,
    /// Smelly *entities* in this file, the unit `Metrics::smells` counts in.
    /// Not smell instances: an entity carrying three smells is one smelly
    /// entity to the tile, and has to be one here too.
    pub smells: u32,
}

/// The per-scope detail behind a reading.
///
/// Held only for the baseline and the latest sample — see
/// [`super::series::Series`]. A ring of five hundred of these on a large repo
/// is tens of megabytes for a question nobody asks of the middle of the
/// series: "what moved" is always asked about the two ends.
#[derive(Clone, Debug, Default)]
pub struct Scopes {
    pub files: BTreeMap<String, FileRow>,
    pub folders: BTreeMap<String, ShapePattern>,
}

/// A commit, as the dashboard names one.
///
/// The subject travels with the SHA because seven hex digits identify a commit
/// to git and to nobody else, and the header has room for the sentence its
/// author already wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Commit {
    /// Short SHA, as `git log --format=%h` spells it.
    pub sha: String,
    /// The subject line, or empty when git gave none.
    pub subject: String,
}

/// A reading, stamped and attributed.
#[derive(Clone, Debug, Serialize)]
pub struct Sample {
    /// Tick number within the session, starting at 0.
    pub seq: u64,
    /// Unix epoch milliseconds — the same stamp [`crate::activity`] uses.
    pub at_ms: u64,
    /// How long the analysis behind this reading took.
    pub analysis_ms: u64,
    /// The commit this reading is *about*: the one checked out for a baseline
    /// (MON-007), or the one in force over the working tree for an ordinary
    /// tick. `None` outside a git repository.
    ///
    /// It is what tells the header that a pinned baseline is no longer the
    /// commit in force, and it is the address a `--log` line otherwise
    /// lacked — a series of numbers nothing can attribute to a state.
    pub head: Option<Commit>,
    /// True for a reading of a *checkout*, taken to be the zero the others
    /// are measured from — not a tick of the session, and not a state the
    /// tree was ever in while anybody was watching.
    ///
    /// In the log because otherwise a `--log` file holds deltas nobody can
    /// reproduce: every line is measured from a state no line records.
    pub baseline: bool,
    /// The paths whose change triggered it, capped at [`CHANGED_CAP`].
    pub changed: Vec<String>,
    /// How many changes there were before the cap.
    pub changed_total: usize,
    pub metrics: Metrics,
    /// The smells behind `metrics.smells`, by kind. Worth a line of its own:
    /// thirty-seven smells that are all Feature Envy and thirty-seven spread
    /// over six kinds are different situations, and the count alone cannot
    /// tell them apart.
    pub smells_by_kind: BTreeMap<String, u32>,
    /// Dropped from the `--log` line: this is the comparison material for
    /// the movers list, and it is large.
    #[serde(skip)]
    pub scopes: Scopes,
}

impl Sample {
    /// Measure `graph`, rooted at `root` (the analyzed path) with `repo_root`
    /// for the rules file — the two differ when someone monitors a
    /// subdirectory of a repo.
    pub fn of(graph: &DependencyGraph, root: &Path, repo_root: &Path, stamp: Stamp<'_>) -> Self {
        let tests = TestCode::of(graph, root);
        let (smelly, in_tests) = production_smells(graph, &tests);
        let (entities, files) = tally_entities(graph, root, &tests);
        let folders = tally_folders(graph, root);

        Sample {
            seq: stamp.seq,
            at_ms: stamp.at_ms,
            analysis_ms: stamp.analysis_ms,
            head: stamp.head,
            baseline: stamp.baseline,
            changed_total: stamp.changed.len(),
            // Spelled like every other path in the reading. The watcher
            // hands over what the filesystem told it, which is absolute;
            // a log line naming `/private/tmp/…/src/util.rs` beside a
            // movers row naming `src/util.rs` is two spellings of one file.
            changed: stamp
                .changed
                .iter()
                .take(CHANGED_CAP)
                .map(|p| rel_path(Path::new(p), root))
                .collect(),
            metrics: assemble(graph, repo_root, Smells(&smelly, in_tests), &entities, &folders),
            smells_by_kind: by_kind(&smelly),
            scopes: Scopes {
                files,
                folders: folders.tiers,
            },
        }
    }

    /// The kinds behind the smell count, worst-represented first.
    pub fn top_smell_kinds(&self, n: usize) -> Vec<(&str, u32)> {
        let mut kinds: Vec<(&str, u32)> = self
            .smells_by_kind
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        // Count descending, then name, so equal counts do not reorder
        // themselves between two ticks that measured the same tree.
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        kinds.truncate(n);
        kinds
    }
}

/// What the caller knows that the graph does not: when this was taken, how
/// long it cost, and what woke it.
///
/// `changed` is borrowed rather than owned so the measuring thread still holds
/// the paths after the call: an analysis that fails has to put them back in
/// the queue, and a `Vec` moved in here would be gone (MON-006).
pub struct Stamp<'a> {
    pub seq: u64,
    pub at_ms: u64,
    pub analysis_ms: u64,
    pub changed: &'a [String],
    /// The commit the reading is about — see [`Sample::head`].
    pub head: Option<Commit>,
    /// Whether this is a checkout's reading — see [`Sample::baseline`].
    pub baseline: bool,
}

/// The smelly production entities and how many the test code beside them
/// would have added — the pair [`production_smells`] returns, named so the
/// two counts cannot be handed over the wrong way round.
struct Smells<'g>(&'g [&'g CodeEntity], usize);

/// Put the three tallies together into the reading.
///
/// A function rather than a literal inside [`Sample::of`]: the struct has
/// fifteen fields drawn from four sources, and holding all of them plus the
/// tallies plus the stamp in view at once is exactly the Overfull Head this
/// dashboard reports on other people's code.
fn assemble(
    graph: &DependencyGraph,
    repo_root: &Path,
    smells: Smells,
    entities: &EntityTally,
    folders: &FolderTally,
) -> Metrics {
    Metrics {
        score: folders.score,
        smells: smells.0.len() as u32,
        smells_in_tests: smells.1 as u32,
        red_entities: entities.red,
        mean_composite: entities.mean_composite,
        entities: entities.count,
        loc: entities.loc,
        mean_cyclomatic: entities.mean_cyclomatic,
        max_cyclomatic: entities.max_cyclomatic,
        over_ceiling: entities.over_ceiling,
        cycles: graph.find_cycles().len() as u32,
        folders_in_cycle: folders.in_cycle,
        mean_cohesion: folders.mean_cohesion,
        mean_instability: folders.mean_instability,
        shape: folders.shape,
        rules: verdict(graph, repo_root),
    }
}

/// How many of each kind the smell count is made of.
fn by_kind(smelly: &[&CodeEntity]) -> BTreeMap<String, u32> {
    let mut counted: BTreeMap<String, u32> = BTreeMap::new();
    for e in smelly {
        for s in &e.metrics.smells {
            *counted.entry(s.label().to_string()).or_default() += 1;
        }
    }
    counted
}

/// The entity-level pass: everything read off [`crate::models::EntityMetrics`].
#[derive(Default)]
struct EntityTally {
    count: u32,
    loc: u32,
    mean_composite: f32,
    mean_cyclomatic: f32,
    max_cyclomatic: u32,
    over_ceiling: u32,
    red: u32,
}

/// One pass over the entities for the scalars, and the same pass for the
/// per-file rows. Together because they read the same fields, and a second
/// walk to collect the rows would be a second place for "which entities
/// count" to drift.
///
/// ## The rows are filtered the way the tiles are (MON-002)
///
/// The row half was written without [`is_listed`] and without `tests`, which
/// made `+1 smell` on a movers row a *different measurement* from the `smells`
/// tile above it — different population, and different unit. A swarm writing a
/// smelly test drew a red row beside a tile reading `=`.
///
/// So the smell count here takes both of [`production_smells`]' filters and
/// counts entities rather than instances. The invariant that buys is worth
/// stating, because it is what a reader assumes when they look at the two
/// halves of the screen together, and it is asserted in the tests:
///
/// > every file row's `smells`, summed, is `metrics.smells`.
///
/// `max_cyclomatic` takes [`is_listed`] and *not* the test filter, on purpose.
/// A test file that got harder to read is a true thing worth a row, and the
/// movers list is the only surface that would ever say so — where its smells
/// are deliberately not the headline's business. The two fields answer to
/// different tiles, so they carry different filters.
fn tally_entities(
    graph: &DependencyGraph,
    root: &Path,
    tests: &TestCode,
) -> (EntityTally, BTreeMap<String, FileRow>) {
    let bar = Thresholds::default();
    let mut tally = EntityTally::default();
    let mut composite_sum = 0.0f64;
    let mut cyclomatic_sum = 0u64;
    let mut cyclomatic_seen = 0u32;
    let mut files: BTreeMap<String, FileRow> = BTreeMap::new();

    for e in graph.entities() {
        let m = &e.metrics;
        tally.count += 1;
        tally.loc += m.loc;
        composite_sum += m.composite_score as f64;
        if m.composite_score > RED {
            tally.red += 1;
        }
        if let Some(cc) = m.cyclomatic {
            cyclomatic_sum += cc as u64;
            cyclomatic_seen += 1;
            tally.max_cyclomatic = tally.max_cyclomatic.max(cc);
        }
        if over_ceiling(m, &bar) {
            tally.over_ceiling += 1;
        }

        if !is_listed(e) {
            continue;
        }
        let row = files.entry(scope_name(&e.file_path, root)).or_default();
        row.max_cyclomatic = row.max_cyclomatic.max(m.cyclomatic.unwrap_or(0));
        if !m.smells.is_empty() && !tests.holds(Some(e)) {
            row.smells += 1;
        }
    }

    tally.mean_composite = mean(composite_sum, tally.count);
    tally.mean_cyclomatic = mean(cyclomatic_sum as f64, cyclomatic_seen);
    (tally, files)
}

/// Whether an entity is past the `bad` band on any of the three difficulty
/// measures.
///
/// The bands come from [`Thresholds`] rather than from literals, so the
/// dashboard's "over ceiling" and the colour the browser UI paints the same
/// entity cannot end up disagreeing.
fn over_ceiling(m: &EntityMetrics, bar: &Thresholds) -> bool {
    let past = |v: Option<u32>, limit: f32| v.is_some_and(|v| v as f32 > limit);
    past(m.cyclomatic, bar.cc.bad)
        || past(m.cognitive_complexity, bar.cognitive.bad)
        || past(m.max_nesting, bar.nest.bad)
}

/// The folder-level pass: the shape ladder, the tier each folder landed in,
/// how many sit in a loop, and the two coupling means.
#[derive(Default)]
struct FolderTally {
    shape: ShapeCounts,
    /// Tier by folder, for the movers list.
    tiers: BTreeMap<String, ShapePattern>,
    in_cycle: u32,
    mean_cohesion: f32,
    mean_instability: f32,
    /// The whole tree's quality score — see [`Metrics::score`] and [`Top`].
    score: f32,
}

/// The shallowest folder seen so far, and the rollup it carried.
///
/// The tree-wide score has to come off *one* folder's rollup, and which one
/// is not always `.`. `graph.rs` enumerates folders from the common parent of
/// the analyzed files, so a tree whose sources all sit in `src/` never
/// produces a rollup for the root the operator named — and a missing rollup
/// defaulting to `0.0` would draw a perfect score over an unmeasured tree,
/// which is the one failure a quality dashboard must not have.
///
/// Shallowest-under-root is well defined in both directions. When the root is
/// enumerated it wins at depth zero; when the enumeration starts below it,
/// every analyzed file hangs off the single folder that starts it, so that
/// folder's rollup covers exactly the same entities the root's would have.
#[derive(Default)]
struct Top {
    depth: Option<usize>,
    score: f32,
}

impl Top {
    /// Offer a folder's rollup. Kept when it sits above whatever is held.
    fn offer(&mut self, name: &str, avg_quality: f32) {
        let depth = match name {
            "." => 0,
            path => path.split('/').count(),
        };
        if self.depth.is_some_and(|held| held <= depth) {
            return;
        }
        self.depth = Some(depth);
        self.score = avg_quality;
    }
}

/// Everything the folder rollups say, in one pass.
fn tally_folders(graph: &DependencyGraph, root: &Path) -> FolderTally {
    let mut shape = ShapeCounts::default();
    let mut top = Top::default();
    let mut tiers = BTreeMap::new();
    let mut in_cycle = 0;
    let mut cohesion_sum = 0.0f64;
    let mut cohesion_seen = 0u32;
    let mut instability_sum = 0.0f64;
    let mut instability_seen = 0u32;

    for f in graph.folder_metrics() {
        // Everything below is about the tree being watched, so a folder
        // outside it is skipped whole rather than half-counted: it must not
        // reach the movers list under an unstable key (see [`scoped_name`]),
        // and it must not reach the tally either, or `ShapeCounts::total`
        // becomes a denominator over a population the numerator was not taken
        // from — `mezz monitor src/` reported 43 folders where `mezz analyze`
        // on the same path reports 41, the two extra being the repo root and
        // the spec directory beside it.
        let Some(name) = scoped_name(Path::new(&f.path), root) else {
            continue;
        };
        top.offer(&name, f.metrics.avg_quality);
        if let Some(s) = &f.metrics.shape {
            shape.count(s.pattern);
            tiers.insert(name, s.pattern);
        }
        if f.metrics.in_cycle {
            in_cycle += 1;
        }
        // `None` is an undefined ratio, not a zero — a folder with no
        // dependency edges has no cohesion to average in, and folding it in
        // as 0.0 would drag the mean down every time somebody added an empty
        // directory.
        if let Some(c) = f.metrics.cohesion {
            cohesion_sum += c as f64;
            cohesion_seen += 1;
        }
        if let Some(i) = f.metrics.instability {
            instability_sum += i as f64;
            instability_seen += 1;
        }
    }

    FolderTally {
        shape,
        tiers,
        in_cycle,
        mean_cohesion: mean(cohesion_sum, cohesion_seen),
        mean_instability: mean(instability_sum, instability_seen),
        score: top.score,
    }
}

/// A scope's path as the movers list should show it: relative to the analyzed
/// root, and `.` for the root itself.
///
/// A path arrives spelled however the entity file paths that built it were —
/// which for a monitored tree is absolute, and an absolute temp path eats a
/// whole row (AN-020).
///
/// Both halves of [`Scopes`] go through here, which is the point (MON-003).
/// Only the folder half used to, and the file half kept `rel_path`'s answer
/// verbatim — so every reading carried a row keyed on the empty string, for
/// the entity whose file path *is* the root. It read `0/0`, so no mover was
/// ever built from it; the row it would have rendered the moment it carried a
/// number has no path in it at all, and there is nothing for a reader to go
/// look at.
fn scope_name(path: &Path, root: &Path) -> String {
    match rel_path(path, root) {
        empty if empty.is_empty() => ".".to_string(),
        relative => relative,
    }
}

/// [`scope_name`] for a caller that would rather drop a scope than name it
/// badly — `None` for anything that does not sit under `root`.
///
/// `rel_path` falls back to the path as given when it cannot relativize, and
/// for a scope outside the analyzed root that fallback is an absolute path.
/// Two samples then key the same folder two ways: the working tree spells it
/// `$HOME/mezzanine-private/spec`, and the baseline — measured in a
/// detached worktree under the temp dir — spells its own ancestors
/// `/`, the home root, `/var/folders/…`. Nothing matches across the pair, so every
/// such folder arrives in one reading and is deleted from the other, and the
/// movers list fills with rows naming a checkout that no longer exists.
///
/// They are dropped rather than re-keyed because the dashboard is rooted at
/// what the operator asked to watch. `mezz monitor src/` analyses the specs
/// beside `src/` too, so the graph legitimately holds folders above and
/// beside the monitored path; none of them are the subject, and a shape tier
/// counted for one is a folder the reader cannot act on from here.
fn scoped_name(path: &Path, root: &Path) -> Option<String> {
    // `strip_prefix` and not a string comparison: the baseline's root arrives
    // with a trailing separator (`git rev-parse --show-prefix` spells it
    // `src/`), and only a component-wise test reads that as the same folder.
    path.strip_prefix(root).ok()?;
    Some(scope_name(path, root))
}

/// What the repo's rules say — see [`Verdict`].
fn verdict(graph: &DependencyGraph, repo_root: &Path) -> Verdict {
    let rules = match check::rules::load(repo_root) {
        Ok(Some(rules)) => rules,
        Ok(None) => return Verdict::Undeclared,
        Err(_) => return Verdict::Unreadable,
    };
    // A file that parses but declares no bar is the same situation as no
    // file: nobody said what "good" is here.
    if rules.is_empty() {
        return Verdict::Undeclared;
    }
    Verdict::Broken(check::violations(graph, &rules, repo_root).len() as u32)
}

/// A mean that is 0.0 over an empty population rather than NaN — a NaN here
/// reaches a sparkline and takes the whole panel with it.
fn mean(sum: f64, n: u32) -> f32 {
    match n {
        0 => 0.0,
        n => (sum / n as f64) as f32,
    }
}
