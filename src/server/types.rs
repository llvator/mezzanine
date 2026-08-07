use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- Commit info struct for the API ---
#[derive(Clone, Serialize)]
pub(crate) struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    pub date: String,
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
    /// `nao.includeDocs` set the same thing.
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
