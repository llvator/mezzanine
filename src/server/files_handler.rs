//! The change as **git** sees it: which files moved, and what is in them.
//!
//! Everything else the UI knows about a diff is a projection of the analysis —
//! `diff.json` is one row per entity, and the ladder, the seed facet and the
//! churn radius all grow from those rows. That makes the graph unable to grade
//! its own paper: a file the analysis never loaded (a markdown page, a
//! lockfile, anything outside the configured languages) has no row to be
//! missing from, so "outside the scope" and "unchanged" reach the reader as the
//! same silence.
//!
//! These two endpoints are the other side of that join (UI-134). They are git
//! calls and nothing else: no analyzer, no worktree, no read of the live graph,
//! so they answer in tens of milliseconds and answer *while* a diff is still
//! computing.
//!
//! Paths are what git prints — relative to the repository top level. That is
//! the same assumption the rest of the diff machinery makes about `repo_root`
//! (see `diff::changed_files`), and it is what makes these paths comparable to
//! the `file_path` of a `diff.json` row without translation.

use std::path::Path;
use std::process::Command;

use axum::{extract::State, http::StatusCode, response::Json};

use super::diff_handler::{STAGED_REF, WORKING_REF};
use crate::diff::parse_name_status;

use super::state::AppState;
use super::types::{ChangedFile, DiffRequest, FileDiffRequest, FileDiffResponse, FileSide};

/// How much of one side of a file the browser is given.
///
/// The same ceiling `render_details` applies to the detail sidecar, for the
/// same reason: a single generated or fixture file must not be able to balloon
/// one response. Past it the text is cut and `truncated` says so, because a
/// silently shortened side would render as a diff that deletes the tail of the
/// file.
const MAX_FILE_BYTES: usize = 256 * 1024;

/// The marker appended to a cut side, so the pane shows why it stops.
const TRUNCATION_NOTE: &str = "\n\n… truncated by mezz (over 256 KB)\n";

// ------------------------------------------------------------------
//  Which git commands this pair of refs calls for
// ------------------------------------------------------------------

/// Which tree the head names, once the two sentinels are recognised.
///
/// `Working` and `Index` are read as themselves rather than as the commits the
/// analysis manufactures for them. For the index that is deliberate: the
/// commit `diff::staged_commit` builds is unreferenced and exists only for the
/// duration of a diff (UI-111), while `git diff --cached` and `git show :path`
/// read the index directly and keep working after it is collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeadSide {
    Working,
    Index,
    Commit(String),
}

/// Read the head a request names. The sentinels are the uppercase literals the
/// diff endpoint already takes; anything else is a ref for git to resolve.
pub(crate) fn head_side(to_ref: &str) -> HeadSide {
    match to_ref {
        WORKING_REF => HeadSide::Working,
        STAGED_REF => HeadSide::Index,
        r => HeadSide::Commit(r.to_string()),
    }
}

/// The `git diff` arguments that compare `from_ref` with this head.
///
/// Returned as owned strings because the commit arm borrows from the head, and
/// a caller holding a `Vec<&str>` across both arms would be borrowing from a
/// temporary.
fn diff_args(from_ref: &str, head: &HeadSide, shape: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["diff".into(), shape.into(), "-z".into()];
    if *head == HeadSide::Index {
        args.push("--cached".into());
    }
    args.push(from_ref.to_string());
    if let HeadSide::Commit(r) = head {
        args.push(r.clone());
    }
    args.push("--".into());
    args
}

