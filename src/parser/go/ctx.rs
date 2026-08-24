//! The context threaded through the declaration walk.
//!
//! It lives here rather than beside the dispatcher for the reason the Java
//! parser's does: the dispatcher calls into the extractors, and the
//! extractors need the context type, so owning it there made the two
//! depend on each other in a ring. Here the dependency runs one way.

use super::bodies::inference::TypeIndex;
use super::packages::Imports;
use crate::parser::language_parser::ParseResult;
use std::path::Path;

/// What every extractor in the walk needs to know, so a recursive call
/// carries one reference instead of six parameters.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    /// The file's own `package x` name. Entities are qualified by it so a
    /// cross-package call written `store.Load()` finds `store.Load`.
    pub package: &'a str,
    /// What this file imported, and under which local names. Read by call
    /// extraction to tell a package qualifier from a variable.
    pub imports: &'a Imports,
    /// What this file's own declarations say about the type of a struct
    /// field or a package-level variable — the hops that turn `s.repo.Find`
    /// into `Repository.Find`.
    pub types: &'a TypeIndex,
    pub result: &'a mut ParseResult,
}
