//! Folder shape — whether the picture a directory draws can be read.
//!
//! Nao's premise is that a code graph is understood through its drawing,
//! and a drawing is only followable when it has a shape: acyclic, layered,
//! and holding that shape at whichever level you zoom to. This pass scores
//! that per folder, over exactly the graph the canvas renders when
//! collapsed to that folder — its immediate children, with each subfolder
//! standing as a single node. Scoring the drawn graph rather than an
//! abstract one is the point: the number is a claim about a picture
//! somebody looks at.
//!
//! It is not a code-quality measure and deliberately reaches no composite
//! score. A folder can be immaculately organised and full of terrible
//! code, or the reverse; keeping the two apart is what lets either be
//! acted on.
//!
//! ## Where the work happens
//!
//! Every cross-file edge lands in exactly one folder's child graph — the
//! one where the two files first part company. Above that folder both
//! endpoints collapse into the same child and the edge is internal; below
//! it, only one endpoint is present. So the whole pass is one walk over
//! the edge list finding each pair's lowest common folder, not a scan of
//! every folder against every edge.

use crate::models::{
    ChildKind, EdgeVerdict, ErasedEdge, FolderPicture, FolderShape, ImportSite, OutsideEdge,
    OutsideVerdict, PictureChild, PictureEdge, ShapeBlocker, ShapePattern, Thresholds,
};
use petgraph::algo::{condensation, tarjan_scc};
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::{HashMap, HashSet};
use std::path::MAIN_SEPARATOR;

/// Weights of the four sub-scores in `compliance`. Acyclicity leads
/// because a cycle is the one defect that leaves a drawing with no reading
/// order at all; the two recursive-ish terms are lighter because either
/// can be legitimately absent.
const W_ACYCLICITY: f32 = 0.40;
const W_LAYERING: f32 = 0.30;
const W_ENTRY: f32 = 0.15;
const W_CHILD: f32 = 0.15;

/// Score every folder in `folders`, keyed the same way as the module
/// rollups so a caller can join them by path.
///
/// `pairs` are cross-file *dependency* edges — the same bucket every other
/// coupling number is computed from, so `References` edges are out for the
/// reason they are out of cohesion and fan-out (UI-091). Duplicates are
/// fine; the pass dedupes.
///
/// `declaration_only` are files holding nothing but module declarations.
/// They are the folder speaking rather than children of it, and are left
/// out of the drawing entirely — see [`build_children`].
///
/// `imports` are the statements those dependencies were written as. The
/// only thing in the model that knows the build erases an edge, and the
/// reason this pass takes them: an arrow every import behind it is
/// `import type` is drawn and not scored (ADR 0026).
pub fn compute<'a>(
    files: impl IntoIterator<Item = &'a str>,
    pairs: &[(String, String)],
    imports: &[ImportSite],
    folders: &HashSet<String>,
    declaration_only: &HashSet<String>,
) -> HashMap<String, FolderShape> {
    let thresholds = Thresholds::default();
    let children = build_children(files, folders, declaration_only);
    let accum = accumulate(pairs, imports, folders);

    // Deepest first, so a folder's subfolders are already scored by the
    // time its `child_compliance` needs them.
    let mut order: Vec<&String> = folders.iter().collect();
    order.sort_by(|a, b| depth(b).cmp(&depth(a)).then(a.cmp(b)));

    let mut shapes: HashMap<String, FolderShape> = HashMap::new();
    for folder in order {
        let kids = children.get(folder).cloned().unwrap_or_default();
        let no_edges = HashSet::new();
        let edges = accum.edges.get(folder).unwrap_or(&no_edges);
        let scores = graph_scores(&kids, &scored(edges, accum.erased_in(folder)));
        let inside = subfolders(&kids, folders, &shapes);
        let shape = FolderShape {
            pattern: ShapePattern::Hierarchical, // replaced below
            compliance: 0.0,
            acyclicity: scores.acyclicity,
            layering: scores.layering,
            arborescence: scores.arborescence,
            entry_concentration: entry_concentration(accum.entries.get(folder)),
            child_compliance: inside.mean_compliance,
            child_count: kids.len() as u32,
            blocker: None, // replaced below
        };
        shapes.insert(
            folder.clone(),
            finish(shape, inside.worst_pattern, &thresholds),
        );
    }
    shapes
}

/// The graph one folder draws, with a verdict on every node and edge.
///
/// Takes the same three inputs as [`compute`] and derives from them the
/// same way, so the picture a reader is shown is the picture the score was
/// computed over rather than a reconstruction that agrees with it most of
/// the time.
///
/// `None` when `folder` is not one of `folders` — a path nobody analysed
/// has no picture, which is a different answer from an empty one.
///
/// Computed for one folder rather than for all of them: the scalars are
/// four floats a folder and free to keep, where a picture is the edge list
/// again.
pub fn picture<'a>(
    folder: &str,
    files: impl IntoIterator<Item = &'a str>,
    pairs: &[(String, String)],
    imports: &[ImportSite],
    folders: &HashSet<String>,
    declaration_only: &HashSet<String>,
) -> Option<FolderPicture> {
    if !folders.contains(folder) {
        return None;
    }
    let kids = build_children(files, folders, declaration_only)
        .remove(folder)
        .unwrap_or_default();
    let accum = accumulate(pairs, imports, folders);
    let no_edges = HashSet::new();
    let edges = accum.edges.get(folder).unwrap_or(&no_edges);
    let no_entries = HashMap::new();
    let entries = accum.entries.get(folder).unwrap_or(&no_entries);
    // The same split the scores are computed over, from the same
    // accumulator — the drawing cannot show one set of arrows while the
    // numbers were taken over another.
    let drawn = scored(edges, accum.erased_in(folder));

    let doors = doors_of(entries);
    Some(FolderPicture {
        folder: folder.to_string(),
        children: drawn_children(folder, &kids, &drawn, folders, entries, &doors),
        edges: drawn_edges(&kids, &drawn),
        erased: erased_list(edges, accum.erased_in(folder)),
        outside: boundary_traffic(folder, pairs, &doors),
        doors,
    })
}

/// The arrows a folder's scores are computed over: everything its children
/// draw between them, minus the ones the build erases (ADR 0026).
///
/// The one definition of that subtraction. `graph_scores` and the drawing
/// both come through here, so `acyclicity`, `layering`, `arborescence`,
/// the levels each child sits on and the reading of every edge are all
/// claims about the same set of arrows.
fn scored(
    edges: &HashSet<(String, String)>,
    erased: &HashSet<(String, String)>,
) -> HashSet<(String, String)> {
    edges.difference(erased).cloned().collect()
}

/// The other half of that subtraction, as the picture reports it.
///
/// An intersection rather than the erasure map straight: that map is
/// keyed by every import statement the folder holds, including ones no
/// dependency edge was ever derived from, and listing an arrow the
/// drawing never had would be inventing one.
fn erased_list(
    edges: &HashSet<(String, String)>,
    erased: &HashSet<(String, String)>,
) -> Vec<ErasedEdge> {
    let mut out: Vec<ErasedEdge> = edges
        .intersection(erased)
        .map(|(from, to)| ErasedEdge {
            from: from.clone(),
            to: to.clone(),
        })
        .collect();
    out.sort();
    out
}

/// Every folder's doors, keyed the same way as [`compute`].
///
/// The Door itself, rather than the `entry_concentration` computed from it.
/// A caller asking "how many doors does this folder have" is asking the
/// question the ratio rounds off — 0.5 is two doors or a door and a breach,
/// and the two want opposite fixes — and asking it here is what keeps one
/// definition of Door in the tool (ADR 0024).
///
/// A folder nothing outside it depends on is absent rather than empty: it
/// has no doors because nobody has ever come in, which is a different
/// answer from having none.
pub fn doors_by_folder(
    pairs: &[(String, String)],
    folders: &HashSet<String>,
) -> HashMap<String, Vec<String>> {
    // No imports: doors are counted over every dependency written, erased
    // or not. `entry_concentration` asks who reaches into the folder and
    // whether they come through one file — a question about how the code
    // reads, which the build does not change (ADR 0026).
    accumulate(pairs, &[], folders)
        .entries
        .iter()
        .map(|(folder, entries)| (folder.clone(), doors_of(entries)))
        .collect()
}

/// The files taking the most dependencies from outside. Every file tied at
/// the maximum, since a tie is a real answer and choosing one of them would
/// invent a breach.
fn doors_of(entries: &HashMap<String, u32>) -> Vec<String> {
    let Some(&busiest) = entries.values().max() else {
        return Vec::new();
    };
    let mut doors: Vec<String> = entries
        .iter()
        .filter(|(_, count)| **count == busiest)
        .map(|(file, _)| file.clone())
        .collect();
    doors.sort();
    doors
}

/// Each circle in the drawing, with the row it sits on.
fn drawn_children(
    folder: &str,
    kids: &[String],
    edges: &HashSet<(String, String)>,
    folders: &HashSet<String>,
    entries: &HashMap<String, u32>,
    doors: &[String],
) -> Vec<PictureChild> {
    let levels = child_levels(kids, edges);
    // Outside traffic is counted per *file* — that is what
    // `entry_concentration` measures — so a subfolder's inbound is the sum
    // over the files inside it, and the door it holds is a file, not itself.
    let mut inbound: HashMap<&str, u32> = HashMap::new();
    for (file, count) in entries {
        if let Some(child) = child_holding(folder, file) {
            *inbound.entry(child).or_insert(0) += count;
        }
    }
    let doors_by_child: HashSet<&str> = doors
        .iter()
        .filter_map(|d| child_holding(folder, d))
        .collect();

    kids.iter()
        .enumerate()
        .map(|(i, path)| PictureChild {
            kind: if folders.contains(path) {
                ChildKind::Folder
            } else {
                ChildKind::File
            },
            level: levels[i],
            inbound: inbound.get(path.as_str()).copied().unwrap_or(0),
            is_door: doors_by_child.contains(path.as_str()),
            path: path.clone(),
        })
        .collect()
}

