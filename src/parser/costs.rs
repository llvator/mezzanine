//! What a library call costs, read from the name it calls (MCP-046).
//!
//! [`super::loops`] counts the loops a body writes. It cannot count the loop
//! a body *calls* — and the commonest accidental O(n²) in every language here
//! is one written loop around one linear library operation:
//!
//! ```text
//! for id in ids {          // 1 loop
//!     if seen.contains(id) // …and a scan of `seen` per iteration
//! }
//! ```
//!
//! A loop count alone reads that as O(n). This module is what recovers the
//! other half, in the same shape [`super::effects`] classifies side effects:
//! a per-language table of names, narrow enough that what is missing is
//! visible rather than wrong.
//!
//! ## Three classes, and no fourth
//!
//! | Class | Reads as |
//! |-------|----------|
//! | [`Cost::Log`] | `log n` — a binary search, an ordered-map probe. |
//! | [`Cost::Linear`] | `n` — one pass over the receiver. |
//! | [`Cost::Linearithmic`] | `n log n` — a sort. |
//!
//! There is deliberately no `Constant` class. A constant-time call changes
//! no answer, and listing one would only invite the reading that a name
//! *absent* from the table is constant. Absent means unrecognised.
//!
//! ## Certain, and receiver-dependent
//!
//! Some names cost the same whatever they are called on. `sort` is
//! `n log n`; `indexOf` walks; `Iterator::position` walks. Those are
//! [`Rule::certain`], and a report may raise its bound on them.
//!
//! Others depend on a type mezz does not have. `seen.contains(x)` is a scan
//! on a `Vec` and a hash probe on a `HashSet`, and the graph reaches this
//! module precisely when it could not bind the receiver — so the type is the
//! one thing unavailable. Those rules are marked uncertain, and a report must
//! print them as a *question* ("linear if `seen` is a sequence") rather than
//! fold them into a bound. Claiming O(n²) on every `HashSet::contains` in a
//! loop would make the tool worse than silence.
//!
//! ## What no table can see
//!
//! Operators. Python's `x in xs`, Rust's index, C++'s `operator[]` on a
//! `std::map` — none is a call, none reaches here, and each can be the
//! linear step that decides the answer. A report built on this module says
//! so rather than implying the list is complete.

use crate::models::file_info::Language;

/// How one library operation scales in the size of what it is called on.
///
/// Ordered cheapest-first, so a report picking the dominating operation can
/// just take the maximum.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Cost {
    Log,
    Linear,
    Linearithmic,
}

impl Cost {
    /// The `n` exponent this contributes: a scan is one pass, a binary
    /// search is none.
    pub(crate) fn exponent(self) -> u32 {
        match self {
            Cost::Log => 0,
            Cost::Linear | Cost::Linearithmic => 1,
        }
    }

    /// Whether it carries a `log n` factor beside its exponent.
    pub(crate) fn has_log(self) -> bool {
        matches!(self, Cost::Log | Cost::Linearithmic)
    }

    /// How the row spells it.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Cost::Log => "log n",
            Cost::Linear => "n",
            Cost::Linearithmic => "n log n",
        }
    }
}

/// One name-to-cost rule.
///
/// Keyed on the **member** rather than the owner, which is the inverse of
/// [`super::effects`] and follows from what the two ask. An effect is a
/// property of the module (`std::fs` writes files); a cost is a property of
/// the operation (`sort` sorts, on whatever it is handed). `owner` is
/// therefore a guard used only where the bare member would be too common —
/// Go's `sort.Slice`, Java's `Collections.sort`.
struct Rule {
    member: &'static str,
    owner: Option<&'static str>,
    cost: Cost,
    /// False when the cost depends on the receiver's type — see the module
    /// header. A report may name these but must not bound on them.
    certain: bool,
}

/// A name that costs the same whatever it is called on.
const fn sure(member: &'static str, cost: Cost) -> Rule {
    Rule { member, owner: None, cost, certain: true }
}

