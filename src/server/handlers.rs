use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        Json,
    },
};
use std::process::Command;
use tokio::sync::broadcast;

use crate::output::{self, JsonRenderer};

use super::state::AppState;
use super::types::{BranchInfo, CommitInfo, RootPathResponse, StagedFile, StashInfo};

/// SSE handler: clients subscribe to reload events.
pub(crate) async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.tx.subscribe();
    let stream = async_stream::stream! {
        // Send an initial "connected" event so the client knows the stream is live.
        yield Ok(Event::default().event("connected").data("ok"));
        loop {
            match rx.recv().await {
                Ok(kind) => {
                    // Two event names, so a client can re-fetch only the
                    // overlay when only the overlay moved (UI-067).
                    yield Ok(Event::default().event(kind.event_name()).data("changed"));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

/// GET /api/branch — what `HEAD` points at in the analyzed checkout.
///
/// Its own endpoint rather than a field on `/api/root`, because it answers a
/// different question and answers it far more often: the root moves when a
/// reader repoints the server, the branch moves whenever they check one out,
/// and the chip re-asks on every `head` event the watcher sends (UI-114).
pub(crate) async fn branch_handler(State(state): State<AppState>) -> Json<BranchInfo> {
    let repo_root = state.repo_root.read().await.clone();
    Json(git_branch(&repo_root))
}

/// Read `HEAD` at `repo_root`. Shared by watch-mode `/api/branch` and
/// serve-mode `/api/repos/{slug}/branch`, the way `git_commits` is.
///
/// `symbolic-ref` rather than `rev-parse --abbrev-ref HEAD`, which answers
/// the literal string `HEAD` when detached — indistinguishable from a branch
/// of that name without a second call to find out which it meant. Asking the
/// symref directly makes "not on a branch" a failed call rather than a value
/// to disambiguate, so the two states cannot be confused.
///
/// Two calls at most, and no error path: git failing to answer *is* the
/// answer for a root that is not a checkout.
pub(crate) fn git_branch(repo_root: &std::path::Path) -> BranchInfo {
    let branch = crate::diff::git_lines(repo_root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .into_iter()
        .next();
    let head_short = crate::diff::git_lines(repo_root, &["rev-parse", "--short", "HEAD"])
        .into_iter()
        .next();
    // An unborn branch has a name and no commit, so the name is what says
    // this is a git repository. A detached HEAD has a commit and no name,
    // and says it the other way round.
    let git = branch.is_some() || head_short.is_some();
    BranchInfo {
        detached: git && branch.is_none(),
        branch,
        head_short,
        git,
    }
}

/// GET /api/commits — list recent commits.
pub(crate) async fn commits_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<CommitInfo>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    Ok(Json(git_commits(&repo_root)?))
}

/// Read the 50 most recent commits of the repository at `repo_root`.
/// Shared by watch-mode `/api/commits` and serve-mode
/// `/api/repos/{slug}/commits`.
pub(crate) fn git_commits(
    repo_root: &std::path::Path,
) -> Result<Vec<CommitInfo>, (StatusCode, String)> {
    let output = Command::new("git")
        .args([
            "log",
            "--oneline",
            "--format=%H|%h|%s|%an|%ad",
            "--date=short",
            "-50",
        ])
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to run git: {}", e),
            )
        })?;

    if !output.status.success() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Not a git repository or git command failed".to_string(),
        ));
    }

    let commits: Vec<CommitInfo> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 5 {
                Some(CommitInfo {
                    hash: parts[0].to_string(),
                    short_hash: parts[1].to_string(),
                    message: parts[2].to_string(),
                    author: parts[3].to_string(),
                    date: parts[4].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(commits)
}

/// GET /api/stashes — list the repository's stash entries.
pub(crate) async fn stashes_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<StashInfo>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    Ok(Json(git_stashes(&repo_root)?))
}

/// The fields `parse_stash_lines` expects, newest stash first.
///
/// `%P` and `%p` are the full and abbreviated parent lists; the first entry
/// of each is the commit the stash was taken on. `%gd` would give the
/// `stash@{N}` selector directly and is deliberately not asked for: with
/// `--date=short` set for `%ad` it renders as `stash@{2026-08-12}` instead,
/// one flag quietly changing the meaning of an unrelated placeholder.
const STASH_FORMAT: &str = "%H|%h|%P|%p|%s|%an|%ad";

/// Read the repository's stash entries. Shared by watch-mode `/api/stashes`
/// and serve-mode `/api/repos/{slug}/stashes`, the way `git_commits` is.
///
/// A repository with no stashes answers with an empty list and a success —
/// that is the common case, and a 500 there would read to the picker as the
/// server being broken rather than as there being nothing to show.
pub(crate) fn git_stashes(
    repo_root: &std::path::Path,
) -> Result<Vec<StashInfo>, (StatusCode, String)> {
    let output = Command::new("git")
        .args([
            "stash",
            "list",
            &format!("--format={}", STASH_FORMAT),
            "--date=short",
        ])
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to run git: {}", e),
            )
        })?;

    if !output.status.success() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Not a git repository or git command failed".to_string(),
        ));
    }

    Ok(parse_stash_lines(&String::from_utf8_lossy(&output.stdout)))
}

