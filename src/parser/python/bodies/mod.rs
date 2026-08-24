//! What a callable body yields, once a declaration has been placed.
//!
//! - [`calls`] — call-site relationships, plus self/local write tracking
//! - [`complexity`] — cyclomatic / cognitive metrics
//! - [`flow`] — synthetic Branch / Loop / try-arm entity emission
//! - [`stdlib`] — built-in name table for call filtering
//!
//! Nothing here reaches back up to a declaration: a body is handed the node
//! it lives in and reports what it found.

pub(super) mod calls;
pub(super) mod complexity;
mod flow;
mod stdlib;