/// Run git in `repo_root` and hand back stdout as it came, bytes decoded
/// lossily.
///
/// `None` when git could not answer at all — not a repository, no git on PATH,
/// a ref that stopped resolving. The callers below turn that into a 500 for
/// the list and an empty side for the content, which are the two honest
/// readings: a list that cannot be built is a failure, and a file absent from a
/// tree is the ordinary way a file is added or deleted.
fn git_output(repo_root: &Path, args: &[String]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ------------------------------------------------------------------
//  Parsing what git prints
// ------------------------------------------------------------------

/// How many lines one path gained and lost, keyed by the path git reports it
/// under.
///
/// Parsed from `git diff --numstat -z`, whose records are
/// `<added>\t<removed>\t<path>` — with the rename case again spelling the path
/// as two further records, and a **binary** file spelling both counts as `-`.
/// A `-` is not a zero: it is "this cannot be counted in lines", which is what
/// makes the file binary in the answer below.
pub(crate) fn parse_numstat(stdout: &str) -> std::collections::HashMap<String, (u32, u32, bool)> {
    let mut records = stdout.split('\0').filter(|r| !r.is_empty());
    let mut counts = std::collections::HashMap::new();
    while let Some(record) = records.next() {
        let mut parts = record.split('\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        // An empty trailing field is how numstat spells a rename: the two
        // paths follow as their own records, destination last.
        let path = if path.is_empty() {
            match (records.next(), records.next()) {
                (Some(_src), Some(dest)) => dest.to_string(),
                _ => break,
            }
        } else {
            path.to_string()
        };
        let binary = add == "-" || del == "-";
        counts.insert(
            path,
            (
                add.parse().unwrap_or(0),
                del.parse().unwrap_or(0),
                binary,
            ),
        );
    }
    counts
}

/// Untracked, non-ignored paths, as additions.
///
/// The same `ls-files --others --exclude-standard` rule `diff::changed_files`
/// uses, so the pane and the watcher agree about what counts as changed in a
/// working tree. Only a working head gets them: the index and a commit are
/// both trees, and a file that was never added is in neither.
fn untracked(repo_root: &Path) -> Vec<String> {
    let args = ["ls-files", "--others", "--exclude-standard", "-z"].map(String::from);
    git_output(repo_root, &args)
        .unwrap_or_default()
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// How many lines an untracked file adds, and whether it can be counted in
/// lines at all.
///
/// Git has no numstat for a path it does not track, so this is the only place
/// the two facts can come from. Both are needed: a new image would otherwise
/// read as `+0 −0` — a real count, and the wrong one — where `binary` says
/// there is nothing here to count. Unreadable falls back to `(0, false)`; the
/// row is still a real addition and the count is decoration on it.
fn untracked_size(repo_root: &Path, path: &str) -> (u32, bool) {
    match std::fs::read(repo_root.join(path)) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => (text.lines().count() as u32, false),
            Err(_) => (0, true),
        },
        Err(_) => (0, false),
    }
}

// ------------------------------------------------------------------
//  POST /api/changed-files
// ------------------------------------------------------------------

/// The changed-file list for one ref pair, git's answer rather than the
/// analysis's.
///
/// Two git calls: `--name-status` for the shape of each change and `--numstat`
/// for its size, joined by path. They are separate invocations because the two
/// flags do not combine into one machine-readable stream, and both are cheap.
pub(crate) async fn changed_files_handler(
    State(state): State<AppState>,
    Json(req): Json<DiffRequest>,
) -> Result<Json<Vec<ChangedFile>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    changed_files(&repo_root, &req.from_ref, &head_side(&req.to_ref))
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "git could not list the changed files for this comparison".to_string(),
            )
        })
}