/// GET /api/staged — the repo-relative paths currently in the index, with the
/// status letter git gives each one.
///
/// Separate from `POST /api/diff` because the diff is expensive — a worktree
/// checkout and a full analysis — and this is two git calls. The picker can
/// show what would be compared, and say that nothing is staged, without paying
/// for the comparison to find out.
pub(crate) async fn staged_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<StagedFile>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    Ok(Json(git_staged(&repo_root)?))
}

/// The index against HEAD, as `<status>\t<path>` rows.
///
/// `--cached` is the whole point: `git diff --name-status` alone answers about
/// the working tree, which is the comparison "Current Changes" already offers.
///
/// An empty list is a success. Nothing staged is the ordinary state of a
/// repository, and a 500 there would read to the picker as the server being
/// broken rather than as there being nothing to show — the same call the stash
/// list makes.
pub(crate) fn git_staged(
    repo_root: &std::path::Path,
) -> Result<Vec<StagedFile>, (StatusCode, String)> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-status", "-z"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to run git: {}", e),
            )
        })?;

    if !output.status.success() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Not a git repository or git command failed".to_string(),
        ));
    }

    Ok(parse_staged_records(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Parse `git diff --cached --name-status -z` output.
///
/// `-z` rather than lines, because a path may contain anything a filesystem
/// allows — including a newline, which line splitting would turn into two
/// half-paths. It also stops git from quoting and escaping non-ASCII names, so
/// what arrives is the path as it is on disk.
///
/// Records alternate status then path, except for a rename or copy, whose
/// status carries a similarity score and is followed by *two* paths — old then
/// new. The new one is what the diff will report, so the old is read and
/// dropped; taking it as the next status instead is what would desynchronise
/// every row after the first rename.
fn parse_staged_records(stdout: &str) -> Vec<StagedFile> {
    let mut records = stdout.split('\0').filter(|r| !r.is_empty());
    let mut files = Vec::new();
    while let Some(status) = records.next() {
        let Some(path) = records.next() else { break };
        let renamed = status.starts_with('R') || status.starts_with('C');
        let path = if renamed {
            // The second path is the destination, and the first was the source.
            match records.next() {
                Some(dest) => dest,
                None => break,
            }
        } else {
            path
        };
        files.push(StagedFile {
            status: status.chars().next().unwrap_or('?').to_string(),
            path: path.to_string(),
        });
    }
    files
}

/// Parse `git stash list --format=STASH_FORMAT` output.
///
/// Split from both ends rather than once across: a stash subject is
/// `WIP on <branch>: <sha> <that commit's subject>`, and a commit subject is
/// free text that may itself contain the separator. The four fixed fields are
/// taken from the left and the two from the right, which leaves whatever is
/// between them as the message intact.
///
/// A row that does not carry every field is dropped. Nothing downstream can
/// use a stash whose commit or base is unknown, and a half-filled row in the
/// picker would offer a comparison that cannot be computed.
fn parse_stash_lines(stdout: &str) -> Vec<StashInfo> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .filter_map(|(i, line)| {
            let head: Vec<&str> = line.splitn(5, '|').collect();
            let [hash, short_hash, parents, short_parents, rest] = head[..] else {
                return None;
            };
            let tail: Vec<&str> = rest.rsplitn(3, '|').collect();
            let [date, author, message] = tail[..] else {
                return None;
            };
            // First parent only. The second is the stashed index and the
            // third the untracked files; neither is the tree this was
            // branched from.
            let base_hash = parents.split_whitespace().next()?;
            let base_short = short_parents.split_whitespace().next()?;
            Some(StashInfo {
                hash: hash.to_string(),
                short_hash: short_hash.to_string(),
                selector: format!("stash@{{{}}}", i),
                base_hash: base_hash.to_string(),
                base_short: base_short.to_string(),
                message: message.to_string(),
                author: author.to_string(),
                date: date.to_string(),
            })
        })
        .collect()
}

/// GET /api/root — get current analyzed root path.
pub(crate) async fn get_root_handler(State(state): State<AppState>) -> Json<RootPathResponse> {
    let path = state.repo_root.read().await.display().to_string();
    Json(RootPathResponse {
        path,
        success: true,
        message: None,
        entity_count: None,
        relationship_count: None,
    })
}

/// GET /api/diff — diff result JSON.
pub(crate) async fn diff_get_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), StatusCode> {
    let data = state
        .diff_result
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match data.as_ref() {
        Some(json) => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json.clone(),
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// GET /api/details/base — base commit details (for side-by-side comparison).
pub(crate) async fn base_details_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), StatusCode> {
    let data = state
        .base_details
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match data.as_ref() {
        Some(json) => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json.clone(),
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// GET /api/graph — full graph JSON.
pub(crate) async fn graph_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), (StatusCode, String)> {
    let graph = state.graph.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Graph lock poisoned: {}", e),
        )
    })?;
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {}", e),
        )
    })?;
    let json = output::render(&graph, &config).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Render failed: {}", e),
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json,
    ))
}

