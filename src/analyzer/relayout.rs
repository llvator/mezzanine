//! What the tree would draw if some files sat somewhere else.
//!
//! Every folder score in mezz is computed from five path-keyed inputs — the
//! file list, the dependency pairs, the import sites, the folder set and
//! the declaration-only set — and a file move changes nothing about a
//! codebase except the spelling of a path. So a counterfactual layout is
//! not a second scoring model: it is the same [`FileGraph`] with its paths
//! rewritten, handed to the same
//! [`compute`](super::folder_shape::compute) and
//! [`picture`](super::folder_shape::picture) the real tree goes through.
//!
//! That equivalence is the whole reason this module is thirty lines of
//! string work rather than a simulator. A simulator would be a second
//! implementation of the scoring rules, free to drift from the first, and
//! the number it reported for a hypothetical would stop agreeing with the
//! number the folder gets once the move is actually made — which is the
//! one property a what-if has to have to be worth asking.
//!
//! ## What a move is not
//!
//! Moving a file changes no dependency. Every file imports exactly what it
//! imported; both endpoints of every edge survive the rewrite. What moves
//! is which folder's drawing each edge lands in — an edge between two
//! files parts company at their lowest common folder, and relocating one
//! end changes where that is.
//!
//! So the gates a pure relayout can honestly clear are the gates about the
//! drawing: breadth, branching, layering, and the boundary the doors are
//! counted over. It cannot decouple anything, and a caller reporting it as
//! decoupling is making the same claim the re-export shim makes on
//! `reshape`'s forbidden list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf, MAIN_SEPARATOR};

use crate::graph::FileGraph;
use crate::models::ImportSite;

/// One proposed relocation: `what` comes to sit inside `into`.
///
/// `what` is a file or a whole folder, in the graph's own path spelling.
/// `into` is the folder it lands in, which need not exist yet — a layout
/// that could only name folders already on disk could not propose one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub what: String,
    pub into: String,
}

impl Move {
    /// Where `path` ends up under this move, or `None` when the move does
    /// not touch it.
    ///
    /// Two cases and no third: `path` *is* the thing being moved, or it
    /// sits under it and rides along keeping its tail. The separator in
    /// the second test is what stops `src/uid` from claiming `src/uidx`,
    /// which is the same boundary check
    /// [`is_inside`](super::folder_shape) makes for the same reason.
    fn applied_to(&self, path: &str) -> Option<String> {
        let landing = join(&self.into, basename(&self.what));
        if path == self.what {
            return Some(landing);
        }
        let tail = path
            .strip_prefix(&self.what)?
            .strip_prefix(MAIN_SEPARATOR)?;
        Some(join(&landing, tail))
    }
}

/// The tree as it would be after `moves`, ready to be scored by the pass
/// that scores the real one.
///
/// Files no move names are carried through untouched rather than dropped,
/// so the result is a whole tree and not a fragment: a folder's shape
/// depends on its siblings' compliance, and scoring a slice would answer a
/// question nobody asked.
pub fn relaid_out(fg: &FileGraph, moves: &[Move]) -> FileGraph {
    let map = rewrite_map(&fg.files, moves);
    let files: Vec<String> = fg.files.iter().map(|f| moved(&map, f)).collect();
    let root = root_of(&fg.folders);
    FileGraph {
        folders: folders_of(&files, root.as_deref()),
        pairs: fg
            .pairs
            .iter()
            .map(|(a, b)| (moved(&map, a), moved(&map, b)))
            .collect(),
        declaration_only: fg
            .declaration_only
            .iter()
            .map(|f| moved(&map, f))
            .collect(),
        imports: fg.imports.iter().map(|i| moved_site(&map, i)).collect(),
        files,
    }
}

/// Every file the moves relocate, old path → new path.
///
/// Built once and applied five times, rather than each input re-deriving
/// the destination for itself. The five have to agree exactly — a pair
/// whose endpoint spelling differs from the file list's is an edge between
/// two files the drawing does not contain — and one map is how they are
/// made to.
///
/// First move wins. A file named by two moves has no single destination,
/// and picking the later one silently would answer a contradictory request
/// with a confident layout; the caller validates for this before asking.
fn rewrite_map(files: &[String], moves: &[Move]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for file in files {
        if let Some(dest) = moves.iter().find_map(|m| m.applied_to(file)) {
            map.insert(file.clone(), dest);
        }
    }
    map
}

/// `path` after the rewrite, or `path` itself when nothing moved it.
fn moved(map: &HashMap<String, String>, path: &str) -> String {
    map.get(path).cloned().unwrap_or_else(|| path.to_string())
}

/// An import statement with both ends rewritten.
///
/// The line number rides along unchanged, which is honest: a moved file
/// keeps its text, so the statement really is still written where it was.
fn moved_site(map: &HashMap<String, String>, site: &ImportSite) -> ImportSite {
    ImportSite {
        from: PathBuf::from(moved(map, &site.from.display().to_string())),
        to: PathBuf::from(moved(map, &site.to.display().to_string())),
        ..site.clone()
    }
}