/// A name whose cost depends on a receiver type mezz does not have.
const fn iffy(member: &'static str, cost: Cost) -> Rule {
    Rule { member, owner: None, cost, certain: false }
}

/// A name claimed only under one owner, because bare it is anybody's.
const fn on(owner: &'static str, member: &'static str, cost: Cost) -> Rule {
    Rule { member, owner: Some(owner), cost, certain: true }
}

/// What one operation costs, and whether mezz is sure.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Op {
    pub(crate) cost: Cost,
    pub(crate) certain: bool,
}

/// Whether mezz carries a cost table for this language.
///
/// The question a report must ask before printing "no library operation
/// found": that sentence and "mezz cannot classify them here" are opposite
/// facts, and the first is what an empty section looks like.
pub(crate) fn has_table(language: Language) -> bool {
    !rules(language).is_empty()
}

/// The languages that have one, in reading order.
pub(crate) const COVERED: &str =
    "Rust, Python, TypeScript/JavaScript (and Svelte), Go, Java, Kotlin, Groovy, Dart, and C/C++";

/// What `owner::member` costs, or `None` when these tables do not recognise
/// it.
///
/// `owner` is the whole qualifier the call arrived with, matched at a
/// segment boundary the way [`super::effects::classify`] matches its own —
/// `std::sort` answers as `sort`.
pub(crate) fn classify(language: Language, owner: Option<&str>, member: &str) -> Option<Op> {
    rules(language)
        .iter()
        .find(|rule| rule.member == member && owner_ok(rule.owner, owner))
        .map(|rule| Op { cost: rule.cost, certain: rule.certain })
}

/// Whether the call's qualifier satisfies the rule's guard. An unguarded
/// rule takes any receiver; a guarded one takes its owner under any path.
fn owner_ok(want: Option<&str>, owner: Option<&str>) -> bool {
    let Some(want) = want else {
        return true;
    };
    let owner = owner.unwrap_or("");
    owner == want
        || owner
            .strip_suffix(want)
            .is_some_and(|head| head.ends_with(['.', ':']))
}

fn rules(language: Language) -> &'static [Rule] {
    match language {
        Language::Rust => RUST,
        Language::Python => PYTHON,
        Language::TypeScript | Language::JavaScript | Language::Svelte => JS,
        Language::Go => GO,
        Language::Java => JAVA,
        Language::Kotlin => KOTLIN,
        Language::Groovy => GROOVY,
        Language::Dart => DART,
        Language::Cpp | Language::C => CPP,
        _ => &[],
    }
}

/// Rust's linear operations are almost all `Iterator` methods, and those are
/// unambiguous: nothing else in the language spells `position` or `any`.
/// `contains`, `insert` and `remove` are the ambiguous three — a scan on
/// `Vec`, a probe on `HashSet` — and a receiver mezz could not bind is
/// exactly the case where the difference is invisible.
///
/// Lazy adapters (`map`, `filter`, `rev`) are absent on purpose: they cost
/// nothing until consumed, and the consumer is in the table.
const RUST: &[Rule] = &[
    sure("sort", Cost::Linearithmic),
    sure("sort_by", Cost::Linearithmic),
    sure("sort_by_key", Cost::Linearithmic),
    sure("sort_by_cached_key", Cost::Linearithmic),
    sure("sort_unstable", Cost::Linearithmic),
    sure("sort_unstable_by", Cost::Linearithmic),
    sure("sort_unstable_by_key", Cost::Linearithmic),
    sure("binary_search", Cost::Log),
    sure("binary_search_by", Cost::Log),
    sure("binary_search_by_key", Cost::Log),
    sure("position", Cost::Linear),
    sure("rposition", Cost::Linear),
    sure("any", Cost::Linear),
    sure("all", Cost::Linear),
    sure("count", Cost::Linear),
    sure("fold", Cost::Linear),
    sure("for_each", Cost::Linear),
    sure("find_map", Cost::Linear),
    sure("filter_map", Cost::Linear),
    sure("max_by_key", Cost::Linear),
    sure("min_by_key", Cost::Linear),
    sure("max_by", Cost::Linear),
    sure("min_by", Cost::Linear),
    sure("collect", Cost::Linear),
    sure("partition", Cost::Linear),
    sure("dedup", Cost::Linear),
    sure("dedup_by", Cost::Linear),
    sure("dedup_by_key", Cost::Linear),
    sure("retain", Cost::Linear),
    sure("to_vec", Cost::Linear),
    sure("extend", Cost::Linear),
    sure("concat", Cost::Linear),
    sure("repeat", Cost::Linear),
    // `find` is `Iterator::find` and `str::find`; both walk.
    sure("find", Cost::Linear),
    // A scan on a sequence or a `&str`, a hash probe on a set or map.
    iffy("contains", Cost::Linear),
    iffy("contains_key", Cost::Linear),
    // `Vec::insert`/`remove` shift the tail; the map versions do not.
    iffy("insert", Cost::Linear),
    iffy("remove", Cost::Linear),
];

