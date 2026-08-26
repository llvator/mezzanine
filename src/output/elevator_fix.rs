//! Repairing the `cr:` anchors that [`super::elevator_drift`] reports
//! as missing.
//!
//! A `cr:` path is a declaration, not a derivation — nothing infers
//! one, and a path matching nothing is reported rather than quietly
//! matched approximately. That rule is what makes the anchor worth
//! trusting, so this module does not relax it: it only applies moves
//! the repository already recorded.
//!
//! Two gates, both of which must pass before a byte is written:
//!
//! 1. **Rename evidence** — `git log -M --diff-filter=R` names a
//!    commit in which the old path became a new one. Git infers
//!    renames by similarity at query time, so a file rewritten in the
//!    same commit that moved it may register as delete+add and yield
//!    nothing. That is a silent no-fix, which is the safe direction.
//! 2. **The target exists** — the same check `--drift` runs, so a
//!    repair can never introduce the drift it was meant to remove.
//!
//! Anything git cannot prove is left alone. Where a dead file path has
//! exactly one same-basename file elsewhere in the tree, that is
//! reported as a *candidate* for a human to confirm — never applied,
//! never counted as repaired. Widening the evidence later is cheap;
//! un-corrupting a spec rewritten on a bad guess is not.

use crate::diff::{git_lines, verify_git_repo};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// How many links of a rename chain (`A → B → C`) to follow before
/// giving up. Also the loop guard: a chain that revisits a path is
/// abandoned rather than followed round again.
const MAX_CHAIN: usize = 10;

/// Directories never walked when indexing basenames for candidates.
const SKIP_DIRS: [&str; 4] = ["target", "node_modules", "dist", "build"];

/// One `cr:` path that no longer resolves, carrying everything needed
/// to rewrite it where the author wrote it.
#[derive(Debug, Clone)]
pub struct CrRef {
    /// Display form of the owning entity (`fu f.spec_health.drift`).
    pub entity: String,
    /// The `.elv` file the entity is defined in.
    pub file: PathBuf,
    /// Byte range of the entity's whole definition, keyword through
    /// closing brace. Bounds the search for the path literal so a
    /// sibling entity claiming the same dead path is not rewritten by
    /// this entity's repair.
    pub span: Range<usize>,
    /// The path exactly as authored, including any trailing slash.
    pub path: String,
}

/// A rewrite git can prove.
#[derive(Debug, Clone)]
pub struct Repair {
    pub new: String,
    /// Commit the rename was recorded in — the receipt an author needs
    /// to check the fix without re-deriving it.
    pub commit: String,
}

/// What `--fix` would do, computed before anything is written so the
/// report can show it either way.
#[derive(Debug, Default)]
pub struct FixPlan {
    /// Dead path → the move git recorded.
    pub repairs: BTreeMap<String, Repair>,
    /// Dead path → the sole same-basename file found. Suggestions
    /// only; never applied.
    pub candidates: BTreeMap<String, String>,
    /// Why there is no evidence at all, when that is the reason the
    /// plan is empty.
    pub note: Option<String>,
}

impl FixPlan {
    pub fn is_empty(&self) -> bool {
        self.repairs.is_empty() && self.candidates.is_empty()
    }
}

/// Work out which of the missing paths can be repaired from history.
///
/// Best-effort throughout: a code root that is not a git repository,
/// or a git that cannot be run, yields an empty plan with a note
/// rather than an error. Drift reporting stays useful without git.
pub fn plan(missing: &[CrRef], code_root: &Path) -> FixPlan {
    let mut plan = FixPlan::default();
    if missing.is_empty() {
        return plan;
    }
    if verify_git_repo(code_root).is_err() {
        plan.note =
            Some("code root is not a git repository — no rename evidence available".to_string());
        return plan;
    }

    let paths: BTreeSet<&str> = missing.iter().map(|m| m.path.as_str()).collect();
    for path in &paths {
        if let Some(repair) = trace_rename(code_root, path) {
            plan.repairs.insert((*path).to_string(), repair);
        }
    }

    let unproven: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|p| !plan.repairs.contains_key(*p))
        .collect();
    if !unproven.is_empty() {
        add_candidates(&unproven, code_root, &mut plan);
    }
    plan
}

