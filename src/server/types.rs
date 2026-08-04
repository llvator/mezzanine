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