/// Python's costly operations are mostly builtins taking an iterable.
/// `max`/`min` are left out: `max(a, b)` is as common as `max(xs)` and mezz
/// cannot see which it has. `in` is an operator and never reaches here.
const PYTHON: &[Rule] = &[
    sure("sorted", Cost::Linearithmic),
    sure("sort", Cost::Linearithmic),
    sure("sum", Cost::Linear),
    sure("any", Cost::Linear),
    sure("all", Cost::Linear),
    sure("join", Cost::Linear),
    sure("index", Cost::Linear),
    sure("count", Cost::Linear),
    sure("reverse", Cost::Linear),
    sure("extend", Cost::Linear),
    sure("startswith", Cost::Linear),
    sure("endswith", Cost::Linear),
    sure("split", Cost::Linear),
    sure("replace", Cost::Linear),
    sure("bisect", Cost::Log),
    sure("bisect_left", Cost::Log),
    sure("bisect_right", Cost::Log),
    // `list.remove` walks; `set.remove` and `dict.pop` do not.
    iffy("remove", Cost::Linear),
    iffy("pop", Cost::Linear),
    iffy("copy", Cost::Linear),
];

/// The cleanest table of the nine. JavaScript keeps its set membership on
/// `Set.has` and its array membership on `includes`, so nothing here is
/// ambiguous — `includes`, `indexOf` and the `Array.prototype` iteration
/// methods all walk, whatever they are called on.
const JS: &[Rule] = &[
    sure("sort", Cost::Linearithmic),
    sure("toSorted", Cost::Linearithmic),
    sure("indexOf", Cost::Linear),
    sure("lastIndexOf", Cost::Linear),
    sure("includes", Cost::Linear),
    sure("find", Cost::Linear),
    sure("findIndex", Cost::Linear),
    sure("findLast", Cost::Linear),
    sure("filter", Cost::Linear),
    sure("map", Cost::Linear),
    sure("forEach", Cost::Linear),
    sure("reduce", Cost::Linear),
    sure("reduceRight", Cost::Linear),
    sure("some", Cost::Linear),
    sure("every", Cost::Linear),
    sure("flat", Cost::Linear),
    sure("flatMap", Cost::Linear),
    sure("join", Cost::Linear),
    sure("concat", Cost::Linear),
    sure("reverse", Cost::Linear),
    sure("splice", Cost::Linear),
    sure("unshift", Cost::Linear),
    sure("search", Cost::Linear),
    sure("replaceAll", Cost::Linear),
    on("Object", "keys", Cost::Linear),
    on("Object", "values", Cost::Linear),
    on("Object", "entries", Cost::Linear),
    on("Object", "assign", Cost::Linear),
    on("Array", "from", Cost::Linear),
];

