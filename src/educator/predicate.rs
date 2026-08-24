//! Predicates — the `match:` vocabulary and its evaluation. Pure data-driven,
//! per ADR-0002: no Rust-handler escape hatch. The four primitives (`eq`,
//! `in`, `absent`, `present`) are compiled from a rule's YAML frontmatter by
//! [`MatchOp::from_yaml`] and evaluated against the attribute map the parser
//! emits per construct instance.
//!
//! The primitives live here rather than in [`super::rules`] because they are
//! the predicate language, not part of a rule's shape — a rule *holds* a
//! predicate the way it holds a body, and this module is what decides what a
//! predicate may say.

use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// All attribute values are strings on the wire — the parser is responsible
/// for normalizing booleans and identifiers to their canonical string form
/// before they reach predicate evaluation.
pub type Attrs = HashMap<String, String>;

/// A single predicate primitive applied to one attribute. Per ADR-0002, this set
/// is intentionally small — extend by adding parser-emitted attributes, not by
/// adding code-handler escape hatches.
#[derive(Debug, Clone)]
pub enum MatchOp {
    Eq(String),
    In(Vec<String>),
    Absent,
    Present,
}

impl MatchOp {
    /// Compile one `match:` entry from a rule's frontmatter. A bare string is
    /// shorthand for `eq`; anything else must be a single-key mapping naming
    /// one of the four primitives.
    pub(super) fn from_yaml(v: &serde_yaml::Value) -> Result<Self> {
        match v {
            serde_yaml::Value::String(s) => Ok(MatchOp::Eq(s.clone())),
            serde_yaml::Value::Mapping(m) => {
                let mut keys: Vec<String> = m
                    .keys()
                    .filter_map(|k| k.as_str().map(|s| s.to_string()))
                    .collect();
                if keys.len() != 1 {
                    return Err(anyhow!(
                        "match operator must be a single-key mapping, got keys: {:?}",
                        keys
                    ));
                }
                let key = keys.remove(0);
                let val = m
                    .get(serde_yaml::Value::String(key.clone()))
                    .ok_or_else(|| anyhow!("missing value for operator {}", key))?;
                match key.as_str() {
                    "eq" => val
                        .as_str()
                        .map(|s| MatchOp::Eq(s.to_string()))
                        .ok_or_else(|| anyhow!("`eq` value must be a string")),
                    "in" => val
                        .as_sequence()
                        .map(|seq| {
                            MatchOp::In(
                                seq.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect(),
                            )
                        })
                        .ok_or_else(|| anyhow!("`in` value must be a sequence")),
                    "absent" => val
                        .as_bool()
                        .filter(|b| *b)
                        .map(|_| MatchOp::Absent)
                        .ok_or_else(|| anyhow!("`absent: true` is the only accepted form")),
                    "present" => val
                        .as_bool()
                        .filter(|b| *b)
                        .map(|_| MatchOp::Present)
                        .ok_or_else(|| anyhow!("`present: true` is the only accepted form")),
                    other => Err(anyhow!(
                        "unknown predicate primitive `{}` (ADR-0002: no Rust-handler escape hatch; \
                         allowed: eq, in, absent, present)",
                        other
                    )),
                }
            }
            _ => Err(anyhow!(
                "match value must be a string (shorthand for `eq`) or a single-key mapping"
            )),
        }
    }
}

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
