//! The file half of `impact` (MCP-038): `path` with no `line`.
//!
//! "What outside this file depends on it, and what does it depend on
//! outside itself" is the first question asked when picking up an
//! unfamiliar file, planning a move, or judging whether a file can go.
//! `impact` could only answer it one entity at a time — `map` the file,
//! call `impact` per entity, merge by hand, and drop the dependents that
//! turned out to be the file's own neighbours. For a three-entity file
//! that is four calls; for a forty-entity file nobody does it, so the
//! question went unanswered.
//!
//! The unit here is the **file**, which is not the sum of the entities in
//! it. Two declarations in one file call each other, so adding up their
//! fan-in double-counts the wiring and overstates what the file owes the
//! outside. Every edge is therefore partitioned on where its other end
//! lives: outside the file it is a dependency in one of the two
//! directions, inside it is cohesion and is counted rather than listed.
//!
//! The same question, one grain coarser, is what `mezz deps` answers on
//! the command line ([`crate::output::deps_report`], CLI-001). This is
//! that answer carried to the agent surface, with the edge labels and the
//! landing entities a listing can afford and a CLI summary cannot.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use crate::check::tally;
use crate::graph::DependencyGraph;
use crate::models::{CodeEntity, Relationship, RelationshipKind};

use super::effects::EffectSurface;
use super::externals::Externals;
use super::tools::{cap_lines, edge_label, lifted, rel_path};

/// Rows named per direction before the rest is summarised — the 40 the
/// entity-level sections use, for the same reason.
const MAX_ROWS: usize = 40;

/// One dependency edge that crosses the file's boundary, read from the
/// file's side: the end outside, the declaration inside it lands on or
/// leaves from, and what kind of edge it is.
struct Crossing<'g> {
    other: &'g CodeEntity,
    here: &'g CodeEntity,
    label: String,
}

/// Every edge touching the file, sorted into the three buckets the report
/// prints.
struct FileEdges<'g> {
    /// Outside entities that reach in — who breaks if this file changes.
    inbound: Vec<Crossing<'g>>,
    /// Outside entities this file reaches — what it owes the rest.
    outbound: Vec<Crossing<'g>>,
    /// Declaration pairs wholly inside the file, deduped: cohesion.
    internal: HashSet<(&'g str, &'g str)>,
    /// Direct targets that are not code in this repo, placed and named
    /// (MCP-039). Counted per call site: one ghost node stands for every
    /// call to the same unbound name, and a file that reaches `Node::walk`
    /// forty times is coupled to it forty times.
    unbound: Externals<'g>,
}

impl<'g> FileEdges<'g> {
    /// Both directions, read from every entity the file holds.
    ///
    /// Seeded with *every* entity, not only the listed ones: a call written
    /// inside an `if` leaves from the branch node, so an edge of the file's
    /// can fail to touch any declaration's id at all. The branch is in the
    /// file, so seeding from the file catches it, and both ends are lifted
    /// to the declaration a reader can open (see [`lifted`]).
    fn of(graph: &'g DependencyGraph, local: &HashSet<&'g str>, file: &Path) -> Self {
        let mut acc = FileEdges {
            inbound: Vec::new(),
            outbound: Vec::new(),
            internal: HashSet::new(),
            unbound: Externals::of_file(graph, file),
        };
        let mut ids: Vec<&str> = local.iter().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let Some(raw) = graph.get_entity(id) else {
                continue;
            };
            let here = lifted(graph, raw).unwrap_or(raw);
            for (dep, rel) in graph.dependencies(id) {
                acc.outward(graph, local, here, dep, rel);
            }
            for (dep, rel) in graph.dependents(id) {
                acc.inward(graph, local, here, dep, rel);
            }
        }
        acc
    }

    /// One edge leaving a declaration of this file.
    fn outward(
        &mut self,
        graph: &'g DependencyGraph,
        local: &HashSet<&'g str>,
        here: &'g CodeEntity,
        dep: &'g CodeEntity,
        rel: &Relationship,
    ) {
        if rel.kind == RelationshipKind::Contains {
            return;
        }
        if dep.tags.contains("ghost") {
            self.unbound.add(dep);
            return;
        }
        let other = lifted(graph, dep).unwrap_or(dep);
        if local.contains(other.id.as_str()) {
            if other.id != here.id {
                self.internal
                    .insert((here.id.as_str(), other.id.as_str()));
            }
            return;
        }
        self.outbound.push(Crossing {
            other,
            here,
            label: edge_label(rel),
        });
    }

    /// One edge arriving at a declaration of this file.
    ///
    /// Same-file arrivals are dropped rather than counted: [`Self::outward`]
    /// already saw that edge from its other end, and counting it twice
    /// would report a file's cohesion as double what it is.
    fn inward(
        &mut self,
        graph: &'g DependencyGraph,
        local: &HashSet<&'g str>,
        here: &'g CodeEntity,
        dep: &'g CodeEntity,
        rel: &Relationship,
    ) {
        if rel.kind == RelationshipKind::Contains || dep.tags.contains("ghost") {
            return;
        }
        let other = lifted(graph, dep).unwrap_or(dep);
        if local.contains(other.id.as_str()) {
            return;
        }
        self.inbound.push(Crossing {
            other,
            here,
            label: edge_label(rel),
        });
    }
}

