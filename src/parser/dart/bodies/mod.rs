//! What a Dart callable body yields, once its declaration has been placed.
//!
//! - [`calls`] — call-site relationships and control-flow arm dispatch
//! - [`flow`] — synthetic `Branch` / `Loop` entities for those arms
//! - [`stdlib`] — core-library name table for call filtering
//!
//! Nothing here reaches back up to a declaration: a body is handed the node
//! it lives in and reports what it found. [`calls`] is the only way in —
//! the arm entities and the name table exist to answer questions only call
//! extraction asks.

pub(super) mod calls;
mod flow;
pub(crate) mod stdlib;
