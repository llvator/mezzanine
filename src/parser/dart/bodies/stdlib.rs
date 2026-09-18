//! Dart core-library method names filtered out during call extraction.
//!
//! Kept deliberately narrow, for the reason spelled out in
//! [`crate::parser::rust::stdlib`]: an unfiltered core name resolves to
//! nothing and is dropped by the resolver, so a false negative here is
//! cheap, while filtering a name a project actually defines silently
//! deletes a real edge. `add`, `get` and `map` earn their place because
//! they are on every collection in the language; `build` and `dispose`
//! deliberately do not, because in a Flutter codebase those are the most
//! interesting edges in the graph.

pub(crate) fn is_core_method(name: &str) -> bool {
    matches!(
        name,
        "toString"
            | "hashCode"
            | "noSuchMethod"
            | "runtimeType"
            | "print"
            | "identical"
            | "assert"
            // Iterable / List / Map / Set surface
            | "add"
            | "addAll"
            | "remove"
            | "removeAt"
            | "removeWhere"
            | "clear"
            | "contains"
            | "containsKey"
            | "containsValue"
            | "indexOf"
            | "elementAt"
            | "map"
            | "where"
            | "firstWhere"
            | "lastWhere"
            | "singleWhere"
            | "expand"
            | "fold"
            | "reduce"
            | "forEach"
            | "any"
            | "every"
            | "join"
            | "sort"
            | "skip"
            | "take"
            | "toList"
            | "toSet"
            | "cast"
            | "putIfAbsent"
            // String surface
            | "substring"
            | "split"
            | "trim"
            | "toLowerCase"
            | "toUpperCase"
            | "startsWith"
            | "endsWith"
            | "replaceAll"
            | "replaceFirst"
            | "padLeft"
            | "padRight"
            | "codeUnitAt"
            // num / conversion
            | "toInt"
            | "toDouble"
            | "toStringAsFixed"
            | "abs"
            | "round"
            | "floor"
            | "ceil"
            | "clamp"
            // Future / Stream plumbing
            | "then"
            | "catchError"
            | "whenComplete"
            | "listen"
            | "cancel"
            | "close"
            | "complete"
            | "completeError"
    )
}
