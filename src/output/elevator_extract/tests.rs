//! Tests for `--extract`.
//!
//! These run the *real* analyzer over a temporary spec directory
//! rather than hand-building an `AnalysisResult`. The extractor
//! depends on things only the post-merge passes produce — the
//! `unresolved` tag, cross-file `Contains` edges — so a hand-built
//! fixture would test a graph shape that never occurs in practice.
//!
//! The load-bearing test is [`slice_reparses_and_passes_check`]:
//! whatever else the emitter gets right, a slice that doesn't parse
//! is not a slice.

use super::*;
use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::models::file_info::Language;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------
// Harness
// ---------------------------------------------------------------

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temp directory that cleans up on drop, so a failing assertion
/// doesn't leave spec fixtures behind in `/tmp`.
struct SpecDir(PathBuf);

impl SpecDir {
    fn new(files: &[(&str, &str)]) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("nao-elv-extract-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).expect("create temp spec dir");
        for (name, body) in files {
            std::fs::write(dir.join(name), body).expect("write spec file");
        }
        Self(dir)
    }
}

impl Drop for SpecDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn analyze(dir: &SpecDir) -> crate::analyzer::AnalysisResult {
    let mut config = Config::for_path(&dir.0);
    config.analysis.languages.insert(Language::Elevator);
    Analyzer::new(config).analyze().expect("analysis succeeds")
}

/// Analyze `files`, extract `selectors`, return the slice.
fn slice_of(files: &[(&str, &str)], selectors: &[&str]) -> Slice {
    let dir = SpecDir::new(files);
    let result = analyze(&dir);
    let sels: Vec<String> = selectors.iter().map(|s| s.to_string()).collect();
    extract(&result, &sels, "test-spec").expect("extraction succeeds")
}

/// Re-analyze emitted slice text as a standalone spec and return the
/// checker's findings. This is the round-trip: emitter → parser →
/// checker, through the same pipeline the CLI uses.
fn check_slice(text: &str) -> Vec<crate::output::elevator_check::Finding> {
    let dir = SpecDir::new(&[("slice.elv", text)]);
    let result = analyze(&dir);
    crate::output::elevator_check::check(&result)
}

fn errors(findings: &[crate::output::elevator_check::Finding]) -> Vec<String> {
    findings
        .iter()
        .filter(|f| matches!(f.severity, crate::output::elevator_check::Severity::Error))
        .map(|f| f.message.clone())
        .collect()
}

/// A spec with two Categories, cross-cutting Concepts, a UI page and
/// a `references:` edge that crosses between Categories — enough
/// shape that every closure rule has something to bite on.
const SPEC: &str = r#"
c library {
    d: "Reusable building blocks."
    cr: "src/library/"
    f protocol
    f storage
}

c admin {
    d: "Operator-facing tooling."
    f audit
}

f protocol {
    d: "Versioned wire format."
    cr: "src/protocol/mod.rs"
    cr.test: "tests/protocol.rs"
    where: settings
    references: audit
    fu encode
    fu decode
}

f storage {
    d: "Durable persistence."
    fu write
}

f audit {
    d: "Who changed what."
}

fu f.protocol.encode {
    d: "Serialize a message."
    cr: "src/protocol/encode.rs"
}

fu f.protocol.decode {
    d: "Parse a message."
}

fu f.storage.write {
    d: "Append a record."
}

concept versioning {
    d: "Backward-compatible schema evolution."
    used_by: protocol, storage
}

ui settings {
    d: "Settings screen."
}
"#;

fn spec() -> Vec<(&'static str, &'static str)> {
    vec![("spec.elv", SPEC)]
}

// ---------------------------------------------------------------
// Closure rules
// ---------------------------------------------------------------

