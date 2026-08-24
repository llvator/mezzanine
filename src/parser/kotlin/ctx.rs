//! The context threaded through the entity-extraction walk, and what a
//! container hands back to it.
//!
//! Both live here rather than beside the dispatcher in `mod.rs`: the
//! dispatcher calls into `containers`, and `containers` needs the context
//! type, so keeping them up there made the two depend on each other in a ring.
//! Owning them here leaves one direction — dispatch and its extractors both
//! depend on this module, and this module depends on neither.

use super::super::language_parser::ParseResult;
use std::path::Path;
use tree_sitter::Node;

/// Shared context threaded through the entity-extraction walk.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub package: &'a str,
    pub result: &'a mut ParseResult,
}

/// What a container declaration leaves behind once it has registered itself:
/// the body still to be walked, and the id whatever that body declares should
/// hang off. Descending is the dispatcher's job alone, which is what keeps
/// `containers` from calling back up into it.
pub(super) enum Descent<'t> {
    /// Walk `body`, attaching what it declares to `owner` (`None` = file level).
    Into {
        owner: Option<String>,
        body: Node<'t>,
    },
    /// Nothing left to walk.
    Stop,
}
