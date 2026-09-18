//! What a callable body yields, once a declaration has been placed.
//!
//! - [`calls`] — call-site relationships, and the inference wiring they need
//! - [`inference`] — parameter and local-variable type inference
//! - [`stdlib`] — built-in name table for call filtering
//!
//! Nothing here reaches back up to a declaration: a body is handed the node
//! it lives in and reports what it found. [`calls`] is the only way in — the
//! two things `declarations` used to reach past it for were a body's metrics
//! and the file's struct-field index, and neither was about what a body
//! yields. They now sit where they are read from.

pub(super) mod calls;
mod inference;
pub(crate) mod stdlib;
