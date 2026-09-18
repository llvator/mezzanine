//! What a Kotlin function body yields, once its declaration has been placed.
//!
//! - [`calls`] — call-site relationships, and the Branch / Loop entities the
//!   arms they sit in become
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics for the body
//! - [`flow`] — the shape a Branch or Loop entity is emitted in
//! - [`stdlib`] — built-in name table for call filtering
//!
//! Nothing here reaches back up to a declaration: a body is handed the node it
//! lives in and reports what it found. [`calls`] and [`complexity`] are the
//! two ways in — one walk for what the body *does*, one for how hard it is to
//! follow. [`flow`] and [`stdlib`] answer questions only the call walk asks.

pub(super) mod calls;
pub(super) mod complexity;
mod flow;
pub(crate) mod stdlib;