/// Go writes its costly operations as package calls, so the package is the
/// guard and almost nothing here is bare. `append` is absent: amortised
/// constant is not a cost worth a row.
const GO: &[Rule] = &[
    on("sort", "Sort", Cost::Linearithmic),
    on("sort", "Stable", Cost::Linearithmic),
    on("sort", "Slice", Cost::Linearithmic),
    on("sort", "SliceStable", Cost::Linearithmic),
    on("sort", "Strings", Cost::Linearithmic),
    on("sort", "Ints", Cost::Linearithmic),
    on("sort", "Float64s", Cost::Linearithmic),
    on("slices", "Sort", Cost::Linearithmic),
    on("slices", "SortFunc", Cost::Linearithmic),
    on("sort", "Search", Cost::Log),
    on("sort", "SearchInts", Cost::Log),
    on("slices", "BinarySearch", Cost::Log),
    on("slices", "Contains", Cost::Linear),
    on("slices", "Index", Cost::Linear),
    on("slices", "IndexFunc", Cost::Linear),
    on("strings", "Contains", Cost::Linear),
    on("strings", "Index", Cost::Linear),
    on("strings", "Split", Cost::Linear),
    on("strings", "Join", Cost::Linear),
    on("strings", "Replace", Cost::Linear),
    on("strings", "ReplaceAll", Cost::Linear),
    on("strings", "Fields", Cost::Linear),
    on("strings", "Repeat", Cost::Linear),
    on("bytes", "Contains", Cost::Linear),
    on("bytes", "Index", Cost::Linear),
    on("bytes", "Split", Cost::Linear),
    sure("copy", Cost::Linear),
];

/// The JDK's collections and streams. `contains` is the ambiguous one —
/// `List` walks, `HashSet` probes — and `indexOf` is not, because only the
/// walking types have it.
const JAVA: &[Rule] = &[
    on("Collections", "sort", Cost::Linearithmic),
    on("Arrays", "sort", Cost::Linearithmic),
    sure("sort", Cost::Linearithmic),
    sure("sorted", Cost::Linearithmic),
    on("Collections", "binarySearch", Cost::Log),
    on("Arrays", "binarySearch", Cost::Log),
    sure("indexOf", Cost::Linear),
    sure("lastIndexOf", Cost::Linear),
    sure("stream", Cost::Linear),
    sure("forEach", Cost::Linear),
    sure("anyMatch", Cost::Linear),
    sure("allMatch", Cost::Linear),
    sure("noneMatch", Cost::Linear),
    sure("findFirst", Cost::Linear),
    sure("collect", Cost::Linear),
    sure("reduce", Cost::Linear),
    sure("addAll", Cost::Linear),
    sure("removeAll", Cost::Linear),
    sure("retainAll", Cost::Linear),
    sure("containsAll", Cost::Linear),
    sure("copyOf", Cost::Linear),
    sure("join", Cost::Linear),
    sure("split", Cost::Linear),
    sure("replaceAll", Cost::Linear),
    iffy("contains", Cost::Linear),
    iffy("remove", Cost::Linear),
];

/// Kotlin's standard library names its eager collection operations, so most
/// of this is unambiguous. `contains` stays ambiguous for the same reason it
/// does in Java, and `in` desugars to it without ever looking like a call.
const KOTLIN: &[Rule] = &[
    sure("sorted", Cost::Linearithmic),
    sure("sortedBy", Cost::Linearithmic),
    sure("sortedWith", Cost::Linearithmic),
    sure("sortedByDescending", Cost::Linearithmic),
    sure("sortedDescending", Cost::Linearithmic),
    sure("sortBy", Cost::Linearithmic),
    sure("sort", Cost::Linearithmic),
    sure("binarySearch", Cost::Log),
    sure("filter", Cost::Linear),
    sure("filterNot", Cost::Linear),
    sure("map", Cost::Linear),
    sure("mapNotNull", Cost::Linear),
    sure("flatMap", Cost::Linear),
    sure("forEach", Cost::Linear),
    sure("any", Cost::Linear),
    sure("all", Cost::Linear),
    sure("none", Cost::Linear),
    sure("count", Cost::Linear),
    sure("first", Cost::Linear),
    sure("firstOrNull", Cost::Linear),
    sure("find", Cost::Linear),
    sure("indexOf", Cost::Linear),
    sure("sumOf", Cost::Linear),
    sure("maxByOrNull", Cost::Linear),
    sure("minByOrNull", Cost::Linear),
    sure("groupBy", Cost::Linear),
    sure("associateBy", Cost::Linear),
    sure("distinct", Cost::Linear),
    sure("joinToString", Cost::Linear),
    sure("toList", Cost::Linear),
    sure("toSet", Cost::Linear),
    sure("toMap", Cost::Linear),
    sure("reversed", Cost::Linear),
    iffy("contains", Cost::Linear),
    iffy("remove", Cost::Linear),
];