/// GET /api/index — file/folder hierarchy.
pub(crate) async fn index_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), (StatusCode, String)> {
    let graph = state.graph.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Graph lock poisoned: {}", e),
        )
    })?;
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {}", e),
        )
    })?;
    let json = JsonRenderer::render_index(&graph, &config).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Render failed: {}", e),
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json,
    ))
}

/// GET /api/details — entity details + file contents.
pub(crate) async fn details_handler(
    State(state): State<AppState>,
) -> Result<([(axum::http::header::HeaderName, &'static str); 1], String), (StatusCode, String)> {
    let graph = state.graph.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Graph lock poisoned: {}", e),
        )
    })?;
    let config = state.config.read().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config lock poisoned: {}", e),
        )
    })?;
    let json = JsonRenderer::render_details(&graph, &config).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Render failed: {}", e),
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `%H|%h|%P|%p|%s|%an|%ad` for a stash taken with `-u`: three parents,
    /// the first being the commit it was taken on.
    const UNTRACKED: &str = "ec157e23ecc|ec157e2|d55c05b74a8 c58de026286 8d47fba789a|d55c05b c58de02 8d47fba|WIP on main: d55c05b c2|Ada|2026-08-12";

    fn one(line: &str) -> StashInfo {
        let mut rows = parse_stash_lines(line);
        assert_eq!(rows.len(), 1, "expected exactly one parsed row");
        rows.pop().unwrap()
    }

    #[test]
    fn base_is_the_first_parent() {
        let s = one(UNTRACKED);
        assert_eq!(s.hash, "ec157e23ecc");
        // Not the second (stashed index) or third (untracked files).
        assert_eq!(s.base_hash, "d55c05b74a8");
        assert_eq!(s.base_short, "d55c05b");
    }

    #[test]
    fn selector_numbers_by_position() {
        let rows = parse_stash_lines(&format!("{}\n{}", UNTRACKED, UNTRACKED));
        assert_eq!(rows[0].selector, "stash@{0}");
        assert_eq!(rows[1].selector, "stash@{1}");
    }

    /// The subject embeds a commit subject, which is free text. Splitting
    /// once across the row would hand the tail of the message to `author`.
    #[test]
    fn message_may_contain_the_separator() {
        let s = one("aaa|aaa|bbb ccc|bbb ccc|WIP on main: bbb fix a|b parsing|Ada|2026-08-12");
        assert_eq!(s.message, "WIP on main: bbb fix a|b parsing");
        assert_eq!(s.author, "Ada");
        assert_eq!(s.date, "2026-08-12");
    }

    #[test]
    fn rows_missing_fields_are_dropped() {
        assert!(parse_stash_lines("aaa|aaa|bbb").is_empty());
        // Every field present but no parent: a stash on a root commit cannot
        // name the tree it came from, so there is nothing to compare against.
        assert!(parse_stash_lines("aaa|aaa|||WIP on main: x|Ada|2026-08-12").is_empty());
    }

    #[test]
    fn no_stashes_is_no_rows_not_an_error() {
        assert!(parse_stash_lines("").is_empty());
        assert!(parse_stash_lines("\n\n").is_empty());
    }

    // --------------------------------------------------------------
    //  The index listing (UI-111)
    // --------------------------------------------------------------

    #[test]
    fn staged_records_are_status_then_path() {
        let rows = parse_staged_records("M\0src/a.rs\0A\0src/b.rs\0D\0src/c.rs\0");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, "M");
        assert_eq!(rows[0].path, "src/a.rs");
        assert_eq!(rows[2].status, "D");
        assert_eq!(rows[2].path, "src/c.rs");
    }

    /// A repository with one commit, at a path unique to this test.
    fn branch_repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mezz-branch-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q", "--initial-branch=main", "."],
            vec!["config", "user.email", "t@t.t"],
            vec!["config", "user.name", "t"],
        ] {
            run_git(&dir, &args);
        }
        dir
    }

    fn run_git(dir: &std::path::Path, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success());
    }

    fn commit(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), "x\n").unwrap();
        run_git(dir, &["add", "."]);
        run_git(dir, &["commit", "-qm", name]);
    }

    #[test]
    fn a_checkout_reports_the_branch_it_is_on() {
        let dir = branch_repo("on-branch");
        commit(&dir, "a.txt");
        run_git(&dir, &["checkout", "-q", "-b", "feat/chip"]);

        let head = git_branch(&dir);
        assert_eq!(head.branch.as_deref(), Some("feat/chip"));
        assert!(!head.detached);
        assert!(head.git);
        assert!(head.head_short.is_some(), "a branch with a commit has one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The state `rev-parse --abbrev-ref HEAD` cannot report without being
    /// asked twice: it answers the literal string `HEAD`, which is also what
    /// a branch of that name would answer.
    #[test]
    fn a_detached_head_is_a_commit_and_not_a_branch_named_head() {
        let dir = branch_repo("detached");
        commit(&dir, "a.txt");
        commit(&dir, "b.txt");
        run_git(&dir, &["checkout", "-q", "HEAD~1"]);

        let head = git_branch(&dir);
        assert_eq!(head.branch, None, "a detached HEAD is on no branch");
        assert!(head.detached);
        assert!(head.git);
        assert!(head.head_short.is_some(), "it still names a commit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A repository with no commits still has a branch — the one the first
    /// commit will land on — and reporting nothing there would read as "not
    /// a git repository", which is a different thing entirely.
    #[test]
    fn an_unborn_branch_has_a_name_and_no_commit() {
        let dir = branch_repo("unborn");
        let head = git_branch(&dir);
        assert_eq!(head.branch.as_deref(), Some("main"));
        assert!(!head.detached);
        assert!(head.git);
        assert_eq!(head.head_short, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `mezz watch` runs against any directory. Not being a checkout is an
    /// answer the chip can act on, not a failure to report.
    #[test]
    fn a_directory_that_is_not_a_checkout_is_not_an_error() {
        let dir = std::env::temp_dir().join(format!("mezz-nogit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let head = git_branch(&dir);
        assert!(!head.git);
        assert_eq!(head.branch, None);
        assert!(!head.detached, "no repository is not a detached one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rename is three records, not two. Reading the source path as the next
    /// row's status is what desynchronises every row after the first rename —
    /// the failure is not the rename itself but everything below it.
    #[test]
    fn a_rename_carries_two_paths_and_the_destination_wins() {
        let rows = parse_staged_records("R100\0src/old.rs\0src/new.rs\0M\0src/after.rs\0");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, "R");
        assert_eq!(
            rows[0].path, "src/new.rs",
            "the destination is what the diff will report"
        );
        // The row after the rename is where a mis-count would show.
        assert_eq!(rows[1].status, "M");
        assert_eq!(rows[1].path, "src/after.rs");
    }

    /// The reason for `-z`. A path may contain a newline, and line splitting
    /// would turn one file into two half-paths that name nothing.
    #[test]
    fn a_path_may_contain_a_newline() {
        let rows = parse_staged_records("M\0src/we\nird.rs\0");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "src/we\nird.rs");
    }

    #[test]
    fn nothing_staged_is_no_rows_not_an_error() {
        assert!(parse_staged_records("").is_empty());
    }

    /// git does not emit a status with no path, but a truncated read is not
    /// worth a panic or a row naming an empty file.
    #[test]
    fn a_dangling_status_is_dropped() {
        assert!(parse_staged_records("M\0").is_empty());
        assert_eq!(parse_staged_records("M\0a.rs\0R100\0old.rs\0").len(), 1);
    }
}
