//! Tests for `mezz monitor`.
//!
//! Two halves, failing for different reasons. The sample tests grade real
//! fixtures through the real analyzer, for the reason `mezz check`'s tests
//! give: every figure on the dashboard is a count over resolved entities, and
//! a hand-built graph would prove nothing about whether the resolution — the
//! part that can actually be wrong — works. The series and movers tests are
//! arithmetic and ordering over readings built by hand, because that is what
//! they are.
//!
//! Nothing here opens a terminal. [`super::draw`] is the only module that
//! could need one, and it was written to have no decisions left in it; where a
//! whole frame is the thing under test it is drawn into ratatui's
//! `TestBackend`, which is a buffer in memory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::baseline::{self, Ends, Plan, Wanted};
use super::compare;
use super::draw;
use super::movers;
use super::series::{Anchor, Series};
use super::watch::{self, Measure, Reading};
use super::sample::{Commit, FileRow, Metrics, Scopes, ShapeCounts, Sample, Stamp, Verdict};
use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::ShapePattern;

/// A throwaway repo the real analyzer walks.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mezz-monitor-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, relative: &str, body: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }

    /// Turn the fixture into a repository. Only the comparison tests need one:
    /// both of their ends are checkouts, so there has to be something to check
    /// out (MON-008).
    fn git_init(&self) {
        for args in [
            vec!["init", "-q", "--initial-branch=main", "."],
            vec!["config", "user.email", "t@t.t"],
            vec!["config", "user.name", "t"],
        ] {
            self.git(&args);
        }
    }

    /// Commit everything written so far, and answer what it resolved to.
    fn commit(&self, message: &str) -> Plan {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
        baseline::at_head(&self.0).expect("the fixture just committed")
    }

    fn git(&self, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// Measure the fixture, with a configuration built from it alone — no
    /// settings file of the developer's reaches it.
    fn sample(&self) -> Sample {
        self.measure(false)
    }

    /// Measure it with test code in the graph, which is what makes the
    /// production/test smell split observable at all.
    fn sample_including_tests(&self) -> Sample {
        self.measure(true)
    }

    fn measure(&self, include_tests: bool) -> Sample {
        self.measure_after(include_tests, Vec::new())
    }

    /// Analyse the whole fixture but measure it as though only `sub` were
    /// being watched — the shape `mezz monitor src/` takes, where the graph
    /// legitimately holds folders above and beside the monitored path.
    fn sample_rooted_at(&self, sub: &str) -> Sample {
        let config = Config::for_path(&self.0);
        let result = Analyzer::new(config).analyze().unwrap();
        let graph = DependencyGraph::from_analysis(&result);
        Sample::of(
            &graph,
            &self.0.join(sub),
            &self.0,
            Stamp {
                seq: 0,
                at_ms: 0,
                analysis_ms: 0,
                changed: &[],
                head: None,
                baseline: false,
            },
        )
    }

    /// Measure as though `changed` is what woke the watcher — the paths
    /// arrive absolute, as the filesystem reports them.
    fn measure_after(&self, include_tests: bool, changed: Vec<String>) -> Sample {
        let mut config = Config::for_path(&self.0);
        config.analysis.include_tests = include_tests;
        let result = Analyzer::new(config).analyze().unwrap();
        let graph = DependencyGraph::from_analysis(&result);
        Sample::of(
            &graph,
            &self.0,
            &self.0,
            Stamp {
                seq: 0,
                at_ms: 0,
                analysis_ms: 0,
                changed: &changed,
                head: None,
                baseline: false,
            },
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A callable holding more distinct names than a reader can keep, which is
/// what `OverfullHead` fires on — the one smell a fixture can trigger from
/// the shape of the source alone, with no thresholds to guess at.
fn overfull(name: &str) -> String {
    let locals: String = (0..16)
        .map(|i| format!("    let name_{i} = {i};\n"))
        .collect();
    let sum: String = (0..16)
        .map(|i| format!("name_{i}"))
        .collect::<Vec<_>>()
        .join(" + ");
    format!("pub fn {name}() -> i32 {{\n{locals}    {sum}\n}}\n")
}

/// One callable carrying **two** smells: `Dispatcher` (past `dispatcher_cc`,
/// fanning out to more callees than `dispatcher_fan_out`, at fewer than
/// `dispatcher_loc_per_branch` lines each) and `OverfullHead` on top.
///
/// The fixture exists because a one-smell entity cannot tell the two possible
/// units apart. Counting smelly *entities* and counting smell *instances* give
/// the same answer everywhere until an entity carries a second smell — and
/// that is not a hypothetical shape: measured over this repo, fifteen entities
/// carry two, six of them this very pair.
fn dispatcher_and_overfull(name: &str) -> String {
    let helpers: String = (0..25)
        .map(|i| format!("fn helper_{i}() -> i32 {{ {i} }}\n"))
        .collect();
    let locals: String = (0..16)
        .map(|i| format!("    let name_{i} = {i};\n"))
        .collect();
    let arms: String = (0..25)
        .map(|i| format!("        {i} => helper_{i}(),\n"))
        .collect();
    let sum: String = (0..16)
        .map(|i| format!("name_{i}"))
        .collect::<Vec<_>>()
        .join(" + ");
    format!(
        "{helpers}\npub fn {name}(n: i32) -> i32 {{\n{locals}    match n {{\n{arms}        _ => {sum},\n    }}\n}}\n"
    )
}

/// A function whose branching is past the `bad` band on cyclomatic
/// complexity (20), so the ceiling count has something to find.
fn branchy(name: &str) -> String {
    let arms: String = (0..25)
        .map(|i| format!("    if n == {i} {{ return {i}; }}\n"))
        .collect();
    format!("pub fn {name}(n: i32) -> i32 {{\n{arms}    -1\n}}\n")
}

#[test]
fn a_repo_that_declares_no_rules_has_no_verdict_rather_than_a_clean_one() {
    let fixture = Fixture::new("no-rules");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");

    assert_eq!(fixture.sample().metrics.rules, Verdict::Undeclared);
}

#[test]
fn a_rules_file_that_declares_nothing_still_has_no_verdict() {
    let fixture = Fixture::new("empty-rules");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    fixture.write(".mezz/rules.json", "{}\n");

    assert_eq!(fixture.sample().metrics.rules, Verdict::Undeclared);
}

/// The state that would otherwise hide inside `Undeclared`: the operator
/// *did* set a bar, and is entitled to know mezz could not read it rather
/// than to be told nothing was declared.
#[test]
fn a_rules_file_that_cannot_be_used_says_so_instead_of_reading_as_undeclared() {
    let fixture = Fixture::new("bad-rules");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    fixture.write(".mezz/rules.json", "{\"rules\": {\"max_wat\": 1}}\n");

    assert_eq!(fixture.sample().metrics.rules, Verdict::Unreadable);
}

#[test]
fn a_declared_rule_that_breaks_is_counted() {
    let fixture = Fixture::new("broken-rule");
    fixture.write(
        "src/app.rs",
        "pub fn one() -> i32 { 1 }\npub fn two() -> i32 { 2 }\n",
    );
    fixture.write(".mezz/rules.json", "{\"rules\": {\"max_entities_per_file\": 1}}\n");

    assert_eq!(fixture.sample().metrics.rules, Verdict::Broken(1));
}

#[test]
fn a_function_past_the_ceiling_is_counted_and_a_plain_one_is_not() {
    let fixture = Fixture::new("ceiling");
    fixture.write("src/plain.rs", "pub fn one() -> i32 { 1 }\n");
    let plain = fixture.sample().metrics;
    assert_eq!(plain.over_ceiling, 0, "a one-line function clears the bar");

    fixture.write("src/gnarly.rs", &branchy("gnarly"));
    let with_gnarly = fixture.sample().metrics;
    assert_eq!(with_gnarly.over_ceiling, 1);
    assert!(
        with_gnarly.max_cyclomatic > 20,
        "the branchy function is past the `bad` band, got {}",
        with_gnarly.max_cyclomatic
    );
}

#[test]
fn a_smell_in_test_code_is_counted_apart_from_one_in_production() {
    let fixture = Fixture::new("test-split");
    fixture.write("src/app.rs", &overfull("in_production"));
    fixture.write("tests/app_test.rs", &overfull("in_a_test"));

    let metrics = fixture.sample_including_tests().metrics;
    assert!(
        metrics.smells + metrics.smells_in_tests >= 2,
        "the fixture is supposed to smell; got {metrics:?}"
    );
    assert_eq!(
        metrics.smells, 1,
        "only the production copy belongs in the headline count"
    );
    assert_eq!(metrics.smells_in_tests, 1);
}

#[test]
fn the_per_file_rows_name_the_files_they_measured() {
    let fixture = Fixture::new("rows");
    fixture.write("src/gnarly.rs", &branchy("gnarly"));

    let sample = fixture.sample();
    let row = sample
        .scopes
        .files
        .get("src/gnarly.rs")
        .expect("the analyzed file has a row, keyed relative to the root");
    assert!(row.max_cyclomatic > 20);
}

/// The invariant that keeps the two halves of the screen honest: the movers
/// list and the `smells` tile are one measurement seen twice, so the rows have
/// to add up to the headline (MON-002). They did not — the row half was
/// written without `is_listed`, without the test split, and in a different
/// unit, so a smelly test drew a red row beside a tile reading `=`.
#[test]
fn every_file_rows_smells_summed_is_the_headline_count() {
    let fixture = Fixture::new("rows-sum");
    fixture.write("src/app.rs", &overfull("in_production"));
    // One entity carrying two smells, so the invariant pins the *unit* as well
    // as the population: a row counting instances would exceed the tile here.
    fixture.write("src/two.rs", &dispatcher_and_overfull("two_smells"));
    fixture.write("tests/app_test.rs", &overfull("in_a_test"));

    let sample = fixture.sample_including_tests();
    let summed: u32 = sample.scopes.files.values().map(|r| r.smells).sum();
    assert_eq!(
        summed, sample.metrics.smells,
        "the rows are the tile, itemised: {:?} against {}",
        sample.scopes.files, sample.metrics.smells
    );
    assert!(
        sample.metrics.smells_in_tests > 0,
        "the fixture is supposed to put a smell on both sides of the split"
    );
}

#[test]
fn a_smell_in_test_code_is_absent_from_the_file_rows_as_well_as_the_headline() {
    let fixture = Fixture::new("rows-tests");
    fixture.write("src/app.rs", &overfull("in_production"));
    fixture.write("tests/app_test.rs", &overfull("in_a_test"));

    let sample = fixture.sample_including_tests();
    let test_row = sample
        .scopes
        .files
        .get("tests/app_test.rs")
        .expect("the test file is still measured — its complexity is real");
    assert_eq!(
        test_row.smells, 0,
        "a smell the headline excludes cannot be a mover row"
    );
    assert_eq!(sample.scopes.files["src/app.rs"].smells, 1);
}

/// An entity carrying three smells is *one* smelly entity to the tile, so it
/// has to be one to its row too. Counting instances made a row claim three
/// times the move the tile agreed to.
#[test]
fn a_file_row_counts_smelly_entities_and_not_smell_instances() {
    let fixture = Fixture::new("rows-unit");
    fixture.write("src/app.rs", &dispatcher_and_overfull("two_smells"));

    let sample = fixture.sample();
    let carried: u32 = sample.smells_by_kind.values().sum();
    assert!(
        carried >= 2,
        "the fixture must put two smells on one entity or this proves nothing, got {:?}",
        sample.smells_by_kind
    );
    assert_eq!(
        sample.metrics.smells, 1,
        "two smells, one smelly entity: {:?}",
        sample.smells_by_kind
    );
    assert_eq!(
        sample.scopes.files["src/app.rs"].smells, 1,
        "and the row counts it the way the tile does — not once per smell"
    );
}

/// `rel_path` answers the empty string for an entity whose file path *is* the
/// analyzed root, and the file pass used to keep that verbatim — so every
/// reading carried a `""` row. It read `0/0` and so never became a mover; the
/// row it would have drawn has no path in it for a reader to go look at
/// (MON-003, and AN-020 for the sibling case).
#[test]
fn a_file_row_is_never_keyed_on_the_empty_string() {
    let fixture = Fixture::new("rows-root-path");
    fixture.write("src/app.rs", &branchy("gnarly"));

    let sample = fixture.sample();
    let named: Vec<&String> = sample.scopes.files.keys().collect();
    assert!(
        named.iter().all(|p| !p.is_empty()),
        "no file row is nameless: {named:?}"
    );
    assert!(
        named.iter().all(|p| !p.starts_with('/')),
        "and none is absolute: {named:?}"
    );
}

/// Folder paths arrive spelled however the entity paths that built them
/// were, which for a monitored tree is absolute. An absolute path eats a
/// whole row of the movers list, and would never match the baseline's
/// spelling if the root were ever restated differently (AN-020).
#[test]
fn folder_rows_are_named_relative_to_the_analyzed_root() {
    let fixture = Fixture::new("folder-paths");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    fixture.write("src/deep/inner.rs", "pub fn two() -> i32 { 2 }\n");

    let sample = fixture.sample();
    let named: Vec<&String> = sample.scopes.folders.keys().collect();
    assert!(
        named.iter().all(|p| !p.starts_with('/')),
        "no folder row is an absolute path: {named:?}"
    );
    assert!(
        named.iter().any(|p| p.as_str() == "src"),
        "the analyzed subfolder is named plainly: {named:?}"
    );
}

/// A folder the analysis covers but the operator is not watching is dropped,
/// not kept under the absolute path `rel_path` falls back to.
///
/// The bug this pins: the baseline is measured in a detached worktree under
/// the temp dir, so a folder outside the analyzed root is spelled
/// `/var/folders/…` there and a path under `$HOME` in the working tree. Nothing matches
/// across the pair, and the movers list fills with rows naming a checkout that
/// has already been removed.
#[test]
fn a_folder_outside_the_watched_root_is_left_out_of_the_reading() {
    let fixture = Fixture::new("outside-root");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    fixture.write("src/deep/inner.rs", "pub fn two() -> i32 { 2 }\n");
    fixture.write("beside/other.rs", "pub fn three() -> i32 { 3 }\n");

    let sample = fixture.sample_rooted_at("src");
    let named: Vec<&String> = sample.scopes.folders.keys().collect();
    assert!(
        named.iter().all(|p| !p.starts_with('/')),
        "no folder row is an absolute path: {named:?}"
    );
    assert!(
        named.iter().all(|p| !p.contains("beside")),
        "a folder outside the watched root is not a row: {named:?}"
    );
    assert!(
        named.iter().any(|p| p.as_str() == "deep"),
        "the watched tree is still named plainly: {named:?}"
    );
}

/// The tally and the rows are one population, so the denominator under
/// `unreadable` counts exactly the folders the movers list can name.
#[test]
fn the_shape_tally_counts_the_folders_the_reading_names() {
    let fixture = Fixture::new("tally-population");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    fixture.write("src/deep/inner.rs", "pub fn two() -> i32 { 2 }\n");
    fixture.write("beside/other.rs", "pub fn three() -> i32 { 3 }\n");

    let sample = fixture.sample_rooted_at("src");
    assert_eq!(
        sample.metrics.shape.total() as usize,
        sample.scopes.folders.len(),
        "every tallied folder is a row and no other: {:?}",
        sample.scopes.folders.keys().collect::<Vec<_>>()
    );
}

/// The headline figure is the analyzer's own rollup, not a mean this module
/// took for itself — the same LOC-weighted number the browser UI's repo tile
/// shows, so an operator can read one against the other.
#[test]
fn the_score_is_the_rollup_the_analyzer_already_computed() {
    let fixture = Fixture::new("score-rollup");
    fixture.write("src/app.rs", &overfull("busy"));
    fixture.write("src/small.rs", "pub fn one() -> i32 { 1 }\n");

    let config = Config::for_path(&fixture.0);
    let result = Analyzer::new(config).analyze().unwrap();
    let graph = DependencyGraph::from_analysis(&result);
    let sample = fixture.sample();

    // The folder whose rollup covers every analyzed entity: the shallowest
    // one the analyzer enumerated.
    let top = graph
        .folder_metrics()
        .iter()
        .min_by_key(|f| Path::new(&f.path).components().count())
        .expect("the fixture has folders");
    assert_eq!(
        sample.metrics.score, top.metrics.avg_quality,
        "the tile is {}'s rollup verbatim",
        top.path
    );
    assert!(
        sample.metrics.score > 0.0,
        "a tree holding an overfull head does not score a perfect zero"
    );
}

/// The root the operator named is not always a folder the analyzer scored.
///
/// `graph.rs` enumerates folders upward from the *common parent* of the
/// analyzed files, so a fixture whose every source sits in `src/` gets no
/// rollup for the directory above it. Reading the score off `.` alone would
/// fall back to `0.00` there — a perfect score painted over a tree nobody
/// measured, which is the one way this tile can lie.
#[test]
fn the_score_survives_a_root_the_analyzer_never_enumerated() {
    let fixture = Fixture::new("score-below-root");
    fixture.write("src/app.rs", &overfull("busy"));
    let sample = fixture.sample();

    assert!(
        !sample
            .scopes
            .folders
            .contains_key("."),
        "the fixture's sources all sit below the root, so the root is unscored: {:?}",
        sample.scopes.folders.keys().collect::<Vec<_>>()
    );
    assert!(
        sample.metrics.score > 0.0,
        "and the score still comes off the folder that does cover them"
    );
}

/// The four tier counts and their total, on one line under the grid — the
/// distribution the `unreadable` tile above is the headline of.
#[test]
fn the_shape_ladder_is_drawn_under_the_grid() {
    let mut series = Series::new(shaped(0, ShapeCounts::default()), 100);
    series.push(shaped(
        1,
        ShapeCounts {
            cyclic: 2,
            tangled: 3,
            hierarchical: 40,
            fractal: 5,
        },
    ));
    let screen = drawn(&view(&series, None), 110, 30);

    assert!(
        screen.contains("folders"),
        "the line names its population:\n{screen}"
    );
    for tier in ["50", "5 fractal", "40 hierarchical", "3 tangled", "2 cyclic"] {
        assert!(
            screen.contains(tier),
            "the ladder is drawn whole — missing {tier:?}:\n{screen}"
        );
    }
}

/// A tier a folder can arrive at from either direction moves without a
/// verdict. `hierarchical` gaining three is a tangle cleaned up or three
/// fractal folders sagging, and the count cannot say which.
#[test]
fn only_the_two_ends_of_the_ladder_are_read_as_good_or_bad() {
    let ends: Vec<(&str, Option<bool>)> = draw::ladder_tiers()
        .iter()
        .map(|(label, up_is_bad)| (*label, *up_is_bad))
        .collect();
    assert_eq!(
        ends,
        vec![
            ("fractal", Some(false)),
            ("hierarchical", None),
            ("tangled", None),
            ("cyclic", Some(true)),
        ]
    );
}

/// A reading carrying a shape distribution, which is all the ladder reads.
fn shaped(seq: u64, shape: ShapeCounts) -> Sample {
    let mut sample = reading(seq, 0);
    sample.metrics.shape = shape;
    sample
}

/// The panel reserves a row for every line the grid draws, at every width.
///
/// The regression: the height was resolved from the panel's *outer* width and
/// the layout from its inner one, so at exactly 78 and 79 columns a two-column
/// height was reserved for a one-column grid and the bottom four tiles were
/// clipped without a mark. A `Paragraph` does not report the lines it could
/// not fit, which is why this is a test and not something a reader would see.
#[test]
fn the_tile_grid_fits_the_height_reserved_for_it() {
    const TILES: u16 = 11;
    for outer in 0..=200u16 {
        // What `rows` will produce, resolved the way `rows` resolves it.
        let drawn = match draw::paired(draw::inner_width(outer)) {
            true => TILES.div_ceil(2),
            false => TILES,
        };
        let reserved = draw::grid_rows(TILES, outer);
        assert!(
            reserved >= drawn,
            "at {outer} columns the panel reserves {reserved} rows for {drawn} lines"
        );
    }
}

/// The watcher reports absolute paths. A reading that carried them raw would
/// name a file one way in its `changed` list and another in its movers row.
#[test]
fn the_changed_list_is_spelled_like_every_other_path_in_the_reading() {
    let fixture = Fixture::new("changed-paths");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    let absolute = fixture.0.join("src/app.rs").display().to_string();

    let sample = fixture.measure_after(false, vec![absolute]);
    assert_eq!(sample.changed, vec!["src/app.rs".to_string()]);
    assert_eq!(sample.changed_total, 1);
}

// ---------------------------------------------------------------------
//  The series
// ---------------------------------------------------------------------

/// A reading carrying one figure, which is all the arithmetic tests read.
fn reading(seq: u64, smells: u32) -> Sample {
    Sample {
        seq,
        at_ms: seq * 1000,
        analysis_ms: 0,
        head: None,
        baseline: false,
        changed: Vec::new(),
        changed_total: 0,
        metrics: Metrics {
            smells,
            ..Metrics::default()
        },
        smells_by_kind: BTreeMap::new(),
        scopes: Scopes::default(),
    }
}

/// The same reading, taken while sitting on a commit.
fn reading_on(seq: u64, smells: u32, sha: &str) -> Sample {
    Sample {
        head: Some(commit(sha)),
        ..reading(seq, smells)
    }
}

/// A reading of a checkout, which is what a commit baseline is made of.
fn checkout_of(sha: &str, smells: u32) -> Sample {
    Sample {
        baseline: true,
        ..reading_on(0, smells, sha)
    }
}

fn commit(sha: &str) -> Commit {
    Commit {
        sha: sha.to_string(),
        subject: format!("whatever {sha} did"),
    }
}

fn smells(m: &Metrics) -> f64 {
    m.smells as f64
}

#[test]
fn the_ring_holds_its_capacity_and_no_more() {
    let mut series = Series::new(reading(0, 1), 3);
    for seq in 1..10 {
        series.push(reading(seq, seq as u32));
    }
    assert_eq!(series.len(), 3);
    assert_eq!(series.latest().seq, 9);
}

#[test]
fn deltas_are_against_the_baseline_and_not_the_previous_reading() {
    let mut series = Series::new(reading(0, 10), 100);
    series.push(reading(1, 12));
    series.push(reading(2, 13));

    assert_eq!(series.delta(smells), 3.0, "13 - 10, not 13 - 12");
    assert_eq!(series.since_baseline(), 2);
}

#[test]
fn re_baselining_moves_the_zero_to_now() {
    let mut series = Series::new(reading(0, 10), 100);
    series.push(reading(1, 13));
    series.rebaseline();
    assert_eq!(series.delta(smells), 0.0);
    assert_eq!(series.since_baseline(), 0);

    series.push(reading(2, 15));
    assert_eq!(series.delta(smells), 2.0, "15 - 13, the new baseline");
}

#[test]
fn a_baseline_older_than_the_ring_still_answers() {
    // The baseline is deliberately not in the ring, so a long session cannot
    // silently start measuring from whatever the ring happens to still hold.
    let mut series = Series::new(reading(0, 10), 2);
    for seq in 1..20 {
        series.push(reading(seq, 20));
    }
    assert_eq!(series.delta(smells), 10.0);
}

#[test]
fn a_flat_series_draws_a_flat_line_and_not_a_full_one() {
    let mut series = Series::new(reading(0, 7), 100);
    series.push(reading(1, 7));
    assert_eq!(series.spark(smells, 8), "▁▁");
}

#[test]
fn a_rising_series_ends_higher_than_it_starts() {
    let mut series = Series::new(reading(0, 1), 100);
    series.push(reading(1, 5));
    series.push(reading(2, 9));
    let spark = series.spark(smells, 8);
    let chars: Vec<char> = spark.chars().collect();
    assert_eq!(chars.len(), 3);
    assert!(chars[0] < chars[2], "{spark} should climb");
}

// ---------------------------------------------------------------------
//  The measuring loop's two rules
// ---------------------------------------------------------------------

/// The floor is a gap *between* runs. Stamped at the start of an analysis
/// instead of its end, a run costing more than the floor satisfied the guard
/// the instant it returned, and the loop re-measured back to back with no idle
/// — so the flag stopped working on exactly the large trees its help text
/// names (MON-005).
///
/// The rule is tested here rather than in the loop because a clock inside the
/// loop is what made it untestable in the first place.
#[test]
fn nothing_is_due_before_the_floor_has_elapsed() {
    let floor = std::time::Duration::from_millis(2000);
    let short = std::time::Duration::from_millis(1999);
    assert!(!watch::due(true, false, short, floor), "under the floor");
    assert!(watch::due(true, false, floor, floor), "at the floor");
}

#[test]
fn nothing_is_due_with_nothing_pending_or_while_paused() {
    let floor = std::time::Duration::from_millis(10);
    let long = std::time::Duration::from_secs(60);
    assert!(!watch::due(false, false, long, floor), "nothing to measure");
    assert!(!watch::due(true, true, long, floor), "the operator paused it");
    assert!(watch::due(true, false, long, floor));
}

fn dashboard() -> super::Dashboard {
    super::Dashboard {
        repo: "fixture".to_string(),
        series: None,
        movers: Vec::new(),
        movers_state: ratatui::widgets::ListState::default(),
        history: 100,
        sink: crate::activity::Sink::quiet(),
        paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rebase: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        pending_base: None,
        failed: 0,
        against: None,
    }
}

/// An analysis that could not complete leaves every number on screen correct
/// about an *older* tree. Drawing them under a green `watching` is the
/// frozen-but-live failure `drive` breaks the loop over, reached by another
/// route (MON-006).
#[test]
fn a_failed_reading_leaves_the_series_alone_and_marks_it_stale() {
    let mut dash = dashboard();
    assert!(dash.record(Reading::Took(Box::new(reading(0, 5)))));
    assert!(
        !dash.record(Reading::Failed),
        "a failure moved nothing, so there are no movers to recompute"
    );

    assert_eq!(dash.failed, 1);
    assert_eq!(
        dash.series.as_ref().unwrap().latest().metrics.smells,
        5,
        "the last good reading still stands — a failure is not a reading of zero"
    );
    assert_eq!(dash.series.as_ref().unwrap().len(), 1);
}

#[test]
fn consecutive_failures_are_counted_and_a_good_reading_clears_them() {
    let mut dash = dashboard();
    dash.record(Reading::Took(Box::new(reading(0, 5))));
    dash.record(Reading::Failed);
    dash.record(Reading::Failed);
    assert_eq!(dash.failed, 2);

    dash.record(Reading::Took(Box::new(reading(1, 7))));
    assert_eq!(dash.failed, 0, "the tree is readable again");
    assert_eq!(dash.series.as_ref().unwrap().latest().metrics.smells, 7);
}

// ---------------------------------------------------------------------
//  The commit baseline (MON-007)
// ---------------------------------------------------------------------

#[test]
fn an_unset_baseline_flag_asks_for_head_and_forgives_a_tree_with_none() {
    let Wanted::Commit { reference, strict } = Wanted::from_flag(None) else {
        panic!("the default is a commit");
    };
    assert_eq!(reference, "HEAD");
    assert!(
        !strict,
        "a directory that is not a repository is not a typo to report"
    );
}

/// The other half of the same rule: a ref the operator typed and git cannot
/// resolve is a mistake, and one that fails before the screen is taken.
#[test]
fn a_ref_the_operator_typed_is_strict() {
    let Wanted::Commit { reference, strict } = Wanted::from_flag(Some("v1.2.3".to_string())) else {
        panic!("a named ref is a commit");
    };
    assert_eq!(reference, "v1.2.3");
    assert!(strict);
}

#[test]
fn the_old_behaviour_is_still_reachable_by_name() {
    assert!(matches!(
        Wanted::from_flag(Some("working".to_string())),
        Wanted::Working
    ));
    assert!(
        matches!(Wanted::from_flag(Some("WORKING".to_string())), Wanted::Working),
        "however it is typed"
    );
}

/// The point of the whole feature: work already in the tree when the session
/// opens is *inside* the first delta, rather than being the zero it is
/// measured from.
#[test]
fn the_first_reading_already_moves_against_a_commit_baseline() {
    let base = checkout_of("aaaaaaa", 10);
    let series = Series::anchored(base, Anchor::Commit(commit("aaaaaaa")), reading(1, 17), 100);

    assert_eq!(series.delta(smells), 7.0, "17 against the commit's 10");
    assert_eq!(
        series.since_baseline(),
        1,
        "one reading stands against this baseline, and it is not the baseline"
    );
    assert_eq!(series.len(), 1, "the commit is not a notch of the session");
}

#[test]
fn a_commit_baseline_says_which_commit() {
    let series = Series::anchored(
        checkout_of("aaaaaaa", 3),
        Anchor::Commit(commit("aaaaaaa")),
        reading_on(1, 3, "aaaaaaa"),
        100,
    );
    assert_eq!(series.anchor(), &Anchor::Commit(commit("aaaaaaa")));
    assert!(
        series.behind_head().is_none(),
        "the pin is still the commit in force"
    );
}

/// The pin is the whole reason `B` exists: a swarm that commits four times an
/// hour must not reset the operator's numbers four times, so the dashboard
/// says the pin has fallen behind instead of quietly moving it.
#[test]
fn a_pinned_baseline_reports_itself_behind_once_head_moves() {
    let mut series = Series::anchored(
        checkout_of("aaaaaaa", 3),
        Anchor::Commit(commit("aaaaaaa")),
        reading_on(1, 5, "aaaaaaa"),
        100,
    );
    assert!(series.behind_head().is_none());

    series.push(reading_on(2, 6, "bbbbbbb"));
    assert_eq!(
        series.behind_head().map(|c| c.sha.clone()),
        Some("aaaaaaa".to_string()),
        "the deltas are still measured from a commit that is no longer HEAD"
    );
    assert_eq!(series.delta(smells), 3.0, "and they did not move on their own");
}

#[test]
fn a_reading_baseline_is_never_behind_head() {
    let mut series = Series::new(reading_on(0, 3, "aaaaaaa"), 100);
    series.push(reading_on(1, 4, "bbbbbbb"));
    assert!(
        series.behind_head().is_none(),
        "a moment does not claim to be a commit, so it cannot fall behind one"
    );
}

/// `B`, arriving an analysis after it was pressed.
#[test]
fn snapping_to_head_moves_the_zero_and_keeps_the_reading_it_is_measured_against() {
    let mut series = Series::anchored(
        checkout_of("aaaaaaa", 10),
        Anchor::Commit(commit("aaaaaaa")),
        reading_on(1, 17, "bbbbbbb"),
        100,
    );
    assert_eq!(series.delta(smells), 7.0);

    series.rebase(checkout_of("bbbbbbb", 15), Anchor::Commit(commit("bbbbbbb")));
    assert_eq!(series.delta(smells), 2.0, "17 against the new commit's 15");
    assert!(series.behind_head().is_none(), "the pin caught up");
    assert_eq!(
        series.since_baseline(),
        1,
        "the reading on screen already stands against the new zero"
    );
}

/// The opening order the measuring thread produces: the commit is measured
/// before the tree is, so its reading arrives with nothing to attach to.
#[test]
fn a_baseline_arriving_before_the_first_reading_is_held_rather_than_drawn() {
    let mut dash = dashboard();
    assert!(
        !dash.record(Reading::Base(Box::new(checkout_of("aaaaaaa", 10)))),
        "a commit's numbers are not a reading of the tree anyone is editing"
    );
    assert!(
        dash.series.is_none(),
        "so nothing is on screen until the tree itself has been read"
    );

    assert!(dash.record(Reading::Took(Box::new(reading_on(1, 17, "aaaaaaa")))));
    let series = dash.series.as_ref().unwrap();
    assert_eq!(series.latest().metrics.smells, 17, "the tree, not the commit");
    assert_eq!(series.delta(smells), 7.0);
    assert_eq!(series.anchor(), &Anchor::Commit(commit("aaaaaaa")));
}

#[test]
fn a_baseline_that_arrives_mid_session_replaces_the_zero_at_once() {
    let mut dash = dashboard();
    dash.record(Reading::Took(Box::new(reading_on(0, 10, "aaaaaaa"))));
    dash.record(Reading::Took(Box::new(reading_on(1, 14, "bbbbbbb"))));
    assert_eq!(dash.series.as_ref().unwrap().delta(smells), 4.0);

    assert!(dash.record(Reading::Base(Box::new(checkout_of("bbbbbbb", 12)))));
    let series = dash.series.as_ref().unwrap();
    assert_eq!(series.delta(smells), 2.0, "14 against the commit's 12");
    assert_eq!(series.latest().seq, 1, "and the tree's last reading still stands");
}

/// A commit is not a tick: it cannot clear the stale marker, because the tree
/// is exactly as unread as it was before.
#[test]
fn a_baseline_reading_does_not_clear_the_stale_count() {
    let mut dash = dashboard();
    dash.record(Reading::Took(Box::new(reading_on(0, 10, "aaaaaaa"))));
    dash.record(Reading::Failed);
    assert_eq!(dash.failed, 1);

    dash.record(Reading::Base(Box::new(checkout_of("bbbbbbb", 9))));
    assert_eq!(
        dash.failed, 1,
        "the tree still could not be read; only a reading of it says otherwise"
    );
}

// ---------------------------------------------------------------------
//  Two commits, one frame (MON-008)
// ---------------------------------------------------------------------

/// A resolved `--baseline` or `--against`, without asking git for one.
fn plan(sha: &str) -> Plan {
    Plan {
        reference: sha.to_string(),
        commit: commit(sha),
    }
}

/// `--against working` is not the old behaviour under a new name: without the
/// flag the head side already *is* the working tree, measured live. Refused
/// with the sentence that says so rather than resolved into a session that
/// watches nothing.
#[test]
fn the_head_side_of_a_comparison_has_to_be_a_commit() {
    let refused = baseline::pinned("working", Path::new("/nonexistent"))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("takes a commit"), "{refused}");
}

/// The other half of MON-007's strictness rule, on the other flag: `--against`
/// is only ever there because the operator typed it, so an unresolvable ref is
/// always a mistake — there is no tolerant default to fall back to.
#[test]
fn an_against_ref_that_does_not_resolve_is_a_mistake_and_not_a_fallback() {
    let refused = baseline::pinned("v9.9.9", Path::new("/nonexistent"))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("cannot resolve --against v9.9.9"), "{refused}");
}

#[test]
fn two_commits_pair_into_a_comparison() {
    let ends = Ends::pair(Some(plan("aaaaaaa")), Some(plan("bbbbbbb"))).unwrap();
    let Ends::Between { from, to } = &ends else {
        panic!("two commits are a comparison");
    };
    assert_eq!(from.commit.sha, "aaaaaaa");
    assert_eq!(to.commit.sha, "bbbbbbb");
    assert_eq!(
        ends.against().map(|c| c.sha.as_str()),
        Some("bbbbbbb"),
        "the header names the state being asked about"
    );
}

/// The one pairing that cannot be drawn: a comparison's two ends both have to
/// be states a second reader can resolve, and `--baseline working` is a moment
/// only the running process can point at.
#[test]
fn a_comparison_cannot_be_measured_from_a_moment() {
    let refused = Ends::pair(None, Some(plan("bbbbbbb")))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("--baseline"), "{refused}");
}

