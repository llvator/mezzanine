//! The context threaded through the entity-extraction walk.
//!
//! It lives apart from [`super`] so that the extractors the dispatcher calls
//! can name it without depending back on the dispatcher that calls them.

use super::super::language_parser::ParseResult;
use std::collections::HashMap;
use std::path::Path;

/// Shared context threaded through the entity-extraction walk.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    /// Class/interface name → (member → declared type), collected once per
    /// file so every method body can type `this.<field>` receivers.
    pub members: &'a HashMap<String, HashMap<String, String>>,
    pub result: &'a mut ParseResult,
}
