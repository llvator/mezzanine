//! Recognition of Python builtins.
//!
//! Used only to SKIP the "prefix with class name" heuristic for unqualified
//! calls inside a method. Without this, `len(x)` inside a class method would
//! be rewritten as `<ClassName>.len`, producing a bogus `ghost_external`
//! entity instead of the `ghost_stdlib` one the graph builder assigns to
//! bare `len`. Builtins are still emitted as edges; this list only affects
//! how the target id is constructed.

pub(super) fn is_bare_builtin(name: &str) -> bool {
    matches!(
        name,
        "print" | "len" | "range" | "enumerate" | "zip" | "map"
            | "filter" | "sorted" | "reversed" | "list" | "dict"
            | "set" | "tuple" | "frozenset" | "str" | "int" | "float"
            | "bool" | "bytes" | "bytearray" | "type" | "isinstance"
            | "issubclass" | "hasattr" | "getattr" | "setattr"
            | "delattr" | "id" | "hash" | "repr" | "ascii" | "bin"
            | "hex" | "oct" | "ord" | "chr" | "abs" | "round" | "min"
            | "max" | "sum" | "pow" | "divmod" | "any" | "all" | "iter"
            | "next" | "open" | "input" | "format" | "vars" | "dir"
            | "help" | "locals" | "globals" | "staticmethod"
            | "classmethod" | "property" | "super" | "object"
            | "callable" | "compile" | "eval" | "exec" | "breakpoint"
            | "exit" | "quit"
    )
}
