//! Extract a named subset of an Elevator spec back into `.elv`
//! source.
//!
//! Every other Elevator artifact ([`elevator_text_renderer`],
//! [`elevator_list`], …) renders *away* from the language — the
//! output is for reading, not re-parsing. This module is the one
//! that renders *back into* it: given a set of selectors it emits a
//! standalone `.elv` file containing only the named entities, their
//! descendants, and the context needed to make that slice parse and
//! pass `--check` on its own.
//!
//! The use case is agent observability. An agent that finished a
//! feature has already deepened the shared spec; extracting the
//! branch it touched into its own working folder produces a small,
//! diffable snapshot of *what it claimed to have worked on* — a
//! record that survives after the shared spec has moved on.
//!
//! ## What lands in a slice
//!
//! - **Members** — the selected entities plus every `Contains`
//!   descendant. Emitted in full: `d:`, `cr:`, link fields, children.
//! - **Concepts** used by a member (`used_by:` edge). Emitted in
//!   full, with `used_by:` pruned to the members that pulled them in.
//! - **Ancestors** of a member. Emitted with `d:` / `cr:` and a
//!   child list pruned to the slice — enough to show *where* the
//!   slice sits, not a claim about their own full contents. Marked
//!   with a trailing comment so a reader can't mistake one for a
//!   complete definition.
//! - **Name-only** entries — the far end of a `references:` /
//!   `where:` edge that leaves the slice. Emitted bodyless so the
//!   edge survives and still resolves; the definition stays in the
//!   source spec.
//!
//! Inbound edges from non-members are dropped: a slice records what
//! the agent touched, not everything that points at it. Containment
//! ancestors are the deliberate exception, because a Feature with no
//! Category above it has lost the only thing that says where it
//! lives.
//!
//! Edges whose target is an unresolved stub are dropped entirely and
//! counted in the header. Emitting a name-only definition for one
//! would *repair* it in the slice — the extract would look healthy
//! while the source spec still has the dangling reference.

use crate::analyzer::AnalysisResult;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;

use super::elevator_text_renderer::{resolve_root, suggest_close_matches};

/// A rendered slice plus the counts the CLI reports on stderr.
#[derive(Debug)]
pub struct Slice {
    /// The `.elv` source. Ends with a newline.
    pub text: String,
    /// Entities emitted as full definitions because they were
    /// selected, descend from a selection, or are a Concept the
    /// slice uses.
    pub members: usize,
    /// Ancestors emitted as pruned context.
    pub ancestors: usize,
    /// Out-of-slice edge targets emitted bodyless.
    pub name_only: usize,
    /// Edges dropped because their target has no definition anywhere
    /// in the source spec.
    pub dropped_unresolved: usize,
}

/// Extract the entities named by `selectors` (and their closure)
/// into standalone `.elv` source.
///
/// `selectors` accept the same forms as `--root` / `--focus`: a bare
/// name (`spec_health`), a kind-prefixed qualified name
/// (`fu.spec_health.drift`), or a full ID
/// (`elevator::f.spec_health`).
///
/// `source_label` is recorded in the header — the path the spec was
/// read from, so a snapshot says where it came from.
///
/// Returns `Err` with a message (including did-you-mean suggestions)
/// when a selector matches nothing, so the caller can exit non-zero
/// rather than silently write an empty slice.
pub fn extract(
    result: &AnalysisResult,
    selectors: &[String],
    source_label: &str,
) -> Result<Slice, String> {
    let entities: Vec<&CodeEntity> = elevator_entities(result);
    if entities.is_empty() {
        return Err("no Elevator entities found — is this a spec directory?".to_string());
    }
    let by_id: HashMap<String, &CodeEntity> =
        entities.iter().map(|e| (e.id.clone(), *e)).collect();

    let seeds = resolve_selectors(selectors, &entities, &by_id)?;
    Ok(extract_seeds(result, &seeds, &selectors.join(", "), source_label))
}

/// The Elevator entities of an analysis, in analysis order.
pub fn elevator_entities(result: &AnalysisResult) -> Vec<&CodeEntity> {
    result
        .entities
        .iter()
        .filter(|e| e.tags.contains("elevator"))
        .collect()
}

