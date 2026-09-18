//! The context threaded through the entity-extraction walk.
//!
//! It lives in its own module rather than beside the dispatcher that builds
//! it: the dispatcher calls into `classes`, `functions` and `assignments`,
//! and each of those needs the context type, so keeping it in `mod.rs` made
//! them depend on each other in a ring. Owning it here leaves one direction —
//! dispatch and its extractors both depend on the context, and the context
//! depends on neither.

use super::super::language_parser::{ImportCondition, ParseResult};
use super::bodies::inference::Returns;
use std::path::Path;

/// Shared context threaded through the entity-extraction walk.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub result: &'a mut ParseResult,
    /// The conditional wrapper the walk is currently inside, if any. Set by
    /// `descend_conditional` and read only when recording imports (PY-024).
    pub import_condition: Option<ImportCondition>,
    /// Whether the walk is inside an `if TYPE_CHECKING:` block, whose
    /// imports never run (AN-022).
    ///
    /// Separate from `import_condition` because it is a different fact.
    /// Every `if` makes an import [`ImportCondition::Guarded`], but only
    /// this one guarantees the module is absent at runtime — a
    /// `if sys.platform == 'win32': import winreg` is just as guarded and
    /// very much executed.
    pub in_type_checking: bool,
    /// What a call to each name declared in this file evaluates to, built
    /// before the walk so a chain can land on a method declared below it
    /// (PY-032).
    pub returns: &'a Returns,
}