#[test]
fn without_the_flag_the_session_still_watches() {
    let watching = Ends::pair(Some(plan("aaaaaaa")), None).unwrap();
    assert!(matches!(watching, Ends::Watching(Some(_))));
    assert!(
        watching.against().is_none(),
        "the head side is the tree, which is not a commit to name"
    );
    assert!(
        matches!(Ends::pair(None, None).unwrap(), Ends::Watching(None)),
        "a tree with no git still watches"
    );
}

// ---------------------------------------------------------------------
//  The comparison, drawn
// ---------------------------------------------------------------------

/// A frame of a session comparing `aaaaaaa` with `bbbbbbb`.
fn compared() -> Series {
    Series::anchored(
        checkout_of("aaaaaaa", 10),
        Anchor::Commit(commit("aaaaaaa")),
        // The head side is a checkout too, and — unlike the zero — is not
        // marked as one in the log.
        reading_on(0, 17, "bbbbbbb"),
        100,
    )
}

fn view<'a>(series: &'a Series, against: Option<&'a Commit>) -> draw::View<'a> {
    draw::View {
        repo: "fixture",
        series: Some(series),
        movers: &[],
        running: None,
        paused: false,
        failed: 0,
        against,
    }
}

/// The whole screen as text, one line per row.
///
/// Drawn into ratatui's in-memory backend rather than asserted piece by piece,
/// because what is being tested is what the operator can read: three of the
/// footer's five keys are gone, the status word is not `watching`, and the
/// border names both states.
fn drawn(view: &draw::View, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let mut state = ratatui::widgets::ListState::default();
    terminal
        .draw(|frame| draw::dashboard(frame, view, &mut state))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .filter_map(|x| buffer.cell((x, y)).map(|c| c.symbol().to_string()))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_comparison_names_both_of_its_states() {
    let series = compared();
    let commit = commit("bbbbbbb");
    let screen = drawn(&view(&series, Some(&commit)), 100, 20);
    assert!(
        screen.contains("aaaaaaa → bbbbbbb"),
        "the border says what is being compared with what:\n{screen}"
    );
    assert!(
        screen.contains("comparison"),
        "and the corner does not claim to be watching a tree:\n{screen}"
    );
    assert!(!screen.contains("watching"), "{screen}");
}

/// The deltas are the point, and they are the same arithmetic a live session
/// does — the head reading against the zero, both of them checkouts.
#[test]
fn a_comparison_measures_the_head_commit_against_the_baseline_one() {
    assert_eq!(compared().delta(smells), 7.0, "17 at bbbbbbb, 10 at aaaaaaa");
}

/// `behind_head` is true of this series — the head reading was taken at a
/// different commit than the pin — and saying so would be about a tree the
/// screen is not describing, offering a key that cannot answer it.
#[test]
fn a_comparison_never_says_head_has_moved() {
    let series = compared();
    assert!(
        series.behind_head().is_some(),
        "the two ends differ, which is what a live session calls falling behind"
    );
    let commit = commit("bbbbbbb");
    let screen = drawn(&view(&series, Some(&commit)), 100, 20);
    assert!(!screen.contains("HEAD has moved"), "{screen}");
}

#[test]
fn a_comparison_offers_only_the_keys_that_can_still_do_something() {
    let series = compared();
    let commit = commit("bbbbbbb");
    let screen = drawn(&view(&series, Some(&commit)), 100, 20);
    assert!(screen.contains("q quit"), "{screen}");
    assert!(screen.contains("↑↓ scroll"), "{screen}");
    for gone in ["re-baseline", "at HEAD", "p pause"] {
        assert!(
            !screen.contains(gone),
            "a comparison cannot answer `{gone}`:\n{screen}"
        );
    }
}

/// Two states are not a series. One notch scaled against itself is a flat
/// block, which on a screen whose whole subject is what moved reads as
/// "nothing is moving".
#[test]
fn a_comparison_draws_no_sparkline() {
    let series = compared();
    let commit = commit("bbbbbbb");
    let still = drawn(&view(&series, Some(&commit)), 100, 20);
    let live = drawn(&view(&series, None), 100, 20);
    assert!(live.contains('▁'), "a watching session draws its notches");
    assert!(!still.contains('▁'), "{still}");
}

/// The three keys the footer stops offering are refused, not merely unlisted:
/// a `b` that quietly moved the zero would leave the header naming two commits
/// while the deltas were measured from neither.
#[test]
fn the_keys_that_move_a_zero_do_nothing_in_a_comparison() {
    let mut dash = dashboard();
    dash.against = Some(commit("bbbbbbb"));
    dash.record(Reading::Base(Box::new(checkout_of("aaaaaaa", 10))));
    dash.record(Reading::Took(Box::new(reading_on(0, 17, "bbbbbbb"))));

    for key in ['b', 'B', 'p'] {
        assert!(!dash.press(key_press(key)), "{key} does not quit either");
    }
    let series = dash.series.as_ref().unwrap();
    assert_eq!(series.anchor(), &Anchor::Commit(commit("aaaaaaa")));
    assert_eq!(series.delta(smells), 7.0, "the zero did not move");
    assert!(
        !dash.rebase.load(std::sync::atomic::Ordering::Relaxed),
        "and no checkout was asked for"
    );
    assert!(!dash.paused.load(std::sync::atomic::Ordering::Relaxed));
    assert!(dash.press(key_press('q')), "quitting still works");
}

fn key_press(c: char) -> ratatui::crossterm::event::KeyEvent {
    ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char(c),
        ratatui::crossterm::event::KeyModifiers::NONE,
    )
}

