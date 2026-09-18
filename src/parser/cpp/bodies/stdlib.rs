//! Names a C++ call graph is better off without.
//!
//! Two tables, because C++ gives away standard-library membership in two
//! different ways. A call written `std::move(x)` says so in its qualifier,
//! and [`is_std_qualified`] reads it. A call written `xs.push_back(y)`
//! says nothing at all — the receiver is a `std::vector` only if some
//! declaration elsewhere said so — and for those the honest filter is a
//! table of the member names the standard containers and strings own.
//!
//! The Java parser's filter is the model, including its escape hatch:
//! a filtered name still emits an edge when the call's result is bound to
//! a local, because binding is the caller saying the value matters.

/// `std::`-rooted, or one of the cast operators, which are keywords
/// wearing a call's syntax.
pub(crate) fn is_std_qualified(target: &str) -> bool {
    target.starts_with("std::")
        || matches!(
            target,
            "static_cast"
                | "dynamic_cast"
                | "reinterpret_cast"
                | "const_cast"
                | "sizeof"
                | "alignof"
                | "typeid"
        )
}

/// Member names owned by the standard containers, strings and streams.
/// Kept to what a dependency graph actually trips over: a name that is
/// missing costs one ghost node, not a wrong edge.
pub(crate) fn is_std_member(name: &str) -> bool {
    matches!(
        name,
        "begin"
            | "end"
            | "cbegin"
            | "cend"
            | "rbegin"
            | "rend"
            | "size"
            | "length"
            | "empty"
            | "clear"
            | "resize"
            | "reserve"
            | "capacity"
            | "push_back"
            | "pop_back"
            | "push_front"
            | "pop_front"
            | "emplace"
            | "emplace_back"
            | "insert"
            | "erase"
            | "find"
            | "count"
            | "at"
            | "front"
            | "back"
            | "data"
            | "get"
            | "reset"
            | "release"
            | "swap"
            | "c_str"
            | "substr"
            | "append"
            | "str"
            | "first"
            | "second"
            | "value"
            | "has_value"
            | "value_or"
            | "close"
            | "flush"
            | "lock"
            | "unlock"
            | "to_string"
    )
}