/// Extract the closure of already-resolved entity `seeds`.
///
/// [`extract`] is this with name resolution in front of it; callers
/// that select entities some other way (by the code path their `cr:`
/// claims, say) come in here and pass their own `selection_label` for
/// the slice header — the one line that records *why* these entities.
///
/// Unlike [`extract`] this cannot fail: the seeds are ids the caller
/// already found in the graph. Seeds that name nothing are ignored,
/// and an empty seed list yields a header-only slice.
pub fn extract_seeds(
    result: &AnalysisResult,
    seeds: &[String],
    selection_label: &str,
    source_label: &str,
) -> Slice {
    let entities: Vec<&CodeEntity> = elevator_entities(result);
    let by_id: HashMap<String, &CodeEntity> =
        entities.iter().map(|e| (e.id.clone(), *e)).collect();
    let seeds: Vec<String> = seeds.iter().filter(|id| by_id.contains_key(*id)).cloned().collect();
    let graph = Containment::build(result, &by_id);

    // Members = seeds + everything below them. A Feature's
    // Functionalities are part of the Feature's scope; selecting the
    // Feature without them would produce a slice that says less than
    // the source did about the branch the agent touched.
    let mut members: BTreeSet<String> = BTreeSet::new();
    for seed in &seeds {
        graph.collect_down(seed, &mut members);
    }

    // Concepts the members use. Pulled in whole — a Concept is the
    // cross-cutting logic a reader needs to make sense of the
    // Features that reference it.
    for concept in links_from(result, &members, "used_by", &by_id, EntityKind::Concept) {
        members.insert(concept);
    }

    // Ancestors, minus anything already a member (a selection can
    // sit above another selection).
    let mut ancestors: BTreeSet<String> = BTreeSet::new();
    for m in &members {
        graph.collect_up(m, &mut ancestors);
    }
    ancestors.retain(|id| !members.contains(id));

    // Out-of-slice link targets. Only members carry their link
    // fields into the slice — ancestors are positional context, and
    // dragging their `references:` targets in would inflate the
    // slice with entities the agent never touched.
    let emitted: HashSet<String> = members.iter().chain(ancestors.iter()).cloned().collect();
    let mut name_only: BTreeSet<String> = BTreeSet::new();
    for link in ["references", "where"] {
        for target in links_from_any(result, &members, link, &by_id) {
            if !emitted.contains(&target) {
                name_only.insert(target);
            }
        }
    }
    // A dangling reference in the source stays dangling there; the
    // slice neither repairs nor reproduces it, it just reports it.
    let dropped_unresolved = name_only
        .iter()
        .filter(|id| by_id[*id].tags.contains("unresolved"))
        .count();
    name_only.retain(|id| !by_id[id].tags.contains("unresolved"));

    let text = Rendering {
        result,
        by_id: &by_id,
        graph: &graph,
        members: &members,
        ancestors: &ancestors,
        name_only: &name_only,
        emitted,
    }
    .render(selection_label, source_label, dropped_unresolved);

    Slice {
        text,
        members: members.len(),
        ancestors: ancestors.len(),
        name_only: name_only.len(),
        dropped_unresolved,
    }
}

// =====================================================================
// Selection
// =====================================================================

fn resolve_selectors(
    selectors: &[String],
    entities: &[&CodeEntity],
    by_id: &HashMap<String, &CodeEntity>,
) -> Result<Vec<String>, String> {
    if selectors.is_empty() {
        return Err("--extract needs at least one entity to extract".to_string());
    }
    let mut out = Vec::new();
    for sel in selectors {
        match resolve_root(sel, entities, by_id) {
            Some(id) => out.push(id),
            None => {
                return Err(format!(
                    "no Elevator entity matches `{}`.{}",
                    sel,
                    suggest_close_matches(sel, entities)
                ))
            }
        }
    }
    Ok(out)
}

/// `Contains` adjacency over Elevator entities, both directions.
///
/// Built from relationships rather than `parent_id` because a
/// containment declared across a missing import never gets a
/// `parent_id` (the analyzer withholds it on purpose), and the slice
/// should still know where that child was meant to sit.
struct Containment {
    children: HashMap<String, Vec<String>>,
    parents: HashMap<String, Vec<String>>,
}