/// The folder every other folder sits under, when there is one.
///
/// Needed because [`folders_of`] has to stop climbing somewhere, and the
/// pass that built the original set stopped at the analysis root. Climbing
/// past it would invent folders above the tree that nothing was analysed
/// in, and each of those would score — a repository whose shape improved
/// because `/Users` was added to the drawing.
fn root_of(folders: &HashSet<String>) -> Option<String> {
    let shortest = folders.iter().min_by_key(|f| f.len())?;
    folders
        .iter()
        .all(|f| f == shortest || is_under(shortest, f))
        .then(|| shortest.clone())
}

/// Whether `path` sits anywhere under `folder` — the separator-boundary
/// test, so `src/uid` does not claim `src/uidx`.
fn is_under(folder: &str, path: &str) -> bool {
    path.strip_prefix(folder)
        .is_some_and(|tail| tail.starts_with(MAIN_SEPARATOR))
}

/// Every folder holding one of `files`, climbing to `root`.
///
/// The same enumeration `enumerate_folder_paths` runs over the real tree,
/// re-run because the move may have emptied a folder or created one that
/// has never existed. Carrying the original set forward instead would
/// leave the emptied folder in the drawing as a childless node, which
/// scores.
fn folders_of(files: &[String], root: Option<&str>) -> HashSet<String> {
    let mut out: HashSet<String> = root.map(|r| r.to_string()).into_iter().collect();
    for file in files {
        let mut cur = parent_dir(file);
        while !cur.is_empty() {
            let fresh = out.insert(cur.clone());
            // Stop at the root, and stop the moment a path is already
            // known — everything above it was inserted by whoever put it
            // there. Without the second test a wide tree walks its own
            // spine once per file.
            if !fresh || root.is_some_and(|r| cur == r) {
                break;
            }
            let up = parent_dir(&cur);
            if up == cur {
                break;
            }
            cur = up;
        }
    }
    out
}

/// The folder holding `path`, or `""` for a path with no separator.
fn parent_dir(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// The last segment of `path`.
pub(crate) fn basename(path: &str) -> &str {
    path.rsplit(MAIN_SEPARATOR).next().unwrap_or(path)
}

/// `folder/name`, with the platform separator and no doubled one.
pub(crate) fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        return name.to_string();
    }
    format!("{}{}{}", folder.trim_end_matches(MAIN_SEPARATOR), MAIN_SEPARATOR, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        s.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    fn graph(files: &[&str], pairs: &[(&str, &str)]) -> FileGraph {
        let files: Vec<String> = files.iter().map(|f| p(f)).collect();
        FileGraph {
            folders: folders_of(&files, Some(&p("src"))),
            pairs: pairs.iter().map(|(a, b)| (p(a), p(b))).collect(),
            declaration_only: HashSet::new(),
            imports: Vec::new(),
            files,
        }
    }

    #[test]
    fn a_moved_file_keeps_its_edges_with_both_ends_rewritten() {
        let fg = graph(
            &["src/a.ts", "src/b.ts"],
            &[("src/a.ts", "src/b.ts")],
        );
        let out = relaid_out(
            &fg,
            &[Move {
                what: p("src/b.ts"),
                into: p("src/core"),
            }],
        );
        assert!(out.files.contains(&p("src/core/b.ts")));
        assert_eq!(out.pairs, vec![(p("src/a.ts"), p("src/core/b.ts"))]);
        assert!(out.folders.contains(&p("src/core")));
    }

    #[test]
    fn moving_a_folder_carries_everything_under_it() {
        let fg = graph(&["src/uid/model/note.ts", "src/app.ts"], &[]);
        let out = relaid_out(
            &fg,
            &[Move {
                what: p("src/uid/model"),
                into: p("src/core"),
            }],
        );
        assert!(out.files.contains(&p("src/core/model/note.ts")));
        assert!(out.folders.contains(&p("src/core/model")));
    }

    #[test]
    fn a_prefix_that_is_not_a_path_boundary_does_not_move() {
        let fg = graph(&["src/uidx/a.ts"], &[]);
        let out = relaid_out(
            &fg,
            &[Move {
                what: p("src/uid"),
                into: p("src/core"),
            }],
        );
        assert_eq!(out.files, vec![p("src/uidx/a.ts")]);
    }

    #[test]
    fn an_emptied_folder_leaves_the_drawing() {
        let fg = graph(&["src/old/a.ts", "src/b.ts"], &[]);
        let out = relaid_out(
            &fg,
            &[Move {
                what: p("src/old/a.ts"),
                into: p("src"),
            }],
        );
        assert!(!out.folders.contains(&p("src/old")));
    }

    #[test]
    fn nothing_climbs_above_the_analysis_root() {
        let fg = graph(&["src/a.ts"], &[]);
        let out = relaid_out(&fg, &[]);
        assert_eq!(out.folders, HashSet::from([p("src")]));
    }
}