/// Follow the rename chain from `path` until a link lands somewhere
/// that exists on disk. Returns nothing unless both gates pass.
fn trace_rename(code_root: &Path, path: &str) -> Option<Repair> {
    let mut current = path.to_string();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for _ in 0..MAX_CHAIN {
        if !seen.insert(current.clone()) {
            return None; // history loops; no answer is better than a guess
        }
        let (next, commit) = rename_of(code_root, &current)?;
        current = next;
        if code_root.join(current.trim_end_matches('/')).exists() {
            return Some(Repair {
                new: current,
                commit,
            });
        }
    }
    None
}

/// The single move git recorded out of `path`, as `(new_path, commit)`.
///
/// A path is tried as a file first, then as a directory — `cr:` paths
/// name directories with and without a trailing slash, and neither
/// spelling tells us which the author meant.
fn rename_of(code_root: &Path, path: &str) -> Option<(String, String)> {
    let log = rename_log(code_root, path);
    let authored_slash = path.ends_with('/');
    if !authored_slash {
        if let Some(hit) = file_rename(path, &log) {
            return Some(hit);
        }
    }
    let prefix = if authored_slash {
        path.to_string()
    } else {
        format!("{}/", path)
    };
    let (moved, commit) = dir_rename(&prefix, &log)?;
    // Keep the author's trailing-slash style rather than imposing one.
    let moved = if authored_slash {
        moved
    } else {
        moved.trim_end_matches('/').to_string()
    };
    Some((moved, commit))
}

/// One `R…  from  to` line from `git log --name-status`, tagged with
/// the commit it came from.
struct RenameEntry {
    commit: String,
    from: String,
    to: String,
}

/// The renames recorded in the commit that removed `path`.
///
/// Asked in two steps, because one is not enough. `git log -M
/// --diff-filter=R -- <path>` finds nothing at all: the pathspec
/// narrows the diff *before* rename detection runs, so the deletion of
/// the old path and the addition of the new one are never paired and
/// the move shows up as a plain delete. So: find the commit that
/// deleted the path, then re-diff that one commit unrestricted, where
/// both halves are visible and `-M` can match them.
///
/// `git_lines` collapses "git said nothing" and "git could not be
/// asked" into the same empty result, which is the right posture here:
/// both mean no evidence, and no evidence means no fix.
fn rename_log(code_root: &Path, path: &str) -> Vec<RenameEntry> {
    let spec = path.trim_end_matches('/');
    let removed_in = git_lines(
        code_root,
        &[
            "log",
            "--diff-filter=D",
            "--format=%H",
            "-n",
            "1",
            "--",
            spec,
        ],
    );
    let Some(commit) = removed_in.into_iter().find(|l| is_sha(l)) else {
        return Vec::new();
    };

    let lines = git_lines(
        code_root,
        &[
            "show",
            "-M",
            "--diff-filter=R",
            "--name-status",
            "--format=",
            &commit,
        ],
    );
    let mut entries = Vec::new();
    for line in lines {
        let mut parts = line.split('\t');
        if !parts.next().is_some_and(|status| status.starts_with('R')) {
            continue;
        }
        let (Some(from), Some(to)) = (parts.next(), parts.next()) else {
            continue;
        };
        entries.push(RenameEntry {
            commit: commit.clone(),
            from: from.to_string(),
            to: to.to_string(),
        });
    }
    entries
}

fn is_sha(line: &str) -> bool {
    line.len() == 40 && line.chars().all(|c| c.is_ascii_hexdigit())
}

fn file_rename(path: &str, log: &[RenameEntry]) -> Option<(String, String)> {
    log.iter()
        .find(|e| e.from == path)
        .map(|e| (e.to.clone(), e.commit.clone()))
}