// ---------------------------------------------------------------------
//  The comparison, measured
// ---------------------------------------------------------------------

/// Both ends of a real comparison, through the real analyzer and two real
/// checkouts.
///
/// The assertion the unit tests cannot make: the two readings are taken in
/// different temporary worktrees, under paths that share nothing, and the
/// movers list can only name a file if both sides keyed it the same way
/// (5de97d0 is the commit where that stopped being true for folders).
#[test]
fn a_comparison_reads_both_commits_and_names_what_moved_between_them() {
    let fixture = Fixture::new("compare");
    fixture.git_init();
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    let from = fixture.commit("plain");
    fixture.write("src/app.rs", &overfull("wide"));
    let to = fixture.commit("overfull");

    let (tx, rx) = std::sync::mpsc::channel::<Reading>();
    let m = measuring_in(&fixture, tx);
    let stop = m.stop.clone();
    let thread = std::thread::spawn(move || compare::run(m, from, to));

    let base = expect_sample(&rx, "the zero");
    let head = expect_sample(&rx, "the head side");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    thread.join().unwrap().unwrap();

    assert_eq!(base.metrics.smells, 0, "the first commit is clean");
    assert_eq!(head.metrics.smells, 1, "the second holds the overfull head");
    let moved = movers::between(&base.scopes, &head.scopes, 10);
    assert_eq!(
        moved.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
        vec!["src/app.rs"],
        "both checkouts key the same file the same way"
    );
}

