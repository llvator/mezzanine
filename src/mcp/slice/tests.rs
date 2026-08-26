//! Tests for `spec_slice`.
//!
//! These run the real analyzer over a temporary project (a spec plus
//! the code it claims) rather than hand-building a graph: the seeds
//! come from `cr:` attributes and the `Contains` edges the post-merge
//! passes produce, neither of which a fixture would reproduce
//! faithfully.
//!
//! The load-bearing assertions are that a folder selects the branch
//! claiming it (not the whole spec), and that a written slice is a
//! standalone spec — it re-analyzes and passes `--check`.

use super::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

// ---------------------------------------------------------------
// Harness
// ---------------------------------------------------------------

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mezz-spec-slice-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&path).unwrap();
        TmpDir(path)
    }

    fn write(&self, rel: &str, body: &str) {
        let full = self.0.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, body).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.0.join(rel)).expect("slice file exists")
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn server_for(dir: &TmpDir) -> McpServer {
    McpServer {
        root: dir.0.canonicalize().unwrap(),
        include_tests: false,
        languages: None,
        graph_cache: std::sync::Mutex::new(HashMap::new()),
        base_cache: std::sync::Mutex::new(HashMap::new()),
        generation: Arc::new(AtomicU64::new(0)),
        shape_baselines: Default::default(),
        rules_spelled_out: Default::default(),
        layout_caveat_spelled_out: Default::default(),
    }
}

/// Two branches under one Category. `f protocol` claims a folder,
/// `f storage` claims a different one, and the Concept is used by the
/// first only — so a slice of the protocol folder must bring the
/// Concept and leave storage out.
const SPEC: &str = r#"
c library {
    d: "Reusable building blocks."
    cr: "src/", "src/storage/"
    f protocol
    f storage
}

f protocol {
    d: "Structured prompts with typed responses."
    cr: "src/protocol/"
    fu creation
}

fu f.protocol.creation {
    d: "Create a protocol."
    cr: "src/protocol/create.rs"
}

f storage {
    d: "Where protocols are persisted."
    cr: "src/storage/"
}

concept validation {
    d: "Every typed response is validated before it is stored."
    used_by: f.protocol
}
"#;

/// A project with the spec above plus the code it claims.
fn project(name: &str) -> TmpDir {
    let dir = TmpDir::new(name);
    dir.write("spec.elv", SPEC);
    dir.write("src/protocol/create.rs", "pub fn create() {}\n");
    dir.write("src/storage/save.rs", "pub fn save() {}\n");
    dir
}

fn slice_for(dir: &TmpDir, args: serde_json::Value) -> String {
    spec_slice(&server_for(dir), &args).expect("slice succeeds")
}

// ---------------------------------------------------------------
// Selection
// ---------------------------------------------------------------

#[test]
fn folder_selects_the_branch_that_claims_it_and_nothing_beside_it() {
    let dir = project("folder");
    let out = slice_for(&dir, json!({"path": "src/protocol"}));

    assert!(
        out.contains("f protocol"),
        "claiming Feature missing:\n{out}"
    );
    assert!(
        out.contains("fu f.protocol.creation"),
        "descendant missing:\n{out}"
    );
    // Concept the branch uses comes along; the sibling branch does not.
    assert!(
        out.contains("concept validation"),
        "used Concept missing:\n{out}"
    );
    assert!(
        !out.contains("f storage"),
        "sibling branch leaked in:\n{out}"
    );
    // The Category above is context, so the slice says where it sits.
    assert!(
        out.contains("c library"),
        "ancestor context missing:\n{out}"
    );
    assert!(
        out.contains("# context"),
        "ancestor not marked as context:\n{out}"
    );
}

#[test]
fn a_file_with_no_claim_falls_back_to_the_narrowest_claim_above_it() {
    let dir = project("fallback");
    let out = slice_for(&dir, json!({"path": "src/storage/save.rs"}));

    assert!(
        out.contains("no claim inside"),
        "fallback not explained:\n{out}"
    );
    assert!(out.contains("src/storage"), "wrong enclosing claim:\n{out}");
    assert!(
        out.contains("f storage"),
        "enclosing Feature missing:\n{out}"
    );
    // `c library` claims `src/` too, but it is wider — it may only
    // appear as context, never as the seed.
    assert!(
        !out.contains("f protocol"),
        "narrowest claim not preferred:\n{out}"
    );
}

/// `c library` claims `src/storage/` too, so it ties with `f storage`
/// on path depth. The Feature is the narrower answer; seeding the
/// Category would drag `f protocol` in with it.
#[test]
fn a_tie_between_a_category_and_its_own_feature_goes_to_the_feature() {
    let dir = project("tie");
    let out = slice_for(&dir, json!({"path": "src/storage/save.rs"}));

    assert!(
        out.contains("Seeded from:\n  f storage"),
        "Category seeded the slice:\n{out}"
    );
    assert!(
        out.contains("# context"),
        "Category should still place the slice:\n{out}"
    );
    assert!(
        !out.contains("f protocol"),
        "sibling branch came along:\n{out}"
    );
}

