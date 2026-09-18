//! What the declaration walk carries with it.
//!
//! Two structs, because the walk holds two different kinds of thing.
//! [`ExtractCtx`] is what the whole file shares — the source, the path,
//! the type index, the result being filled — and is threaded by mutable
//! reference. [`Scope`] is where in the declaration tree the walk
//! currently stands, and is copied down each branch, so an extractor
//! cannot change its caller's idea of which namespace it is in.
//!
//! Both live here rather than beside the dispatcher for the reason the
//! Java and Go parsers give: the dispatcher calls into the extractors, and
//! the extractors need these types, so owning them up there made the two
//! modules depend on each other in a ring.

use super::bodies::inference::TypeIndex;
use crate::models::Visibility;
use crate::parser::language_parser::ParseResult;
use std::path::Path;

/// What every extractor in the walk needs, so a recursive call carries one
/// reference instead of four parameters.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    /// What this file's own declarations say about the types of things —
    /// the hops that turn `repo_->save` into `Repository::save`.
    pub types: &'a TypeIndex,
    pub result: &'a mut ParseResult,
}

/// Where in the file's declaration tree the walk currently stands.
#[derive(Clone, Copy)]
pub(super) struct Scope<'a> {
    /// The enclosing namespace path — `app::core`, or empty at file
    /// scope. What every declaration's `qualified_name` is built from.
    pub namespace: &'a str,
    /// The entity that will own what is declared here.
    pub parent_id: Option<&'a str>,
    /// The class or struct name, when this is a class body. What a
    /// member's qualified name, a bare sibling call and a field write are
    /// qualified by.
    pub owner: Option<&'a str>,
    /// The access section a class body is currently in. C++ has no
    /// modifier per member — an `access_specifier` opens a section and
    /// everything after it inherits that access until the next one.
    pub access: Visibility,
}

impl<'a> Scope<'a> {
    /// The scope a file starts in: no namespace, no owner, and public,
    /// which is what a namespace-scope declaration is.
    pub(super) fn file() -> Self {
        Self {
            namespace: "",
            parent_id: None,
            owner: None,
            access: Visibility::Public,
        }
    }
}
