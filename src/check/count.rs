//! What the two rules over entities count: declarations, and elements.
//!
//! The other three rules a project can declare are about the edges between
//! files and live in [`super::structure`]; these two never look outside the
//! file they are counting.
//!
//! Nothing new is measured here. Both rules are counts of entities mezz
//! already resolved, which is the point of putting the gate in the tool: the
//! hand-written test this feature came from counted *import statements* and
//! missed six re-export shims, because a count of what a file says is not a
//! count of what it declares (ADR 0024).
//!
//! The two counts partition the graph, so nothing is missed and nothing is
//! graded twice:
//!
//! - an entity owned by a class-like entity in the same file is one of that
//!   entity's **elements**;
//! - every other listed entity is a **declaration** of its file.
//!
//! A Rust `mod` is deliberately not class-like: it is a file's inside, so
//! what it holds keeps counting toward the file rather than becoming the
//! module's elements.

use std::collections::BTreeMap;
use std::path::Path;

use crate::graph::DependencyGraph;
use crate::mcp::is_listed;
use crate::models::{CodeEntity, EntityKind};

use super::rules::{Rule, Rules};
use super::{repo_relative, Violation};

/// Every breach of the two rules counted over entities, in no particular
/// order — the parent sorts one list over all five rules.
pub(super) fn violations(
    graph: &DependencyGraph,
    rules: &Rules,
    repo_root: &Path,
) -> Vec<Violation> {
    let mut found = Vec::new();
    if let Some(bar) = rules.bar(Rule::MaxEntitiesPerFile) {
        found.extend(per_file(graph, rules, repo_root, bar));
    }
    if let Some(bar) = rules.bar(Rule::MaxElementsPerEntity) {
        found.extend(per_entity(graph, rules, repo_root, bar));
    }
    found
}

/// `max_entities_per_file`, reported against line 1: the subject is the file
/// itself, and an author fixing it splits the file rather than one line of it.
fn per_file(graph: &DependencyGraph, rules: &Rules, repo_root: &Path, bar: u32) -> Vec<Violation> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for entity in graph.entities() {
        if !is_listed(entity) || is_element(graph, entity) {
            continue;
        }
        let file = repo_relative(repo_root, &entity.file_path);
        if rules.is_exempt(&file) {
            continue;
        }
        *counts.entry(file).or_default() += 1;
    }
    counts
        .into_iter()
        .filter(|(_, declared)| *declared > bar)
        .map(|(file, declared)| Violation {
            rule: Rule::MaxEntitiesPerFile,
            path: file,
            line: Some(1),
            subject: None,
            measured: declared,
            names: Vec::new(),
            bar,
        })
        .collect()
}

/// `max_elements_per_entity`, reported at the line the entity was declared on.
fn per_entity(
    graph: &DependencyGraph,
    rules: &Rules,
    repo_root: &Path,
    bar: u32,
) -> Vec<Violation> {
    let mut found = Vec::new();
    for entity in graph.entities() {
        if !is_listed(entity) {
            continue;
        }
        let Some(measured) = elements(graph, entity) else {
            continue;
        };
        if measured <= bar {
            continue;
        }
        let file = repo_relative(repo_root, &entity.file_path);
        if rules.is_exempt(&file) {
            continue;
        }
        found.push(Violation {
            rule: Rule::MaxElementsPerEntity,
            path: file,
            line: Some(entity.span.start.line as u32 + 1),
            subject: Some(entity.name.clone()),
            measured,
            names: Vec::new(),
            bar,
        });
    }
    found
}

/// How many elements an entity has, or `None` when the rule has nothing to
/// say about this kind — a type alias and a constant hold nothing, and
/// asking them the question would invent an answer.
///
/// Fields reach the graph two ways depending on the language: as child
/// entities (TypeScript properties, Java fields) or as a count on the parent
/// with no entity of their own (Rust, Go, Python). Both are added, minus the
/// overlap, so the same record scores the same in every language rather than
/// scoring zero in half of them.
fn elements(graph: &DependencyGraph, entity: &CodeEntity) -> Option<u32> {
    if entity.kind.is_callable() {
        // Not every parser fills `param_count` — Kotlin's leaves it unset —
        // and a rule that read a missing metric as zero would exempt a whole
        // language from the half of it that counts parameters.
        return Some(
            entity
                .metrics
                .param_count
                .unwrap_or(entity.parameters.len() as u32),
        );
    }
    if !holds_members(entity.kind) {
        return None;
    }
    let children = graph.children(&entity.id);
    let members = children.iter().filter(|c| is_member(c)).count() as u32;
    let field_entities = children.iter().filter(|c| is_field(c)).count() as u32;
    let fields = entity.metrics.field_count.unwrap_or(0);
    Some(members + fields.saturating_sub(field_entities))
}

/// The kinds whose contents are *members* rather than declarations of the
/// file: what the shape rules call fields, variants and methods.
///
/// `File` and `Module` are absent deliberately. Both are namespaces — a
/// file's inside — and what they hold is counted by `max_entities_per_file`,
/// where an author can act on it by splitting the file.
fn holds_members(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Class
            | EntityKind::AbstractClass
            | EntityKind::Dataclass
            | EntityKind::Struct
            | EntityKind::Interface
            | EntityKind::Trait
            | EntityKind::Enum
            | EntityKind::Component
            | EntityKind::Service
    )
}

/// Whether an entity is one of its parent's elements rather than one of its
/// file's declarations. Same file required: a Rust `impl` written next door
/// is a declaration of the file it was written in, which is the file whose
/// author has to read it.
fn is_element(graph: &DependencyGraph, entity: &CodeEntity) -> bool {
    graph
        .parent(&entity.id)
        .is_some_and(|parent| holds_members(parent.kind) && parent.file_path == entity.file_path)
}

/// A child that counts as one of its parent's elements.
///
/// Wider than [`is_listed`], which drops fields and variables because a
/// *listing* of a file should not be half field names. Here they are exactly
/// what rule 4 counts, and only the synthetic nodes — parameters, branches,
/// loops, imports — are dropped.
fn is_member(entity: &CodeEntity) -> bool {
    !entity.tags.contains("ghost")
        && !matches!(
            entity.kind,
            EntityKind::Parameter
                | EntityKind::Branch
                | EntityKind::Loop
                | EntityKind::Import
                | EntityKind::File
        )
}

/// A child that is a field, and so already counted by `field_count` in the
/// languages that report both.
fn is_field(entity: &CodeEntity) -> bool {
    is_member(entity) && matches!(entity.kind, EntityKind::Variable | EntityKind::Property)
}