/// Each line in the drawing, and how it reads.
fn drawn_edges(kids: &[String], edges: &HashSet<(String, String)>) -> Vec<PictureEdge> {
    let levels = child_levels(kids, edges);
    let loops = loop_ids(kids, edges);
    let index: HashMap<&str, usize> = kids
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();

    let mut sorted: Vec<&(String, String)> = edges.iter().collect();
    sorted.sort();
    sorted
        .into_iter()
        .filter_map(|(from, to)| {
            let (&f, &t) = (index.get(from.as_str())?, index.get(to.as_str())?);
            // Same loop first: inside one, levels are shared by
            // construction and would read every edge as a skip, which
            // would charge the cycle twice over exactly as the scores
            // refuse to.
            let verdict = if loops[f].is_some() && loops[f] == loops[t] {
                EdgeVerdict::Back
            } else if levels[t] == levels[f] + 1 {
                EdgeVerdict::Step
            } else {
                EdgeVerdict::Skip
            };
            Some(PictureEdge {
                from: from.clone(),
                to: to.clone(),
                verdict,
            })
        })
        .collect()
}

/// Which row each child draws on. Members of one dependency loop share a
/// row, the loop having no internal order to lay out.
fn child_levels(kids: &[String], edges: &HashSet<(String, String)>) -> Vec<u32> {
    let mut out = vec![0_u32; kids.len()];
    if kids.is_empty() {
        return out;
    }
    let condensed = condensation(child_graph(kids, edges), true);
    let levels = levels_of(&drawn_dag(&condensed));
    for node in condensed.node_indices() {
        for &kid in &condensed[node] {
            out[kid] = levels[node.index()];
        }
    }
    out
}

/// Which row each node draws on in a drawing given as edges alone —
/// [`child_levels`] for a caller holding a kept edge list and no longer
/// holding the folder that produced it.
///
/// Exists for the one question an edge list cannot answer on its own:
/// whether a ratio moved because the drawing changed or because the rows
/// under it were re-assigned. `reshape` keeps the previous call's edges
/// precisely so a score change can be attributed, and attributing it
/// means re-running *this* assignment over that record rather than a
/// second one written to agree with it.
///
/// Children no edge touches are absent from the result, which is exactly
/// what the caller wants: `layering` never looks at them, and a node with
/// no row has no reading to have changed.
pub fn levels_by_name<'a>(
    edges: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> HashMap<String, u32> {
    let edges: HashSet<(String, String)> = edges
        .into_iter()
        .map(|(f, t)| (f.to_string(), t.to_string()))
        .collect();
    let mut nodes: Vec<String> = edges
        .iter()
        .flat_map(|(f, t)| [f.clone(), t.clone()])
        .collect();
    nodes.sort();
    nodes.dedup();
    let levels = child_levels(&nodes, &edges);
    nodes.into_iter().zip(levels).collect()
}

/// Which dependency loop each child belongs to, if any. Distinct ids
/// rather than a flag: two children can sit in two different loops, and
/// the edge between those is an ordinary edge.
fn loop_ids(kids: &[String], edges: &HashSet<(String, String)>) -> Vec<Option<usize>> {
    let mut out = vec![None; kids.len()];
    if kids.is_empty() {
        return out;
    }
    let graph = child_graph(kids, edges);
    for (id, scc) in tarjan_scc(&graph)
        .iter()
        .filter(|s| s.len() > 1)
        .enumerate()
    {
        for node in scc {
            out[graph[*node]] = Some(id);
        }
    }
    out
}

/// Every dependency crossing the folder's boundary, one hop and never
/// transitively — the same restraint the note-link closure keeps, and for
/// the same reason: two hops off a busy folder is most of the repo, and
/// this has to stay a narrowing to be worth drawing.
fn boundary_traffic(
    folder: &str,
    pairs: &[(String, String)],
    doors: &[String],
) -> Vec<OutsideEdge> {
    let doors: HashSet<&str> = doors.iter().map(String::as_str).collect();
    let distinct: HashSet<&(String, String)> = pairs.iter().collect();
    let mut out: Vec<OutsideEdge> = distinct
        .into_iter()
        .filter_map(|(src, tgt)| {
            let (from_in, to_in) = (is_inside(folder, src), is_inside(folder, tgt));
            // Both ends inside is the child graph's business; both outside
            // is somebody else's folder.
            if from_in == to_in {
                return None;
            }
            let (outside, inside) = if to_in { (src, tgt) } else { (tgt, src) };
            let verdict = if !to_in {
                OutsideVerdict::Exit
            } else if doors.contains(inside.as_str()) {
                OutsideVerdict::Entry
            } else {
                OutsideVerdict::Breach
            };
            Some(OutsideEdge {
                outside: outside.clone(),
                child: child_holding(folder, inside)?.to_string(),
                inside: inside.clone(),
                verdict,
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.child, &a.inside, &a.outside).cmp(&(&b.child, &b.inside, &b.outside)));
    out
}

/// Whether `path` sits anywhere under `folder`. Compared on a separator
/// boundary, or `src/parsed` would read as being inside `src/parse`.
fn is_inside(folder: &str, path: &str) -> bool {
    if folder.is_empty() {
        return true;
    }
    path.len() > folder.len()
        && path.starts_with(folder)
        && path[folder.len()..].starts_with(MAIN_SEPARATOR)
}

/// The immediate child of `folder` that `path` sits in — the subfolder one
/// level down, or the file itself when it lives in the folder direct.
/// Returned as a slice of `path`, which is what makes it usable as a map
/// key without allocating per lookup.
fn child_holding<'a>(folder: &str, path: &'a str) -> Option<&'a str> {
    if !is_inside(folder, path) {
        return None;
    }
    let start = if folder.is_empty() {
        0
    } else {
        folder.len() + 1
    };
    Some(match path[start..].find(MAIN_SEPARATOR) {
        Some(i) => &path[..start + i],
        None => path,
    })
}

/// Fill in the derived fields once the measured ones are known.
fn finish(
    mut shape: FolderShape,
    worst_child: Option<ShapePattern>,
    t: &Thresholds,
) -> FolderShape {
    shape.compliance = compliance_of(&shape);
    let (pattern, blocker) = classify(&shape, worst_child, t);
    shape.pattern = pattern;
    shape.blocker = blocker;
    shape
}

/// Weighted mean over whichever sub-scores are defined. An absent term
/// drops out of the denominator rather than scoring zero — a folder
/// nothing depends on has no entry concentration to answer for.
///
/// `arborescence` is deliberately not among them (ADR 0013). It gates
/// `Fractal` and is reported beside the blend, never inside it: adding a
/// fifth term would move every compliance number in the repo for a
/// property the other four were never measuring, and a score that shifts
/// for two unrelated reasons can be acted on for neither.
fn compliance_of(s: &FolderShape) -> f32 {
    let terms = [
        (W_ACYCLICITY, Some(s.acyclicity)),
        (W_LAYERING, s.layering),
        (W_ENTRY, s.entry_concentration),
        (W_CHILD, s.child_compliance),
    ];
    let mut numerator = 0.0_f32;
    let mut weight = 0.0_f32;
    for (w, value) in terms {
        if let Some(v) = value {
            numerator += w * v;
            weight += w;
        }
    }
    if weight > 0.0 {
        numerator / weight
    } else {
        1.0
    }
}

/// Walk the ladder from the bottom: each tier is a veto on the ones above.
///
/// Returns the tier alongside the gate that capped it, so the two can
/// never disagree about why a folder landed where it did.
fn classify(
    s: &FolderShape,
    worst_child: Option<ShapePattern>,
    t: &Thresholds,
) -> (ShapePattern, Option<ShapeBlocker>) {
    if s.acyclicity < 1.0 {
        return (
            ShapePattern::Cyclic,
            Some(ShapeBlocker::Cycles(s.acyclicity)),
        );
    }
    if let Some(layering) = s.layering.filter(|l| *l < t.shape_layering) {
        return (
            ShapePattern::Tangled,
            Some(ShapeBlocker::Layering(layering)),
        );
    }
    match short_of_fractal(s, worst_child, t) {
        Some(blocker) => (ShapePattern::Hierarchical, Some(blocker)),
        None => (ShapePattern::Fractal, None),
    }
}

/// The gate keeping an acyclic, layered folder out of `Fractal`, or `None`
/// when it clears them all.
///
/// The gates are an AND, so the tier does not depend on this order — only
/// which one gets reported does. They are asked in the order a reader
/// would fix them: how many children there are before anything about the
/// edges between them, this folder's own drawing before anything one level
/// down, a broken folder inside before a wide front door, and the blended
/// score last, because it is the only one that is not a thing you can go
/// and look at.
fn short_of_fractal(
    s: &FolderShape,
    worst_child: Option<ShapePattern>,
    t: &Thresholds,
) -> Option<ShapeBlocker> {
    own_drawing_gate(s, t).or_else(|| deeper_gate(s, worst_child, t))
}