/// The list itself, split from the handler so a test can call it with a repo
/// on disk and no server.
pub(crate) fn changed_files(
    repo_root: &Path,
    from_ref: &str,
    head: &HeadSide,
) -> Option<Vec<ChangedFile>> {
    let rows = parse_name_status(&git_output(
        repo_root,
        &diff_args(from_ref, head, "--name-status"),
    )?);
    let counts = parse_numstat(&git_output(
        repo_root,
        &diff_args(from_ref, head, "--numstat"),
    )?);

    let mut files: Vec<ChangedFile> = rows
        .into_iter()
        .map(|r| {
            let (additions, deletions, binary) =
                counts.get(&r.path).copied().unwrap_or((0, 0, false));
            ChangedFile {
                status: r.status,
                path: r.path,
                old_path: r.old_path,
                additions,
                deletions,
                binary,
                untracked: false,
            }
        })
        .collect();

    if *head == HeadSide::Working {
        for path in untracked(repo_root) {
            let (additions, binary) = untracked_size(repo_root, &path);
            files.push(ChangedFile {
                status: "A".to_string(),
                path,
                old_path: None,
                additions,
                deletions: 0,
                binary,
                untracked: true,
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Some(files)
}

// ------------------------------------------------------------------
//  POST /api/file-diff
// ------------------------------------------------------------------

/// Both sides of one file, as text for the pane to diff.
///
/// A side that does not exist comes back absent, which is how an addition and
/// a deletion say so: the alternative — an empty string — is a real state a
/// file can be in, and the two must not collapse.
pub(crate) async fn file_diff_handler(
    State(state): State<AppState>,
    Json(req): Json<FileDiffRequest>,
) -> Json<FileDiffResponse> {
    let repo_root = state.repo_root.read().await.clone();
    let base_path = req.base_path.as_deref().unwrap_or(&req.path);
    let base = blob_at(&repo_root, &format!("{}:{}", req.from_ref, base_path));
    let head = head_content(&repo_root, &head_side(&req.to_ref), &req.path);
    Json(FileDiffResponse {
        binary: base.as_ref().is_some_and(|s| s.binary)
            || head.as_ref().is_some_and(|s| s.binary),
        base,
        head,
    })
}

/// Whether a client-supplied path names something inside the repository.
///
/// Git rejects `HEAD:../elsewhere` itself, so this guards the one side that
/// does not go through git: a `WORKING` head reads the file off disk, and
/// `repo_root.join("../../../etc/passwd")` would resolve exactly as written.
/// The list this path comes from is git's own output and never contains such a
/// thing — but the endpoint takes a path from the client, not from the list,
/// and a read that leaves the repository is not something to leave to the
/// caller's good manners.
///
/// A rooted path is refused for the same reason: `join` on an absolute path
/// discards the root entirely.
fn is_inside_repo(path: &str) -> bool {
    use std::path::Component;
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// The head side of one file, from whichever tree the head names.
fn head_content(repo_root: &Path, head: &HeadSide, path: &str) -> Option<FileSide> {
    match head {
        // The file on disk. A working head is the tree the server is watching,
        // so this is the same bytes the canvas was built from.
        HeadSide::Working if !is_inside_repo(path) => None,
        HeadSide::Working => std::fs::read(repo_root.join(path)).ok().map(side_of),
        // `:path` is git's spelling of "stage 0 of the index" — the index read
        // as itself, not as the commit a diff manufactures from it.
        HeadSide::Index => blob_at(repo_root, &format!(":{}", path)),
        HeadSide::Commit(r) => blob_at(repo_root, &format!("{}:{}", r, path)),
    }
}

/// One blob, by any revision spelling git accepts after a colon.
///
/// `git show` is asked for bytes rather than text because the answer decides
/// whether this is a file the pane can render at all: invalid UTF-8 is the
/// definition of binary here, and decoding it lossily first would turn a PNG
/// into a wall of replacement characters that reads as a text diff.
fn blob_at(repo_root: &Path, spec: &str) -> Option<FileSide> {
    let out = Command::new("git")
        .args(["show", spec])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(side_of(out.stdout))
}

/// Decide what one side of the comparison is: text, cut text, or not text.
fn side_of(bytes: Vec<u8>) -> FileSide {
    let Ok(mut text) = String::from_utf8(bytes) else {
        return FileSide {
            text: String::new(),
            binary: true,
            truncated: false,
        };
    };
    let truncated = text.len() > MAX_FILE_BYTES;
    if truncated {
        // On a char boundary, or the truncation panics on a multi-byte
        // sequence that happens to straddle the ceiling.
        let mut cut = MAX_FILE_BYTES;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(TRUNCATION_NOTE);
    }
    FileSide {
        text,
        binary: false,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_row_is_marked_rather_than_counted_as_zero() {
        // git spells both counts `-` for a file it cannot count in lines.
        let counts = parse_numstat("-\t-\tassets/logo.png\x0012\t3\tsrc/lib.rs\x00");
        assert_eq!(counts.get("assets/logo.png"), Some(&(0, 0, true)));
        assert_eq!(counts.get("src/lib.rs"), Some(&(12, 3, false)));
    }

    #[test]
    fn numstat_keys_a_rename_under_its_destination() {
        // The path field is empty and the two paths follow as their own
        // records, so the count has to be attached to the second one — the
        // same path `--name-status` reports.
        let counts = parse_numstat("4\t2\t\0old/a.rs\0new/a.rs\0");
        assert_eq!(counts.get("new/a.rs"), Some(&(4, 2, false)));
        assert!(!counts.contains_key("old/a.rs"));
    }

    #[test]
    fn the_index_is_compared_with_cached_and_a_commit_is_not() {
        let staged = diff_args("HEAD", &HeadSide::Index, "--name-status");
        assert!(staged.iter().any(|a| a == "--cached"));
        assert_eq!(staged.last().unwrap(), "--");

        let working = diff_args("HEAD", &HeadSide::Working, "--name-status");
        assert!(!working.iter().any(|a| a == "--cached"));
        assert!(
            !working.iter().any(|a| a == "HEAD~1"),
            "a working head names no second ref: git compares against the tree"
        );

        let commit = diff_args("HEAD~1", &HeadSide::Commit("HEAD".into()), "--numstat");
        assert_eq!(commit[commit.len() - 3..], ["HEAD~1", "HEAD", "--"]);
    }

    #[test]
    fn the_sentinels_are_read_as_trees_and_everything_else_as_a_ref() {
        assert_eq!(head_side("WORKING"), HeadSide::Working);
        assert_eq!(head_side("STAGED"), HeadSide::Index);
        assert_eq!(head_side("c0ff6d2"), HeadSide::Commit("c0ff6d2".into()));
        // The lowercase literals are what `diff.json` reports, not what these
        // endpoints take: they reach git as refs and fail to resolve, which
        // is why the browser maps them before sending (`headRefFor`).
        assert_eq!(head_side("working"), HeadSide::Commit("working".into()));
    }

    #[test]
    fn a_side_over_the_ceiling_is_cut_and_says_so() {
        let side = side_of(vec![b'a'; MAX_FILE_BYTES + 10]);
        assert!(side.truncated);
        assert!(!side.binary);
        assert!(side.text.ends_with(TRUNCATION_NOTE));
    }

    #[test]
    fn a_side_that_is_not_utf8_is_binary_rather_than_replacement_characters() {
        let side = side_of(vec![0x89, 0x50, 0x4e, 0x47, 0xff, 0xfe]);
        assert!(side.binary);
        assert!(side.text.is_empty());
    }

    /// A repository with one commit and a working tree that has moved on in
    /// every way the pane has to report.
    fn repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mezz-files-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        for args in [
            vec!["init", "-q", "--initial-branch=main", "."],
            vec!["config", "user.email", "t@t.t"],
            vec!["config", "user.name", "t"],
        ] {
            git(&dir, &args);
        }
        std::fs::write(dir.join("src/kept.rs"), "fn kept() {}\n").unwrap();
        std::fs::write(dir.join("src/gone.rs"), "fn gone() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# one\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "base"]);
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success());
    }

    #[test]
    fn a_working_head_lists_edits_deletions_and_untracked_files_alike() {
        let dir = repo("working");
        std::fs::write(dir.join("README.md"), "# one\n# two\n").unwrap();
        std::fs::remove_file(dir.join("src/gone.rs")).unwrap();
        std::fs::write(dir.join("src/new.rs"), "fn new() {}\n").unwrap();

        let files = changed_files(&dir, "HEAD", &HeadSide::Working).unwrap();
        let by_path = |p: &str| files.iter().find(|f| f.path == p).cloned();

        assert_eq!(by_path("README.md").unwrap().status, "M");
        assert_eq!(by_path("README.md").unwrap().additions, 1);
        assert_eq!(by_path("src/gone.rs").unwrap().status, "D");
        // The untracked file is the one `git diff` says nothing about, and the
        // one the reader most wants: it is the code they just wrote.
        let fresh = by_path("src/new.rs").expect("untracked files are additions");
        assert_eq!(fresh.status, "A");
        assert!(fresh.untracked);
        assert_eq!(fresh.additions, 1);
        assert!(by_path("src/kept.rs").is_none(), "unchanged files are absent");

        // An untracked *binary* file has no numstat either, and `+0 −0` would
        // be a real count of the wrong thing.
        std::fs::write(dir.join("logo.png"), [0x89u8, 0x50, 0x4e, 0x47, 0xff]).unwrap();
        let png = changed_files(&dir, "HEAD", &HeadSide::Working)
            .unwrap()
            .into_iter()
            .find(|f| f.path == "logo.png")
            .expect("untracked whatever it holds");
        assert!(png.binary);
        assert_eq!(png.additions, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_index_lists_only_what_was_added_to_it() {
        let dir = repo("index");
        std::fs::write(dir.join("README.md"), "# staged\n").unwrap();
        std::fs::write(dir.join("src/kept.rs"), "fn kept() { () }\n").unwrap();
        git(&dir, &["add", "README.md"]);

        let files = changed_files(&dir, "HEAD", &HeadSide::Index).unwrap();
        assert_eq!(files.len(), 1, "only the staged path: {:?}", files);
        assert_eq!(files[0].path, "README.md");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_that_leaves_the_repository_reads_nothing() {
        // The `WORKING` side is a filesystem read, so it is the one that has to
        // refuse this itself — git refuses `HEAD:../x` on its own.
        for escape in ["../secrets", "a/../../secrets", "/etc/passwd", ""] {
            assert!(!is_inside_repo(escape), "{escape} must not be read");
        }
        assert!(is_inside_repo("src/server/files_handler.rs"));
        assert!(is_inside_repo("./README.md"));

        let dir = repo("escape");
        assert!(
            head_content(&dir, &HeadSide::Working, "../../../etc/passwd").is_none(),
            "a working head must not read outside the tree it watches"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_files_two_sides_come_from_the_two_trees_it_names() {
        let dir = repo("sides");
        std::fs::write(dir.join("src/kept.rs"), "fn kept() { () }\n").unwrap();

        let base = blob_at(&dir, "HEAD:src/kept.rs").expect("the committed side");
        assert_eq!(base.text, "fn kept() {}\n");
        let head = head_content(&dir, &HeadSide::Working, "src/kept.rs").unwrap();
        assert_eq!(head.text, "fn kept() { () }\n");
        // A path that is in neither tree has no side at all, which is how an
        // addition and a deletion are told apart from an empty file.
        assert!(blob_at(&dir, "HEAD:src/never.rs").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