/// Where a directory went, but only when its files agree unanimously.
///
/// Git tracks files, so a directory move is visible only as the moves
/// of the files under it. Every file that left `prefix` in the chosen
/// commit must land under one common parent, keeping its path relative
/// to the directory intact. A directory whose contents scattered — or
/// one file of which was renamed as it moved — yields nothing, because
/// picking a winner among disagreeing evidence is the inference this
/// module exists to avoid.
fn dir_rename(prefix: &str, log: &[RenameEntry]) -> Option<(String, String)> {
    let commit = log
        .iter()
        .find(|e| e.from.starts_with(prefix))?
        .commit
        .clone();
    let mut target: Option<String> = None;
    for entry in log
        .iter()
        .filter(|e| e.commit == commit && e.from.starts_with(prefix))
    {
        let rel = entry.from.strip_prefix(prefix)?;
        let moved = entry.to.strip_suffix(rel)?;
        match &target {
            None => target = Some(moved.to_string()),
            Some(seen) if seen == moved => {}
            Some(_) => return None,
        }
    }
    target.map(|t| (t, commit))
}

/// Note the dead file paths that have exactly one same-basename file
/// left in the tree. Suggestions for a human, not repairs.
fn add_candidates(unproven: &[&str], code_root: &Path, plan: &mut FixPlan) {
    let wanted: BTreeSet<&str> = unproven
        .iter()
        .filter(|p| !p.ends_with('/'))
        .filter_map(|p| basename(p))
        .collect();
    if wanted.is_empty() {
        return;
    }
    let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    index_basenames(code_root, code_root, &wanted, &mut index);

    for path in unproven {
        let Some(name) = basename(path) else { continue };
        let Some(found) = index.get(name) else {
            continue;
        };
        if let [only] = found.as_slice() {
            plan.candidates.insert((*path).to_string(), only.clone());
        }
    }
}

fn basename(path: &str) -> Option<&str> {
    path.rsplit('/').next().filter(|s| !s.is_empty())
}

/// Walk the tree once, recording code-root-relative paths for the
/// basenames anyone actually asked about. Skipping hidden and build
/// directories keeps `target/` copies of a moved file from making an
/// otherwise unique basename look ambiguous.
fn index_basenames(
    dir: &Path,
    code_root: &Path,
    wanted: &BTreeSet<&str>,
    index: &mut BTreeMap<String, Vec<String>>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || SKIP_DIRS.contains(&name) {
            continue;
        }
        if path.is_dir() {
            index_basenames(&path, code_root, wanted, index);
        } else if wanted.contains(name) {
            if let Some(rel) = relative_to(&path, code_root) {
                index.entry(name.to_string()).or_default().push(rel);
            }
        }
    }
}

