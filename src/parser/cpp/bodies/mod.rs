//! What a C++ body yields, once its declaration has been placed.
//!
//! - [`calls`] — call sites, constructions, field writes, and the arms
//!   they fire inside
//! - [`flow`] — the synthetic `Branch` / `Loop` entities for those arms
//! - [`inference`] — what a written name's type is, as far as one file
//!   can tell
//! - [`stdlib`] — the two filters that keep `std::` out of the graph
//!
//! Nothing here reaches back up into a declaration: a body is handed the
//! node it lives in and reports what it found. [`calls`] is the only way
//! in — the other three answer questions only call extraction asks.

pub(super) mod calls;
mod flow;
pub(super) mod inference;
pub(crate) mod stdlib;