/// The order and the marking, which is what a `--log` of a comparison is read
/// back through: exactly one of the two lines is the state the other is
/// measured from.
#[test]
fn the_zero_arrives_first_and_is_the_only_line_marked_as_one() {
    let fixture = Fixture::new("compare-order");
    fixture.git_init();
    fixture.write("src/app.rs", "pub fn one() -> i32 { 1 }\n");
    let from = fixture.commit("first");
    fixture.write("src/app.rs", "pub fn one() -> i32 { 2 }\n");
    let to = fixture.commit("second");
    let (from_sha, to_sha) = (from.commit.sha.clone(), to.commit.sha.clone());

    let (tx, rx) = std::sync::mpsc::channel::<Reading>();
    let m = measuring_in(&fixture, tx);
    let stop = m.stop.clone();
    let thread = std::thread::spawn(move || compare::run(m, from, to));

    let first = rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap();
    let Reading::Base(base) = first else {
        panic!("the zero is measured and sent before the state it is measured from");
    };
    let second = rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap();
    let Reading::Took(head) = second else {
        panic!("the head side is a reading, not a second zero");
    };
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    thread.join().unwrap().unwrap();

    assert!(base.baseline, "the zero says so");
    assert_eq!(base.head.as_ref().map(|c| c.sha.clone()), Some(from_sha));
    assert!(
        !head.baseline,
        "and the head side does not, or a log of this session has two zeros and no reading"
    );
    assert_eq!(head.head.as_ref().map(|c| c.sha.clone()), Some(to_sha));
}

