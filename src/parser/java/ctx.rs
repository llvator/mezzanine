//! The context threaded through the entity-extraction walk.
//!
//! It lives here rather than beside the dispatcher in `declarations/mod.rs`:
//! the dispatcher calls into `callables`, and `callables` needs the context
//! type, so keeping it up there made the two depend on each other in a ring.
//! Owning it here leaves one direction — dispatch and its extractors both
//! depend on this module, and this module depends on neither.

use crate::parser::language_parser::ParseResult;
use std::path::Path;

/// Shared context threaded through the entity-extraction walk so that each
/// recursive call doesn't have to plumb 5+ parameters. Only the dispatch
/// layer uses this; leaf `parse_*` helpers keep their simpler signatures.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub package: &'a str,
    pub result: &'a mut ParseResult,
}
