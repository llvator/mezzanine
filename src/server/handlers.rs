use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        Json,
    },
};
use std::process::Command;
use tokio::sync::broadcast;

use crate::activity;
use crate::output::{self, JsonRenderer};

use super::state::AppState;
use super::types::{
    ActivityQuery, BranchInfo, BranchRef, CommitInfo, CommitsQuery, MergeBaseQuery,
    RootPathResponse, StagedFile, StashInfo,
};

/// What one turn of the SSE loop decided to do.
///
/// The loop reads two channels — completion pings and activity notices —
/// and each has the same three outcomes. Naming them keeps the `select!`
/// arms down to one expression apiece; inlining the matches instead put the
/// whole thing over the complexity gate.
enum Tick {
    Send(Box<Event>),
    /// A lagged receiver. Skipped rather than reported: the client resyncs
    /// through `/api/activity?since=`, and a status feed's correct response
    /// to falling behind is to lose the middle, not the connection.
    Skip,
    Stop,
}

/// SSE handler: clients subscribe to reload events and activity notices.
pub(crate) async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.tx.subscribe();
    let mut notices = state.activity.subscribe();
    let stream = async_stream::stream! {
        // Send an initial "connected" event so the client knows the stream is live.
        yield Ok(Event::default().event("connected").data("ok"));
        loop {
            let tick = tokio::select! {
                reload = rx.recv() => reload_tick(reload),
                notice = notices.recv() => notice_tick(notice),
            };
            match tick {
                Tick::Send(event) => yield Ok(*event),
                Tick::Skip => continue,
                Tick::Stop => break,
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

/// A completion ping. Three event names, so a client can re-fetch only the
/// overlay when only the overlay moved (UI-067).
fn reload_tick(reload: Result<super::state::ReloadKind, broadcast::error::RecvError>) -> Tick {
    match reload {
        Ok(kind) => Tick::Send(Box::new(
            Event::default().event(kind.event_name()).data("changed"),
        )),
        Err(broadcast::error::RecvError::Lagged(_)) => Tick::Skip,
        Err(broadcast::error::RecvError::Closed) => Tick::Stop,
    }
}

/// An activity notice. Unlike the three above this carries its payload
/// inline rather than telling the client to come and get it: the whole point
/// is to say something *during* work the client is otherwise blind to, and a
/// round trip per status line would be a request every few hundred
/// milliseconds for the length of an analysis (UI-138).
fn notice_tick(notice: Result<activity::Update, broadcast::error::RecvError>) -> Tick {
    match notice {
        Ok(update) => match serde_json::to_string(&update) {
            Ok(json) => Tick::Send(Box::new(Event::default().event("activity").data(json))),
            // Unreachable for this shape, and not worth dropping the stream
            // over if it ever stops being.
            Err(_) => Tick::Skip,
        },
        Err(broadcast::error::RecvError::Lagged(_)) => Tick::Skip,
        Err(broadcast::error::RecvError::Closed) => Tick::Stop,
    }
}

/// GET /api/activity — what the engine is doing, and what it recently said.
///
/// Exists alongside the stream rather than instead of it, for the two moments
/// the stream cannot cover: a page opened while a run is already in flight,
/// which has missed the notices that would have told it, and one that
/// reconnects after a drop. `since` is the last `seq` the client saw, so the
/// answer is exactly the gap — `0`, the default, means "everything you have".
pub(crate) async fn activity_handler(
    State(state): State<AppState>,
    Query(query): Query<ActivityQuery>,
) -> Json<activity::Snapshot> {
    Json(state.activity.snapshot(query.since.unwrap_or(0)))
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

/// Run git at `repo_root` and hand back its stdout.
///
/// The two failures it folds together are the two a caller answers the same
/// way: git absent from the machine, and git refusing the command. Neither is
/// something the reader chose, and both leave the endpoint with nothing to
/// report but that it could not answer.
///
/// Shared by every git-reading endpoint in this module rather than copied
/// per handler, which is what let `/api/commits` quietly grow a second
/// spelling of the same twenty lines.
fn git_stdout(
    repo_root: &std::path::Path,
    args: &[&str],
) -> Result<String, (StatusCode, String)> {
    let output = Command::new("git")
        .args(args)
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

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A ref this server will hand to git, or a 400 saying why not.
///
/// One rule, and it is about the command line rather than about git history:
/// everything after a git subcommand is positional until something looks like
/// a flag, so a "ref" spelled `--output=/tmp/x` would be read as one. These
/// endpoints are the only place a ref arrives from a query string, so this is
/// where it is checked.
///
/// A ref that simply does not exist is *not* rejected here. git answers that
/// question better than a pre-check could, and does it for `HEAD~3`,
/// `origin/main@{yesterday}` and every other spelling this function would
/// otherwise have to learn.
fn usable_ref(git_ref: &str) -> Result<&str, (StatusCode, String)> {
    let trimmed = git_ref.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Not a ref this server will resolve: {:?}", git_ref),
        ));
    }
    Ok(trimmed)
}

/// The commit fields the picker shows, subject last.
///
/// Last because it is the one field that can contain the separator: a subject
/// is free text, and taking `%s` from the middle hands its tail to the author
/// and shifts the date off the end. Five fixed fields from the left and the
/// remainder left whole is the rule [`parse_stash_lines`] already follows,
/// arrived at from the same trap.
///
/// `%P` is here for the picker's `From`, which names the oldest commit the
/// reader wants *included* (UI-151). The tree that comparison starts from is
/// therefore that commit's parent, and the picker can only translate the one
/// into the other if the listing says what the parent is. Asked here rather
/// than resolved per click, because `%P` costs nothing on a walk git is
/// already doing and a round trip per click would make the range flicker.
const COMMIT_FORMAT: &str = "--format=%H|%h|%P|%an|%ad|%s";

/// How many commits a listing carries when the caller does not say.
pub(crate) const COMMIT_WINDOW: usize = 50;

/// The most a caller may ask for. A picker list is scrolled, not read, and a
/// repository's whole history rendered into one modal is a long wait for rows
/// nobody reaches.
const MAX_COMMIT_WINDOW: usize = 500;

/// GET /api/commits — list recent commits, of `HEAD` or of any ref.
pub(crate) async fn commits_handler(
    State(state): State<AppState>,
    Query(query): Query<CommitsQuery>,
) -> Result<Json<Vec<CommitInfo>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    Ok(Json(git_commits(
        &repo_root,
        query.git_ref.as_deref(),
        query.limit.unwrap_or(COMMIT_WINDOW),
    )?))
}

/// Read the most recent commits reachable from `git_ref` — `HEAD` when it is
/// `None`. Shared by watch-mode `/api/commits` and serve-mode
/// `/api/repos/{slug}/commits`.
///
/// The `--` is not decoration. `git log <ref>` reads its argument as a ref
/// *or* as a path, and a branch and a directory can share a name; the
/// separator says which one was meant, so a repository with a `docs/` branch
/// and a `docs/` folder still lists commits.
pub(crate) fn git_commits(
    repo_root: &std::path::Path,
    git_ref: Option<&str>,
    limit: usize,
) -> Result<Vec<CommitInfo>, (StatusCode, String)> {
    let git_ref = usable_ref(git_ref.unwrap_or("HEAD"))?;
    let count = format!("-{}", limit.clamp(1, MAX_COMMIT_WINDOW));
    let stdout = git_stdout(
        repo_root,
        &[
            "log",
            COMMIT_FORMAT,
            "--date=short",
            &count,
            git_ref,
            "--",
        ],
    )?;
    Ok(parse_commit_lines(&stdout))
}

/// One commit, named by any ref. `None` when the ref resolves to nothing —
/// which is how [`git_merge_base`] reports unrelated histories.
fn commit_at(
    repo_root: &std::path::Path,
    git_ref: &str,
) -> Result<Option<CommitInfo>, (StatusCode, String)> {
    let stdout = git_stdout(
        repo_root,
        &["log", "-1", COMMIT_FORMAT, "--date=short", git_ref, "--"],
    )?;
    Ok(parse_commit_lines(&stdout).into_iter().next())
}

/// Parse `git log COMMIT_FORMAT` output. A row missing a field is dropped:
/// nothing downstream can do anything with a half-named commit, and a blank
/// row in the picker would offer a comparison that cannot be computed.
fn parse_commit_lines(stdout: &str) -> Vec<CommitInfo> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(6, '|').collect();
            let [hash, short_hash, parents, author, date, message] = parts[..] else {
                return None;
            };
            Some(CommitInfo {
                hash: hash.to_string(),
                short_hash: short_hash.to_string(),
                // First parent only, and `None` on the root commit. A merge's
                // second parent is the side that was merged in, so a range
                // starting at a merge starts at the branch it landed on —
                // which is the history the picker is listing. The root has no
                // earlier tree at all, and saying so is what lets the picker
                // decline that one click instead of sending git a `~1` that
                // cannot resolve.
                parent_hash: parents.split_whitespace().next().map(str::to_string),
                message: message.to_string(),
                author: author.to_string(),
                date: date.to_string(),
            })
        })
        .collect()
}