impl Containment {
    fn build(result: &AnalysisResult, by_id: &HashMap<String, &CodeEntity>) -> Self {
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        for rel in &result.relationships {
            if rel.kind != RelationshipKind::Contains {
                continue;
            }
            if !by_id.contains_key(&rel.source_id) || !by_id.contains_key(&rel.target_id) {
                continue;
            }
            let kids = children.entry(rel.source_id.clone()).or_default();
            if !kids.contains(&rel.target_id) {
                kids.push(rel.target_id.clone());
            }
            let ps = parents.entry(rel.target_id.clone()).or_default();
            if !ps.contains(&rel.source_id) {
                ps.push(rel.source_id.clone());
            }
        }
        Self { children, parents }
    }

    fn children_of(&self, id: &str) -> &[String] {
        self.children.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    fn collect_down(&self, id: &str, acc: &mut BTreeSet<String>) {
        if !acc.insert(id.to_string()) {
            return;
        }
        for child in self.children_of(id) {
            self.collect_down(child, acc);
        }
    }

    fn collect_up(&self, id: &str, acc: &mut BTreeSet<String>) {
        for parent in self.parents.get(id).map(|v| v.as_slice()).unwrap_or(&[]) {
            if acc.insert(parent.clone()) {
                self.collect_up(parent, acc);
            }
        }
    }
}

/// IDs targeted by a `link:<kind>` edge out of any of `sources`,
/// filtered to one entity kind.
fn links_from(
    result: &AnalysisResult,
    sources: &BTreeSet<String>,
    link: &str,
    by_id: &HashMap<String, &CodeEntity>,
    kind: EntityKind,
) -> Vec<String> {
    links_from_any(result, sources, link, by_id)
        .into_iter()
        .filter(|id| by_id[id].kind == kind)
        .collect()
}

fn links_from_any(
    result: &AnalysisResult,
    sources: &BTreeSet<String>,
    link: &str,
    by_id: &HashMap<String, &CodeEntity>,
) -> Vec<String> {
    let mut out = Vec::new();
    for rel in &result.relationships {
        if rel.kind != RelationshipKind::References
            || rel.metadata.get("link").map(String::as_str) != Some(link)
            || !sources.contains(&rel.source_id)
            || !by_id.contains_key(&rel.target_id)
        {
            continue;
        }
        if !out.contains(&rel.target_id) {
            out.push(rel.target_id.clone());
        }
    }
    out
}

// =====================================================================
// Emission
// =====================================================================

/// Kinds in the order a hand-written spec introduces them: the
/// widest grouping first, leaves last. Also the order `--list` uses,
/// so the two artifacts read the same way.
const KIND_ORDER: [EntityKind; 6] = [
    EntityKind::Extension,
    EntityKind::Category,
    EntityKind::Feature,
    EntityKind::Functionality,
    EntityKind::Concept,
    EntityKind::UiPage,
];

/// The fully-resolved slice, ready to emit. Bundled rather than
/// threaded through argument lists because every emission step needs
/// most of it — the closure sets, the entity lookup, and the
/// containment graph are one value with one lifetime.
struct Rendering<'a> {
    result: &'a AnalysisResult,
    by_id: &'a HashMap<String, &'a CodeEntity>,
    graph: &'a Containment,
    members: &'a BTreeSet<String>,
    ancestors: &'a BTreeSet<String>,
    name_only: &'a BTreeSet<String>,
    /// `members ∪ ancestors` — everything that gets a body. Precomputed
    /// because child-list pruning consults it once per child reference.
    emitted: HashSet<String>,
}

