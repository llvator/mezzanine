//! What a callable body yields, once a declaration has been placed.
//!
//! - [`calls`] — call-site relationships and control-flow arm dispatch
//! - [`flow`] — synthetic `Branch` / `Loop` entities (TS-002)
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics (TS-003)
//! - [`inference`] — parameter, local and class-member type inference
//! - [`stdlib`] — built-in name table for call filtering
//!
//! Nothing here reaches back up to a declaration: a body is handed the node
//! it lives in and reports what it found.

pub(super) mod calls;
pub(super) mod complexity;
mod flow;
pub(super) mod inference;
pub(crate) mod stdlib;