#[test]
fn selecting_a_feature_pulls_its_functionalities() {
    let s = slice_of(&spec(), &["f.protocol"]);
    assert!(s.text.contains("fu f.protocol.encode {"));
    assert!(s.text.contains("fu f.protocol.decode {"));
    // ...and nothing from the sibling Feature.
    assert!(!s.text.contains("fu f.storage.write {"));
}

#[test]
fn ancestors_come_along_with_child_lists_pruned() {
    let s = slice_of(&spec(), &["f.protocol"]);
    assert_eq!(s.ancestors, 1, "only `c library` is above the selection");
    // The Category is present and marked as context, not as a
    // complete definition of itself.
    assert!(s.text.contains("c library {    # context"));
    // Its `f storage` child is pruned away — the slice makes no
    // claim about a Feature the agent never touched.
    assert!(s.text.contains("    f protocol"));
    assert!(!s.text.contains("    f storage"));
    // Ancestor keeps its own identity fields.
    assert!(s.text.contains("d: \"Reusable building blocks.\""));
    assert!(s.text.contains("cr: \"src/library/\""));
}

#[test]
fn concepts_used_by_the_slice_are_carried_with_pruned_used_by() {
    let s = slice_of(&spec(), &["f.protocol"]);
    assert!(s.text.contains("concept versioning {"));
    // `storage` also uses the Concept but is outside the slice, so
    // it must not appear in the reconstructed `used_by:`.
    assert!(s.text.contains("used_by: protocol\n"));
    assert!(!s.text.contains("storage"));
}

#[test]
fn out_of_slice_reference_targets_are_name_only() {
    let s = slice_of(&spec(), &["f.protocol"]);
    // `f protocol` has `references: audit` (a Feature under the
    // other Category) and `where: settings` (a UI page). Both edges
    // survive; neither definition does.
    assert!(s.text.contains("references: audit"));
    assert!(s.text.contains("where: settings"));
    assert_eq!(s.name_only, 2);
    assert!(s.text.contains("\nf audit\n"), "bodyless definition");
    assert!(s.text.contains("\nui settings\n"), "bodyless definition");
    assert!(!s.text.contains("Who changed what"));
    assert!(!s.text.contains("Settings screen"));
    // The Category that owns the referenced Feature is not dragged in.
    assert!(!s.text.contains("c admin"));
}

#[test]
fn ancestors_do_not_drag_in_their_own_link_targets() {
    // Selecting a leaf makes `f protocol` an ancestor. Its
    // `references:`/`where:` edges point outside the touched scope
    // and must not inflate the slice.
    let s = slice_of(&spec(), &["fu.protocol.encode"]);
    assert!(s.text.contains("f protocol {    # context"));
    assert!(!s.text.contains("references:"));
    assert!(!s.text.contains("where:"));
    assert_eq!(s.name_only, 0);
}

#[test]
fn multiple_selectors_merge_into_one_slice() {
    let s = slice_of(&spec(), &["fu.protocol.encode", "f.storage"]);
    assert!(s.text.contains("fu f.protocol.encode {"));
    assert!(s.text.contains("fu f.storage.write {"));
    assert!(!s.text.contains("fu f.protocol.decode {"));
    // One shared Category ancestor, listing both branches.
    assert_eq!(s.ancestors, 2, "c library plus f protocol");
}

#[test]
fn code_refs_survive_including_tagged_ones() {
    let s = slice_of(&spec(), &["f.protocol"]);
    assert!(s.text.contains("cr: \"src/protocol/mod.rs\""));
    assert!(s.text.contains("cr.test: \"tests/protocol.rs\""));
    assert!(s.text.contains("cr: \"src/protocol/encode.rs\""));
}

#[test]
fn header_records_provenance_and_shape() {
    let s = slice_of(&spec(), &["f.protocol"]);
    assert!(s.text.contains("# Source:    test-spec"));
    assert!(s.text.contains("# Selection: f.protocol"));
    assert!(s.text.contains("2 functionalities"));
}

