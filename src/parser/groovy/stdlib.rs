//! Recognition of Groovy standard-library / common method names.
//!
//! Groovy inherits Java's stdlib plus its own collection / closure idioms
//! (`each`, `collect`, `findAll`, `inject`, …). We only filter the bare
//! names that consistently produce noise in the call graph — anything
//! ambiguous (e.g. `each` may be user-defined too) stays in.

pub(crate) fn is_stdlib_method(name: &str) -> bool {
    matches!(
        name,
        // Java Object/Collection ancestry
        "toString" | "hashCode" | "equals" | "clone" | "compareTo"
        | "valueOf" | "values" | "ordinal" | "name"
        | "get" | "set" | "add" | "remove" | "put" | "contains" | "containsKey"
        | "size" | "isEmpty" | "clear" | "iterator" | "hasNext" | "next"
        | "length" | "charAt" | "substring" | "trim" | "toLowerCase" | "toUpperCase"
        | "append" | "insert" | "delete" | "replace"
        | "println" | "print" | "printf" | "format"
        | "close" | "flush" | "read" | "write"
        | "getClass" | "notify" | "notifyAll" | "wait"
        // Groovy collection helpers — overwhelmingly hit on stdlib types,
        // not user-defined ones. Filtering them keeps `each { ... }` /
        // `collect { ... }` from drowning the graph.
        | "each" | "eachWithIndex" | "collect" | "collectEntries"
        | "findAll" | "find" | "any" | "every" | "inject" | "sort" | "unique"
        | "asImmutable" | "asList" | "asType"
    )
}
