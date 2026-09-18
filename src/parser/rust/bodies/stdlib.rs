//! Filter list for stdlib / common-method noise.
//!
//! Kept intentionally narrow — names like `new`, `default`, `walk`, `fmt`,
//! etc. collide with real project methods and are filtered by match context
//! (qualified names) rather than here.

pub(crate) fn is_stdlib_function(name: &str) -> bool {
    matches!(
        name,
        "clone"
            | "to_string"
            | "to_owned"
            | "into"
            | "unwrap"
            | "expect"
            | "ok"
            | "err"
            | "is_some"
            | "is_none"
            | "and_then"
            | "or_else"
            | "unwrap_or"
            | "unwrap_or_else"
            | "len"
            | "is_empty"
            | "iter"
            | "into_iter"
            | "collect"
            | "filter_map"
            | "any"
            | "all"
            | "trim"
            | "trim_start"
            | "trim_end"
            | "format"
            | "println"
            | "print"
            | "eprintln"
            | "eprint"
            | "writeln"
            | "flush"
            | "as_ref"
            | "as_mut"
            | "borrow"
            | "borrow_mut"
            | "deref"
            | "deref_mut"
            | "as_str"
            | "as_bytes"
    )
}