/// The file-level report: one hop in each direction, grouped by the file at
/// the other end.
pub(super) fn report(graph: &DependencyGraph, file: &Path, root: &Path) -> String {
    let named = rel_path(file, root);
    let local: HashSet<&str> = graph
        .entities()
        .filter(|e| e.file_path == file)
        .map(|e| e.id.as_str())
        .collect();

    let mut body = vec![format!("# Impact of {named} — the file"), String::new()];

    // Said rather than answered. An empty seed set does not mean the file
    // depends on nothing and nothing depends on it; it means mezz holds
    // nothing from the file, and neither direction can be answered for it.
    // The difference matters most to the reader about to delete it.
    if local.is_empty() {
        body.push(
            "No declarations from this file are in the graph — it was not parsed, the \
             session's settings exclude it, or it declares nothing. Neither direction \
             can be answered for it."
                .to_string(),
        );
        return body.join("\n");
    }

    let edges = FileEdges::of(graph, &local, file);
    body.push(
        "Asked of a file, so the unit is the file: one hop in each direction, edges \
         inside the file counted rather than listed. Pass `line` as well to target the \
         entity spanning it, with its transitive blast radius."
            .to_string(),
    );

    body.extend(section(
        &Section {
            title: "Depended on by",
            blurb: "what reaches into this file, and who breaks if its contract changes",
            legend: "One row per dependent, under the file it lives in; `→` names the \
                     declarations here it lands on.",
            arrow: "→",
        },
        &edges.inbound,
        root,
    ));
    if edges.inbound.is_empty() {
        body.push(
            "_No **resolved** dependents, which is not the same as none._ References \
             mezz could not resolve are attached to a ghost of the same name rather \
             than to anything in this file, and call sites in a form the parser doesn't \
             reach produce no edge at all. Call `dead_code` before treating the file as \
             deletable."
                .to_string(),
        );
    }

    body.extend(section(
        &Section {
            title: "Depends on",
            blurb: "what this file reaches outside itself, and so owes the rest of \
                    the tree",
            legend: "One row per dependency, under the file it lives in; `←` names the \
                     declarations here that reach it.",
            arrow: "←",
        },
        &edges.outbound,
        root,
    ));
    body.extend(edges.unbound.sections("this file"));
    // One hop, like the rest of the file view: this is what the file's own
    // declarations do to the world, not what everything under them does
    // (MCP-043). Pass `line` for the walk.
    body.extend(EffectSurface::of_file(graph, file, &local).section("this file", None, root));

    body.push(String::new());
    body.push(format!(
        "## Internal ({}) — edges wholly inside this file",
        tally(edges.internal.len(), "edge")
    ));
    body.push(
        "Counted, not listed: a file's own wiring is what makes it cohesive, not a work \
         list, and it is the part a per-entity walk would have double-counted into the \
         two sections above. Containment is excluded — every declaration contains its \
         own body."
            .to_string(),
    );

    cap_lines(
        body,
        "Target one entity instead: `impact` with `path` and `line`.",
    )
}

/// The fixed prose of one direction, so the heading, its legend and its
/// arrow cannot drift apart.
struct Section {
    title: &'static str,
    blurb: &'static str,
    legend: &'static str,
    arrow: &'static str,
}

/// One outside entity, and everything it touches on this side of the
/// boundary.
///
/// One row per outside entity, never per edge. A type used by seven of a
/// file's declarations is one thing to know about and seven identical rows
/// to read past, and a section whose rows and whose count measure different
/// populations is one a reader cannot check.
struct Landing<'g> {
    other: &'g CodeEntity,
    /// The distinct edge kinds involved, in label order.
    labels: BTreeSet<&'g str>,
    /// The declarations here it reaches or is reached from, in reading
    /// order.
    here: BTreeSet<(usize, &'g str)>,
}

impl<'g> Landing<'g> {
    fn of(other: &'g CodeEntity) -> Self {
        Landing {
            other,
            labels: BTreeSet::new(),
            here: BTreeSet::new(),
        }
    }
}

/// How many declarations of this file one row names before it counts the
/// rest. The row exists to say *what* is on the other side; which four of
/// this file's declarations touch it is the detail, and the file has at
/// most a screenful of declarations anyway.
const MAX_HERE: usize = 4;