/// Groovy on the JDK, plus the closure methods it adds to every collection.
const GROOVY: &[Rule] = &[
    sure("sort", Cost::Linearithmic),
    sure("toSorted", Cost::Linearithmic),
    sure("each", Cost::Linear),
    sure("eachWithIndex", Cost::Linear),
    sure("collect", Cost::Linear),
    sure("collectMany", Cost::Linear),
    sure("findAll", Cost::Linear),
    sure("findResult", Cost::Linear),
    sure("any", Cost::Linear),
    sure("every", Cost::Linear),
    sure("inject", Cost::Linear),
    sure("groupBy", Cost::Linear),
    sure("indexOf", Cost::Linear),
    sure("join", Cost::Linear),
    sure("unique", Cost::Linear),
    sure("reverse", Cost::Linear),
    sure("split", Cost::Linear),
    iffy("contains", Cost::Linear),
    iffy("find", Cost::Linear),
    iffy("remove", Cost::Linear),
];

/// Dart's `Iterable` methods are lazy, but their consumers are here and the
/// eager `List` methods are unambiguous. `contains` on a `Set` is the one
/// probe hiding among scans.
const DART: &[Rule] = &[
    sure("sort", Cost::Linearithmic),
    sure("indexOf", Cost::Linear),
    sure("lastIndexOf", Cost::Linear),
    sure("where", Cost::Linear),
    sure("map", Cost::Linear),
    sure("forEach", Cost::Linear),
    sure("firstWhere", Cost::Linear),
    sure("lastWhere", Cost::Linear),
    sure("singleWhere", Cost::Linear),
    sure("indexWhere", Cost::Linear),
    sure("any", Cost::Linear),
    sure("every", Cost::Linear),
    sure("reduce", Cost::Linear),
    sure("fold", Cost::Linear),
    sure("expand", Cost::Linear),
    sure("toList", Cost::Linear),
    sure("toSet", Cost::Linear),
    sure("join", Cost::Linear),
    sure("removeWhere", Cost::Linear),
    sure("addAll", Cost::Linear),
    iffy("contains", Cost::Linear),
    iffy("remove", Cost::Linear),
];