impl Rendering<'_> {
    fn render(
        &self,
        selection_label: &str,
        source_label: &str,
        dropped_unresolved: usize,
    ) -> String {
        let mut out = String::new();
        self.write_header(&mut out, selection_label, source_label, dropped_unresolved);

        for kind in KIND_ORDER {
            let ids = self.members.iter().chain(self.ancestors.iter());
            for id in sorted_of_kind(self.by_id, ids, kind) {
                self.write_definition(&mut out, self.by_id[&id], self.ancestors.contains(&id));
            }
        }

        if !self.name_only.is_empty() {
            let _ = writeln!(
                out,
                "# Referenced from this slice, defined outside it. Names only —"
            );
            let _ = writeln!(out, "# the definitions stay in the source spec.");
            for kind in KIND_ORDER {
                for id in sorted_of_kind(self.by_id, self.name_only.iter(), kind) {
                    let e = self.by_id[&id];
                    let _ = writeln!(out, "{} {}", keyword(e.kind), def_name(e));
                }
            }
            let _ = writeln!(out);
        }

        out
    }

    fn write_header(
        &self,
        out: &mut String,
        selection_label: &str,
        source_label: &str,
        dropped_unresolved: usize,
    ) {
        let _ = writeln!(out, "# Elevator slice — a generated subset of a spec.");
        let _ = writeln!(out, "#");
        let _ = writeln!(out, "# Source:    {}", source_label);
        let _ = writeln!(out, "# Selection: {}", selection_label);
        let _ = writeln!(out, "# Slice:     {}", kind_summary(self.by_id, self.members));
        let context_note = !self.ancestors.is_empty();
        if context_note {
            let _ = writeln!(
                out,
                "# Context:   {} ancestor(s), child lists pruned to this slice",
                self.ancestors.len()
            );
        }
        if !self.name_only.is_empty() {
            let _ = writeln!(
                out,
                "# Outbound:  {} name-only reference target(s)",
                self.name_only.len()
            );
        }
        if dropped_unresolved > 0 {
            let _ = writeln!(
                out,
                "# Dropped:   {} edge(s) to entities the source spec never defines",
                dropped_unresolved
            );
        }
        if context_note {
            let _ = writeln!(out, "#");
            let _ = writeln!(
                out,
                "# Entities trailed by `context` are ancestors: they place the slice in"
            );
            let _ = writeln!(
                out,
                "# the hierarchy and do not describe their own full contents."
            );
        }
        let _ = writeln!(out);
    }

    fn write_definition(&self, out: &mut String, e: &CodeEntity, is_ancestor: bool) {
        let marker = if is_ancestor { "    # context" } else { "" };
        let _ = writeln!(out, "{} {} {{{}", keyword(e.kind), def_name(e), marker);

        if let Some(d) = &e.documentation {
            let _ = writeln!(out, "    d: \"{}\"", escape(d));
        }
        for attr in &e.attributes {
            if let Some((key, path)) = split_code_ref(attr) {
                let _ = writeln!(out, "    {}: \"{}\"", key, escape(path));
            }
        }

        // Ancestors carry structure only. Their own outbound links
        // point at parts of the spec the slice makes no claim about.
        if !is_ancestor {
            let mut own = BTreeSet::new();
            own.insert(e.id.clone());
            for field in ["where", "references"] {
                let targets = links_from_any(self.result, &own, field, self.by_id);
                self.write_ref_list(out, field, targets);
            }
            // `used_by:` is authored on the Concept and lists its
            // consumers — an inbound edge, so it reads backwards off
            // the graph and is pruned to slice members. Same reason
            // inbound edges are dropped elsewhere: a slice is what the
            // agent touched, not everything that touches it.
            if e.kind == EntityKind::Concept {
                let consumers = inbound_used_by(self.result, &e.id, self.members, self.by_id);
                self.write_ref_list(out, "used_by", consumers);
            }
        }

        // Children, pruned to what the slice actually emits. For a
        // member every child is already in — the pruning only bites on
        // ancestors, which is exactly the point.
        let mut kids: Vec<&String> = self
            .graph
            .children_of(&e.id)
            .iter()
            .filter(|id| self.emitted.contains(*id))
            .collect();
        kids.sort();
        for kid in kids {
            let c = self.by_id[kid];
            let _ = writeln!(out, "    {} {}", keyword(c.kind), ref_name(c));
        }

        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    /// Write one `<field>: a, b, c` line, dropping targets that would
    /// dangle. A target outside the slice is fine — it gets a
    /// name-only definition further down — but one the source spec
    /// never defines stays dropped, so the slice doesn't paper over
    /// the source's gap.
    fn write_ref_list(&self, out: &mut String, field: &str, mut targets: Vec<String>) {
        targets.retain(|id| {
            self.emitted.contains(id) || !self.by_id[id].tags.contains("unresolved")
        });
        if targets.is_empty() {
            return;
        }
        targets.sort_by(|a, b| self.by_id[a].qualified_name.cmp(&self.by_id[b].qualified_name));
        let refs: Vec<String> = targets.iter().map(|id| ref_name(self.by_id[id])).collect();
        let _ = writeln!(out, "    {}: {}", field, refs.join(", "));
    }
}