fn relative_to(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// A `.elv` file that `apply` rewrote.
pub struct Applied {
    pub file: PathBuf,
    /// The dead paths this file no longer contains, newly spelled.
    pub paths: Vec<String>,
}

/// What a `--fix` run actually did.
///
/// Successes and the failure are reported together rather than
/// collapsed into a `Result`: a write that fails on the third file does
/// not un-write the first two, and a report claiming "nothing was
/// written" would then be a lie the author acts on.
#[derive(Default)]
pub struct ApplyOutcome {
    pub applied: Vec<Applied>,
    pub error: Option<String>,
}

/// Rewrite every proven repair in place.
///
/// Edits are textual and bounded by each entity's own definition, so
/// comments, blank lines, attribute order and the other paths on a
/// `cr: "a", "b"` line all survive byte-identical. They are applied
/// back-to-front so an earlier replacement of a different length
/// cannot shift a later one's offset.
pub fn apply(missing: &[CrRef], repairs: &BTreeMap<String, Repair>) -> ApplyOutcome {
    let mut by_file: BTreeMap<&Path, Vec<&CrRef>> = BTreeMap::new();
    for m in missing.iter().filter(|m| repairs.contains_key(&m.path)) {
        by_file.entry(m.file.as_path()).or_default().push(m);
    }

    let mut outcome = ApplyOutcome::default();
    for (file, refs) in by_file {
        match rewrite_file(file, &refs, repairs) {
            Ok(None) => {}
            Ok(Some(applied)) => outcome.applied.push(applied),
            Err(e) => {
                outcome.error = Some(e);
                return outcome;
            }
        }
    }
    outcome
}

/// Rewrite one `.elv`. `Ok(None)` means the file held no literal to
/// replace — the paths were there when the spec was parsed, so this is
/// a spec edited underneath the run, not a normal outcome.
fn rewrite_file(
    file: &Path,
    refs: &[&CrRef],
    repairs: &BTreeMap<String, Repair>,
) -> Result<Option<Applied>, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {}", file.display(), e))?;
    let edits = edits_for(&text, refs, repairs);
    if edits.is_empty() {
        return Ok(None);
    }
    let mut buf = text;
    for (start, len, replacement) in &edits {
        buf.replace_range(*start..*start + *len, replacement);
    }
    write_atomic(file, &buf)?;

    let mut paths: Vec<String> = refs
        .iter()
        .filter(|r| repairs.contains_key(&r.path))
        .map(|r| r.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    Ok(Some(Applied {
        file: file.to_path_buf(),
        paths,
    }))
}

/// Byte edits for one file as `(offset, length, replacement)`, sorted
/// last-first and de-duplicated by offset.
///
/// The literal is located rather than reconstructed because the
/// Elevator lexer has no escape handling — it copies bytes until the
/// closing quote — so the parsed path is exactly the source text
/// between the quotes. If escapes are ever added, this stops being
/// true and the locator has to move to token offsets.
fn edits_for(
    text: &str,
    refs: &[&CrRef],
    repairs: &BTreeMap<String, Repair>,
) -> Vec<(usize, usize, String)> {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for m in refs {
        let Some(repair) = repairs.get(&m.path) else {
            continue;
        };
        let Some(region) = text.get(m.span.clone()) else {
            continue;
        };
        let needle = format!("\"{}\"", m.path);
        let replacement = format!("\"{}\"", repair.new);
        for (offset, _) in region.match_indices(&needle) {
            edits.push((m.span.start + offset, needle.len(), replacement.clone()));
        }
    }
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    edits.dedup_by_key(|e| e.0);
    edits
}

/// Write via `<path>.tmp` + rename, the convention every other writer
/// here follows: a reader never observes a half-rewritten spec, and an
/// interrupted `--fix` leaves the original intact.
fn write_atomic(path: &Path, body: &str) -> Result<(), String> {
    let tmp = path.with_extension("elv.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(commit: &str, from: &str, to: &str) -> RenameEntry {
        RenameEntry {
            commit: commit.into(),
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn a_directory_move_is_read_off_its_files() {
        let log = vec![
            entry("a1", "src/output/drift.rs", "src/report/drift.rs"),
            entry("a1", "src/output/list.rs", "src/report/list.rs"),
        ];
        assert_eq!(
            dir_rename("src/output/", &log),
            Some(("src/report/".to_string(), "a1".to_string()))
        );
    }

    #[test]
    fn a_directory_whose_files_scattered_yields_nothing() {
        let log = vec![
            entry("a1", "src/output/drift.rs", "src/report/drift.rs"),
            entry("a1", "src/output/list.rs", "src/cli/list.rs"),
        ];
        assert_eq!(dir_rename("src/output/", &log), None);
    }

    /// A file renamed *as* it moved breaks the suffix match. Bailing is
    /// the point: the directory's new name is no longer derivable from
    /// that file, and picking one of the disagreeing answers is the
    /// inference this module refuses to make.
    #[test]
    fn a_file_renamed_while_moving_yields_nothing() {
        let log = vec![
            entry("a1", "src/output/drift.rs", "src/report/drift.rs"),
            entry("a1", "src/output/list.rs", "src/report/listing.rs"),
        ];
        assert_eq!(dir_rename("src/output/", &log), None);
    }

    #[test]
    fn only_renames_out_of_the_directory_count() {
        // A rename *into* the directory matches the pathspec on its
        // target side and must not vote on where the directory went.
        let log = vec![
            entry("a1", "src/output/drift.rs", "src/report/drift.rs"),
            entry("a1", "src/cli/old.rs", "src/output/new.rs"),
        ];
        assert_eq!(
            dir_rename("src/output/", &log),
            Some(("src/report/".to_string(), "a1".to_string()))
        );
    }

    #[test]
    fn nested_files_still_pin_the_parent() {
        let log = vec![entry("a1", "src/output/deep/x.rs", "src/report/deep/x.rs")];
        assert_eq!(
            dir_rename("src/output/", &log),
            Some(("src/report/".to_string(), "a1".to_string()))
        );
    }

    #[test]
    fn a_commit_sha_is_told_from_a_status_line() {
        assert!(is_sha("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_sha("R100\tsrc/a.rs\tsrc/b.rs"));
        assert!(!is_sha("0123456789abcdef"));
    }

    #[test]
    fn siblings_on_the_same_line_survive_a_rewrite() {
        let text = "f x {\n    cr: \"src/a.rs\", \"src/b.rs\"   # both\n}\n";
        let refs = [CrRef {
            entity: "f x".into(),
            file: PathBuf::from("spec.elv"),
            span: 0..text.len(),
            path: "src/a.rs".into(),
        }];
        let borrowed: Vec<&CrRef> = refs.iter().collect();
        let repairs = BTreeMap::from([(
            "src/a.rs".to_string(),
            Repair {
                new: "src/moved/a.rs".into(),
                commit: "a1".into(),
            },
        )]);

        let edits = edits_for(text, &borrowed, &repairs);
        let mut buf = text.to_string();
        for (start, len, replacement) in &edits {
            buf.replace_range(*start..*start + *len, replacement);
        }
        assert_eq!(
            buf,
            "f x {\n    cr: \"src/moved/a.rs\", \"src/b.rs\"   # both\n}\n"
        );
    }

    /// The literal must be found inside the owning entity only. A
    /// second entity claiming the same dead path is repaired by its own
    /// `CrRef`, not by this one reaching outside its span.
    #[test]
    fn a_rewrite_stays_inside_its_own_entity() {
        let text = "f a {\n    cr: \"src/x.rs\"\n}\n\nf b {\n    cr: \"src/x.rs\"\n}\n";
        let first_end = text.find("\n\nf b").unwrap();
        let refs = [CrRef {
            entity: "f a".into(),
            file: PathBuf::from("spec.elv"),
            span: 0..first_end,
            path: "src/x.rs".into(),
        }];
        let borrowed: Vec<&CrRef> = refs.iter().collect();
        let repairs = BTreeMap::from([(
            "src/x.rs".to_string(),
            Repair {
                new: "src/y.rs".into(),
                commit: "a1".into(),
            },
        )]);

        let edits = edits_for(text, &borrowed, &repairs);
        assert_eq!(edits.len(), 1, "{:?}", edits);
        assert!(edits[0].0 < first_end);
    }

    #[test]
    fn edits_apply_back_to_front() {
        let text = "f x {\n    cr: \"a\", \"bb\"\n}\n";
        let refs = [
            CrRef {
                entity: "f x".into(),
                file: PathBuf::from("spec.elv"),
                span: 0..text.len(),
                path: "a".into(),
            },
            CrRef {
                entity: "f x".into(),
                file: PathBuf::from("spec.elv"),
                span: 0..text.len(),
                path: "bb".into(),
            },
        ];
        let borrowed: Vec<&CrRef> = refs.iter().collect();
        let repairs = BTreeMap::from([
            (
                "a".to_string(),
                Repair {
                    new: "aaaaaa".into(),
                    commit: "a1".into(),
                },
            ),
            (
                "bb".to_string(),
                Repair {
                    new: "b".into(),
                    commit: "a1".into(),
                },
            ),
        ]);

        let edits = edits_for(text, &borrowed, &repairs);
        let mut buf = text.to_string();
        for (start, len, replacement) in &edits {
            buf.replace_range(*start..*start + *len, replacement);
        }
        assert_eq!(buf, "f x {\n    cr: \"aaaaaa\", \"b\"\n}\n");
    }

    #[test]
    fn a_non_git_code_root_yields_a_note_not_an_error() {
        let dir = std::env::temp_dir().join(format!("mezz-fix-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = [CrRef {
            entity: "f x".into(),
            file: dir.join("spec.elv"),
            span: 0..0,
            path: "src/gone.rs".into(),
        }];
        let plan = plan(&missing, &dir);
        assert!(plan.repairs.is_empty());
        assert!(plan.note.is_some(), "a missing git repo must say so");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
