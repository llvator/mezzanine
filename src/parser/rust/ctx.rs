//! The context threaded through the entity-extraction walk.
//!
//! It lives in its own module rather than beside the dispatcher that builds
//! it: the dispatcher calls into `impls` and `functions`, and those two need
//! the context type, so keeping it in `mod.rs` made the three depend on each
//! other in a ring. Owning it here leaves one direction — dispatch and its
//! extractors both depend on the context, and the context depends on neither.

use super::super::language_parser::ParseResult;
use std::collections::HashMap;
use std::path::Path;

/// Shared context threaded through the entity-extraction walk so that each
/// recursive call doesn't have to plumb 4+ parameters. Only the dispatch
/// layer uses this; leaf `parse_*` helpers keep their simpler signatures.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub result: &'a mut ParseResult,
    pub impl_sources: &'a mut Vec<(String, String)>,
    /// Struct name → (field name → type), collected before the walk so an
    /// impl block can type `self.<field>` receivers (AN-010).
    pub struct_fields: &'a HashMap<String, HashMap<String, String>>,
}