/// GET /api/merge-base — the commit two refs last had in common (UI-143).
///
/// The endpoint exists because comparing two branches by their tips answers a
/// question almost nobody asks. A branch that was cut a week ago differs from
/// `main` by its own work *and* by everything that landed on main since, and a
/// tip-to-tip comparison reports the second half as the branch having removed
/// it. This is the same mistake the stash picker already refuses to make by
/// pairing a stash with its own first parent instead of with HEAD (UI-107) —
/// here the right base is where the two branches diverged.
///
/// `null`, not a 404, when the two share no history. That is a fact about the
/// pair rather than a failed lookup, and the picker says so and falls back to
/// the tip.
pub(crate) async fn merge_base_handler(
    State(state): State<AppState>,
    Query(query): Query<MergeBaseQuery>,
) -> Result<Json<Option<CommitInfo>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    Ok(Json(git_merge_base(&repo_root, &query.from, &query.to)?))
}

/// Where two refs diverged, as a commit. `None` for unrelated histories.
pub(crate) fn git_merge_base(
    repo_root: &std::path::Path,
    from: &str,
    to: &str,
) -> Result<Option<CommitInfo>, (StatusCode, String)> {
    let from = usable_ref(from)?;
    let to = usable_ref(to)?;
    // `git_lines` rather than `git_stdout`: two refs with no common ancestor
    // make `merge-base` exit non-zero, and that is an answer — "they never
    // met" — not a server that failed to look.
    let Some(sha) = crate::diff::git_lines(repo_root, &["merge-base", from, to])
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    commit_at(repo_root, &sha)
}