/// Whichever kind of reading it is — the assertions that do not care live here.
fn expect_sample(rx: &std::sync::mpsc::Receiver<Reading>, what: &str) -> Sample {
    match rx.recv_timeout(std::time::Duration::from_secs(120)) {
        Ok(Reading::Base(sample)) | Ok(Reading::Took(sample)) => *sample,
        other => panic!("{what} never arrived: {}", named(&other)),
    }
}

fn named(reading: &std::result::Result<Reading, std::sync::mpsc::RecvTimeoutError>) -> String {
    match reading {
        Ok(Reading::Failed) => "a failed reading".to_string(),
        Ok(_) => "a reading".to_string(),
        Err(e) => e.to_string(),
    }
}

/// A measuring thread's worth of setup, pointed at a fixture repository.
fn measuring_in(fixture: &Fixture, tx: std::sync::mpsc::Sender<Reading>) -> Measure {
    Measure {
        config: Config::for_path(&fixture.0),
        root: fixture.0.clone(),
        repo_root: fixture.0.clone(),
        debounce: std::time::Duration::from_millis(1000),
        min_interval: std::time::Duration::from_millis(2000),
        tx,
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rebase: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        baseline: None,
        log: None,
    }
}

// ---------------------------------------------------------------------
//  The movers
// ---------------------------------------------------------------------