/// One direction, grouped by the file at the other end and then by the
/// entity inside it.
fn section<'g>(kind: &Section, crossings: &'g [Crossing<'g>], root: &Path) -> Vec<String> {
    let grouped = landings(crossings, root);
    let entities: usize = grouped.values().map(BTreeMap::len).sum();

    let mut body = vec![
        String::new(),
        format!(
            "## {} ({} in {}) — {}",
            kind.title,
            counted(entities, "entity", "entities"),
            counted(grouped.len(), "file", "files"),
            kind.blurb,
        ),
    ];
    if grouped.is_empty() {
        return body;
    }
    body.push(kind.legend.to_string());

    let mut budget = MAX_ROWS;
    let (mut dropped_rows, mut dropped_files) = (0usize, 0usize);
    for (file, rows) in grouped {
        let shown = rows.len().min(budget);
        if shown == 0 {
            dropped_files += 1;
            dropped_rows += rows.len();
            continue;
        }
        body.push(match shown == rows.len() {
            true => format!("### {} ({})", file, rows.len()),
            false => format!("### {} ({} of {})", file, shown, rows.len()),
        });
        body.extend(
            rows.values()
                .take(shown)
                .map(|row| row_line(row, kind.arrow)),
        );
        dropped_rows += rows.len() - shown;
        budget -= shown;
    }
    if dropped_rows > 0 {
        let whole = match dropped_files {
            0 => String::new(),
            n => format!(" — {} not named here at all", counted(n, "file", "files")),
        };
        body.push(format!(
            "… and {} more{}. The cap is {} rows; ask `impact` about one entity of this \
             file for the rest.",
            dropped_rows, whole, MAX_ROWS,
        ));
    }
    body
}