/// The gates a reader can settle by looking at the picture in front of
/// them: how many children there are, and how their edges converge.
fn own_drawing_gate(s: &FolderShape, t: &Thresholds) -> Option<ShapeBlocker> {
    // First, because regrouping children redraws the very picture every
    // other gate is measured over (ADR 0014). A folder of thirty files
    // asked to fix a merge first would do that work against a drawing that
    // is about to stop existing; asked to group first, it re-measures
    // against the shape it actually means to keep. This is also the reason
    // breadth is worth reporting even though it is a convention: it is the
    // one finding that invalidates the others.
    if s.child_count > t.shape_max_children {
        return Some(ShapeBlocker::Breadth(s.child_count));
    }
    // Then the most local thing left: the merge is in the picture already
    // on screen, not one level down and not in a blend. `layering` has
    // already passed by here, so this is asking the question it
    // deliberately does not — a shared helper steps one level cleanly and
    // still leaves the drawing merging rather than branching (ADR 0013).
    s.arborescence
        .filter(|a| *a < t.shape_arborescence)
        .map(ShapeBlocker::Merges)
}

/// The gates that are about what is *inside* the children, who reaches in
/// from outside, and the blend — none of which the folder's own drawing
/// answers on its own.
fn deeper_gate(
    s: &FolderShape,
    worst_child: Option<ShapePattern>,
    t: &Thresholds,
) -> Option<ShapeBlocker> {
    // Two separate gates on the same children, because they catch
    // different failures. The tier floor answers "is any one of them
    // unreadable?", which no average should be allowed to average away;
    // the mean answers "are they broadly in order?", and a single tangled
    // child can hide behind well-behaved siblings in it.
    if let Some(pattern) = worst_child.filter(|p| *p < ShapePattern::Hierarchical) {
        return Some(ShapeBlocker::ChildPattern(pattern));
    }
    if let Some(mean) = s.child_compliance.filter(|c| *c < t.shape_child) {
        return Some(ShapeBlocker::ChildCompliance(mean));
    }
    if let Some(entry) = s.entry_concentration.filter(|e| *e < t.shape_entry) {
        return Some(ShapeBlocker::Entry(entry));
    }
    // Fractal is a claim about self-similarity, so it needs a structure to
    // be similar to. A folder of children with no edges between them draws
    // a legible picture — a row of dots — but an unrelated one, and saying
    // "fractal" of it would read as praise for an accident. Only a folder
    // small enough to have no shape to speak of is waved through.
    if s.layering.is_none() && s.child_count > 1 {
        return Some(ShapeBlocker::Unstructured);
    }
    if s.compliance < t.shape_compliance {
        return Some(ShapeBlocker::Compliance(s.compliance));
    }
    None
}

/// What the subfolders directly inside this one came out at — the two
/// recursive facts a parent needs, gathered in one pass.
struct Inside {
    mean_compliance: Option<f32>,
    worst_pattern: Option<ShapePattern>,
}

fn subfolders(
    kids: &[String],
    folders: &HashSet<String>,
    shapes: &HashMap<String, FolderShape>,
) -> Inside {
    // `kids` arrives sorted, so the summation order is pinned and the mean
    // does not drift in its last bits between identical runs (AN-002).
    let inner: Vec<&FolderShape> = kids
        .iter()
        .filter(|k| folders.contains(*k))
        .filter_map(|k| shapes.get(k))
        .collect();
    if inner.is_empty() {
        return Inside {
            mean_compliance: None,
            worst_pattern: None,
        };
    }
    let total: f32 = inner.iter().map(|s| s.compliance).sum();
    Inside {
        mean_compliance: Some(total / inner.len() as f32),
        worst_pattern: inner.iter().map(|s| s.pattern).min(),
    }
}

/// Share of the traffic arriving from outside that lands on one file.
fn entry_concentration(doors: Option<&HashMap<String, u32>>) -> Option<f32> {
    let doors = doors?;
    let total: u32 = doors.values().sum();
    let busiest = doors.values().copied().max()?;
    if total == 0 {
        return None;
    }
    Some(busiest as f32 / total as f32)
}

// ------------------------------------------------------------------
//  The child graph
// ------------------------------------------------------------------

/// What one folder's collapsed child graph scores on its own terms —
/// everything measurable without looking outside the folder or below it.
struct GraphScores {
    acyclicity: f32,
    layering: Option<f32>,
    arborescence: Option<f32>,
}

/// The folder's collapsed child graph, node weights being indices back
/// into `nodes`.
///
/// One builder for the scoring and for the drawing, so the two can never
/// disagree about which edges exist — which is the whole reason the
/// picture is produced by this module rather than assembled beside it.
fn child_graph(nodes: &[String], edges: &HashSet<(String, String)>) -> DiGraph<usize, ()> {
    let mut graph: DiGraph<usize, ()> = DiGraph::new();
    let mut index: HashMap<&str, NodeIndex> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        index.insert(node.as_str(), graph.add_node(i));
    }
    // Sorted so the graph is built identically on every run.
    let mut sorted: Vec<&(String, String)> = edges.iter().collect();
    sorted.sort();
    for (src, tgt) in sorted {
        if let (Some(&a), Some(&b)) = (index.get(src.as_str()), index.get(tgt.as_str())) {
            graph.add_edge(a, b, ());
        }
    }
    graph
}

/// Score one folder's collapsed child graph.
fn graph_scores(nodes: &[String], edges: &HashSet<(String, String)>) -> GraphScores {
    if nodes.is_empty() {
        return GraphScores {
            acyclicity: 1.0,
            layering: None,
            arborescence: None,
        };
    }
    let graph = child_graph(nodes, edges);

    let tangled: usize = tarjan_scc(&graph)
        .iter()
        .filter(|scc| scc.len() > 1)
        .map(|scc| scc.len())
        .sum();
    let acyclicity = 1.0 - tangled as f32 / nodes.len() as f32;

    // Collapse the loops before measuring the rest. A cycle has already
    // been charged to `acyclicity`; letting it also erase the layering
    // would report one defect twice and leave the reader nothing to
    // compare a cyclic folder against.
    let dag = drawn_dag(&condensation(graph, true));
    GraphScores {
        acyclicity,
        layering: layering_of(&dag),
        arborescence: arborescence_of(&dag),
    }
}

/// The condensed child graph as plain adjacency, self-loops and duplicate
/// edges dropped. Both remaining sub-scores are ratios over the distinct
/// edges actually drawn, so they have to be counting the same set.
struct DrawnDag {
    /// Successors of each node.
    out: Vec<HashSet<usize>>,
    /// How many distinct nodes depend on each node.
    in_degree: Vec<usize>,
    /// Distinct edges, which is the denominator of `layering`.
    edges: usize,
}

fn drawn_dag(dag: &DiGraph<Vec<usize>, ()>) -> DrawnDag {
    let mut out: Vec<HashSet<usize>> = vec![HashSet::new(); dag.node_count()];
    for edge in dag.edge_indices() {
        let Some((a, b)) = dag.edge_endpoints(edge) else {
            continue;
        };
        if a != b {
            out[a.index()].insert(b.index());
        }
    }
    let mut in_degree = vec![0_usize; dag.node_count()];
    for targets in &out {
        for &to in targets {
            in_degree[to] += 1;
        }
    }
    let edges = out.iter().map(HashSet::len).sum();
    DrawnDag {
        out,
        in_degree,
        edges,
    }
}

/// The longest path reaching each node from a source — the level it draws
/// on. Shared by everything downstream, so a node cannot sit on one level
/// for one number and a different one for the next.
///
/// Kahn's algorithm over the in-degrees already counted. The levels do not
/// depend on which valid topological order it happens to walk, `max` being
/// commutative, so no ordering is imposed on the frontier.
fn levels_of(dag: &DrawnDag) -> Vec<u32> {
    let mut level = vec![0_u32; dag.out.len()];
    let mut remaining = dag.in_degree.clone();
    let mut frontier: Vec<usize> = (0..dag.out.len()).filter(|n| remaining[*n] == 0).collect();
    while let Some(node) = frontier.pop() {
        let here = level[node];
        for &next in &dag.out[node] {
            level[next] = level[next].max(here + 1);
            remaining[next] -= 1;
            if remaining[next] == 0 {
                frontier.push(next);
            }
        }
    }
    level
}

/// Share of edges that step exactly one level down. `None` when there are
/// no edges to measure.
fn layering_of(dag: &DrawnDag) -> Option<f32> {
    if dag.edges == 0 {
        return None;
    }
    let level = levels_of(dag);
    let mut tight = 0_usize;
    for (from, targets) in dag.out.iter().enumerate() {
        for &to in targets {
            if level[to] == level[from] + 1 {
                tight += 1;
            }
        }
    }
    Some(tight as f32 / dag.edges as f32)
}

/// Share of the drawn edges that would survive in a spanning *tree* — one
/// incoming edge per child, which is what makes a drawing branch instead of
/// merge. Every edge beyond the first arriving at a child is a merge, and
/// merges are what `layering` deliberately does not charge for.
///
/// An edge ratio over the same denominator as `layering`, on purpose: the
/// two sit side by side and must be read on the same scale. It also means
/// they are measured or unmeasured together — a folder whose children have
/// no edges between them has neither.
///
/// Degrades in proportion rather than collapsing: one merge among ten edges
/// costs 0.1, where counting *nodes* with a single parent would score the
/// commonest real shape — a handful of files and one shared helper — at
/// zero and make the number useless for ranking.
///
/// A child no edge reaches costs the same as a merge (ADR 0022). Counting
/// only the nodes with a parent left the ones with none free, so a folder of files
/// that never mention each other scored a perfect 1.00 — 15 children and a
/// single edge read as a flawless tree. That is a forest, and only a tree
/// has the self-similarity the top tier claims, so every root past the
/// first joins the denominator. One root is what a tree has; the rest are
/// the folder failing to be one.
fn arborescence_of(dag: &DrawnDag) -> Option<f32> {
    if dag.edges == 0 {
        return None;
    }
    let reached: usize = dag.in_degree.iter().filter(|d| **d > 0).count();
    let strays = dag.in_degree.len() - reached;
    Some(reached as f32 / (dag.edges + strays.saturating_sub(1)) as f32)
}