fn scopes(files: &[(&str, u32, u32)], folders: &[(&str, ShapePattern)]) -> Scopes {
    Scopes {
        files: files
            .iter()
            .map(|(path, cplx, smells)| {
                (
                    path.to_string(),
                    FileRow {
                        max_cyclomatic: *cplx,
                        smells: *smells,
                    },
                )
            })
            .collect(),
        folders: folders
            .iter()
            .map(|(path, pattern)| (path.to_string(), *pattern))
            .collect(),
    }
}

#[test]
fn a_file_that_got_more_complex_is_listed_as_worse() {
    let base = scopes(&[("src/app.rs", 22, 0)], &[]);
    let now = scopes(&[("src/app.rs", 31, 0)], &[]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].path, "src/app.rs");
    assert!(moved[0].worse);
    assert_eq!(moved[0].what, "cplx 22→31");
}

#[test]
fn a_file_that_improved_is_listed_too_and_after_the_regressions() {
    let base = scopes(&[("src/better.rs", 40, 0), ("src/worse.rs", 5, 0)], &[]);
    let now = scopes(&[("src/better.rs", 33, 0), ("src/worse.rs", 6, 0)], &[]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 2);
    assert_eq!(moved[0].path, "src/worse.rs", "regressions come first");
    assert!(moved[0].worse);
    assert!(!moved[1].worse);
}

