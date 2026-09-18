//! The two 2026-08-29 field reports, as fixtures.
//!
//! Both are about the same silence from two directions: a folder whose
//! listing is smaller than the folder, saying nothing about the difference.

use super::*;
use crate::mcp::tools::map;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

/// A throwaway tree. The prefix carries neither "test" nor "spec": the
/// walker's test heuristic matches on the whole path string, so the default
/// temp-dir prefix would exclude every fixture file wholesale and the
/// census would have nothing to count.
struct TmpDir(PathBuf);

impl TmpDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mezz-mcp-census-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&path).unwrap();
        TmpDir(path)
    }

    fn write(&self, rel: &str, body: &str) {
        let full = self.0.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, body).unwrap();
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn server_for(dir: &TmpDir) -> McpServer {
    McpServer {
        root: dir.0.canonicalize().unwrap(),
        include_tests: false,
        languages: None,
        graph_cache: Mutex::new(HashMap::new()),
        base_cache: Mutex::new(HashMap::new()),
        generation: Arc::new(AtomicU64::new(0)),
        shape_baselines: Default::default(),
        rules_spelled_out: Default::default(),
        layout_caveat_spelled_out: Default::default(),
    }
}

/// The reporter's own fixture: a view model mezz reads, a template it has
/// no parser for, and a barrel that parses to nothing.
fn astro_cell(name: &str) -> TmpDir {
    let dir = TmpDir::new(name);
    dir.write(
        "src/viewmodel.ts",
        "export interface CardVM { title: string }\n\
         export function buildCardVM(title: string): CardVM {\n  return { title };\n}\n",
    );
    dir.write(
        "src/Card.astro",
        "---\nimport { buildCardVM } from './viewmodel';\n\
         const vm = buildCardVM(Astro.props.title);\n---\n<h1>{vm.title}</h1>\n",
    );
    dir.write(
        "src/index.ts",
        "export { buildCardVM } from './viewmodel';\n\
         export type { CardVM } from './viewmodel';\n",
    );
    dir
}

/// Field report, 2026-08-29: `map` listed 8 of a folder's 15 files and said
/// nothing about the 7 it skipped, so an agent told to "use `map` to
/// understand the shape of a folder before reading files" read `19 files`
/// over an Astro `src/pages` and concluded the site had no pages.
///
/// The count alone is the fix the reporter asked for — "enough to stop
/// anyone treating the listing as the folder".
#[test]
fn the_header_says_how_many_files_the_folder_holds_not_just_how_many_it_listed() {
    let dir = astro_cell("unparsed");
    let out = map(&server_for(&dir), &json!({ "path": "src", "depth": 1 })).unwrap().into_text();

    assert!(
        out.contains("2 of 3 files listed"),
        "the listing is still passing itself off as the folder:\n{out}"
    );
    assert!(
        out.contains(".astro"),
        "the note does not say which extension no parser reads:\n{out}"
    );
}

/// The other half of the same report: an unqualified count is worse than a
/// qualified one only when it is wrong, so a folder mezz read whole must
/// keep saying `N files` and print no caveat at all.
#[test]
fn a_folder_the_analysis_read_whole_carries_no_caveat() {
    let dir = TmpDir::new("whole");
    dir.write("src/a.ts", "export function a(): number { return 1; }\n");
    dir.write("src/b.ts", "export function b(): number { return 2; }\n");

    let out = map(&server_for(&dir), &json!({ "path": "src", "depth": 1 })).unwrap().into_text();
    assert!(
        out.contains("2 files —") && !out.contains("of 2 files listed"),
        "a complete listing should not qualify itself:\n{out}"
    );
    assert!(
        !out.contains("Not listed"),
        "nothing was missing and the caveat printed anyway:\n{out}"
    );
}

/// Field report, 2026-08-29 (second): a re-export barrel is analysed and
/// then gets no row, because `map` groups by *entity* and a file of pure
/// re-exports has none. `src/domain/index.ts` — the door 25 files import
/// from, and the only one the folder's own rules allow them to name — was
/// the one file the map of that folder did not mention.
///
/// An absent row and an absent file look identical from the outside.
#[test]
fn a_file_the_analysis_read_keeps_its_row_even_with_nothing_to_list() {
    let dir = astro_cell("barrel");
    let out = map(&server_for(&dir), &json!({ "path": "src", "depth": 1 })).unwrap().into_text();

    assert!(
        out.contains("index.ts (0 entities)"),
        "the folder's door is still invisible:\n{out}"
    );
    assert!(
        out.contains("viewmodel.ts (2 entities)"),
        "the ordinary rows changed shape:\n{out}"
    );
}

/// Test files are a documented exclusion, and still have to be counted:
/// unexplained, `8 of 15` reads as a defect, and the reader goes looking
/// for one. Named, it reads as their own setting working.
#[test]
fn excluded_tests_are_counted_and_named_as_the_setting_that_excluded_them() {
    // Not "excluded-tests": the heuristic matches the whole path string, so
    // that name would exclude the production file too — which the census
    // duly reported as `0 of 2 files listed`, and the old header would have
    // reported as `0 files`.
    let dir = TmpDir::new("excluded-suite");
    dir.write("src/a.ts", "export function a(): number { return 1; }\n");
    dir.write("src/a.test.ts", "it('a', () => {});\n");

    let out = map(&server_for(&dir), &json!({ "path": "src", "depth": 1 })).unwrap().into_text();
    assert!(
        out.contains("1 of 2 files listed"),
        "the excluded test is uncounted:\n{out}"
    );
    assert!(
        out.contains("`include_tests` is off"),
        "the note does not say what excluded it:\n{out}"
    );
}

/// The census counts the folder, not the repository: a `.gitignore`d build
/// directory is not something `map` failed to show, and a census reporting
/// `2 of 4213 files listed` would be worse than none.
#[test]
fn an_ignored_directory_is_not_counted_against_the_listing() {
    let dir = TmpDir::new("ignored");
    fs::create_dir_all(dir.0.join(".git")).unwrap();
    dir.write(".gitignore", "src/build/\n");
    dir.write("src/a.ts", "export function a(): number { return 1; }\n");
    dir.write("src/build/bundle.js", "console.log(1);\n");
    dir.write("src/build/bundle.map", "{}\n");

    let out = map(&server_for(&dir), &json!({ "path": "src", "depth": 1 })).unwrap().into_text();
    assert!(
        out.contains("1 files —") && !out.contains("files listed"),
        "an ignored directory was counted as unlisted:\n{out}"
    );
}

/// The note names extensions so a reader can dismiss `.png` as fast as they
/// act on `.astro`, and it never drops one in silence.
#[test]
fn the_note_caps_the_extensions_it_names_and_counts_the_rest() {
    let census = FolderCensus {
        files: BTreeMap::new(),
        kinds: BTreeMap::new(),
        unread: BTreeMap::from([(Unread::Unsupported, 5)]),
        extensions: [".astro", ".png", ".svg", ".toml", ".yml"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    assert_eq!(
        census.unread_note(),
        Some("_Not listed: 5 no parser reads (.astro, .png, .svg, and 2 more)._".to_string())
    );
}