// ---------------------------------------------------------------
// Round-trip — the property that makes this an .elv emitter and not
// just another text renderer.
// ---------------------------------------------------------------

#[test]
fn slice_reparses_and_passes_check() {
    for selectors in [
        vec!["f.protocol"],
        vec!["fu.protocol.encode"],
        vec!["c.library"],
        vec!["concept.versioning"],
        vec!["fu.protocol.encode", "f.storage"],
    ] {
        let s = slice_of(&spec(), &selectors);
        let findings = check_slice(&s.text);
        assert!(
            errors(&findings).is_empty(),
            "slice for {:?} failed --check:\n{}\n--- slice ---\n{}",
            selectors,
            errors(&findings).join("\n"),
            s.text
        );
    }
}

#[test]
fn slice_of_a_whole_category_reproduces_its_subtree() {
    let s = slice_of(&spec(), &["c.library"]);
    for expected in [
        "c library {",
        "f protocol {",
        "f storage {",
        "fu f.protocol.encode {",
        "fu f.protocol.decode {",
        "fu f.storage.write {",
        "concept versioning {",
    ] {
        assert!(
            s.text.contains(expected),
            "missing `{}`:\n{}",
            expected,
            s.text
        );
    }
    // Selecting the top of a branch leaves nothing above it.
    assert_eq!(s.ancestors, 0);
    assert!(!s.text.contains("# context"));
}

#[test]
fn cross_file_containment_is_followed() {
    let files = vec![
        (
            "root.elv",
            "import \"features.elv\"\nc library { d: \"Blocks.\" f protocol }\n",
        ),
        (
            "features.elv",
            "f protocol { d: \"Wire format.\" fu encode }\nfu f.protocol.encode { d: \"Serialize.\" }\n",
        ),
    ];
    let s = slice_of(&files, &["f.protocol"]);
    // The Category is declared in a different file than the Feature;
    // the ancestor walk has to cross that boundary.
    assert_eq!(s.ancestors, 1);
    assert!(s.text.contains("c library {    # context"));
    assert!(s.text.contains("fu f.protocol.encode {"));
    assert!(errors(&check_slice(&s.text)).is_empty());
}

// ---------------------------------------------------------------
// Honest failure modes
// ---------------------------------------------------------------

#[test]
fn dangling_reference_is_dropped_not_repaired() {
    let files = vec![(
        "spec.elv",
        "c library { d: \"Blocks.\" f protocol }\nf protocol { d: \"Wire.\" references: ghost }\n",
    )];
    let s = slice_of(&files, &["f.protocol"]);
    assert_eq!(s.dropped_unresolved, 1);
    assert!(!s.text.contains("ghost"));
    assert!(s.text.contains("# Dropped:   1 edge(s)"));
    // A slice that emitted `f ghost` would look healthier than the
    // spec it came from.
    assert!(errors(&check_slice(&s.text)).is_empty());
}

#[test]
fn unknown_selector_errors_with_suggestions() {
    let dir = SpecDir::new(&spec());
    let result = analyze(&dir);
    let err = extract(&result, &["f.proto".to_string()], "test-spec")
        .expect_err("a name that matches nothing should not produce an empty slice");
    assert!(err.contains("no Elevator entity matches `f.proto`"));
    assert!(err.contains("Did you mean"));
    assert!(err.contains("protocol"));
}

#[test]
fn empty_selector_list_is_rejected() {
    let dir = SpecDir::new(&spec());
    let result = analyze(&dir);
    let err = extract(&result, &[], "test-spec").expect_err("no selectors is an error");
    assert!(err.contains("at least one entity"));
}

#[test]
fn descriptions_that_cannot_be_lexed_are_sanitised() {
    // The lexer has no escapes, so a quote or newline in a
    // description would emit a file that doesn't parse.
    assert_eq!(escape("a \"quoted\" word"), "a 'quoted' word");
    assert_eq!(escape("line\none"), "line one");
}