/// GET /api/branches — the branches a comparison can be made from (UI-143).
pub(crate) async fn branches_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<BranchRef>>, (StatusCode, String)> {
    let repo_root = state.repo_root.read().await.clone();
    Ok(Json(git_branches(&repo_root)?))
}

/// The refname first, so the namespace decides `remote` rather than the name
/// guessing at it, and the subject last, for [`COMMIT_FORMAT`]'s reason.
///
/// `%(HEAD)` is `*` on the branch the checkout is on and a space on every
/// other. It sits in the middle deliberately: `git_lines` trims each row, and
/// a single-space field at either end would be trimmed away along with the
/// row's own whitespace.
const BRANCH_FORMAT: &str = "--format=%(refname)|%(objectname)|%(objectname:short)|%(HEAD)|%(authorname)|%(committerdate:short)|%(contents:subject)";

/// How many branches the picker is handed. Sorted by most recently committed
/// to, so the cut falls on branches nobody has touched in months — and a
/// reviewer who wants one of those can still type its name into `From`.
const BRANCH_WINDOW: usize = 200;

/// Every local branch and remote-tracking branch, most recently worked on
/// first.
pub(crate) fn git_branches(
    repo_root: &std::path::Path,
) -> Result<Vec<BranchRef>, (StatusCode, String)> {
    let count = format!("--count={}", BRANCH_WINDOW);
    let stdout = git_stdout(
        repo_root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            BRANCH_FORMAT,
            &count,
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse_branch_lines(&stdout))
}

