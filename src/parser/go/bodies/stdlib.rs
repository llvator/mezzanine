//! The names a bare call can have that are not calls to anything in the
//! project.
//!
//! Go's filtering problem is narrower than Java's. A Java parser has to
//! guess, from the name alone, whether `format(…)` is the JDK's or the
//! project's, and guesses conservatively over a long list of common method
//! names. Go spells the difference: anything outside the current package is
//! written `pkg.Name`, and [`super::super::packages`] already knows which
//! `pkg`s are the standard library. What is left for a name table is the
//! genuinely unqualified set — the built-in functions and the predeclared
//! type names used as conversions — which is closed, small, and cannot
//! collide with a project function, because the compiler would reject the
//! shadowing name in the same file.
//!
//! Everything else stays. A bare call in Go is a call to this package, and
//! that is exactly the edge worth drawing.

use super::super::helpers::is_predeclared_type;

/// Predeclared functions and type conversions that carry no dependency.
///
/// The type half lives in [`is_predeclared_type`] rather than here,
/// because `UsesType` extraction needs exactly that half and neither list
/// should be able to drift from the other.
pub(crate) fn is_builtin(name: &str) -> bool {
    is_predeclared_type(name)
        || matches!(
            name,
            // Built-in functions. `new` is recognised before this is
            // reached — see `calls::handle_call` — because `new(Order)`
            // builds an Order and is worth an edge.
            "append"
                | "cap"
                | "clear"
                | "close"
                | "complex"
                | "copy"
                | "delete"
                | "imag"
                | "len"
                | "make"
                | "max"
                | "min"
                | "new"
                | "panic"
                | "print"
                | "println"
                | "real"
                | "recover"
        )
}
