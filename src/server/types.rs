use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Query string of `GET /api/activity` (UI-138).
///
/// `since` is optional rather than defaulted to zero by the client, because
/// the two mean different things to a reader of the request log: no `since`
/// is a first load, `since=0` is a client that has deliberately asked to
/// start over.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ActivityQuery {
    pub since: Option<u64>,
}

// --- Commit info struct for the API ---
#[derive(Clone, Serialize)]
pub(crate) struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    /// First parent, or `None` on a root commit (UI-151).
    ///
    /// The picker's `From` names the oldest commit the reader wants included,
    /// and the tree a comparison starts from is the one before it. This is
    /// that tree, named so the translation happens where the reader can see
    /// what it did rather than inside the diff.
    pub parent_hash: Option<String>,
    pub message: String,
    pub author: String,
    pub date: String,
}

/// Query string of `GET /api/commits` (UI-143).
///
/// Both fields optional, and the default is the answer the endpoint gave
/// before they existed: the fifty most recent commits reachable from `HEAD`.
/// `ref` is what makes a *second* branch's commits reachable at all — without
/// it the picker could only ever list the branch the checkout happens to be
/// on, which is the one branch a reviewer already has.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct CommitsQuery {
    /// A branch, tag, or commit to walk back from. `HEAD` when absent.
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub limit: Option<usize>,
}

/// Query string of `GET /api/merge-base` (UI-143).
///
/// Named `from`/`to` rather than `a`/`b` because that is the direction the
/// answer is *for*: the merge base is the base a comparison of `to` against
/// `from` should use, and git's own argument order is symmetric in a way the
/// question is not.
#[derive(Debug, Deserialize)]
pub(crate) struct MergeBaseQuery {
    pub from: String,
    pub to: String,
}

/// One branch, as `GET /api/branches` reports it (UI-143).
///
/// `name` is a usable ref — `main`, `feat/chip`, `origin/main` — and is what
/// crosses back on a comparison. A hash would be the safer choice for a
/// *stash* (see [`StashInfo`]), where the label outlives the thing it names;
/// here it would be the worse one, because a reviewer asking about `main` is
/// asking about wherever main is, and the log line reads as the name they
/// picked.
///
/// `remote` is derived from the ref's namespace rather than from its name: a
/// local branch may be called `feat/chip` and a remote-tracking one
/// `origin/main`, so a slash tells the two apart in neither direction.
///
/// The tip rides along so the picker can say what it would compare without a
/// request per row, and `is_head` so the branch the checkout is already on can
/// be marked rather than looked up separately.
#[derive(Clone, Serialize)]
pub(crate) struct BranchRef {
    pub name: String,
    pub remote: bool,
    pub is_head: bool,
    pub tip: String,
    pub tip_short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
}

/// One `git stash list` entry, as the picker needs it.
///
/// `base_hash` rides along rather than being left to the client, because the
/// only correct base for a stash is the commit it was taken on — its own
/// first parent. Pairing it against HEAD instead reports every commit landed
/// since the stash as something the stash removed (UI-107).
///
/// `selector` is the `stash@{N}` label and is display only. N is a position
/// in the list, and pushing a stash renumbers every entry below it, so the
/// ref that crosses the API is always `hash`.
#[derive(Clone, Serialize)]
pub(crate) struct StashInfo {
    pub hash: String,
    pub short_hash: String,
    pub selector: String,
    pub base_hash: String,
    pub base_short: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

/// One path in the index, as `GET /api/staged` reports it.
///
/// No hash of any kind. The commit that names the index is manufactured inside
/// the diff call and is unreferenced, so there is nothing here worth a client
/// holding on to — the ref it sends is the `STAGED` sentinel and the server
/// resolves it afresh (UI-111).
///
/// `status` is git's own letter — `M`, `A`, `D`, `R`, `C`, `T` — kept as given
/// rather than translated, so a letter this code has not met still arrives at
/// the reader intact.
#[derive(Clone, Serialize)]
pub(crate) struct StagedFile {
    pub status: String,
    pub path: String,
}

/// One path in the change, as `POST /api/changed-files` reports it (UI-134).
///
/// Git's answer, not the analysis's: this list is what the graph's reading of
/// a diff is checked *against*, so it deliberately carries files no analysis
/// would ever load — markdown, lockfiles, images.
///
/// `status` is git's own letter, kept as given for the reason `StagedFile`
/// keeps it: a letter this code has not met still reaches the reader intact.
///
/// `additions` / `deletions` are line counts, and are 0 for a `binary` file —
/// git spells those counts `-`, which is "cannot be counted in lines" rather
/// than "no lines moved", and `binary` is what carries that difference.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChangedFile {
    pub status: String,
    pub path: String,
    /// Where a rename or copy came from. The pane needs it to ask for the
    /// right base side: the file has two names, and `from_ref:<new path>` is
    /// a path that did not exist there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    /// Never added to the index, so `git diff` says nothing about it at all.
    /// Marked rather than merged into `A`, because it is the one row whose
    /// base side genuinely does not exist in any tree.
    pub untracked: bool,
}

