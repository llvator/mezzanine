//! Predicate evaluation. Pure data-driven, per ADR-0002 — no Rust-handler
//! escape hatch. The four primitives (`eq`, `in`, `absent`, `present`) are
//! evaluated against an attribute map the parser emits per construct instance.

use super::rules::MatchOp;
use std::collections::HashMap;

/// All attribute values are strings on the wire — the parser is responsible
/// for normalizing booleans and identifiers to their canonical string form
/// before they reach predicate evaluation.
pub type Attrs = HashMap<String, String>;

/// Returns true when every entry in `predicate` matches the corresponding
/// attribute. An empty predicate map matches anything (defensive — callers
/// should already have promoted predicate-less rules to the General bucket).
pub fn matches(predicate: &HashMap<String, MatchOp>, attrs: &Attrs) -> bool {
    predicate.iter().all(|(key, op)| match op {
        MatchOp::Eq(expected) => attrs.get(key).is_some_and(|v| v == expected),
        MatchOp::In(set) => attrs.get(key).is_some_and(|v| set.iter().any(|s| s == v)),
        MatchOp::Absent => !attrs.contains_key(key),
        MatchOp::Present => attrs.contains_key(key),
    })
}
