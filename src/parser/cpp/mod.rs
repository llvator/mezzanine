//! C++ parser — the entry point, and the phases one file runs through.
//!
//! The two halves of the walk each have a folder:
//! - [`declarations`] — what the file declares, and the dispatcher over it
//! - [`bodies`] — what a callable body yields once its declaration is placed
//!
//! What both of them read from a grammar node, and write their findings
//! into, sits here beside them:
//! - [`ctx`] — the context threaded through the walk, and the scope copied
//!   down it
//! - [`complexity`] — cyclomatic / cognitive / nesting metrics for a body
//! - [`doc_comments`] — the comment run above a declaration, and the file
//!   header
//! - [`helpers`] — declarators, parameters, specifiers, qualified names
//! - [`types`] — `UsesType` edges, emitted as the walk places each
//!   declaration
//!
//! # Why one parser serves two languages
//!
//! [`CppParser`] answers for `Language::C` as well as `Language::Cpp`, and
//! reports whichever it was built for. C++ is a superset of C for
//! everything this module reads — a C translation unit is a subset of the
//! grammar's declarations, statements and expressions — so the alternative
//! was a second parser that would differ from this one only by the arms it
//! never reaches. The cost is in the other direction and is small: a `.c`
//! file using `class` as an identifier parses as C++ and produces error
//! nodes, which is the case §A4 already covers.
//!
//! It also settles `.h`, which C and C++ share and which
//! `Language::from_extension` resolves to `Language::C`. Most C++ classes
//! are *declared* in a `.h` and only defined in a `.cpp`; a C++ parser
//! that did not claim the header would miss the class bodies entirely and
//! see a file of out-of-line definitions belonging to types it never met.
//!
//! # Three facts about C++ that shape the walk
//!
//! * **The name is inside the declarator, not beside it.** `Order*
//!   Factory::make(int)` is a `type` of `Order` and a declarator that
//!   reads outward-in, so [`helpers::declared_name`] and
//!   [`helpers::function_declarator`] stand in for the
//!   `child_by_field_name("name")` every other parser here can use.
//! * **A member function is a top-level declaration.** `double
//!   Order::total() const {…}` is a sibling of the class, usually in a
//!   different file. The receiver in the declarator binds the two, exactly
//!   as Go's does.
//! * **Declarations are order-independent and preprocessor-wrapped.** The
//!   type index is built in a pre-pass over the whole tree so a call
//!   through a field declared at the bottom of a class resolves the same
//!   as one through a field at the top, and `#ifdef` blocks are walked
//!   through rather than around.
//!
//! # What this parser does not do yet
//!
//! Each of these is a criterion from `docs/agents/parser-completeness.md`
//! that a decision was made about, rather than one nobody looked at.
//!
//! * **§A2, `is_reexport` and `is_type_only`.** Neither is expressible.
//!   A header that includes another and re-exposes its names does so by
//!   having included it, with no syntax saying whether that was the
//!   intent; and every `#include` is compiled, so nothing is erased from
//!   the build the way a TypeScript type-only import is.
//! * **§B8, promoting locals.** Not done. The metric that C++ bodies most
//!   need — how many names a reader holds at once — is already computed
//!   by [`crate::parser::working_set`], and a `WritesTo` node per local
//!   would add graph weight without adding an answer.
//! * **§C4, `UsesFn`.** Only `UsesValue` is emitted, for `SCREAMING_CASE`
//!   names read without being called — the `#define`, the `constexpr`
//!   limit, the enum constant. C++ naming does not distinguish a function
//!   named as a value from a variable read, so the criterion is met with
//!   one kind, which `RelationshipKind::is_dependency` treats identically.
//! * **§E1, `graph::module_segment`.** Deliberately not registered. That
//!   index keys a free function by its *file stem*, which is the module
//!   system in Rust and Python and is not one in C++: a name is reached
//!   through its namespace, which this parser already writes into
//!   `qualified_name`. Registering `order::total` for `order.cpp` would
//!   invent a qualifier no call site ever spells.
//!
//! Template *instantiation* is likewise out of scope: `Repository<int>` is
//! recorded as a use of `Repository`, because that is the entity a reader
//! navigates to and the only one the file declares.

pub(crate) mod bodies;
mod complexity;
mod ctx;
mod declarations;
mod doc_comments;
mod helpers;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Parser, Tree};

/// Parses C++ and C. `language` is what the parser reports back — the two
/// share a grammar, not an identity.
pub struct CppParser {
    parser: Parser,
    language: Language,
}

impl CppParser {
    /// A parser for C++ (`.cpp`, `.cc`, `.cxx`, `.hpp`, `.hxx`).
    pub fn new() -> Self {
        Self::for_language(Language::Cpp)
    }

    /// A parser reporting `language`, which must be `Cpp` or `C`.
    pub fn for_language(language: Language) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::language())
            .expect("Failed to set C++ language");
        Self { parser, language }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse C++ code"))
    }
}

impl Default for CppParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for CppParser {
    fn language(&self) -> Language {
        self.language
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::for_language(self.language);
        let tree = parser.parse_tree(content)?;

        let mut result = ParseResult::new();
        declarations::extract_file(tree.root_node(), path, content, &mut result);

        Ok(result)
    }
}
