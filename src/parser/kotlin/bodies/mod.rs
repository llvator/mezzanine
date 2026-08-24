//! What a Kotlin function body yields, once its declaration has been placed.
//!
//! - [`calls`] — call-site relationships
//! - [`stdlib`] — built-in name table for call filtering
//!
//! Nothing here reaches back up to a declaration: a body is handed the node it
//! lives in and reports what it found. [`calls`] is the only way in — the name
//! table exists to answer "is this call worth an edge", which is a question
//! only call extraction asks.

pub(super) mod calls;
mod stdlib;