/// Parse `git for-each-ref BRANCH_FORMAT` output.
///
/// Two rows are deliberately dropped. Anything outside `refs/heads` and
/// `refs/remotes` is not a branch, whatever it was asked for. And
/// `refs/remotes/<remote>/HEAD` is a symbolic ref at whatever the remote calls
/// its default branch — a second name for a row already in this list, which
/// would read as two branches whose tips can never differ.
fn parse_branch_lines(stdout: &str) -> Vec<BranchRef> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(7, '|').collect();
            let [refname, tip, tip_short, head, author, date, subject] = parts[..] else {
                return None;
            };
            let (name, remote) = branch_name(refname)?;
            if name.ends_with("/HEAD") {
                return None;
            }
            Some(BranchRef {
                name,
                remote,
                is_head: head.trim() == "*",
                tip: tip.to_string(),
                tip_short: tip_short.to_string(),
                subject: subject.to_string(),
                author: author.to_string(),
                date: date.to_string(),
            })
        })
        .collect()
}

/// A full refname as the short name a reviewer types, and whether it is
/// remote-tracking. `None` for a ref in neither namespace.
fn branch_name(refname: &str) -> Option<(String, bool)> {
    if let Some(name) = refname.strip_prefix("refs/heads/") {
        return Some((name.to_string(), false));
    }
    refname
        .strip_prefix("refs/remotes/")
        .map(|name| (name.to_string(), true))
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
    let format = format!("--format={}", STASH_FORMAT);
    let stdout = git_stdout(repo_root, &["stash", "list", &format, "--date=short"])?;
    Ok(parse_stash_lines(&stdout))
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
    let stdout = git_stdout(repo_root, &["diff", "--cached", "--name-status", "-z"])?;
    Ok(parse_staged_records(&stdout))
}