/// Consumers of `concept_id` that are inside the slice. The parser
/// emits `used_by:` as consumer → Concept, so reconstructing the
/// field means walking the edge backwards.
fn inbound_used_by(
    result: &AnalysisResult,
    concept_id: &str,
    members: &BTreeSet<String>,
    by_id: &HashMap<String, &CodeEntity>,
) -> Vec<String> {
    let mut out = Vec::new();
    for rel in &result.relationships {
        if rel.kind == RelationshipKind::References
            && rel.target_id == concept_id
            && rel.metadata.get("link").map(String::as_str) == Some("used_by")
            && members.contains(&rel.source_id)
            && by_id.contains_key(&rel.source_id)
            && !out.contains(&rel.source_id)
        {
            out.push(rel.source_id.clone());
        }
    }
    out
}

// =====================================================================
// Naming and formatting
// =====================================================================

fn keyword(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Extension => "e",
        EntityKind::Category => "c",
        EntityKind::Feature => "f",
        EntityKind::Functionality => "fu",
        EntityKind::Concept => "concept",
        EntityKind::UiPage => "ui",
        _ => "?",
    }
}

/// Name for a top-level definition. Functionalities get the `f.`
/// prefix hand-written specs use (`fu f.spec_health.drift`); every
/// other kind is a bare qualified name.
fn def_name(e: &CodeEntity) -> String {
    match e.kind {
        EntityKind::Functionality => format!("f.{}", e.qualified_name),
        _ => e.qualified_name.clone(),
    }
}

/// Name for a reference inside a body. Same form as [`def_name`] —
/// the parser strips the kind prefix and, for Functionalities, keys
/// off the remaining dot to know the name is already qualified. Emitting
/// the qualified form everywhere means a child reference resolves the
/// same whether or not its parent is the Feature that owns it.
fn ref_name(e: &CodeEntity) -> String {
    def_name(e)
}

/// Split a `cr:`/`cr.<tag>:` attribute into its field key and path.
/// Non-`cr` attributes (other languages hang their own things here)
/// return `None` and are skipped.
fn split_code_ref(attr: &str) -> Option<(String, &str)> {
    if let Some(rest) = attr.strip_prefix("cr.") {
        let (tag, path) = rest.split_once(':')?;
        Some((format!("cr.{}", tag), path))
    } else {
        attr.strip_prefix("cr:").map(|p| ("cr".to_string(), p))
    }
}

/// The lexer has no escape sequences: a string runs to the next `"`
/// and may not cross a line. Anything parsed from `.elv` already
/// satisfies that, so this only bites on descriptions that reached
/// the graph some other way — where mangling one character beats
/// emitting a file that won't parse.
fn escape(s: &str) -> String {
    s.replace('"', "'").replace(['\n', '\r'], " ")
}

fn sorted_of_kind<'a, I>(
    by_id: &HashMap<String, &CodeEntity>,
    ids: I,
    kind: EntityKind,
) -> Vec<String>
where
    I: Iterator<Item = &'a String>,
{
    let mut out: Vec<String> = ids
        .filter(|id| by_id.get(*id).map(|e| e.kind) == Some(kind))
        .cloned()
        .collect();
    out.sort_by(|a, b| by_id[a].qualified_name.cmp(&by_id[b].qualified_name));
    out
}

fn kind_summary(by_id: &HashMap<String, &CodeEntity>, ids: &BTreeSet<String>) -> String {
    let labels = [
        (EntityKind::Extension, "extension", "extensions"),
        (EntityKind::Category, "category", "categories"),
        (EntityKind::Feature, "feature", "features"),
        (EntityKind::Functionality, "functionality", "functionalities"),
        (EntityKind::Concept, "concept", "concepts"),
        (EntityKind::UiPage, "UI page", "UI pages"),
    ];
    let parts: Vec<String> = labels
        .iter()
        .filter_map(|(kind, one, many)| {
            let n = ids.iter().filter(|id| by_id[*id].kind == *kind).count();
            if n == 0 {
                None
            } else {
                Some(format!("{} {}", n, if n == 1 { one } else { many }))
            }
        })
        .collect();
    if parts.is_empty() {
        "empty".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests;
