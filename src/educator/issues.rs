//! Load-time diagnostics. Both content kinds report problems the same way —
//! a rule with an unknown construct-kind and a lesson with a bad `level` are
//! the same shape of complaint — so the vocabulary lives here rather than
//! inside either loader.
//!
//! Errors drop the item from the index; warnings keep it. The full list ends
//! up on [`super::corpus::Educator::issues`] and `/api/educator/diagnostics`.

use serde::Serialize;
use std::path::PathBuf;

/// A single load-time issue surfaced by a loader or a validator.
#[derive(Debug, Clone, Serialize)]
pub struct LoadIssue {
    pub path: PathBuf,
    /// Identifier of the offending item. Named for rules because that is the
    /// wire name `/api/educator/diagnostics` has always used; lessons put
    /// their own id here too.
    pub rule_id: Option<String>,
    pub severity: LoadIssueSeverity,
    pub field: Option<String>,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoadIssueSeverity {
    Error,
    Warning,
}