/// `<algorithm>` is the table. Its names are unqualified as often as not
/// (ADL, or a `using namespace std`), so the `std` owner is not required.
/// `find` and `count` are the ambiguous pair: the free algorithms walk, and
/// the `std::map` / `std::unordered_map` members of the same name do not.
const CPP: &[Rule] = &[
    sure("sort", Cost::Linearithmic),
    sure("stable_sort", Cost::Linearithmic),
    sure("partial_sort", Cost::Linearithmic),
    sure("sort_heap", Cost::Linearithmic),
    sure("lower_bound", Cost::Log),
    sure("upper_bound", Cost::Log),
    sure("binary_search", Cost::Log),
    sure("equal_range", Cost::Log),
    sure("find_if", Cost::Linear),
    sure("find_if_not", Cost::Linear),
    sure("count_if", Cost::Linear),
    sure("accumulate", Cost::Linear),
    sure("transform", Cost::Linear),
    sure("for_each", Cost::Linear),
    sure("copy", Cost::Linear),
    sure("remove_if", Cost::Linear),
    sure("reverse", Cost::Linear),
    sure("fill", Cost::Linear),
    sure("equal", Cost::Linear),
    sure("search", Cost::Linear),
    sure("max_element", Cost::Linear),
    sure("min_element", Cost::Linear),
    sure("all_of", Cost::Linear),
    sure("any_of", Cost::Linear),
    sure("none_of", Cost::Linear),
    sure("strlen", Cost::Linear),
    sure("strcmp", Cost::Linear),
    sure("memcpy", Cost::Linear),
    iffy("find", Cost::Linear),
    iffy("count", Cost::Linear),
    iffy("erase", Cost::Linear),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The case the module exists for: one linear library call, recognised
    /// by name, in a language whose table has no ambiguity to hedge.
    #[test]
    fn a_linear_scan_is_recognised_from_its_name() {
        assert_eq!(
            classify(Language::TypeScript, Some("ids"), "indexOf"),
            Some(Op { cost: Cost::Linear, certain: true })
        );
        assert_eq!(
            classify(Language::Rust, Some("xs"), "position"),
            Some(Op { cost: Cost::Linear, certain: true })
        );
    }

    /// A sort costs `n log n` in every one of the nine, and each spells it
    /// its own way.
    #[test]
    fn every_language_recognises_its_sort() {
        let cases = [
            (Language::Rust, None, "sort_by_key"),
            (Language::Python, None, "sorted"),
            (Language::TypeScript, None, "sort"),
            (Language::Go, Some("sort"), "Slice"),
            (Language::Java, Some("Collections"), "sort"),
            (Language::Kotlin, None, "sortedBy"),
            (Language::Groovy, None, "toSorted"),
            (Language::Dart, None, "sort"),
            (Language::Cpp, Some("std"), "stable_sort"),
        ];
        for (language, owner, member) in cases {
            assert_eq!(
                classify(language, owner, member).map(|op| op.cost),
                Some(Cost::Linearithmic),
                "{language:?} did not recognise `{member}`"
            );
        }
    }

    /// The honesty the module turns on: a name whose cost is the receiver's
    /// type is marked uncertain, so a report can ask rather than claim. A
    /// `HashSet::contains` in a loop is not O(n²).
    #[test]
    fn a_receiver_dependent_name_is_marked_rather_than_claimed() {
        let contains = classify(Language::Rust, Some("seen"), "contains").expect("in the table");
        assert_eq!(contains.cost, Cost::Linear);
        assert!(!contains.certain, "a bare `contains` cannot be claimed linear");

        // …and the unambiguous neighbour still is. `includes` exists on
        // arrays and strings; a JS `Set` spells membership `has`.
        let includes = classify(Language::TypeScript, Some("ids"), "includes").expect("in table");
        assert!(includes.certain);
    }

    /// An owner guard claims its own name, under a qualifier or bare, and
    /// nothing outside it — Go's `sort.Slice` is a sort, somebody's
    /// `store.Slice` is not.
    #[test]
    fn an_owner_guard_claims_its_package_and_no_other() {
        assert!(classify(Language::Go, Some("sort"), "Slice").is_some());
        assert!(classify(Language::Cpp, Some("std"), "lower_bound").is_some());
        assert_eq!(classify(Language::Go, Some("store"), "Slice"), None);
        assert_eq!(classify(Language::Go, Some("resort"), "Slice"), None);
        assert_eq!(classify(Language::Go, None, "Slice"), None);
    }

    /// Absence is unrecognised, not constant. A lazy adapter and an
    /// amortised push are both deliberately missing.
    #[test]
    fn an_unrecognised_name_is_not_claimed() {
        assert_eq!(classify(Language::Rust, Some("xs"), "len"), None);
        assert_eq!(classify(Language::Rust, Some("xs"), "push"), None);
        assert_eq!(classify(Language::Go, None, "append"), None);
        // `max(a, b)` is as common as `max(xs)`, so Python claims neither.
        assert_eq!(classify(Language::Python, None, "max"), None);
    }

    /// Silence has to be distinguishable from a finding of nothing.
    #[test]
    fn a_language_without_a_table_is_reported_as_one() {
        assert!(has_table(Language::Rust));
        assert!(has_table(Language::Dart));
        assert!(!has_table(Language::Ruby));
        assert_eq!(classify(Language::Ruby, None, "sort"), None);
    }
}
