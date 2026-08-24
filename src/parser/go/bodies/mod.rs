//! What a Go function or method body yields, once its declaration has been
//! placed.
//!
//! - [`calls`] — call-site relationships and control-flow arm dispatch
//! - [`flow`] — synthetic `Branch` / `Loop` entities for those arms
//! - [`inference`] — which type a written name carries, per file and per body
//! - [`stdlib`] — the built-in functions a call edge should not be drawn to
//!
//! Nothing here reaches back up to a declaration: a body is handed the node
//! it lives in and reports what it found. [`inference`] is the exception in
//! shape only — its file-level index is built from declarations, by the
//! declaration walk, and handed down.

pub(super) mod calls;
mod flow;
pub(super) mod inference;
mod stdlib;