#[test]
fn an_exact_file_claim_beats_the_folder_claim_above_it() {
    let dir = project("exact-file");
    let out = slice_for(&dir, json!({"path": "src/protocol/create.rs"}));

    assert!(
        !out.contains("no claim inside"),
        "should not have fallen back:\n{out}"
    );
    assert!(
        out.contains("fu f.protocol.creation"),
        "claiming Functionality missing:\n{out}"
    );
}

#[test]
fn a_folder_the_spec_never_mentions_is_an_error_not_an_empty_slice() {
    let dir = project("unclaimed");
    dir.write("vendor/thing.rs", "pub fn thing() {}\n");
    let err = spec_slice(&server_for(&dir), &json!({"path": "vendor"})).unwrap_err();
    assert!(
        err.to_string().contains("No Elevator entity claims"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn a_project_without_a_spec_says_so() {
    let dir = TmpDir::new("no-spec");
    dir.write("src/main.rs", "fn main() {}\n");
    let err = spec_slice(&server_for(&dir), &json!({"path": "src"})).unwrap_err();
    assert!(
        err.to_string().contains("No Elevator"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn path_is_required() {
    let dir = project("no-path");
    let err = spec_slice(&server_for(&dir), &json!({})).unwrap_err();
    assert!(
        err.to_string().contains("needs a `path`"),
        "unexpected error: {err:#}"
    );
}

// ---------------------------------------------------------------
// Writing
// ---------------------------------------------------------------

#[test]
fn written_slice_is_a_standalone_spec_that_passes_check() {
    let dir = project("write");
    let out = slice_for(
        &dir,
        json!({"path": "src/protocol", "out": "tickets/T-1/spec-slice.elv"}),
    );
    assert!(
        out.contains("→ tickets/T-1/spec-slice.elv"),
        "target not reported:\n{out}"
    );
    // The response is provenance, not the body: the file is the artifact.
    assert!(
        !out.contains("d: \"Create a protocol.\""),
        "body leaked into response:\n{out}"
    );

    let text = dir.read("tickets/T-1/spec-slice.elv");
    assert!(text.contains("f protocol"), "slice body missing:\n{text}");

    // Round-trip: re-analyze the slice on its own and check it.
    let slice_dir = TmpDir::new("write-check");
    slice_dir.write("slice.elv", &text);
    let mut config = crate::config::Config::for_path(&slice_dir.0);
    config
        .analysis
        .languages
        .insert(crate::models::file_info::Language::Elevator);
    let result = crate::Analyzer::new(config)
        .analyze()
        .expect("re-analysis succeeds");
    let errors: Vec<String> = crate::output::elevator_check::check(&result)
        .into_iter()
        .filter(|f| matches!(f.severity, crate::output::elevator_check::Severity::Error))
        .map(|f| f.message)
        .collect();
    assert!(errors.is_empty(), "slice does not check clean: {errors:#?}");
}

#[test]
fn an_existing_file_is_never_replaced_without_being_asked() {
    let dir = project("overwrite");
    dir.write("tickets/T-1/spec-slice.elv", "# hand-written\n");

    let err = spec_slice(
        &server_for(&dir),
        &json!({"path": "src/protocol", "out": "tickets/T-1/spec-slice.elv"}),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("overwrite=true"),
        "unexpected error: {err:#}"
    );
    assert_eq!(dir.read("tickets/T-1/spec-slice.elv"), "# hand-written\n");

    slice_for(
        &dir,
        json!({"path": "src/protocol", "out": "tickets/T-1/spec-slice.elv", "overwrite": true}),
    );
    assert!(dir
        .read("tickets/T-1/spec-slice.elv")
        .contains("f protocol"));
}

#[test]
fn writes_cannot_leave_the_project_root() {
    let dir = project("escape");
    let server = server_for(&dir);

    let err = resolve_out(&server, "../outside.elv", &json!({})).unwrap_err();
    assert!(
        err.to_string().contains("escapes it"),
        "unexpected error: {err:#}"
    );

    let err = resolve_out(&server, "/tmp/outside.elv", &json!({})).unwrap_err();
    assert!(
        err.to_string().contains("absolute"),
        "unexpected error: {err:#}"
    );

    // A `..` that stays inside is fine — it normalizes back in.
    let ok = resolve_out(&server, "tickets/../notes/slice.elv", &json!({})).unwrap();
    assert_eq!(ok, server.root.join("notes/slice.elv"));
}

// ---------------------------------------------------------------
// Path handling
// ---------------------------------------------------------------

#[test]
fn target_paths_normalize_to_the_form_cr_uses() {
    let dir = project("normalize");
    let server = server_for(&dir);

    assert_eq!(
        target_folder(&server, &json!({"path": "./src/protocol/"})).unwrap(),
        "src/protocol"
    );
    let absolute = server.root.join("src/protocol").display().to_string();
    assert_eq!(
        target_folder(&server, &json!({"path": absolute})).unwrap(),
        "src/protocol"
    );

    let err = target_folder(&server, &json!({"path": "."})).unwrap_err();
    assert!(
        err.to_string().contains("project root"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn depth_ranks_narrower_claims_higher() {
    assert!(depth("src/mcp/tools.rs") > depth("src/mcp"));
    assert!(depth("src/mcp") > depth("src"));
    assert_eq!(depth(""), 0);
}