// ------------------------------------------------------------------
//  Folder tree and edge placement
// ------------------------------------------------------------------

/// What each folder's collapsed graph needs, filled by one pass over the
/// edge list.
#[derive(Default)]
struct Accum {
    /// Folder → the distinct child-to-child edges of its drawn graph.
    edges: HashMap<String, HashSet<(String, String)>>,
    /// Folder → file inside it → how many outside files depend on that
    /// file. The spread of this map is the folder's entry concentration.
    entries: HashMap<String, HashMap<String, u32>>,
    /// Folder → the child-to-child arrows the build erases (ADR 0026).
    erased: HashMap<String, HashSet<(String, String)>>,
}

impl Accum {
    /// One folder's erased arrows, empty for a folder with none — so a
    /// caller subtracting them does not have to carry a spare `HashSet`
    /// to borrow from.
    fn erased_in(&self, folder: &str) -> &HashSet<(String, String)> {
        static NONE: std::sync::OnceLock<HashSet<(String, String)>> = std::sync::OnceLock::new();
        self.erased
            .get(folder)
            .unwrap_or_else(|| NONE.get_or_init(HashSet::new))
    }
}

fn accumulate(
    pairs: &[(String, String)],
    imports: &[ImportSite],
    folders: &HashSet<String>,
) -> Accum {
    let distinct: HashSet<&(String, String)> = pairs.iter().collect();
    let mut accum = Accum::default();
    for (src, tgt) in distinct {
        let (from, to, shared) = parting(src, tgt);
        let lca = from[shared - 1];
        if folders.contains(lca) {
            let entry = accum.edges.entry(lca.to_string()).or_default();
            entry.insert((child_on(&from, shared, src), child_on(&to, shared, tgt)));
        }
        // Every folder below the parting point holds the target but not
        // the source, so this edge crosses each of their boundaries.
        for folder in &to[shared..] {
            *accum
                .entries
                .entry((*folder).to_string())
                .or_default()
                .entry(tgt.clone())
                .or_insert(0) += 1;
        }
    }
    accum.erased = erasure(imports, folders);
    accum
}

/// Where two files part company: their ancestor chains and how much of
/// the front of them is shared. Both chains open at the root, so they
/// always share a prefix and `shared` is never zero — which is what makes
/// `from[shared - 1]` the folder whose drawing the edge lands in.
fn parting<'a>(src: &'a str, tgt: &'a str) -> (Vec<&'a str>, Vec<&'a str>, usize) {
    let from = ancestors(parent_dir(src));
    let to = ancestors(parent_dir(tgt));
    let shared = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    (from, to, shared)
}

/// Which arrows the build erases, folder by folder (AN-025).
///
/// **Every** import statement behind the arrow, never any. One ordinary
/// `import { f }` beside an `import type { T }` and the target is in the
/// bundle, so the arrow is an ordinary dependency however many erased
/// lines sit beside it. `src/utils → src/types` in the browser UI is
/// written both ways in one file, four lines apart, and an any-site rule
/// would discount it.
///
/// An arrow no import statement backs is *absent* here rather than false:
/// a call edge or a bare package specifier knows nothing about the build,
/// and answering on its behalf is the guess this exists not to make.
/// `scored` subtracts a set, so absent and false come to the same thing —
/// the arrow is counted.
fn erasure(
    imports: &[ImportSite],
    folders: &HashSet<String>,
) -> HashMap<String, HashSet<(String, String)>> {
    let mut every: HashMap<String, HashMap<(String, String), bool>> = HashMap::new();
    for site in imports {
        let (Some(src), Some(tgt)) = (site.from.to_str(), site.to.to_str()) else {
            continue;
        };
        let (from, to, shared) = parting(src, tgt);
        let lca = from[shared - 1];
        if !folders.contains(lca) {
            continue;
        }
        let pair = (child_on(&from, shared, src), child_on(&to, shared, tgt));
        *every.entry(lca.to_string()).or_default().entry(pair).or_insert(true) &= site.is_type_only;
    }
    every
        .into_iter()
        .map(|(folder, arrows)| {
            let all: HashSet<(String, String)> = arrows
                .into_iter()
                .filter(|(_, erased)| *erased)
                .map(|(pair, _)| pair)
                .collect();
            (folder, all)
        })
        .collect()
}

/// Which child of the shared folder this endpoint sits in — the subfolder
/// one level down, or the file itself when it lives in the folder direct.
fn child_on(chain: &[&str], shared: usize, file: &str) -> String {
    chain
        .get(shared)
        .map_or_else(|| file.to_string(), |c| c.to_string())
}

/// Immediate children of each folder — the files directly inside it and
/// the subfolders directly under it, sorted and mixed in one list because
/// the drawn graph does not distinguish them.
///
/// A file holding nothing but module declarations is not among them
/// (ADR 0022). A `mod.rs` that names its siblings and says nothing else *is* the folder,
/// spelled the way the language requires; drawing it as a child put a node
/// in the picture that nothing points at and nothing points from, because
/// `mod foo;` is containment and never became an edge. It sat at level 0
/// beside the real door and cost the folder nothing, the one shape
/// `arborescence` charges for being a merge rather than a stray.
///
/// Recording `mod foo;` as an edge instead was the alternative, and it is
/// worse: the language obliges the file to name every sibling, so the
/// folder's mod.rs would become the parent of everything and every child a
/// sibling already depends on would gain a second parent. That is a merge
/// the code does not contain.
fn build_children<'a>(
    files: impl IntoIterator<Item = &'a str>,
    folders: &HashSet<String>,
    declaration_only: &HashSet<String>,
) -> HashMap<String, Vec<String>> {
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for file in files {
        if declaration_only.contains(file) {
            continue;
        }
        let parent = parent_dir(file);
        if folders.contains(parent) {
            children
                .entry(parent.to_string())
                .or_default()
                .push(file.to_string());
        }
    }
    for folder in folders {
        let parent = parent_dir(folder);
        if parent != folder.as_str() && folders.contains(parent) {
            children
                .entry(parent.to_string())
                .or_default()
                .push(folder.clone());
        }
    }
    for kids in children.values_mut() {
        kids.sort();
    }
    children
}

/// Every ancestor directory of `dir`, root first, ending with `dir`
/// itself. Slices of the original rather than components joined back
/// together, which would drop the leading separator of an absolute path.
fn ancestors(dir: &str) -> Vec<&str> {
    let mut out = vec![""];
    for (i, c) in dir.char_indices() {
        if c == MAIN_SEPARATOR && i > 0 {
            out.push(&dir[..i]);
        }
    }
    if !dir.is_empty() {
        out.push(dir);
    }
    out
}

/// Everything before the last separator — the root, spelled `""`, for a
/// path that has none.
fn parent_dir(path: &str) -> &str {
    match path.rfind(MAIN_SEPARATOR) {
        Some(i) => &path[..i],
        None => "",
    }
}