/// The crossings collapsed to one landing per outside entity, filed under
/// the file that entity lives in. Both maps are ordered — files by path,
/// entities by position — so two runs over one graph print identically
/// (AN-002).
fn landings<'g>(
    crossings: &'g [Crossing<'g>],
    root: &Path,
) -> BTreeMap<String, BTreeMap<(usize, &'g str), Landing<'g>>> {
    let mut grouped: BTreeMap<String, BTreeMap<(usize, &str), Landing>> = BTreeMap::new();
    for crossing in crossings {
        let landing = grouped
            .entry(rel_path(&crossing.other.file_path, root))
            .or_default()
            .entry((crossing.other.span.start.line, crossing.other.id.as_str()))
            .or_insert_with(|| Landing::of(crossing.other));
        landing.labels.insert(crossing.label.as_str());
        landing
            .here
            .insert((crossing.here.span.start.line, crossing.here.name.as_str()));
    }
    grouped
}

/// `- calls ·exact function `dispatch` L405 → `impact`, `map``.
fn row_line(row: &Landing, arrow: &str) -> String {
    let named: Vec<String> = row
        .here
        .iter()
        .take(MAX_HERE)
        .map(|(_, name)| format!("`{name}`"))
        .collect();
    let rest = match row.here.len().saturating_sub(MAX_HERE) {
        0 => String::new(),
        n => format!(", +{n} more"),
    };
    format!(
        "- {} {} `{}` L{} {} {}{}",
        row.labels.iter().copied().collect::<Vec<_>>().join(", "),
        row.other.kind.display_name(),
        row.other.name,
        row.other.span.start.line + 1,
        arrow,
        named.join(", "),
        rest,
    )
}

/// `1 entity` / `4 entities`. [`tally`] covers the regular nouns here; this
/// is the one that does not take an `s`.
fn counted(n: usize, singular: &str, plural: &str) -> String {
    match n {
        1 => format!("1 {singular}"),
        _ => format!("{n} {plural}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{EntityKind, Span};
    use std::path::PathBuf;

    /// `line` is 0-based, as a span is; the report prints it 1-based.
    fn entity(file: &str, line: usize, name: &str) -> CodeEntity {
        CodeEntity::new(
            name,
            EntityKind::Function,
            file,
            Span::from_positions(line, 0, line, 0),
        )
    }

    fn graph_of(entities: Vec<CodeEntity>, edges: &[(usize, usize)]) -> DependencyGraph {
        let relationships = edges
            .iter()
            .map(|&(s, t)| {
                Relationship::new(&entities[s].id, &entities[t].id, RelationshipKind::Calls)
            })
            .collect();
        DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn report_on(graph: &DependencyGraph, file: &str) -> String {
        report(graph, &PathBuf::from(file), Path::new(""))
    }

    /// MCP-038's first acceptance criterion, and the reason the file cannot
    /// be answered by summing its entities: `a` and `b` call each other, so
    /// a per-entity walk reports each as a dependent of the other. Neither
    /// is a dependent of the *file*.
    #[test]
    fn a_files_own_wiring_is_counted_and_never_reported_as_an_outside_edge() {
        let graph = graph_of(
            vec![
                entity("many.ts", 0, "a"),
                entity("many.ts", 1, "b"),
                entity("caller.ts", 0, "caller"),
            ],
            // b → a and a → b inside the file; caller → a from outside.
            &[(1, 0), (0, 1), (2, 0)],
        );

        let out = report_on(&graph, "many.ts");

        assert!(
            out.contains("## Depended on by (1 entity in 1 file)"),
            "the file's own neighbours leaked into its dependents:\n{out}"
        );
        assert!(
            out.contains("`caller` L1 → `a`"),
            "the outside dependent is missing or unlanded:\n{out}"
        );
        assert!(
            out.contains("## Depends on (0 entities in 0 files)"),
            "an internal edge was reported as a dependency of the file:\n{out}"
        );
        assert!(
            out.contains("## Internal (2 edges)"),
            "the internal edges were not counted:\n{out}"
        );
    }

    /// Both directions name the file at the other end, because that is the
    /// unit a reader plans a move or a deletion in.
    #[test]
    fn dependents_and_dependencies_are_grouped_by_the_other_ends_file() {
        let graph = graph_of(
            vec![
                entity("core.ts", 0, "run"),
                entity("caller.ts", 0, "one"),
                entity("caller.ts", 1, "two"),
                entity("helper.ts", 0, "helper"),
            ],
            &[(1, 0), (2, 0), (0, 3)],
        );

        let out = report_on(&graph, "core.ts");

        assert!(
            out.contains("## Depended on by (2 entities in 1 file)")
                && out.contains("### caller.ts (2)"),
            "the dependents are not grouped by their file:\n{out}"
        );
        assert!(
            out.contains("## Depends on (1 entity in 1 file)")
                && out.contains("### helper.ts (1)")
                && out.contains("`helper` L1 ← `run`"),
            "the dependency is not grouped by its file:\n{out}"
        );
    }

    /// `caller.ts` holds one declaration whose call to `core.ts` is written
    /// inside an `if`, so the edge on the wire leaves the *branch* node.
    ///
    /// One fixture for the two tests below, which read the same graph from
    /// its two ends — the file that is called and the file that calls.
    fn call_from_inside_a_branch() -> DependencyGraph {
        let caller = entity("caller.ts", 0, "caller");
        let branch = CodeEntity::new(
            "if",
            EntityKind::Branch,
            "caller.ts",
            caller.span,
        )
        .with_parent(caller.id.clone());
        graph_of(vec![entity("core.ts", 0, "run"), caller, branch], &[(2, 0)])
    }

    /// A call written inside an `if` leaves from the branch node, which is
    /// in the file but is not a declaration anyone can open. Both ends are
    /// lifted: the branch is reported as the function around it, and the
    /// edge is not lost for having an unlisted end.
    #[test]
    fn an_edge_written_inside_a_branch_belongs_to_the_declaration_around_it() {
        let out = report_on(&call_from_inside_a_branch(), "core.ts");

        assert!(
            out.contains("`caller` L1 → `run`"),
            "the caller was lost to its branch node:\n{out}"
        );
        assert!(!out.contains("`if`"), "a body scope was named:\n{out}");
    }

    /// The other half of the same lift, from the file the branch is in: the
    /// file's outgoing edge is written in a branch and never touches the
    /// declaration's id, so a walk seeded on declarations alone misses it.
    #[test]
    fn a_dependency_written_inside_a_branch_still_belongs_to_the_file() {
        let out = report_on(&call_from_inside_a_branch(), "caller.ts");

        assert!(
            out.contains("## Depends on (1 entity in 1 file)")
                && out.contains("`run` L1 ← `caller`"),
            "the branch's call is not the file's dependency:\n{out}"
        );
    }

    /// A file the graph holds nothing from says so, rather than answering
    /// "nothing depends on this" to a reader about to delete it (CLI-001).
    #[test]
    fn a_file_the_graph_does_not_hold_says_so_rather_than_answering() {
        let graph = graph_of(vec![entity("other.ts", 0, "a")], &[]);

        let out = report_on(&graph, "missing.ts");

        assert!(out.contains("No declarations from this file"), "{out}");
        assert!(!out.contains("## Depended on by"), "{out}");
    }

    /// Zero resolved dependents is printed with the caveat the entity-level
    /// section prints, for the same reason: unresolved references attach to
    /// a ghost, so this direction cannot see its own misses.
    #[test]
    fn zero_dependents_says_what_it_does_not_know() {
        let graph = graph_of(vec![entity("leaf.ts", 0, "a")], &[]);

        let out = report_on(&graph, "leaf.ts");

        assert!(
            out.contains("## Depended on by (0 entities in 0 files)")
                && out.contains("not the same as none"),
            "a bare zero was reported with no caveat:\n{out}"
        );
    }
}