/// Which file the reader opened, and against what.
///
/// The refs ride along rather than being read from server state: the list this
/// came from named a pair, and a second call that resolved the pair afresh
/// could answer about a different comparison than the row that was clicked.
#[derive(Deserialize)]
pub(crate) struct FileDiffRequest {
    pub from_ref: String,
    pub to_ref: String,
    pub path: String,
    /// The path on the base side, when a rename means it differs.
    #[serde(default)]
    pub base_path: Option<String>,
}

/// One side of a file comparison.
///
/// `binary` and `truncated` are stated rather than left for the reader to
/// infer from the text: a cut file renders as a diff that deletes its own
/// tail, and a binary one as a wall of replacement characters — both look like
/// a wrong diff rather than a limit.
#[derive(Clone, Serialize)]
pub(crate) struct FileSide {
    pub text: String,
    pub binary: bool,
    pub truncated: bool,
}

/// Both sides of one file. A side that does not exist is absent — an addition
/// has no base, a deletion has no head, and neither is an empty file.
#[derive(Serialize)]
pub(crate) struct FileDiffResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<FileSide>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<FileSide>,
    pub binary: bool,
}

// --- Diff request/response ---
#[derive(Deserialize)]
pub(crate) struct DiffRequest {
    pub from_ref: String,
    pub to_ref: String,
}

#[derive(Serialize)]
pub(crate) struct DiffResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<DiffSummaryResponse>,
}

#[derive(Serialize)]
pub(crate) struct DiffSummaryResponse {
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
    pub modified_source: usize,
    pub modified_impact: usize,
}

// --- Root path request/response ---
#[derive(Deserialize)]
pub(crate) struct RootPathRequest {
    pub path: String,
}

// --- Analysis-scope request/response ---
//
// Drives `POST /api/analysis/scope`: replaces the language filter the
// analyzer uses and re-runs analysis from scratch. `languages = None`
// (or absent) means "no filter" — analyze every supported language.
// `Some(empty)` is treated the same as `None` so the UI can clear the
// filter by sending `[]`.
#[derive(Deserialize)]
pub(crate) struct AnalysisScopeRequest {
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    /// Analyze Markdown alongside the code. Widens, so it survives the
    /// `languages` filter — the CLI's `--include-docs` and the extension's
    /// `mezz.includeDocs` set the same thing.
    ///
    /// `None` means "leave it as the server was started"; a client that
    /// never sends the field cannot turn off a `--include-docs` the operator
    /// passed on the command line.
    #[serde(default)]
    pub include_docs: Option<bool>,
    /// Where the Elevator spec lives, for a repo that keeps it somewhere the
    /// walk would miss or mixes it with `.elv` files that aren't spec — see
    /// [`crate::config::AnalysisConfig::spec_dir`].
    ///
    /// Three states on the wire, because two are not enough: absent means
    /// "leave it alone" (an older client must not clear an operator's
    /// `--spec-dir`), the empty string means "clear it — every `.elv` under
    /// the root again", and a path means that path. Relative resolves
    /// against the analyzed root.
    ///
    /// This one *may* name a directory outside the root, unlike the settings
    /// file's key. The asymmetry is deliberate and is the same one `/api/root`
    /// already lives with: this request comes from a page the operator opened
    /// on their own machine, not from a file that arrived with a clone.
    #[serde(default)]
    pub spec_dir: Option<String>,
}