/// How many levels down a folder sits, used to score the deepest first.
fn depth(path: &str) -> usize {
    if path.is_empty() {
        return 0;
    }
    path.matches(MAIN_SEPARATOR).count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sep(path: &str) -> String {
        path.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    /// Build the folder set the way `enumerate_module_paths` does: every
    /// ancestor directory of every file.
    fn folders_of(files: &[String]) -> HashSet<String> {
        let mut out = HashSet::new();
        for file in files {
            for ancestor in ancestors(parent_dir(file)) {
                out.insert(ancestor.to_string());
            }
        }
        out
    }

    fn run(files: &[&str], pairs: &[(&str, &str)]) -> HashMap<String, FolderShape> {
        run_declaring(files, pairs, &[])
    }

    /// An import statement, as the erasure join reads it.
    fn site(from: &str, to: &str, type_only: bool) -> ImportSite {
        ImportSite {
            from: std::path::PathBuf::from(sep(from)),
            to: std::path::PathBuf::from(sep(to)),
            line: 0,
            is_reexport: false,
            is_type_only: type_only,
        }
    }

    /// As [`run`], with the import statements the dependencies were
    /// written as (ADR 0026).
    fn run_written(
        files: &[&str],
        pairs: &[(&str, &str)],
        imports: &[ImportSite],
    ) -> HashMap<String, FolderShape> {
        let files: Vec<String> = files.iter().map(|f| sep(f)).collect();
        let pairs: Vec<(String, String)> = pairs.iter().map(|(a, b)| (sep(a), sep(b))).collect();
        let folders = folders_of(&files);
        compute(
            files.iter().map(String::as_str),
            &pairs,
            imports,
            &folders,
            &HashSet::new(),
        )
    }

    /// As [`draw`], with the import statements behind the arrows.
    fn draw_written(
        files: &[&str],
        pairs: &[(&str, &str)],
        imports: &[ImportSite],
        folder: &str,
    ) -> FolderPicture {
        let files: Vec<String> = files.iter().map(|f| sep(f)).collect();
        let pairs: Vec<(String, String)> = pairs.iter().map(|(a, b)| (sep(a), sep(b))).collect();
        let folders = folders_of(&files);
        picture(
            &sep(folder),
            files.iter().map(String::as_str),
            &pairs,
            imports,
            &folders,
            &HashSet::new(),
        )
        .unwrap_or_else(|| panic!("no picture for {folder}"))
    }

    /// As [`run`], with `declaring` naming the files that hold nothing but
    /// module declarations.
    fn run_declaring(
        files: &[&str],
        pairs: &[(&str, &str)],
        declaring: &[&str],
    ) -> HashMap<String, FolderShape> {
        let files: Vec<String> = files.iter().map(|f| sep(f)).collect();
        let pairs: Vec<(String, String)> = pairs.iter().map(|(a, b)| (sep(a), sep(b))).collect();
        let folders = folders_of(&files);
        let declaring: HashSet<String> = declaring.iter().map(|f| sep(f)).collect();
        compute(
            files.iter().map(String::as_str),
            &pairs,
            &[],
            &folders,
            &declaring,
        )
    }

    fn at<'a>(shapes: &'a HashMap<String, FolderShape>, path: &str) -> &'a FolderShape {
        shapes
            .get(&sep(path))
            .unwrap_or_else(|| panic!("no shape for {path}"))
    }

    #[test]
    fn a_chain_of_files_is_a_clean_hierarchy() {
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")],
        );
        let src = at(&shapes, "src");
        assert_eq!(src.acyclicity, 1.0);
        assert_eq!(src.layering, Some(1.0));
        assert_eq!(src.child_count, 3);
        assert_eq!(src.pattern, ShapePattern::Fractal);
    }

    #[test]
    fn a_loop_between_children_reads_as_cyclic() {
        let shapes = run(
            &["src/a.rs", "src/b.rs"],
            &[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/a.rs")],
        );
        let src = at(&shapes, "src");
        assert_eq!(src.acyclicity, 0.0);
        assert_eq!(src.pattern, ShapePattern::Cyclic);
    }

    #[test]
    fn a_shortcut_past_the_middle_layer_is_tangled() {
        // a → b → c plus a → c. The shortcut jumps a level, which is the
        // edge a reader has to hold in their head while following the rest.
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/c.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
        );
        let src = at(&shapes, "src");
        assert_eq!(src.acyclicity, 1.0);
        assert_eq!(src.layering, Some(2.0 / 3.0));
        assert_eq!(src.pattern, ShapePattern::Tangled);
    }

    #[test]
    fn a_shared_dependency_is_not_a_crossing() {
        // Two files leaning on one helper is the healthy reuse shape, and
        // must not be scored like a tangle.
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/util.rs"],
            &[("src/a.rs", "src/util.rs"), ("src/b.rs", "src/util.rs")],
        );
        assert_eq!(at(&shapes, "src").layering, Some(1.0));
    }

    // --- Branching, which layering deliberately does not measure ---

    #[test]
    fn a_shared_dependency_is_a_merge_even_though_it_layers_cleanly() {
        // The whole reason arborescence exists (ADR 0013). The same two
        // edges are perfect on one measure and marked down on the other,
        // and both readings are correct: nothing skips a level, and the
        // drawing still converges instead of branching.
        //
        // ADR 0013 wrote this shape at 0.50, counting only the merge. It
        // is 1/3 under ADR 0022, which also charges the second root: `a`
        // and `b` answer to nothing, so the folder draws two trees that
        // happen to share a leaf rather than one tree. The tier does not
        // move — 0.50 was already under the 0.70 bar — and the reading it
        // gates on is unchanged.
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/util.rs"],
            &[("src/a.rs", "src/util.rs"), ("src/b.rs", "src/util.rs")],
        );
        let src = at(&shapes, "src");
        assert_eq!(src.layering, Some(1.0));
        assert_eq!(src.arborescence, Some(1.0 / 3.0));
        assert_eq!(src.pattern, ShapePattern::Hierarchical);
        assert_eq!(src.blocker, Some(ShapeBlocker::Merges(1.0 / 3.0)));
    }

    #[test]
    fn a_chain_branches_perfectly() {
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")],
        );
        assert_eq!(at(&shapes, "src").arborescence, Some(1.0));
    }

    #[test]
    fn one_merge_among_many_edges_costs_proportionally() {
        // A hub everything leans on scores far worse than a hub two files
        // lean on. Counting nodes-with-one-parent instead would score both
        // at zero and rank the folder that needs work level with the one
        // that barely does.
        let mild = run(
            &["src/a.rs", "src/b.rs", "src/c.rs", "src/hub.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/c.rs"),
                ("src/c.rs", "src/hub.rs"),
                ("src/a.rs", "src/hub.rs"),
            ],
        );
        let severe = run(
            &["src/a.rs", "src/b.rs", "src/c.rs", "src/hub.rs"],
            &[
                ("src/a.rs", "src/hub.rs"),
                ("src/b.rs", "src/hub.rs"),
                ("src/c.rs", "src/hub.rs"),
            ],
        );
        assert_eq!(at(&mild, "src").arborescence, Some(0.75));
        // 1 edge surviving a spanning tree out of 3 drawn plus 2 roots past
        // the first: the hub's three dependents answer to nothing, which is
        // the same folder read twice over.
        assert_eq!(at(&severe, "src").arborescence, Some(0.2));
        assert!(at(&severe, "src").arborescence < at(&mild, "src").arborescence);
    }

    #[test]
    fn a_child_no_edge_reaches_costs_the_same_as_a_merge() {
        // ADR 0022. Both folders draw one edge. The first is a tree with
        // one root; the second is that tree plus three files nothing in the
        // folder mentions, which is not a tree at any zoom level. Counting
        // only the nodes that have a parent scored them identically.
        let tree = run(&["src/a.rs", "src/b.rs"], &[("src/a.rs", "src/b.rs")]);
        let bag = run(
            &["src/a.rs", "src/b.rs", "src/x.rs", "src/y.rs", "src/z.rs"],
            &[("src/a.rs", "src/b.rs")],
        );
        assert_eq!(at(&tree, "src").arborescence, Some(1.0));
        assert_eq!(at(&bag, "src").arborescence, Some(0.25));
    }

    #[test]
    fn one_root_is_free_because_a_tree_has_one() {
        // The charge starts at the *second* root. A folder with a single
        // head and everything hanging off it must score a clean 1.00, or
        // the measure would mark down the shape it exists to reward.
        let shapes = run(
            &["src/mod.rs", "src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/mod.rs", "src/a.rs"),
                ("src/mod.rs", "src/b.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
        );
        assert_eq!(at(&shapes, "src").arborescence, Some(1.0));
    }

    #[test]
    fn a_file_that_only_declares_modules_is_the_folder_not_a_child() {
        // ADR 0022. `bodies/mod.rs` names its three siblings and says
        // nothing else, and `mod foo;` is containment that never becomes an
        // edge — so drawn as a child it is a second root that no edge can
        // ever reach, on a folder that is otherwise a clean tree.
        let files = &[
            "src/bodies/mod.rs",
            "src/bodies/calls.rs",
            "src/bodies/flow.rs",
            "src/bodies/stdlib.rs",
        ];
        let pairs = &[
            ("src/bodies/calls.rs", "src/bodies/flow.rs"),
            ("src/bodies/calls.rs", "src/bodies/stdlib.rs"),
        ];
        let drawn = run(files, pairs);
        let undrawn = run_declaring(files, pairs, &["src/bodies/mod.rs"]);
        assert_eq!(at(&drawn, "src/bodies").child_count, 4);
        assert_eq!(at(&drawn, "src/bodies").arborescence, Some(2.0 / 3.0));
        assert_eq!(at(&undrawn, "src/bodies").child_count, 3);
        assert_eq!(at(&undrawn, "src/bodies").arborescence, Some(1.0));
    }

    #[test]
    fn a_module_file_that_also_holds_code_stays_a_child() {
        // The exemption is for a file that is *only* declarations. One that
        // parses its own entities is a participant in the drawing, and the
        // caller reports it as such.
        let files = &["src/mod.rs", "src/a.rs", "src/b.rs"];
        let pairs = &[("src/mod.rs", "src/a.rs"), ("src/mod.rs", "src/b.rs")];
        assert_eq!(at(&run(files, pairs), "src").child_count, 3);
    }

    #[test]
    fn branching_is_unmeasured_exactly_when_layering_is() {
        // Both are ratios over the same drawn edges, so a folder may never
        // report one and withhold the other — a reader comparing them side
        // by side would be comparing different denominators.
        let shapes = run(
            &[
                "src/lone.rs",
                "src/pair/a.rs",
                "src/pair/b.rs",
                "src/loop/x.rs",
                "src/loop/y.rs",
            ],
            &[
                ("src/pair/a.rs", "src/pair/b.rs"),
                ("src/loop/x.rs", "src/loop/y.rs"),
                ("src/loop/y.rs", "src/loop/x.rs"),
            ],
        );
        assert!(!shapes.is_empty());
        for (path, shape) in &shapes {
            assert_eq!(
                shape.layering.is_none(),
                shape.arborescence.is_none(),
                "{path} reports layering {:?} beside arborescence {:?}",
                shape.layering,
                shape.arborescence,
            );
        }
    }

    #[test]
    fn a_loop_is_not_charged_as_a_merge_as_well() {
        // The cycle is condensed away before branching is counted, for the
        // reason layering condenses it: acyclicity has already charged for
        // it, and one defect must not be billed twice.
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/a.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
        );
        let src = at(&shapes, "src");
        assert_eq!(src.pattern, ShapePattern::Cyclic);
        // a and b collapse to one node, leaving a single edge into c.
        assert_eq!(src.arborescence, Some(1.0));
    }

    #[test]
    fn branching_stays_out_of_the_blended_score() {
        // ADR 0013: arborescence gates the tier and is reported beside
        // compliance, never inside it. Two folders identical but for their
        // merges must therefore blend to the same number and differ only in
        // tier — which is what lets the existing four keep their meaning.
        let branching = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")],
        );
        let merging = run(
            &["src/a.rs", "src/b.rs", "src/util.rs"],
            &[("src/a.rs", "src/util.rs"), ("src/b.rs", "src/util.rs")],
        );
        assert_eq!(
            at(&branching, "src").compliance,
            at(&merging, "src").compliance
        );
        assert_eq!(at(&branching, "src").pattern, ShapePattern::Fractal);
        assert_eq!(at(&merging, "src").pattern, ShapePattern::Hierarchical);
    }

    #[test]
    fn a_diamond_still_layers_cleanly() {
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/a.rs", "src/c.rs"),
                ("src/b.rs", "src/d.rs"),
                ("src/c.rs", "src/d.rs"),
            ],
        );
        assert_eq!(at(&shapes, "src").layering, Some(1.0));
    }

    #[test]
    fn an_edge_lands_in_the_folder_where_the_paths_part() {
        // One edge from `src/api/handler.rs` into `src/db/pool.rs`. At
        // `src` it joins the two subfolders; inside either it is invisible.
        let shapes = run(
            &["src/api/handler.rs", "src/db/pool.rs"],
            &[("src/api/handler.rs", "src/db/pool.rs")],
        );
        assert_eq!(at(&shapes, "src").child_count, 2);
        assert_eq!(at(&shapes, "src").layering, Some(1.0));
        // The endpoints are alone in their own folders, so those draw a
        // single dot each and have no edges of their own.
        assert_eq!(at(&shapes, "src/api").layering, None);
        assert_eq!(at(&shapes, "src/db").layering, None);
    }

    #[test]
    fn one_door_in_beats_many() {
        // Everything outside `db` goes through `pool.rs`.
        let concentrated = run(
            &["src/a.rs", "src/b.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[
                ("src/a.rs", "src/db/pool.rs"),
                ("src/b.rs", "src/db/pool.rs"),
            ],
        );
        assert_eq!(at(&concentrated, "src/db").entry_concentration, Some(1.0));

        // Same two callers, now reaching past the front door.
        let pierced = run(
            &["src/a.rs", "src/b.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[
                ("src/a.rs", "src/db/pool.rs"),
                ("src/b.rs", "src/db/rows.rs"),
            ],
        );
        assert_eq!(at(&pierced, "src/db").entry_concentration, Some(0.5));
        assert!(at(&pierced, "src/db").compliance < at(&concentrated, "src/db").compliance);
    }

    #[test]
    fn nothing_depending_on_a_folder_leaves_its_entry_unmeasured() {
        let shapes = run(&["src/a.rs", "src/b.rs"], &[("src/a.rs", "src/b.rs")]);
        assert_eq!(at(&shapes, "src").entry_concentration, None);
    }

    #[test]
    fn a_tangled_child_pulls_its_parent_below_fractal() {
        let shapes = run(
            &[
                "src/top.rs",
                "src/mess/a.rs",
                "src/mess/b.rs",
                "src/mess/c.rs",
            ],
            &[
                ("src/top.rs", "src/mess/a.rs"),
                ("src/mess/a.rs", "src/mess/b.rs"),
                ("src/mess/b.rs", "src/mess/c.rs"),
                ("src/mess/a.rs", "src/mess/c.rs"),
            ],
        );
        assert_eq!(at(&shapes, "src/mess").pattern, ShapePattern::Tangled);
        // `src` is a clean two-node picture on its own terms, but the
        // recursion is what stops it claiming to be fractal.
        assert_eq!(at(&shapes, "src").acyclicity, 1.0);
        assert_eq!(at(&shapes, "src").layering, Some(1.0));
        assert_eq!(at(&shapes, "src").pattern, ShapePattern::Hierarchical);
    }

    #[test]
    fn unrelated_files_are_legible_but_not_fractal() {
        // Nothing connects these, so there is no shape to be self-similar
        // with — the drawing is a row of dots.
        let shapes = run(&["src/a.rs", "src/b.rs", "src/c.rs"], &[]);
        let src = at(&shapes, "src");
        assert_eq!(src.layering, None);
        assert_eq!(src.pattern, ShapePattern::Hierarchical);
    }

    #[test]
    fn a_folder_holding_one_file_is_a_single_legible_node() {
        let shapes = run(&["src/only.rs"], &[]);
        assert_eq!(at(&shapes, "src").child_count, 1);
        assert_eq!(at(&shapes, "src").pattern, ShapePattern::Fractal);
    }

    #[test]
    fn sibling_folders_do_not_merge_on_a_shared_name_prefix() {
        // `src/parser` and `src/parsed` share a string prefix but no
        // folder. The edge between them belongs to `src`.
        let shapes = run(
            &["src/parser/a.rs", "src/parsed/b.rs"],
            &[("src/parser/a.rs", "src/parsed/b.rs")],
        );
        assert_eq!(at(&shapes, "src").child_count, 2);
        assert_eq!(at(&shapes, "src/parser").layering, None);
    }

    #[test]
    fn duplicate_edges_between_the_same_files_count_once() {
        let once = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")],
        );
        let twice = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/c.rs"),
            ],
        );
        assert_eq!(at(&once, "src").layering, at(&twice, "src").layering);
    }

    // --- The picture behind the numbers ---

    fn draw(files: &[&str], pairs: &[(&str, &str)], folder: &str) -> FolderPicture {
        draw_declaring(files, pairs, folder, &[])
    }

    fn draw_declaring(
        files: &[&str],
        pairs: &[(&str, &str)],
        folder: &str,
        declaring: &[&str],
    ) -> FolderPicture {
        let files: Vec<String> = files.iter().map(|f| sep(f)).collect();
        let pairs: Vec<(String, String)> = pairs.iter().map(|(a, b)| (sep(a), sep(b))).collect();
        let folders = folders_of(&files);
        let declaring: HashSet<String> = declaring.iter().map(|f| sep(f)).collect();
        picture(
            &sep(folder),
            files.iter().map(String::as_str),
            &pairs,
            &[],
            &folders,
            &declaring,
        )
        .unwrap_or_else(|| panic!("no picture for {folder}"))
    }

    fn verdict_of(p: &FolderPicture, from: &str, to: &str) -> EdgeVerdict {
        p.edges
            .iter()
            .find(|e| e.from == sep(from) && e.to == sep(to))
            .unwrap_or_else(|| panic!("no edge {from} -> {to}"))
            .verdict
    }

    #[test]
    fn a_folder_nobody_analysed_has_no_picture() {
        let files = vec![sep("src/a.rs")];
        let folders = folders_of(&files);
        assert!(picture(
            &sep("nope"),
            files.iter().map(String::as_str),
            &[],
            &[],
            &folders,
            &HashSet::new()
        )
        .is_none());
    }

    #[test]
    fn the_picture_draws_the_graph_the_score_is_computed_over() {
        // ADR 0012's drawn graph: immediate children, each subfolder as one
        // node. If these ever part company the number stops being a claim
        // about anything a reader can look at.
        let p = draw(
            &["src/top.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[("src/top.rs", "src/db/pool.rs")],
            "src",
        );
        let drawn: Vec<&str> = p.children.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(drawn, vec![sep("src/db"), sep("src/top.rs")]);
        let kinds: Vec<ChildKind> = p.children.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![ChildKind::Folder, ChildKind::File]);

        let shapes = run(
            &["src/top.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[("src/top.rs", "src/db/pool.rs")],
        );
        assert_eq!(at(&shapes, "src").child_count as usize, p.children.len());
    }

    #[test]
    fn an_edge_says_whether_it_steps_or_skips() {
        // The same fixture `a_shortcut_past_the_middle_layer_is_tangled`
        // scores: this is the drawing that earned it layering 2/3, and the
        // one edge the reader has to go and look at is named.
        let p = draw(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/c.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
            "src",
        );
        assert_eq!(verdict_of(&p, "src/a.rs", "src/b.rs"), EdgeVerdict::Step);
        assert_eq!(verdict_of(&p, "src/b.rs", "src/c.rs"), EdgeVerdict::Step);
        assert_eq!(verdict_of(&p, "src/a.rs", "src/c.rs"), EdgeVerdict::Skip);

        let steps = p
            .edges
            .iter()
            .filter(|e| e.verdict == EdgeVerdict::Step)
            .count();
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/c.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
        );
        // The share of stepping edges IS the layering score. Two answers to
        // one question is the failure this module is arranged to prevent.
        assert_eq!(
            at(&shapes, "src").layering,
            Some(steps as f32 / p.edges.len() as f32),
        );
    }

    #[test]
    fn an_edge_inside_a_loop_reads_as_back_and_not_as_a_skip() {
        let p = draw(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/a.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
            "src",
        );
        assert_eq!(verdict_of(&p, "src/a.rs", "src/b.rs"), EdgeVerdict::Back);
        assert_eq!(verdict_of(&p, "src/b.rs", "src/a.rs"), EdgeVerdict::Back);
        // Out of the loop and down one: an ordinary edge, and charging it
        // would bill the cycle twice exactly as the scores refuse to.
        assert_eq!(verdict_of(&p, "src/a.rs", "src/c.rs"), EdgeVerdict::Step);
        // Loop members share a row, a loop having no order to lay out.
        let level = |path: &str| {
            p.children
                .iter()
                .find(|c| c.path == sep(path))
                .unwrap()
                .level
        };
        assert_eq!(level("src/a.rs"), level("src/b.rs"));
    }

    #[test]
    fn two_separate_loops_do_not_merge_into_one() {
        // The edge between two different cycles is an ordinary edge, so
        // loop membership has to be an identity and not a flag.
        let p = draw(
            &["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/a.rs"),
                ("src/c.rs", "src/d.rs"),
                ("src/d.rs", "src/c.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
            "src",
        );
        assert_eq!(verdict_of(&p, "src/a.rs", "src/c.rs"), EdgeVerdict::Step);
    }

    #[test]
    fn an_outsider_reaching_past_the_front_door_is_a_breach() {
        let p = draw(
            &["src/a.rs", "src/b.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[
                ("src/a.rs", "src/db/pool.rs"),
                ("src/b.rs", "src/db/pool.rs"),
                ("src/a.rs", "src/db/rows.rs"),
            ],
            "src/db",
        );
        assert_eq!(p.doors, vec![sep("src/db/pool.rs")]);
        let breaches: Vec<&str> = p
            .outside
            .iter()
            .filter(|o| o.verdict == OutsideVerdict::Breach)
            .map(|o| o.inside.as_str())
            .collect();
        assert_eq!(breaches, vec![sep("src/db/rows.rs")]);
        assert_eq!(
            p.outside
                .iter()
                .filter(|o| o.verdict == OutsideVerdict::Entry)
                .count(),
            2,
        );
    }

    #[test]
    fn a_tie_for_busiest_leaves_both_files_doors() {
        // Choosing one would invent a breach the reader would go looking
        // for and not find.
        let p = draw(
            &["src/a.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[
                ("src/a.rs", "src/db/pool.rs"),
                ("src/a.rs", "src/db/rows.rs"),
            ],
            "src/db",
        );
        assert_eq!(p.doors, vec![sep("src/db/pool.rs"), sep("src/db/rows.rs")]);
        assert!(p.outside.iter().all(|o| o.verdict == OutsideVerdict::Entry));
    }

    #[test]
    fn traffic_leaving_the_folder_is_not_a_defect() {
        let p = draw(
            &["src/db/pool.rs", "src/util.rs"],
            &[("src/db/pool.rs", "src/util.rs")],
            "src/db",
        );
        assert_eq!(p.outside.len(), 1);
        assert_eq!(p.outside[0].verdict, OutsideVerdict::Exit);
        assert_eq!(p.outside[0].outside, sep("src/util.rs"));
        assert_eq!(p.outside[0].inside, sep("src/db/pool.rs"));
        assert!(p.doors.is_empty());
    }

    #[test]
    fn boundary_traffic_stops_at_one_hop() {
        // `far.rs` depends on `near.rs`, which depends into the folder.
        // Only `near.rs` is in the picture: two hops off a busy folder is
        // most of the repo, and this has to stay a narrowing.
        let p = draw(
            &["src/far.rs", "src/near.rs", "src/db/pool.rs"],
            &[
                ("src/far.rs", "src/near.rs"),
                ("src/near.rs", "src/db/pool.rs"),
            ],
            "src/db",
        );
        let outsiders: Vec<&str> = p.outside.iter().map(|o| o.outside.as_str()).collect();
        assert_eq!(outsiders, vec![sep("src/near.rs")]);
    }

    #[test]
    fn a_breach_names_the_file_and_the_circle_it_lands_on() {
        // The line attaches to the subfolder the canvas draws, and the fix
        // is in the file inside it. Reporting only one of the two makes the
        // picture unreadable or the instruction unfollowable.
        let p = draw(
            &["src/a.rs", "src/db/inner/rows.rs", "src/db/pool.rs"],
            &[("src/a.rs", "src/db/inner/rows.rs")],
            "src/db",
        );
        assert_eq!(p.outside.len(), 1);
        assert_eq!(p.outside[0].inside, sep("src/db/inner/rows.rs"));
        assert_eq!(p.outside[0].child, sep("src/db/inner"));
    }

    #[test]
    fn a_sibling_folder_sharing_a_name_prefix_is_outside() {
        // `src/parsed` is not inside `src/parse`, and a prefix test that
        // did not stop at the separator would swallow it whole.
        let p = draw(
            &["src/parse/a.rs", "src/parsed/b.rs"],
            &[("src/parsed/b.rs", "src/parse/a.rs")],
            "src/parse",
        );
        assert_eq!(p.children.len(), 1);
        assert_eq!(p.outside.len(), 1);
        assert_eq!(p.outside[0].outside, sep("src/parsed/b.rs"));
    }

    #[test]
    fn the_root_folder_draws_like_any_other() {
        // The root is spelled `""`, which every path is inside, so the
        // boundary tests have to hold for a folder with no name.
        let p = draw(&["a.rs", "src/b.rs"], &[("a.rs", "src/b.rs")], "");
        let drawn: Vec<&str> = p.children.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(drawn, vec!["a.rs", "src"]);
        assert!(p.outside.is_empty(), "nothing is outside the root");
    }

    #[test]
    fn inbound_counts_land_on_the_child_that_holds_the_file() {
        let p = draw(
            &[
                "src/a.rs",
                "src/b.rs",
                "src/db/inner/rows.rs",
                "src/db/pool.rs",
            ],
            &[
                ("src/a.rs", "src/db/inner/rows.rs"),
                ("src/b.rs", "src/db/inner/rows.rs"),
                ("src/a.rs", "src/db/pool.rs"),
            ],
            "src/db",
        );
        let inbound = |path: &str| {
            p.children
                .iter()
                .find(|c| c.path == sep(path))
                .unwrap()
                .inbound
        };
        assert_eq!(inbound("src/db/inner"), 2);
        assert_eq!(inbound("src/db/pool.rs"), 1);
        // The door is the busiest FILE, which is what entry_concentration
        // measures, and the circle holding it is marked so the drawing can
        // say which one it is.
        assert_eq!(p.doors, vec![sep("src/db/inner/rows.rs")]);
        let doors: Vec<&str> = p
            .children
            .iter()
            .filter(|c| c.is_door)
            .map(|c| c.path.as_str())
            .collect();
        assert_eq!(doors, vec![sep("src/db/inner")]);
    }

    // --- What is holding a folder back ---

    #[test]
    fn a_cyclic_folder_is_blocked_by_its_loop() {
        let shapes = run(
            &["src/a.rs", "src/b.rs"],
            &[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/a.rs")],
        );
        assert_eq!(at(&shapes, "src").blocker, Some(ShapeBlocker::Cycles(0.0)));
    }

    #[test]
    fn a_tangled_folder_is_blocked_by_its_layering() {
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[
                ("src/a.rs", "src/b.rs"),
                ("src/b.rs", "src/c.rs"),
                ("src/a.rs", "src/c.rs"),
            ],
        );
        assert_eq!(
            at(&shapes, "src").blocker,
            Some(ShapeBlocker::Layering(2.0 / 3.0))
        );
    }

    #[test]
    fn a_pierced_folder_is_blocked_by_its_doors() {
        // Two outsiders reaching two different files inside `db`, so the
        // folder is a clean picture that is not an honest single node.
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[
                ("src/a.rs", "src/db/pool.rs"),
                ("src/b.rs", "src/db/rows.rs"),
            ],
        );
        let db = at(&shapes, "src/db");
        assert_eq!(db.pattern, ShapePattern::Hierarchical);
        assert_eq!(db.blocker, Some(ShapeBlocker::Entry(0.5)));
    }

    #[test]
    fn a_parent_is_blocked_by_the_child_that_broke_the_recursion() {
        // `src` draws a clean picture of its own; the tangle is one level
        // down, and naming it is the difference between "this folder is
        // only hierarchical" and "go and look at src/mess".
        let shapes = run(
            &[
                "src/top.rs",
                "src/mess/a.rs",
                "src/mess/b.rs",
                "src/mess/c.rs",
            ],
            &[
                ("src/top.rs", "src/mess/a.rs"),
                ("src/mess/a.rs", "src/mess/b.rs"),
                ("src/mess/b.rs", "src/mess/c.rs"),
                ("src/mess/a.rs", "src/mess/c.rs"),
            ],
        );
        assert_eq!(
            at(&shapes, "src").blocker,
            Some(ShapeBlocker::ChildPattern(ShapePattern::Tangled))
        );
    }

    #[test]
    fn a_row_of_dots_is_blocked_for_having_no_structure() {
        let shapes = run(&["src/a.rs", "src/b.rs", "src/c.rs"], &[]);
        assert_eq!(at(&shapes, "src").blocker, Some(ShapeBlocker::Unstructured));
    }

    #[test]
    fn nothing_holds_a_fractal_folder_back() {
        let shapes = run(
            &["src/a.rs", "src/b.rs", "src/c.rs"],
            &[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")],
        );
        let src = at(&shapes, "src");
        assert_eq!(src.pattern, ShapePattern::Fractal);
        assert_eq!(src.blocker, None);
    }

    #[test]
    fn a_blocker_is_present_exactly_when_the_tier_is_not_fractal() {
        // The tier and the reason are derived in one walk, so no folder
        // may report a verdict its blocker disagrees with.
        let shapes = run(
            &[
                "src/top.rs",
                "src/mess/a.rs",
                "src/mess/b.rs",
                "src/mess/c.rs",
                "src/db/pool.rs",
                "src/db/rows.rs",
                "src/loop/x.rs",
                "src/loop/y.rs",
            ],
            &[
                ("src/top.rs", "src/mess/a.rs"),
                ("src/mess/a.rs", "src/mess/b.rs"),
                ("src/mess/b.rs", "src/mess/c.rs"),
                ("src/mess/a.rs", "src/mess/c.rs"),
                ("src/top.rs", "src/db/pool.rs"),
                ("src/mess/a.rs", "src/db/rows.rs"),
                ("src/loop/x.rs", "src/loop/y.rs"),
                ("src/loop/y.rs", "src/loop/x.rs"),
            ],
        );
        assert!(!shapes.is_empty());
        for (path, shape) in &shapes {
            assert_eq!(
                shape.blocker.is_none(),
                shape.pattern == ShapePattern::Fractal,
                "{path} is {:?} with blocker {:?}",
                shape.pattern,
                shape.blocker,
            );
        }
    }

    /// A clean chain of nine files steps one level down at every edge and
    /// would be fractal on the drawing alone. Breadth is the gate that
    /// says a picture nobody can take in at once is not self-similar, so
    /// it has to be able to stop a folder that passes everything else.
    #[test]
    fn a_wide_folder_is_held_back_however_cleanly_its_edges_step() {
        let files: Vec<String> = (0..9).map(|i| format!("src/wide/f{i}.rs")).collect();
        let pairs: Vec<(String, String)> = (0..8)
            .map(|i| {
                (
                    format!("src/wide/f{i}.rs"),
                    format!("src/wide/f{}.rs", i + 1),
                )
            })
            .collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let pair_refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let shapes = run(&refs, &pair_refs);
        let wide = at(&shapes, "src/wide");

        assert_eq!(wide.child_count, 9);
        assert_eq!(wide.layering, Some(1.0), "every edge steps one level");
        assert_eq!(wide.pattern, ShapePattern::Hierarchical);
        assert_eq!(wide.blocker, Some(ShapeBlocker::Breadth(9)));
    }

    /// The bar is a ceiling, not a target: a folder sitting exactly on it
    /// passes. Off-by-one here would silently demote every folder built to
    /// the number the tool prints.
    #[test]
    fn a_folder_exactly_on_the_bar_clears_it() {
        let t = Thresholds::default();
        let count = t.shape_max_children as usize;
        let files: Vec<String> = (0..count).map(|i| format!("src/fit/f{i}.rs")).collect();
        let pairs: Vec<(String, String)> = (0..count - 1)
            .map(|i| (format!("src/fit/f{i}.rs"), format!("src/fit/f{}.rs", i + 1)))
            .collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let pair_refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let shapes = run(&refs, &pair_refs);
        let fit = at(&shapes, "src/fit");

        assert_eq!(fit.child_count, t.shape_max_children);
        assert!(
            !matches!(fit.blocker, Some(ShapeBlocker::Breadth(_))),
            "a folder on the bar was blocked by it: {:?}",
            fit.blocker,
        );
    }

    /// Breadth is asked before the edge gates, so a folder that is both
    /// wide and merging is told to group first — the merge is measured
    /// over a drawing that grouping is about to redraw.
    #[test]
    fn breadth_is_reported_before_a_merge_in_the_same_folder() {
        let mut files: Vec<String> = (0..8).map(|i| format!("src/both/f{i}.rs")).collect();
        files.push("src/both/shared.rs".to_string());
        // Everything leans on one helper: arborescence is far below its bar.
        let pairs: Vec<(String, String)> = (0..8)
            .map(|i| {
                (
                    format!("src/both/f{i}.rs"),
                    "src/both/shared.rs".to_string(),
                )
            })
            .collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let pair_refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let shapes = run(&refs, &pair_refs);
        let both = at(&shapes, "src/both");

        assert!(
            both.arborescence.is_some_and(|a| a < 0.7),
            "fixture should also be merging: {:?}",
            both.arborescence,
        );
        assert_eq!(
            both.blocker,
            Some(ShapeBlocker::Breadth(9)),
            "the wide folder should be told to group before it is told to un-merge",
        );
    }

    // --------------------------------------------------------------
    //  Arrows the build erases (AN-025, ADR 0026)
    // --------------------------------------------------------------

    /// AN-022's own fixture. Two files reach one type module, both of them
    /// with `import type`, and neither reaches the bundle. Before this
    /// ticket the folder was `tangled` at `layering` 0.67; the arrows the
    /// compiler deletes were two thirds of the denominator.
    #[test]
    fn an_arrow_written_only_as_import_type_is_not_scored() {
        let files = &["src/main.ts", "src/a.ts", "src/vocab.ts"];
        let pairs = &[
            ("src/main.ts", "src/a.ts"),
            ("src/main.ts", "src/vocab.ts"),
            ("src/a.ts", "src/vocab.ts"),
        ];
        let counted = run_written(files, pairs, &[]);
        assert_eq!(at(&counted, "src").layering, Some(2.0 / 3.0));
        assert_eq!(at(&counted, "src").pattern, ShapePattern::Tangled);

        let shipped = run_written(
            files,
            pairs,
            &[
                site("src/main.ts", "src/vocab.ts", true),
                site("src/a.ts", "src/vocab.ts", true),
            ],
        );
        assert_eq!(at(&shipped, "src").layering, Some(1.0));

        // And the consequence a reviewer should see: `vocab.ts` is now a
        // child nothing reaches, so ADR 0022 charges it as a second root
        // and `arborescence` gates the folder where `layering` used to.
        // Discounting an arrow is not the same move as deleting the file
        // it points at — this fixture is three files of which one ships
        // nothing, and it says so.
        assert_eq!(at(&shipped, "src").arborescence, Some(0.5));
        assert_eq!(at(&shipped, "src").pattern, ShapePattern::Hierarchical);
    }

    /// The every-site rule, which is the whole of the correctness here.
    /// `blockReason.ts` in the browser UI imports from `types/graph` twice,
    /// three lines apart, one of the two erased. The dependency survives
    /// the build, so the arrow survives the subtraction — and the level it
    /// skips is still charged for.
    #[test]
    fn one_value_import_among_them_keeps_the_arrow() {
        let files = &["src/a.ts", "src/b.ts", "src/types.ts"];
        let pairs = &[
            ("src/a.ts", "src/b.ts"),
            ("src/b.ts", "src/types.ts"),
            ("src/a.ts", "src/types.ts"),
        ];
        let mixed = run_written(
            files,
            pairs,
            &[
                site("src/a.ts", "src/types.ts", true),
                site("src/a.ts", "src/types.ts", false),
            ],
        );
        assert_eq!(at(&mixed, "src").layering, Some(2.0 / 3.0));

        // The same fixture with the surviving line taken away is the
        // control: one statement is the whole difference.
        let all_erased = run_written(files, pairs, &[site("src/a.ts", "src/types.ts", true)]);
        assert_eq!(at(&all_erased, "src").layering, Some(1.0));
    }

    /// An arrow no import statement backs is absent, not erased. A call
    /// edge or a bare package specifier knows nothing about the build, and
    /// discounting one would be a guess.
    #[test]
    fn an_arrow_with_no_import_behind_it_is_still_scored() {
        let shapes = run_written(
            &["src/a.ts", "src/b.ts", "src/c.ts"],
            &[("src/a.ts", "src/b.ts"), ("src/a.ts", "src/c.ts")],
            &[site("src/a.ts", "src/b.ts", true)],
        );
        // `a → c` is the only arrow left, and it steps one level down.
        assert_eq!(at(&shapes, "src").layering, Some(1.0));
    }

    /// The subtraction is per *arrow*, not per file. A subfolder reached
    /// by one erased import and one ordinary one is reached.
    #[test]
    fn a_subfolder_arrow_survives_if_any_file_under_it_is_imported_for_real() {
        let shapes = run_written(
            &["src/top.rs", "src/mid.rs", "src/db/pool.rs", "src/db/rows.rs"],
            &[
                ("src/top.rs", "src/mid.rs"),
                ("src/top.rs", "src/db/pool.rs"),
                ("src/mid.rs", "src/db/rows.rs"),
            ],
            &[
                site("src/top.rs", "src/db/pool.rs", true),
                site("src/mid.rs", "src/db/rows.rs", false),
            ],
        );
        // `top → db` and `mid → db` collapse into one arrow, and one of the
        // two statements behind it ships.
        assert_eq!(at(&shapes, "src").layering, Some(1.0));
        assert_eq!(at(&shapes, "src").child_count, 3);
    }

    /// The `FolderPicture` contract: an arrow that is not counted is not
    /// drawn as though it were, and is not silently gone either.
    #[test]
    fn the_picture_lists_an_erased_arrow_apart_from_the_scored_ones() {
        let p = draw_written(
            &["src/main.ts", "src/a.ts", "src/vocab.ts"],
            &[
                ("src/main.ts", "src/a.ts"),
                ("src/main.ts", "src/vocab.ts"),
                ("src/a.ts", "src/vocab.ts"),
            ],
            &[
                site("src/main.ts", "src/vocab.ts", true),
                site("src/a.ts", "src/vocab.ts", true),
            ],
            "src",
        );
        let drawn: Vec<(&str, &str)> = p
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        assert_eq!(drawn, vec![(sep("src/main.ts").as_str(), sep("src/a.ts").as_str())]);
        let erased: Vec<(&str, &str)> = p
            .erased
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        assert_eq!(
            erased,
            vec![
                (sep("src/a.ts").as_str(), sep("src/vocab.ts").as_str()),
                (sep("src/main.ts").as_str(), sep("src/vocab.ts").as_str()),
            ],
        );
    }

    /// An import statement with no dependency edge derived from it — a
    /// side-effect import, or a name the resolver could not place — is not
    /// an arrow, so it is not listed as an erased one.
    #[test]
    fn an_import_the_drawing_never_had_is_not_listed_as_erased() {
        let p = draw_written(
            &["src/a.ts", "src/b.ts"],
            &[],
            &[site("src/a.ts", "src/b.ts", true)],
            "src",
        );
        assert!(p.edges.is_empty());
        assert!(p.erased.is_empty(), "{:?}", p.erased);
    }

    /// A loop written in `import type` is not a loop the runtime has, and
    /// `acyclicity` is measured over the same subtraction as the two
    /// ratios beside it — one filter, not one per score.
    #[test]
    fn a_loop_the_build_erases_is_not_a_cycle() {
        let shapes = run_written(
            &["src/a.ts", "src/b.ts"],
            &[("src/a.ts", "src/b.ts"), ("src/b.ts", "src/a.ts")],
            &[site("src/b.ts", "src/a.ts", true)],
        );
        let src = at(&shapes, "src");
        assert_eq!(src.acyclicity, 1.0);
        assert_eq!(src.layering, Some(1.0));
    }
}