#[test]
fn an_unchanged_file_is_not_a_mover() {
    let base = scopes(&[("src/app.rs", 4, 1)], &[]);
    let now = scopes(&[("src/app.rs", 4, 1)], &[]);

    assert!(movers::between(&base, &now, 10).is_empty());
}

#[test]
fn a_folder_that_fell_down_the_shape_ladder_is_listed() {
    let base = scopes(&[], &[("src/analyzer", ShapePattern::Fractal)]);
    let now = scopes(&[], &[("src/analyzer", ShapePattern::Tangled)]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 1);
    assert!(moved[0].worse);
    assert_eq!(moved[0].what, "shape fractal→tangled");
}

#[test]
fn a_folder_that_climbed_the_ladder_is_not_a_regression() {
    let base = scopes(&[], &[("src/analyzer", ShapePattern::Cyclic)]);
    let now = scopes(&[], &[("src/analyzer", ShapePattern::Hierarchical)]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 1);
    assert!(!moved[0].worse);
}

#[test]
fn a_new_file_appears_only_once_it_is_carrying_something() {
    let base = scopes(&[], &[]);
    let quiet = scopes(&[("src/small.rs", 2, 0)], &[]);
    assert!(
        movers::between(&base, &quiet, 10).is_empty(),
        "every file a swarm adds is not a finding"
    );

    let smelly = scopes(&[("src/small.rs", 2, 1)], &[]);
    let moved = movers::between(&base, &smelly, 10);
    assert_eq!(moved.len(), 1);
    assert!(moved[0].what.starts_with("new"));
}