/// `GET /api/analysis/scope` — what the analyzer is *currently* configured
/// to read. The panel seeds itself from this instead of assuming: without it
/// a UI that loads against a server started with `--include-docs` shows the
/// toggle off, and the first Apply turns docs off for real.
#[derive(Serialize)]
pub(crate) struct AnalysisScopeState {
    /// Canonical filter names, or `null` for "no filter".
    pub languages: Option<Vec<String>>,
    pub include_docs: bool,
    /// The spec directory as configured — by flag, by settings file, or by
    /// an earlier POST — or `null` when every `.elv` under the root counts.
    pub spec_dir: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct AnalysisScopeResponse {
    pub success: bool,
    pub languages: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationship_count: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct RootPathResponse {
    pub path: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationship_count: Option<usize>,
}

// --- Scope request/response ---
#[derive(Deserialize)]
pub(crate) struct ScopeRequest {
    pub entity_id: String,
    pub mode: String,
    #[serde(default = "default_depth")]
    pub depth: usize,
    #[serde(default)]
    pub excluded_files: Vec<String>,
    /// What `exports.refactor_prompt`'s Context section carries (SRV-016):
    /// `full` (default), `ranges`, or `hybrid`. Absent keeps today's
    /// behaviour, so existing callers are unaffected.
    #[serde(default)]
    pub prompt_context: Option<String>,
}

fn default_depth() -> usize {
    1
}

#[derive(Serialize)]
pub(crate) struct ScopeEntity {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub line: usize,
    pub end_line: usize,
    pub source_code: Option<String>,
    pub reasons: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct ScopeExports {
    pub paths: String,
    pub ranges: String,
    pub entity_context: String,
    pub full_files: String,
    /// Paste-ready instruction for a coding agent asked to reduce the
    /// selected entity's refactor pressure (SRV-010). Unlike the four
    /// exports above — which are raw context — this one carries the ask,
    /// the measured evidence for it, and the entity context inline.
    pub refactor_prompt: String,
}

#[derive(Serialize)]
pub(crate) struct ScopeResponse {
    pub entities: Vec<ScopeEntity>,
    pub reason_summary: HashMap<String, usize>,
    pub files: Vec<String>,
    pub token_count_entities: usize,
    pub token_count_files: usize,
    pub exports: ScopeExports,
}

/// Traversal rule: which relationship kind, which direction, and why.
pub(crate) struct TraversalRule {
    pub kind: crate::models::RelationshipKind,
    pub direction: petgraph::Direction,
    pub reason: &'static str,
}

/// What `HEAD` points at in the analyzed checkout — the answer to "which
/// branch is this graph?" (UI-114).
///
/// Not an error type. A root that is not a git checkout is an ordinary state
/// of this server — `mezz watch` runs against any directory — so it answers
/// `git: false` and the chip that reads this simply says nothing, rather than
/// a 500 the caller would have to translate back into "there is no branch".
#[derive(Clone, Serialize)]
pub(crate) struct BranchInfo {
    /// The branch `HEAD` is on. `None` when `HEAD` is detached, and when the
    /// root is not a checkout at all — `detached` and `git` tell those apart.
    ///
    /// Present on an unborn branch too: a repository with no commits still
    /// has a branch name, and that is the branch the next commit lands on.
    pub branch: Option<String>,
    /// `HEAD` names a commit rather than a branch. The canvas is then a
    /// checkout that belongs to no branch, which is worth saying outright:
    /// nothing the reader edits here is on their way anywhere.
    pub detached: bool,
    /// Abbreviated `HEAD` commit. `None` on an unborn branch, which has none.
    pub head_short: Option<String>,
    /// Whether git could answer about this root at all.
    pub git: bool,
}