/// The index's rows, as the picker needs them.
///
/// The parse itself is `diff::parse_name_status`'s, where the rest of this
/// codebase's git plumbing lives: `git diff --name-status -z` has one shape
/// whoever asks for it, and its trap — a rename is *three* records, so
/// reading the source path as the next row's status desynchronises every row
/// after it — is not a thing to get right twice. This is the projection onto
/// what the index list shows: git's status letter and the path it lands on.
///
/// The source path of a rename is dropped rather than carried. The picker
/// lists what is staged; the file's history before it got there is a question
/// only the diff asks (UI-134).
fn parse_staged_records(stdout: &str) -> Vec<StagedFile> {
    crate::diff::parse_name_status(stdout)
        .into_iter()
        .map(|r| StagedFile {
            status: r.status,
            path: r.path,
        })
        .collect()
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

    // --------------------------------------------------------------
    //  Comparing across branches (UI-143)
    // --------------------------------------------------------------

    /// The trap `COMMIT_FORMAT` moves the subject to the end for: a subject
    /// is free text, and splitting the row across every separator hands its
    /// tail to the author and pushes the date off the end.
    #[test]
    fn a_commit_subject_may_contain_the_separator() {
        let rows = parse_commit_lines("aaa|aaa|bbb|Ada|2026-08-31|fix a|b parsing");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "fix a|b parsing");
        assert_eq!(rows[0].author, "Ada");
        assert_eq!(rows[0].date, "2026-08-31");
        assert_eq!(rows[0].parent_hash.as_deref(), Some("bbb"));
    }

    #[test]
    fn a_commit_row_missing_fields_is_dropped() {
        assert!(parse_commit_lines("aaa|aaa|Ada").is_empty());
        assert!(parse_commit_lines("").is_empty());
    }

    /// The reason the ref goes through `usable_ref` at all: everything after
    /// a git subcommand is positional until it looks like a flag.
    #[test]
    fn a_ref_that_would_read_as_a_flag_is_refused() {
        assert!(usable_ref("--output=/tmp/x").is_err());
        assert!(usable_ref("  ").is_err());
        assert_eq!(usable_ref(" main ").unwrap(), "main");
        assert!(usable_ref("origin/main").is_ok());
        assert!(usable_ref("HEAD~3").is_ok());
    }

    /// A refname's namespace says whether it is remote-tracking. Its *name*
    /// does not, in either direction: a local branch may be `feat/chip` and a
    /// remote-tracking one `origin/main`.
    #[test]
    fn a_slash_in_a_name_does_not_make_a_branch_remote() {
        let rows = parse_branch_lines(
            "refs/heads/feat/chip|aaa111|aaa|*|Ada|2026-08-31|the chip\n\
             refs/remotes/origin/main|bbb222|bbb| |Bo|2026-08-30|land it\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "feat/chip");
        assert!(!rows[0].remote);
        assert!(rows[0].is_head, "`*` marks the branch HEAD is on");
        assert_eq!(rows[1].name, "origin/main");
        assert!(rows[1].remote);
        assert!(!rows[1].is_head);
    }

    /// `origin/HEAD` is a symref at the remote's default branch — a second
    /// name for a row already in the list, and a pair of branches whose tips
    /// can never differ.
    #[test]
    fn the_remotes_head_symref_is_not_offered_as_a_branch() {
        let rows = parse_branch_lines(
            "refs/remotes/origin/HEAD|bbb222|bbb| |Bo|2026-08-30|land it\n\
             refs/tags/v1|ccc333|ccc| |Cy|2026-08-29|release\n",
        );
        assert!(rows.is_empty(), "neither a symref nor a tag is a branch");
    }

    /// The whole point of the `ref` parameter: a reviewer on `main` can list
    /// the commits of a branch they are not on.
    #[test]
    fn commits_can_be_listed_for_a_branch_the_checkout_is_not_on() {
        let dir = branch_repo("cross-branch");
        commit(&dir, "base.txt");
        run_git(&dir, &["checkout", "-q", "-b", "feature"]);
        commit(&dir, "only-on-feature.txt");
        run_git(&dir, &["checkout", "-q", "main"]);

        let on_head = git_commits(&dir, None, 50).unwrap();
        assert_eq!(on_head.len(), 1, "main has only the base commit");

        let on_feature = git_commits(&dir, Some("feature"), 50).unwrap();
        assert_eq!(on_feature.len(), 2);
        assert_eq!(on_feature[0].message, "only-on-feature.txt");

        let listed = git_branches(&dir).unwrap();
        let names: Vec<&str> = listed.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"feature"));
        let head = listed.iter().find(|b| b.is_head).unwrap();
        assert_eq!(head.name, "main", "the checkout is back on main");
        let feature = listed.iter().find(|b| b.name == "feature").unwrap();
        assert_eq!(feature.subject, "only-on-feature.txt");
        assert!(!feature.tip.is_empty() && !feature.tip_short.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_listing_is_capped_at_what_was_asked_for() {
        let dir = branch_repo("window");
        for name in ["a.txt", "b.txt", "c.txt"] {
            commit(&dir, name);
        }
        assert_eq!(git_commits(&dir, None, 2).unwrap().len(), 2);
        // Zero would be a listing nobody can pick from; the clamp makes the
        // smallest answer one commit rather than none.
        assert_eq!(git_commits(&dir, None, 0).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What separates a review of a branch from a diff of two tips: the base
    /// is where they parted, not where the other one has got to since.
    #[test]
    fn two_branches_diverge_at_their_last_common_commit() {
        let dir = branch_repo("merge-base");
        commit(&dir, "base.txt");
        let fork = String::from_utf8_lossy(
            &Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&dir)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        run_git(&dir, &["checkout", "-q", "-b", "feature"]);
        commit(&dir, "on-feature.txt");
        run_git(&dir, &["checkout", "-q", "main"]);
        // Main moves on. A tip-to-tip comparison would report this commit as
        // something the feature branch deleted.
        commit(&dir, "landed-on-main.txt");

        let base = git_merge_base(&dir, "main", "feature").unwrap().unwrap();
        assert_eq!(base.hash, fork);
        assert_eq!(base.message, "base.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two roots share no commit. That is a fact about the pair, not a lookup
    /// that failed, so the picker is told `null` and falls back to the tip.
    #[test]
    fn unrelated_histories_have_no_common_commit() {
        let dir = branch_repo("unrelated");
        commit(&dir, "base.txt");
        run_git(&dir, &["checkout", "-q", "--orphan", "other"]);
        commit(&dir, "elsewhere.txt");

        assert!(git_merge_base(&dir, "main", "other").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