/// The single largest improvement anyone can make to a bad file is to delete
/// it, and until MON-004 that was the one move the list could not show: the
/// `smells` tile fell and the list under it said nothing had moved.
#[test]
fn a_deleted_file_is_listed_as_an_improvement_naming_what_left() {
    let base = scopes(&[("src/awful.rs", 31, 6)], &[]);
    let now = scopes(&[], &[]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].path, "src/awful.rs");
    assert!(!moved[0].worse, "a deletion is never a regression");
    assert_eq!(moved[0].what, "deleted  was cplx 31  -6 smell");
}

/// The mirror of the floor `arrived` applies, and for the mirror reason: a
/// swarm clearing two hundred generated files must not fill the list.
#[test]
fn a_deleted_file_below_the_floor_is_not_a_row() {
    let base = scopes(&[("src/tiny.rs", 2, 0)], &[]);
    let now = scopes(&[], &[]);

    assert!(movers::between(&base, &now, 10).is_empty());
}

#[test]
fn a_deletion_and_an_arrival_rank_on_the_same_scale() {
    let base = scopes(&[("src/gone.rs", 10, 3)], &[]);
    let now = scopes(&[("src/new.rs", 10, 1)], &[]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 2);
    // The arrival is the regression, so it leads regardless of weight; the
    // point here is that both are present and both carry their numbers.
    assert_eq!(moved[0].path, "src/new.rs");
    assert!(moved[0].worse);
    assert_eq!(moved[1].path, "src/gone.rs");
    assert!(!moved[1].worse);
}

/// A folder that did not exist at the baseline and now reads `cyclic` is the
/// loudest thing that can happen to a tree's shape — and it is exactly what a
/// swarm asked to extract a module produces. It used to get no row at all.
#[test]
fn a_folder_the_baseline_never_saw_is_listed_with_its_tier() {
    let base = scopes(&[], &[]);
    let now = scopes(&[], &[("src/extracted", ShapePattern::Cyclic)]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].path, "src/extracted");
    assert!(moved[0].worse, "a new folder below hierarchical is a fall");
    assert_eq!(moved[0].what, "new  cyclic");
}

#[test]
fn a_new_folder_that_is_readable_is_not_a_regression() {
    let base = scopes(&[], &[]);
    let now = scopes(&[], &[("src/extracted", ShapePattern::Fractal)]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 1);
    assert!(!moved[0].worse, "somebody extracted a clean module");
    assert_eq!(moved[0].what, "new  fractal");
}

/// Never a regression, and that is a fact about the screen rather than a
/// judgement: `unreadable` is the only shape figure drawn, and a folder
/// leaving can lower it or leave it alone.
#[test]
fn a_deleted_folder_is_listed_and_is_never_a_regression() {
    let base = scopes(&[], &[("src/tangle", ShapePattern::Tangled)]);
    let now = scopes(&[], &[]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].path, "src/tangle");
    assert!(!moved[0].worse);
    assert_eq!(moved[0].what, "deleted  was tangled");
}

/// Two ticks that measured the same tree must not reorder the list under the
/// reader's cursor. The union walk is the risk: a `HashSet` merge would make
/// equal-weight rows swap places between ticks.
#[test]
fn the_union_walk_is_still_stable_between_two_identical_ticks() {
    let base = scopes(
        &[("src/gone_a.rs", 20, 2), ("src/gone_b.rs", 20, 2)],
        &[("src/left_a", ShapePattern::Cyclic), ("src/left_b", ShapePattern::Cyclic)],
    );
    let now = scopes(
        &[("src/new_a.rs", 20, 2), ("src/new_b.rs", 20, 2)],
        &[("src/here_a", ShapePattern::Cyclic), ("src/here_b", ShapePattern::Cyclic)],
    );

    let once: Vec<String> = movers::between(&base, &now, 20)
        .into_iter()
        .map(|m| m.path)
        .collect();
    let again: Vec<String> = movers::between(&base, &now, 20)
        .into_iter()
        .map(|m| m.path)
        .collect();
    assert_eq!(once, again);
    assert_eq!(once.len(), 8, "every arrival, departure and appearance: {once:?}");
}

#[test]
fn a_bigger_move_outranks_a_smaller_one() {
    let base = scopes(&[("src/a.rs", 10, 0), ("src/b.rs", 10, 0)], &[]);
    let now = scopes(&[("src/a.rs", 11, 0), ("src/b.rs", 25, 0)], &[]);

    let moved = movers::between(&base, &now, 10);
    assert_eq!(moved[0].path, "src/b.rs");
}

#[test]
fn the_list_is_capped() {
    let base = scopes(&[], &[]);
    let rows: Vec<(&str, u32, u32)> = vec![
        ("src/a.rs", 30, 1),
        ("src/b.rs", 30, 1),
        ("src/c.rs", 30, 1),
    ];
    let now = scopes(&rows, &[]);

    assert_eq!(movers::between(&base, &now, 2).len(), 2);
}

/// The shape counts are a ladder tally, and `unreadable` is the pair a
/// reader cannot follow — not all four tiers, and not just the cyclic ones.
#[test]
fn unreadable_folders_are_the_cyclic_and_tangled_ones() {
    let counts = ShapeCounts {
        cyclic: 1,
        tangled: 2,
        hierarchical: 4,
        fractal: 8,
    };
    assert_eq!(counts.unreadable(), 3);
}

/// The two halves of the drawn figure are counted over one population: the
/// denominator is every tier of the same ladder the numerator takes two rungs
/// of, so `unreadable` can never exceed `total` and the share is always a real
/// one.
#[test]
fn the_shape_total_is_every_tier_of_the_ladder() {
    let counts = ShapeCounts {
        cyclic: 1,
        tangled: 2,
        hierarchical: 4,
        fractal: 8,
    };
    assert_eq!(counts.total(), 15);
    assert!(counts.unreadable() <= counts.total());
}

/// A tree nothing has been measured in draws `0 / 0` rather than dividing.
/// The tile is two counts and never a quotient, which is what lets it say
/// "nothing measured" without a special case.
#[test]
fn an_unmeasured_tree_has_no_folders_in_any_tier() {
    let counts = ShapeCounts::default();
    assert_eq!(counts.total(), 0);
    assert_eq!(counts.unreadable(), 0);
}
