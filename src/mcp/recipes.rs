//! Recipes — the recognisable situations behind a blocked gate, and the
//! one move each of them asks for.
//!
//! [`super::reshape`] names the gate that capped a folder's tier and lists
//! what failed it. That is enough for a reader who already knows what the
//! listed thing *is*, and not enough for one who does not: two merge points
//! printed as two identical bullets can want opposite treatment, and the
//! generic advice attached to the gate is right about one of them.
//!
//! The elevator parser is the case this module was written from. It was
//! held back by `arborescence` with two merge points reported side by side:
//!
//! ```text
//! ast.rs   ← emit.rs, grammar.rs
//! lexer.rs ← grammar.rs, mod.rs
//! ```
//!
//! The first is a producer and a consumer over a shared syntax tree — the
//! contract between two phases of a pipeline, and not a defect at all. Both
//! remedies the gate's own prose offers for it are ones the tool forbids
//! elsewhere in the same output: splitting the tree duplicates it, and
//! routing one phase's access through the other is a re-export shim. The
//! second is a parent tokenizing purely to hand the tokens on, which is a
//! real ownership mistake and was the whole fix.
//!
//! So the classification lives here, and it is made from evidence rather
//! than from the shape of the prose:
//!
//! - **The drawing** decides [`MergeShape::PassThrough`] — a triangle is a
//!   fact about three edges and needs nothing else.
//! - **The entity graph** decides [`MergeShape::SharedContract`] — whether
//!   the dependents actually reach for the same *type* out of the shared
//!   child, which is what makes it a contract rather than a coincidence.
//!
//! The second is the one that lets this tool say "leave it alone", which
//! nothing in `reshape` could say before. A gate that is never allowed to
//! be wrong about a folder teaches agents to launder its false positives
//! into real damage.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, MAIN_SEPARATOR};

use super::format::{listed, num, MAX_LISTED};
use crate::graph::DependencyGraph;
use crate::models::{
    ChildKind, EdgeVerdict, EntityKind, FolderPicture, OutsideVerdict, PictureEdge, Thresholds,
};

/// Entity kinds that make a file a contract rather than a helper. A
/// dependent reaching for one of these is agreeing on a shape; a dependent
/// reaching for a function is borrowing a behaviour, and two files
/// borrowing the same behaviour is ordinary reuse the gate is right about.
fn is_contract_kind(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Struct
            | EntityKind::Class
            | EntityKind::Dataclass
            | EntityKind::AbstractClass
            | EntityKind::Interface
            | EntityKind::Trait
            | EntityKind::Enum
            | EntityKind::TypeAlias
    )
}

/// One entity a dependent reaches for. Carries whether it is a contract
/// kind rather than the kind itself, because `EntityKind` is not `Ord` and
/// the only question asked of it here is that one.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Used {
    name: String,
    contract: bool,
}

/// What a merge point turned out to be.
pub(super) enum MergeShape {
    /// One parent reaches the shared child both directly and through
    /// another parent. The direct edge is redundant with the routed one,
    /// so removing it costs nothing and fixes the merge.
    PassThrough { parent: String, middle: String },
    /// Independent siblings agreeing on the same types out of a shared
    /// child — a producer and a consumer over one data contract. Not a
    /// defect, and not fixable without making the code worse.
    SharedContract { types: Vec<String> },
    /// Everything else: a genuinely shared helper with no redundant path
    /// to it and no agreed type behind it.
    Plain,
}

/// Which entities of `child` each of `parents` reaches for, keyed by the
/// parent's path. Built from the entity graph rather than the folder
/// drawing, because the drawing has already collapsed every one of these
/// references down to a single edge — which is exactly the detail needed
/// to tell a shared contract from a shared junk drawer.
fn usage(
    graph: &DependencyGraph,
    root: &Path,
    child: &str,
    parents: &[String],
) -> BTreeMap<String, BTreeSet<Used>> {
    let by_id: HashMap<&str, _> = graph.entities().map(|e| (e.id.as_str(), e)).collect();
    let mut out: BTreeMap<String, BTreeSet<Used>> = BTreeMap::new();
    for rel in graph.relationships() {
        let (Some(from), Some(to)) = (
            by_id.get(rel.source_id.as_str()),
            by_id.get(rel.target_id.as_str()),
        ) else {
            continue;
        };
        if relative(&to.file_path, root) != child {
            continue;
        }
        let holder = relative(&from.file_path, root);
        if parents.contains(&holder) {
            out.entry(holder).or_default().insert(Used {
                name: to.name.clone(),
                contract: is_contract_kind(to.kind),
            });
        }
    }
    out
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Names every parent reaches for, and that at least two of them agree on.
/// The intersection rather than the union: one parent using a type says
/// nothing, two parents using the same type is the contract.
fn agreed_types(used: &BTreeMap<String, BTreeSet<Used>>) -> Vec<String> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for names in used.values() {
        for entity in names.iter().filter(|u| u.contract) {
            *seen.entry(entity.name.as_str()).or_default() += 1;
        }
    }
    seen.into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Decide what one merge point is, from the drawing first and the entity
/// graph second.
///
/// Order matters. A triangle is checked before a contract because it is
/// the stronger claim: it names a specific redundant edge and survives
/// whatever the two files are for, while a shared contract is an argument
/// about intent that the tool should only reach for once the cheap
/// structural explanation is ruled out.
pub(super) fn classify(
    p: &FolderPicture,
    graph: &DependencyGraph,
    root: &Path,
    child: &str,
    parents: &[String],
) -> MergeShape {
    for parent in parents {
        let routed = parents
            .iter()
            .find(|middle| *middle != parent && has_edge(p, parent, middle));
        if let Some(middle) = routed {
            return MergeShape::PassThrough {
                parent: parent.clone(),
                middle: middle.clone(),
            };
        }
    }
    let types = agreed_types(&usage(graph, root, child, parents));
    if types.is_empty() {
        MergeShape::Plain
    } else {
        MergeShape::SharedContract { types }
    }
}

fn has_edge(p: &FolderPicture, from: &str, to: &str) -> bool {
    p.edges.iter().any(|e| e.from == from && e.to == to)
}

/// The sibling `from` already reaches `to` through, if there is one.
///
/// This is the triangle from [`MergeShape::PassThrough`], asked of a
/// single edge so the layering gate can use the same test. It is a much
/// narrower question than "does `to` have several parents": a shared
/// target is ordinary, while a *redundant path* to it is an edge that
/// carries nothing the routed one does not, and only the second is worth
/// telling someone to remove.
pub(super) fn routed_through(p: &FolderPicture, from: &str, to: &str) -> Option<String> {
    p.edges
        .iter()
        .filter(|e| e.from == from && e.to != to)
        .map(|e| e.to.clone())
        .find(|middle| has_edge(p, middle, to))
}

/// Whether the edge `from → to` is also one of the folder's level skips,
/// which is what makes a single removal worth two gates.
fn also_skips(p: &FolderPicture, from: &str, to: &str) -> bool {
    p.edges
        .iter()
        .any(|e| e.from == from && e.to == to && e.verdict == EdgeVerdict::Skip)
}

/// One classified merge point, as the lines that go under it.
pub(super) fn merge_lines(
    p: &FolderPicture,
    graph: &DependencyGraph,
    root: &Path,
    child: &str,
    parents: &[String],
) -> Vec<String> {
    match classify(p, graph, root, child, parents) {
        MergeShape::PassThrough { parent, middle } => {
            let mut body = vec![
                header(child, parents, "pass-through producer"),
                format!(
                    "  `{parent}` reaches `{child}` directly *and* through `{middle}`. If what \
                     `{parent}` needs is only there to feed `{middle}`, give `{middle}` the job \
                     and `{parent}` stops naming `{child}`'s vocabulary at all. Check that \
                     first: two files depending on the same third one is an ordinary shape, and \
                     the path alone does not prove the direct edge is spare."
                ),
            ];
            if also_skips(p, &parent, child) {
                body.push(format!(
                    "  **This edge is also the folder's level skip.** `{parent} → {child}` is \
                     failing `layering` and `branching` at once, and one removal clears both. \
                     Start here."
                ));
            }
            body.push(
                "  The test that separates this from a re-export shim: afterwards, does the \
                 parent still name the shared child's types? If it does, nothing moved."
                    .to_string(),
            );
            body
        }
        MergeShape::SharedContract { types } => vec![
            header(child, parents, "shared contract"),
            format!(
                "  Both agree on {} out of it, and neither depends on the other. That is a \
                 producer and a consumer over one data contract, which is the correct shape \
                 for a pipeline and the shape `branching` cannot express.",
                quoted(&types),
            ),
            "  **Leave it.** Splitting the contract duplicates it; routing one dependent \
             through the other is a re-export shim. Both are on the forbidden list below, \
             and both would raise this number while making the folder worse."
                .to_string(),
        ],
        MergeShape::Plain => vec![
            header(child, parents, "shared helper"),
            "  No redundant path to it and no type its dependents agree on, so this is the \
             case the gate is describing plainly: one file serving several callers. Split it \
             if it serves them for unrelated reasons — see the usage breakdown below — and \
             otherwise leave it and clear a different merge."
                .to_string(),
        ],
    }
}

/// What each dependent actually reaches for, and whether that makes a
/// split available.
///
/// This is the measurement behind the gate's standing advice to "split a
/// helper that serves two unrelated purposes". Printing that line without
/// checking is a guess; a helper whose dependents use overlapping sets has
/// no such split in it, and an agent sent looking for one will invent it.
pub(super) fn split_lines(
    graph: &DependencyGraph,
    root: &Path,
    written: &Written,
    child: &str,
    parents: &[String],
) -> Vec<String> {
    let used = usage(graph, root, child, parents);
    if used.len() < 2 {
        return Vec::new();
    }
    let mut body = vec![
        String::new(),
        format!("What each dependent reaches for out of `{child}`:"),
        String::new(),
    ];
    let exclusive = exclusive_to_each(&used);
    for (parent, names) in &used {
        let only: &[&str] = exclusive
            .get(parent.as_str())
            .map_or(&[], |names| names.as_slice());
        body.push(format!(
            "- `{parent}` uses {} of its entities — {}{}",
            names.len(),
            if only.is_empty() {
                "nothing exclusively.".to_string()
            } else {
                format!("exclusively: {}", quoted_str(only))
            },
            written_note(written, parent, child),
        ));
    }
    body.push(String::new());
    let splittable = exclusive.values().filter(|s| !s.is_empty()).count() > 1;
    // Whether anything at all is reached for by more than one dependent.
    // This is what decides if the split dissolves the merge or merely
    // tidies it, and getting it wrong in either direction misprices the
    // work: promising a tier that will not come, or talking an agent out
    // of the one split that would have.
    let shares_nothing = used
        .values()
        .flat_map(|names| names.iter().map(|u| u.name.as_str()))
        .collect::<Vec<_>>()
        .iter()
        .all(|name| {
            used.values()
                .filter(|names| names.iter().any(|u| u.name == *name))
                .count()
                == 1
        });
    body.push(if splittable && shares_nothing {
        format!(
            "The dependents divide `{child}` completely — nothing in it is reached for by \
             more than one of them. Moving each group into the file that uses it does not \
             just tidy the folder, it dissolves this merge: the edge has nothing left to \
             carry and the gate clears."
        )
    } else if splittable {
        format!(
            "Each dependent has its own corner of `{child}`, so moving each exclusive group \
             into the file that uses it is a real change and worth making for the reading. \
             Be clear about what it buys: the edge stays, because both still need what they \
             share, so expect `branching` not to move."
        )
    } else {
        format!(
            "The dependents overlap rather than divide, so there is no split in `{child}` to \
             find. Do not manufacture one."
        )
    });
    body
}

/// For each parent, what only it uses. Computed against the union of the
/// others rather than pairwise, so a name used by two of three parents is
/// exclusive to neither.
fn exclusive_to_each(used: &BTreeMap<String, BTreeSet<Used>>) -> BTreeMap<&str, Vec<&str>> {
    let mut out = BTreeMap::new();
    for (parent, names) in used {
        let others: BTreeSet<&str> = used
            .iter()
            .filter(|(o, _)| *o != parent)
            .flat_map(|(_, n)| n.iter().map(|u| u.name.as_str()))
            .collect();
        let only: Vec<&str> = names
            .iter()
            .map(|u| u.name.as_str())
            .filter(|name| !others.contains(name))
            .collect();
        out.insert(parent.as_str(), only);
    }
    out
}

// ------------------------------------------------------------------
//  Shared vocabulary — the leaf that is not a layer
// ------------------------------------------------------------------

/// What this tool says about a definition several files need. Kept in one
/// place because two gates arrive at the same file from opposite
/// directions: [`super::reshape`]'s condensation note meets it as the
/// *result* of a fix — the file a broken loop leaves behind — while the
/// layering gate meets it as the thing standing between a folder and the
/// next rung. Worded twice, the tool would end up arguing with itself
/// about one file.
pub(super) const CONTRACT_NOT_DEFECT: &str =
    "a definition several files need is a contract rather than a defect";

/// How many siblings have to lean on a leaf before it reads as the
/// folder's vocabulary rather than as one file's helper.
///
/// Three, not two. Two dependents is the ordinary merge point
/// [`classify`] already has three readings for, and what this waiver says
/// is stronger than any of them: that the gate's own advice does not
/// apply here. The case it was written from had six.
const VOCABULARY_DEPENDENTS: usize = 3;

/// How many declarations a leaf may hold before "a handful of small
/// declarations and nothing else" stops describing what the reader will
/// open.
const VOCABULARY_DECLARATIONS: usize = 20;

/// How long one of those declarations may be. A type alias is vocabulary;
/// a long type with logic hanging off it is a file that could have a
/// layer inside it, and this must not speak for one.
const VOCABULARY_LINES: u32 = 60;

/// Whether a kind declares something rather than does something. Wider
/// than [`is_contract_kind`] by exactly one: several files agreeing on
/// one constant is the same shape as several files agreeing on one type,
/// even though a constant is not something the merge gate can call an
/// agreed *type*.
fn is_declaration_kind(kind: EntityKind) -> bool {
    is_contract_kind(kind) || matches!(kind, EntityKind::Constant)
}

/// Entities that sit in a file without being one of the things a
/// dependent reaches for: the file and module nodes standing for the file
/// itself, its imports, and the synthetic nodes a body walk leaves
/// behind. Skipped rather than counted, since a `mod tests` block or an
/// import list is not evidence either way about what the file declares.
fn is_scaffolding(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::File
            | EntityKind::Module
            | EntityKind::Import
            | EntityKind::Parameter
            | EntityKind::Branch
            | EntityKind::Loop
    )
}

/// One shared vocabulary leaf.
pub(super) struct Leaf {
    /// The siblings that depend on it.
    pub dependents: Vec<String>,
    /// What it declares, which is all it holds.
    pub declares: Vec<String>,
}

/// A folder's vocabulary leaves, keyed by child path.
pub(super) type Vocabulary = BTreeMap<String, Leaf>;

/// Children that several siblings depend on and that depend on no sibling
/// themselves, with the siblings that lean on them.
///
/// The drawing's half of the test, asked before the entity graph is
/// opened, and asked of `p.edges` rather than of the edges' kinds.
///
/// Which now means the scored drawing, so a leaf reached *only* by arrows
/// the build erases has stopped being one (ADR 0026). MCP-016 named that
/// as the consequence of discounting and it is the right one: a shared
/// vocabulary leaf is a finding about a shape the folder has, and a folder
/// whose only path to that leaf is deleted at compile time does not have
/// it. The erased arrows are listed under the drawing either way.
fn leaned_on_leaves(p: &FolderPicture) -> BTreeMap<&str, Vec<String>> {
    let mut dependents: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &p.edges {
        dependents.entry(&e.to).or_default().insert(&e.from);
    }
    for e in &p.edges {
        dependents.remove(e.from.as_str());
    }
    dependents
        .into_iter()
        .filter(|(_, from)| from.len() >= VOCABULARY_DEPENDENTS)
        .map(|(child, from)| (child, from.into_iter().map(String::from).collect()))
        .collect()
}

/// The folder's shared vocabulary leaves: high in-degree, zero
/// out-degree, and a handful of small declarations and nothing else.
///
/// Every part of it is evidence rather than intent, which is what lets
/// the guidance built on it say "neither fix applies" without inventing a
/// verdict. The drawing settles the two degrees; the entity graph settles
/// the contents, and settles them strictly — one free function at the top
/// of the file and this is a module with behaviour in it, which is
/// somewhere a layer *could* live and which the gate's ordinary advice is
/// entitled to be about.
///
/// A false positive here is expensive in a way a false negative is not: a
/// missed leaf leaves the reader with advice that is merely generic,
/// while a wrong one tells them to leave alone the very layer they should
/// have removed.
///
/// [ADR 0025](../../../docs/adr/0025-a-leaf-several-files-share-is-not-a-layer.md).
pub(super) fn vocabulary(p: &FolderPicture, graph: &DependencyGraph, root: &Path) -> Vocabulary {
    let leaves = leaned_on_leaves(p);
    if leaves.is_empty() {
        return Vocabulary::new();
    }
    // `None` once the file has been caught holding something that is not
    // a small declaration; the entry stays, so the disqualification
    // survives the rest of the scan.
    let mut declared: BTreeMap<&str, Option<Vec<String>>> =
        leaves.keys().map(|c| (*c, Some(Vec::new()))).collect();
    for e in graph.entities().filter(|e| e.parent_id.is_none()) {
        let file = relative(&e.file_path, root);
        let Some(slot) = declared.get_mut(file.as_str()) else {
            continue;
        };
        if is_scaffolding(e.kind) {
            continue;
        }
        match slot {
            Some(names) if is_declaration_kind(e.kind) && e.metrics.loc <= VOCABULARY_LINES => {
                names.push(e.name.clone());
            }
            _ => *slot = None,
        }
    }
    leaves
        .into_iter()
        .filter_map(|(child, dependents)| {
            let mut declares = declared.remove(child).flatten()?;
            if declares.is_empty() || declares.len() > VOCABULARY_DECLARATIONS {
                return None;
            }
            declares.sort();
            let leaf = Leaf {
                dependents,
                declares,
            };
            Some((child.to_string(), leaf))
        })
        .collect()
}

/// The vocabulary leaves that the listed level-skipping edges land on, as
/// the paragraphs that stand where advice would otherwise go.
pub(super) fn vocabulary_lines(
    vocab: &Vocabulary,
    graph: &DependencyGraph,
    root: &Path,
    skips: &[&PictureEdge],
) -> Vec<String> {
    let landed: BTreeSet<&str> = skips
        .iter()
        .map(|e| e.to.as_str())
        .filter(|to| vocab.contains_key(*to))
        .collect();
    let mut body = Vec::new();
    for child in landed.into_iter().take(MAX_LISTED) {
        let Some(leaf) = vocab.get(child) else {
            continue;
        };
        body.push(leaf_phrase(child, leaf));
        let check = vocabulary_check(child, &usage(graph, root, child, &leaf.dependents));
        if !check.is_empty() {
            body.push(String::new());
            body.push(check);
        }
        body.push(String::new());
    }
    body
}

/// What a vocabulary leaf is, and why the two structural fixes miss it.
///
/// Neither is refused on a technicality. Routing is refused because the
/// only way to route a *declaration* through a sibling is for that
/// sibling to re-export it, which this tool forbids two sections further
/// down; removal is refused because there is no layer under the leaf to
/// remove, so what a reader would actually be doing is relocating the
/// declarations one at a time.
fn leaf_phrase(child: &str, leaf: &Leaf) -> String {
    let count = leaf.declares.len();
    format!(
        "**`{child}` is shared vocabulary, not a layer.** {} of this folder's children \
         depend on it and it depends on none of them, and it holds {count} small \
         {} and nothing else: {}. Neither of the two fixes this gate is built around \
         reaches that. Routing a \
         dependent's use of a declaration through a sibling means that sibling \
         re-exporting it — the shim on the forbidden list below, which leaves every \
         dependent coupled to exactly what it was coupled to. And there is no layer \
         here to remove: nothing sits under `{child}`, so taking it away moves {count} \
         {} into the files that use them rather than deleting a hop. What is left is \
         the case this tool already has a sentence for: {CONTRACT_NOT_DEFECT}.",
        leaf.dependents.len(),
        if count == 1 {
            "declaration"
        } else {
            "declarations"
        },
        quoted(&leaf.declares),
        if count == 1 {
            "declaration"
        } else {
            "declarations"
        },
    )
}

/// What to check instead, measured rather than suggested.
///
/// Silent when the entity graph resolved fewer than two of the
/// dependents' uses — the same guard [`split_lines`] keeps, and for the
/// same reason: a verdict about how a file divides, drawn from one
/// dependent's imports, is invented. Saying nothing leaves the reader
/// with the paragraph above, which is true on its own.
fn vocabulary_check(child: &str, used: &BTreeMap<String, BTreeSet<Used>>) -> String {
    if used.len() < 2 {
        return String::new();
    }
    let exclusive = exclusive_to_each(used);
    let owners: Vec<&str> = exclusive
        .iter()
        .filter(|(_, only)| !only.is_empty())
        .map(|(parent, _)| *parent)
        .collect();
    if owners.len() < 2 {
        return format!(
            "What to check instead is whether this is one vocabulary or two, and here it \
             is one: every declaration in `{child}` that a dependent reaches for, another \
             reaches for too. There is no line through it to split along and no dependent \
             holding it up on its own. Leave it, and spend this gate on a skip that is \
             not this one."
        );
    }
    format!(
        "What to check instead is whether this is one vocabulary or two. {} each reach \
         for declarations no other dependent uses, so there may be a line through \
         `{child}`. Read those groups before moving anything: if they share nothing, \
         `{child}` is two vocabularies filed together and splitting it removes these \
         edges rather than re-routing them; if one of them is a single declaration a \
         single dependent uses, that definition belongs in the dependent and the edge \
         goes with it.",
        quoted_str(&owners),
    )
}

// ------------------------------------------------------------------
//  Cycles — the bottom rung, and the one that blocks every other gate
// ------------------------------------------------------------------

/// The loops in the drawing, each as its own list of children.
///
/// Read straight off the `Back` verdicts, which is exact rather than a
/// second opinion: the analyzer marks an edge `Back` precisely when both
/// of its ends sit in the same many-child cycle, so two children joined by
/// back edges are in the same loop and two children in different loops are
/// never joined by one. Following those edges *undirected* therefore
/// recovers the same grouping the analyzer already scored `acyclicity`
/// against, without re-running a decomposition that could disagree with it.
fn loops_in(p: &FolderPicture) -> Vec<Vec<String>> {
    let mut neighbours: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in p.edges.iter().filter(|e| e.verdict == EdgeVerdict::Back) {
        neighbours.entry(&e.from).or_default().push(&e.to);
        neighbours.entry(&e.to).or_default().push(&e.from);
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    for start in neighbours.keys().copied() {
        if seen.contains(start) {
            continue;
        }
        let mut group = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            group.push(node.to_string());
            stack.extend(neighbours.get(node).into_iter().flatten().copied());
        }
        group.sort();
        out.push(group);
    }
    // Biggest first: the loop holding most of the folder is the one whose
    // shape a reader is actually struggling with.
    out.sort_by_key(|g| (std::cmp::Reverse(g.len()), g.first().cloned()));
    out
}

/// The child of this folder that holds `path`, which is the file itself
/// when it sits directly inside and the subfolder otherwise. The drawing
/// collapses a subfolder to one node, so entity traffic into any file
/// beneath it is traffic on that node's edges.
fn child_holding<'a>(children: &'a [String], path: &str) -> Option<&'a str> {
    children.iter().map(String::as_str).find(|c| {
        path == *c
            || path
                .strip_prefix(*c)
                .is_some_and(|rest| rest.starts_with(MAIN_SEPARATOR))
    })
}

/// What each drawn edge actually carries: the distinct entities the source
/// child reaches for in the target child.
///
/// This is the number that separates a definition living in the wrong file
/// from the folder's real structure. One collapsed arrow can stand for a
/// single `use` of one type or for forty call sites, and the drawing shows
/// them identically — which is exactly why an agent handed a flat list of
/// loop edges cannot tell which one is cheap to cut.
fn edge_traffic(
    graph: &DependencyGraph,
    root: &Path,
    p: &FolderPicture,
) -> BTreeMap<(String, String), BTreeSet<String>> {
    let by_id: HashMap<&str, _> = graph.entities().map(|e| (e.id.as_str(), e)).collect();
    let children: Vec<String> = p.children.iter().map(|c| c.path.clone()).collect();
    let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for rel in graph.relationships() {
        let (Some(from), Some(to)) = (
            by_id.get(rel.source_id.as_str()),
            by_id.get(rel.target_id.as_str()),
        ) else {
            continue;
        };
        let src = child_holding(&children, &relative(&from.file_path, root));
        let dst = child_holding(&children, &relative(&to.file_path, root));
        if let (Some(a), Some(b)) = (src, dst) {
            if a != b {
                out.entry((a.to_string(), b.to_string()))
                    .or_default()
                    .insert(to.name.clone());
            }
        }
    }
    out
}

/// How many edges of one loop to name before summarising. Safe to truncate
/// here in a way it would not be on an unordered list: the edges are
/// ranked cheapest-to-cut first, so what falls off the end is provably the
/// part nobody should be cutting.
const CUTS_PER_LOOP: usize = 6;

/// The `Cycles` instruction: how many loops there are, and for each, which
/// edge is cheapest to cut.
pub(super) fn cycle_lines(p: &FolderPicture, graph: &DependencyGraph, root: &Path) -> Vec<String> {
    let groups = loops_in(p);
    let traffic = edge_traffic(graph, root, p);
    let back = p.edges.len()
        - p.edges
            .iter()
            .filter(|e| e.verdict != EdgeVerdict::Back)
            .count();

    let mut body = vec![format!(
        "Break the loops among these children. {} of {} edges run inside one, so the \
         drawing has no reading order at all — every other property is measured on \
         top of this one.",
        back,
        p.edges.len(),
    )];
    body.push(String::new());
    if groups.len() > 1 {
        body.push(format!(
            "**There are {} separate loops here, and clearing one does not clear the \
             others.** Each needs its own cut, and `acyclicity` stays below 1.00 — so \
             the verdict stays `cyclic` — until every one of them is broken. Plan for \
             {} changes, not one.",
            groups.len(),
            groups.len(),
        ));
        body.push(String::new());
    }
    for (i, group) in groups.iter().enumerate() {
        body.extend(loop_section(i + 1, groups.len(), group, p, &traffic));
    }
    body.push(
        "Cut the cheapest edge that is genuinely a back edge. An edge carrying one \
         entity is usually a definition sitting in the wrong file, and moving that \
         definition into a file both ends can depend on breaks the loop without \
         touching either caller. An edge carrying twenty is the folder's real \
         structure, and inverting it is a rewrite that will not survive review."
            .to_string(),
    );
    body
}

/// One loop: who is in it, and its edges ranked by what they carry.
fn loop_section(
    n: usize,
    total: usize,
    group: &[String],
    p: &FolderPicture,
    traffic: &BTreeMap<(String, String), BTreeSet<String>>,
) -> Vec<String> {
    let inside: Vec<&PictureEdge> = p
        .edges
        .iter()
        .filter(|e| e.verdict == EdgeVerdict::Back)
        .filter(|e| group.iter().any(|c| c == &e.from) && group.iter().any(|c| c == &e.to))
        .collect();
    let mut ranked: Vec<(usize, &PictureEdge)> = inside
        .iter()
        .map(|e| {
            let weight = traffic
                .get(&(e.from.clone(), e.to.clone()))
                .map_or(0, BTreeSet::len);
            (weight, *e)
        })
        .collect();
    ranked.sort_by_key(|(w, e)| (*w, e.from.clone(), e.to.clone()));

    let heading = if total > 1 {
        format!(
            "**Loop {n} of {total}** — {} children, {} edges:",
            group.len(),
            inside.len()
        )
    } else {
        format!(
            "**One loop**, {} children and {} edges:",
            group.len(),
            inside.len()
        )
    };
    let mut body = vec![heading, String::new()];
    for (weight, e) in ranked.iter().take(CUTS_PER_LOOP) {
        body.push(format!(
            "- `{} → {}` — carries {}",
            e.from,
            e.to,
            carried(*weight, traffic.get(&(e.from.clone(), e.to.clone()))),
        ));
    }
    if ranked.len() > CUTS_PER_LOOP {
        body.push(format!(
            "- … and {} heavier {}, not worth cutting before the ones above.",
            ranked.len() - CUTS_PER_LOOP,
            if ranked.len() - CUTS_PER_LOOP == 1 {
                "edge"
            } else {
                "edges"
            },
        ));
    }
    body.push(String::new());
    body
}

/// What one edge carries, naming the entities while there are few enough
/// for the name to be the actionable part.
fn carried(weight: usize, names: Option<&BTreeSet<String>>) -> String {
    if weight == 0 {
        return "nothing this analysis resolved — check it is a real dependency".to_string();
    }
    let listed: Vec<&str> = names
        .into_iter()
        .flatten()
        .map(String::as_str)
        .take(4)
        .collect();
    if weight <= 4 {
        format!(
            "{weight} {}: {}",
            if weight == 1 { "entity" } else { "entities" },
            quoted_str(&listed),
        )
    } else {
        format!("{weight} entities, including {}", quoted_str(&listed))
    }
}

// ------------------------------------------------------------------
//  What a collapsed edge is hiding
// ------------------------------------------------------------------

/// For every edge that ends at a subfolder, the files inside it that are
/// actually reached.
///
/// The drawing collapses each subfolder to one node, which is what makes
/// it readable — and also what lets a folder look tidy while a sibling
/// reaches past its door into three of its files. One arrow is drawn
/// either way.
///
/// The ladder does charge for this: a subfolder's own
/// `entry_concentration` counts sibling traffic as arriving from outside,
/// and that feeds the parent's `child_compliance`. But it arrives as a
/// mean, one level down, with nothing on the parent's own drawing to say
/// which arrow earned it. `src/parser/rust` is the case — `declarations →
/// bodies` is one line over three landing points, and the folder reads as
/// messy on the canvas while every number beside it looks well.
pub(super) fn landings(
    graph: &DependencyGraph,
    root: &Path,
    p: &FolderPicture,
) -> BTreeMap<(String, String), BTreeSet<String>> {
    let folders: BTreeSet<&str> = p
        .children
        .iter()
        .filter(|c| c.kind == ChildKind::Folder)
        .map(|c| c.path.as_str())
        .collect();
    if folders.is_empty() {
        return BTreeMap::new();
    }
    let by_id: HashMap<&str, _> = graph.entities().map(|e| (e.id.as_str(), e)).collect();
    let children: Vec<String> = p.children.iter().map(|c| c.path.clone()).collect();
    let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for rel in graph.relationships() {
        let (Some(from), Some(to)) = (
            by_id.get(rel.source_id.as_str()),
            by_id.get(rel.target_id.as_str()),
        ) else {
            continue;
        };
        let target = relative(&to.file_path, root);
        let (Some(a), Some(b)) = (
            child_holding(&children, &relative(&from.file_path, root)),
            child_holding(&children, &target),
        ) else {
            continue;
        };
        if a != b && folders.contains(b) {
            out.entry((a.to_string(), b.to_string()))
                .or_default()
                .insert(target);
        }
    }
    out
}

/// Each subfolder child's own door(s), read from its own picture.
///
/// Needed to tell a landing that arrives *at* the door from one that goes
/// round it. Without it the note can only count, and counting alone reads
/// as "none of these is the door" — which on the first real use it was
/// put to was false and was repeated back as fact.
pub(super) fn child_doors(
    graph: &DependencyGraph,
    root: &Path,
    p: &FolderPicture,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for child in p.children.iter().filter(|c| c.kind == ChildKind::Folder) {
        let absolute = root.join(&child.path).display().to_string();
        let Some(inner) = graph.folder_picture(&absolute) else {
            continue;
        };
        let doors: BTreeSet<String> = inner.relative_to(root).doors.into_iter().collect();
        out.insert(child.path.clone(), doors);
    }
    out
}

/// The note appended to one drawn edge, when the node it ends at is a
/// subfolder being entered at more than one point.
///
/// Silent for a single landing — that is a folder with a door, and saying
/// so on every line would bury the ones without.
///
/// The door is named apart from the rest. `declarations → bodies` lands on
/// `calls.rs`, which *is* `bodies`' door, and on `complexity.rs`, which is
/// not; calling the whole edge a piercing was wrong about half of it.
pub(super) fn landing_note(
    landings: &BTreeMap<(String, String), BTreeSet<String>>,
    doors: &BTreeMap<String, BTreeSet<String>>,
    from: &str,
    to: &str,
) -> String {
    let Some(files) = landings.get(&(from.to_string(), to.to_string())) else {
        return String::new();
    };
    if files.len() < 2 {
        return String::new();
    }
    landing_phrase(files, doors.get(to))
}

/// The three readings of a multi-file landing, worded apart from the
/// lookup that finds it.
///
/// Split out so the sentence can grow without the caller's shape moving —
/// the repo's complexity gate charges any increase on an existing
/// function, and it charged this one the first time the door was named.
fn landing_phrase(files: &BTreeSet<String>, doors: Option<&BTreeSet<String>>) -> String {
    let count = files.len();
    let at_door: Vec<&str> = files
        .iter()
        .filter(|f| doors.is_some_and(|d| d.contains(*f)))
        .map(String::as_str)
        .collect();
    let past: Vec<&str> = files
        .iter()
        .filter(|f| doors.is_none_or(|d| !d.contains(*f)))
        .map(String::as_str)
        .collect();
    if past.is_empty() {
        return format!(
            " — lands on {count} of its files, and every one of them is a door of that \
             folder: {}",
            quoted_str(&at_door),
        );
    }
    if at_door.is_empty() {
        return format!(
            " — **reaches past the door**, landing on {count} of its files, none of them \
             its door: {}",
            quoted_str(&past),
        );
    }
    format!(
        " — **reaches past the door**, landing on {count} of its files: {} {} its door, \
         {} {} not",
        quoted_str(&at_door),
        if at_door.len() == 1 { "is" } else { "are" },
        quoted_str(&past),
        if past.len() == 1 { "is" } else { "are" },
    )
}

// ------------------------------------------------------------------
//  Where the arrow was written
// ------------------------------------------------------------------

/// How many import statements one drawn edge cites before the rest
/// become a count. Three is enough to show that a repeated dependency is
/// repeated, and short enough that the edge stays one line.
const CITED: usize = 3;

/// One import statement as the note needs it: the file it was written in,
/// the line it was written on, and what kind of statement it was.
///
/// Ordered by file then line, which is the reading order inside one arrow.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Cite {
    from: String,
    line: usize,
    is_reexport: bool,
    is_type_only: bool,
}

/// Every drawn edge's citations, keyed by the child pair the edge joins.
pub(super) type Written = BTreeMap<(String, String), Vec<String>>;

/// The import statements behind each drawn edge, keyed by the child pair
/// they connect (AN-024).
///
/// Built the way [`landings`] is, from the same folder picture, so a note
/// and the arrow it hangs off cannot disagree about which children an
/// edge joins.
///
/// An edge with no entry here is not an error and is common: a dependency
/// created by a call or a type reference has no import statement of its
/// own, and a specifier naming a package rather than a path resolves to
/// no file. Silence is the honest answer for both — a cited line that did
/// not create the edge is the defect this exists to end.
fn edge_sites(
    graph: &DependencyGraph,
    root: &Path,
    p: &FolderPicture,
) -> BTreeMap<(String, String), Vec<Cite>> {
    let children: Vec<String> = p.children.iter().map(|c| c.path.clone()).collect();
    let mut found: BTreeMap<(String, String), Vec<Cite>> = BTreeMap::new();
    for site in graph.import_sites() {
        let (from, to) = (relative(&site.from, root), relative(&site.to, root));
        let (Some(a), Some(b)) = (
            child_holding(&children, &from),
            child_holding(&children, &to),
        ) else {
            continue;
        };
        if a == b {
            continue;
        }
        found
            .entry((a.to_string(), b.to_string()))
            .or_default()
            .push(Cite {
                from,
                line: site.line,
                is_reexport: site.is_reexport,
                is_type_only: site.is_type_only,
            });
    }
    // Reading order within one arrow is the source file and then the line
    // down it. `import_sites` is sorted by the *target* first, which is
    // the wrong key here and put line 4 above line 2.
    for sites in found.values_mut() {
        sites.sort();
    }
    found
}

/// The import statements behind each drawn edge, rendered.
pub(super) fn written(graph: &DependencyGraph, root: &Path, p: &FolderPicture) -> Written {
    edge_sites(graph, root, p)
        .into_iter()
        .map(|(pair, sites)| (pair, sites.iter().map(citation).collect()))
        .collect()
}

/// One statement, as the line it was typed on and what kind it was.
///
/// The re-export mark is the half with the teaching in it. `reshape`'s own
/// rules say a re-export shim is not a door, and an edge that arrived
/// through one reads as a direct dependency on a file the author never
/// named until the line saying otherwise is printed beside it.
///
/// The type-only mark is there for the same reason and cost the same
/// argument: an `import type` is not in the emitted bundle, and a reader
/// told only that the arrow exists has no way to tell it from one that is.
fn citation(cite: &Cite) -> String {
    format!(
        "`{}:{}`{}{}",
        cite.from,
        cite.line + 1,
        if cite.is_reexport { " (re-export)" } else { "" },
        if cite.is_type_only { " (type-only)" } else { "" },
    )
}

/// The re-exports written between one folder's children (AN-024).
///
/// Listed apart from the edges because they are not edges. A
/// `export { X } from './y'` forwards a name without using it: no entity
/// in the shim depends on anything, so the drawing draws no arrow for it
/// and the folder can read clean while a shim sits in the middle of it.
///
/// That absence is the whole problem. `reshape` puts a re-export shim on
/// its forbidden list and then shows the reader a picture in which the
/// one they just added is invisible — which is the moment the rule would
/// have taught something, missed.
pub(super) fn reexports(graph: &DependencyGraph, root: &Path, p: &FolderPicture) -> Vec<String> {
    let children: Vec<String> = p.children.iter().map(|c| c.path.clone()).collect();
    let mut out = Vec::new();
    for site in graph.import_sites().iter().filter(|s| s.is_reexport) {
        let (from, to) = (relative(&site.from, root), relative(&site.to, root));
        let (Some(a), Some(b)) = (
            child_holding(&children, &from),
            child_holding(&children, &to),
        ) else {
            continue;
        };
        if a != b {
            out.push(format!("`{}:{}` → `{}`", from, site.line + 1, to));
        }
    }
    out
}

/// The note appended to one drawn edge: where it was written.
///
/// The *edge's* location, never an endpoint's. A file's declaration line
/// and the line importing it coincide often enough that citing the target
/// entity's span reads as attribution while pointing at an unrelated
/// place — an import at the foot of one file, of a symbol declared at the
/// head of another, is two lines with nothing to do with each other.
///
/// Several statements between the same pair are all real, so the count is
/// kept rather than collapsed: a file importing one module on three lines
/// has three places to change.
pub(super) fn written_note(written: &Written, from: &str, to: &str) -> String {
    let Some(cites) = written.get(&(from.to_string(), to.to_string())) else {
        return String::new();
    };
    let shown: Vec<&str> = cites.iter().take(CITED).map(String::as_str).collect();
    let rest = cites.len() - shown.len();
    format!(
        " — written at {}{}",
        shown.join(", "),
        if rest > 0 {
            format!(", and {rest} more")
        } else {
            String::new()
        },
    )
}

// ------------------------------------------------------------------
//  Entry — the gate whose bar is not always reachable
// ------------------------------------------------------------------

/// The `Entry` instruction: which files outsiders land on, and — before
/// asking for anything — whether the bar can be reached at all.
///
/// `entry_concentration` is `busiest / total`, so its achievable values
/// are the fractions with `total` as denominator. When `total` is small
/// the ladder is coarse: two arriving dependencies can only score 0.50 or
/// 1.00, and a 0.60 bar then means "put every inbound edge on one file" —
/// which, for a folder that genuinely exports two things to two callers,
/// is only reachable through the re-export this tool forbids.
///
/// [ADR 0014](../../../docs/adr/0014-breadth-gates-fractal-and-a-gate-may-be-wrong.md)
/// established that saying so is the tool's job, and
/// [ADR 0017](../../../docs/adr/0017-a-gate-whose-bar-is-unreachable-says-so.md)
/// gave `Entry` the vocabulary.
///
/// The waiver is offered last, not first. `src/parser/rust` was this
/// instruction's founding example — `RustParser` through `mod.rs`, a type-name
/// normaliser through `type_names.rs` — and it turned out to be clearable
/// after all: the normaliser was pure string work, so it could move out of the
/// folder to `src/parser/rust_type_names.rs` and leave one door behind. Only
/// something that closes over the folder's interior is stuck there, which is
/// why the text asks about purity before conceding
/// ([ADR 0020](../../../docs/adr/0020-a-pure-function-can-leave-the-folder-it-grew-in.md)).
pub(super) fn entry_lines(p: &FolderPicture, t: &Thresholds, v: f32) -> Vec<String> {
    let inbound: Vec<u32> = p
        .children
        .iter()
        .map(|c| c.inbound)
        .filter(|n| *n > 0)
        .collect();
    let total: u32 = inbound.iter().sum();
    let busiest = inbound.iter().copied().max().unwrap_or(0);
    // Smallest count on one file that clears the bar, found by asking the
    // gate's own question at each step rather than by multiplying out.
    // `ceil(0.6 * 5)` is 4, not 3, because `0.6f32` is a shade above 0.6 —
    // and a recipe that disagrees with the gate it serves is worse than no
    // recipe.
    let needed = (0..=total)
        .find(|k| *k as f32 / total.max(1) as f32 >= t.shape_entry)
        .unwrap_or(total);

    let mut body = vec![format!(
        "Narrow the ways in. {} of the {} dependencies arriving from outside land on \
         the busiest file ({}), so the folder is not an honest single node when the \
         canvas collapses it.",
        num(Some(v)),
        total,
        p.doors.join(", "),
    )];
    body.push(String::new());
    body.extend(entry_offenders(p, busiest));
    body.push(String::new());
    if needed >= total && total > 0 {
        body.push(format!(
            "**This gate cannot be cleared here without collapsing to a single door.** \
             With {total} dependencies arriving, the only score above {:.2} is 1.00 — \
             every one of them on one file. Two mechanisms would deliver that and both \
             are dead ends: a re-export is the first item on the forbidden list below, \
             and moving the definition into the door usually re-introduces a cycle. \
             Before taking the waiver, try the third: **if what an outsider reaches for \
             is pure — closing over none of this folder's state, context or walk order \
             — move it out of the folder entirely.** Only something interior is stuck \
             here, so purity is the test, and afterwards the caller depends on a \
             standalone thing instead of on this folder's insides, which is a real \
             decoupling rather than a shim. If every arriving dependency does land on \
             something genuinely interior, then **the gate is wrong about this folder: \
             leave it, and take the tier below.**",
            t.shape_entry,
        ));
        return body;
    }
    body.push(format!(
        "The fix is to give the folder a front door that carries the traffic: {needed} \
         of the {total} arriving dependencies would have to land on one file to clear \
         {:.2}. The thing outsiders need should be reachable through that file, and \
         the files behind it should stop being part of anybody else's vocabulary. A \
         re-export that leaves every caller still coupled to the file behind it does \
         not count — see below.",
        t.shape_entry,
    ));
    body
}

/// The specific crossings to act on, or an honest account of why there are
/// none to name.
///
/// Every file tied at the busiest count is a door, so a folder whose
/// traffic splits evenly has *no* breaches — and the instruction used to
/// print "outsiders pierce it at 0 other points:" above an empty list.
fn entry_offenders(p: &FolderPicture, busiest: u32) -> Vec<String> {
    let breaches: Vec<String> = p
        .outside
        .iter()
        .filter(|o| o.verdict == OutsideVerdict::Breach)
        .map(|o| format!("{} → {}", o.outside, o.inside))
        .collect();
    if !breaches.is_empty() {
        let mut body = vec![
            format!(
                "Outsiders reach past the door at {} {}:",
                breaches.len(),
                if breaches.len() == 1 {
                    "point"
                } else {
                    "points"
                },
            ),
            String::new(),
        ];
        body.extend(listed(breaches));
        return body;
    }
    vec![format!(
        "There is no single breach to close: the traffic splits evenly, with {} files \
         tied as the busiest at {busiest} {} each. Every arriving dependency already \
         lands on a door — there are just several of them.",
        p.doors.len(),
        if busiest == 1 {
            "dependency"
        } else {
            "dependencies"
        },
    )]
}

/// The breadth gate's instruction: group, and group by the drawing.
///
/// The failure mode this is written against is an agent that reads "seven"
/// as arithmetic and files the surplus alphabetically, or deletes code to
/// get under the bar. Both clear the gate and neither makes the folder
/// readable, which would make the gate exactly the cosmetic ratchet the
/// rest of this tool exists to prevent.
pub(super) fn breadth_lines(p: &FolderPicture, t: &Thresholds, count: u32) -> Vec<String> {
    let mut body = vec![
        format!(
            "Group these {count} children into subfolders until this level holds at most {}. \
             Fractal is a claim that the shape survives being looked at from further away, \
             and a level nobody can take in at one glance has no shape to survive.",
            t.shape_max_children,
        ),
        String::new(),
        "Group by the drawing, not by the names. The edges above already say which children \
         belong together: a set that depends on each other and is reached from one of them is \
         a subfolder with a door built in, and that door is what keeps the level below this \
         one readable too."
            .to_string(),
        String::new(),
        "Expect the tier to *drop* before it rises. A new subfolder is a new child that must \
         itself be at least hierarchical, so `reshape` will start reporting on the inside of \
         what you just made. That is the gate working, not a regression to undo."
            .to_string(),
    ];
    let sources: Vec<&str> = p
        .children
        .iter()
        .filter(|c| c.level == 0)
        .map(|c| c.path.as_str())
        .collect();
    if !sources.is_empty() && sources.len() < p.children.len() {
        body.push(String::new());
        body.push(format!(
            "Nothing here depends on {}, so they are the natural roots to group *around* \
             rather than things to file away.",
            quoted_str(&sources),
        ));
    }
    body
}

/// The bullet every classification opens with. One shape for all three so
/// a reader scanning the list compares like with like, and the dependent
/// count stays where it was before the classification existed.
fn header(child: &str, parents: &[String], label: &str) -> String {
    format!(
        "- **{child}** ← {} ({} dependents) — {label}.",
        parents.join(", "),
        parents.len(),
    )
}

fn quoted(names: &[String]) -> String {
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    quoted_str(&refs)
}

fn quoted_str(names: &[&str]) -> String {
    let shown: Vec<String> = names.iter().take(6).map(|n| format!("`{n}`")).collect();
    if names.len() > 6 {
        format!("{} and {} more", shown.join(", "), names.len() - 6)
    } else {
        shown.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ChildKind, CodeEntity, ImportSite, OutsideEdge, PictureChild, PictureEdge, Position,
        Relationship, RelationshipKind, Span,
    };

    fn span(line: usize) -> Span {
        Span::new(Position::new(line, 0, 0), Position::new(line, 0, 0))
    }

    /// One entity in a file, returned with the id the graph will key it by.
    fn entity(graph: &mut DependencyGraph, file: &str, name: &str, kind: EntityKind) -> String {
        let e = CodeEntity::new(name, kind, file, span(1));
        let id = e.id.clone();
        graph.add_entity(e);
        id
    }

    fn uses(graph: &mut DependencyGraph, from: &str, to: &str) {
        graph.add_relationship(Relationship::new(from, to, RelationshipKind::UsesType));
    }

    fn child(path: &str, level: u32) -> PictureChild {
        PictureChild {
            path: path.to_string(),
            kind: ChildKind::File,
            level,
            inbound: 0,
            is_door: false,
        }
    }

    fn edge(from: &str, to: &str) -> PictureEdge {
        PictureEdge {
            from: from.to_string(),
            to: to.to_string(),
            verdict: EdgeVerdict::Step,
        }
    }

    fn picture(children: Vec<PictureChild>, edges: Vec<PictureEdge>) -> FolderPicture {
        FolderPicture {
            folder: "p".to_string(),
            children,
            edges,
            erased: Vec::new(),
            outside: Vec::new(),
            doors: Vec::new(),
        }
    }

    /// The elevator parser's `ast.rs`, in miniature: a producer and a
    /// consumer that both name the same struct out of a shared child.
    /// That is the case `branching` is wrong about, and the one verdict
    /// this module exists to be able to reach.
    #[test]
    fn two_dependents_naming_the_same_type_are_a_contract_not_a_defect() {
        let mut graph = DependencyGraph::default();
        let def_stmt = entity(&mut graph, "ast.rs", "DefStmt", EntityKind::Struct);
        let emit = entity(&mut graph, "emit.rs", "emit", EntityKind::Function);
        let parse = entity(&mut graph, "grammar.rs", "parse_file", EntityKind::Function);
        uses(&mut graph, &emit, &def_stmt);
        uses(&mut graph, &parse, &def_stmt);

        let p = picture(
            vec![
                child("emit.rs", 1),
                child("grammar.rs", 1),
                child("ast.rs", 2),
            ],
            vec![edge("emit.rs", "ast.rs"), edge("grammar.rs", "ast.rs")],
        );
        let parents = vec!["emit.rs".to_string(), "grammar.rs".to_string()];
        let shape = classify(&p, &graph, Path::new(""), "ast.rs", &parents);

        assert!(
            matches!(&shape, MergeShape::SharedContract { types } if types == &["DefStmt"]),
            "a shared type between independent siblings was not read as a contract",
        );
        let body = merge_lines(&p, &graph, Path::new(""), "ast.rs", &parents).join("\n");
        assert!(body.contains("**Leave it.**"), "{body}");
    }

    /// The same drawing, but the two dependents reach for functions rather
    /// than a type. Borrowing the same behaviour is ordinary shared-helper
    /// reuse, which the gate is right about — the waiver must not extend
    /// to it.
    #[test]
    fn two_dependents_sharing_only_functions_are_not_waived() {
        let mut graph = DependencyGraph::default();
        let helper = entity(&mut graph, "util.rs", "slugify", EntityKind::Function);
        let a = entity(&mut graph, "a.rs", "run", EntityKind::Function);
        let b = entity(&mut graph, "b.rs", "run", EntityKind::Function);
        uses(&mut graph, &a, &helper);
        uses(&mut graph, &b, &helper);

        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 0), child("util.rs", 1)],
            vec![edge("a.rs", "util.rs"), edge("b.rs", "util.rs")],
        );
        let parents = vec!["a.rs".to_string(), "b.rs".to_string()];

        assert!(matches!(
            classify(&p, &graph, Path::new(""), "util.rs", &parents),
            MergeShape::Plain
        ));
    }

    /// A triangle outranks a contract. Two dependents can agree on a type
    /// *and* one of them reach the child redundantly; the redundant edge
    /// is still removable, and saying "leave it" would strand it.
    #[test]
    fn a_redundant_path_outranks_an_agreed_type() {
        let mut graph = DependencyGraph::default();
        let token = entity(&mut graph, "lexer.rs", "Token", EntityKind::Enum);
        let m = entity(&mut graph, "mod.rs", "parse", EntityKind::Function);
        let g = entity(&mut graph, "grammar.rs", "parse_file", EntityKind::Function);
        uses(&mut graph, &m, &token);
        uses(&mut graph, &g, &token);

        let p = picture(
            vec![
                child("mod.rs", 0),
                child("grammar.rs", 1),
                child("lexer.rs", 2),
            ],
            vec![
                edge("mod.rs", "grammar.rs"),
                edge("mod.rs", "lexer.rs"),
                edge("grammar.rs", "lexer.rs"),
            ],
        );
        let parents = vec!["grammar.rs".to_string(), "mod.rs".to_string()];

        assert!(matches!(
            classify(&p, &graph, Path::new(""), "lexer.rs", &parents),
            MergeShape::PassThrough { .. }
        ));
    }

    /// The standing advice to "split a helper serving two unrelated
    /// purposes" is a measurement, not a suggestion. When the dependents
    /// divide the child cleanly the split is real; when they overlap there
    /// is nothing to find, and the tool has to say so rather than send an
    /// agent looking.
    #[test]
    fn a_split_is_only_offered_when_the_dependents_actually_divide() {
        let mut graph = DependencyGraph::default();
        let shared = entity(&mut graph, "ast.rs", "DefStmt", EntityKind::Struct);
        let for_parser = entity(&mut graph, "ast.rs", "from_keyword", EntityKind::Function);
        let for_emit = entity(&mut graph, "ast.rs", "entity_id", EntityKind::Function);
        let g = entity(&mut graph, "grammar.rs", "parse_file", EntityKind::Function);
        let e = entity(&mut graph, "emit.rs", "emit", EntityKind::Function);
        for (from, to) in [
            (&g, &shared),
            (&g, &for_parser),
            (&e, &shared),
            (&e, &for_emit),
        ] {
            uses(&mut graph, from, to);
        }

        let parents = vec!["emit.rs".to_string(), "grammar.rs".to_string()];
        let body = split_lines(&graph, Path::new(""), &BTreeMap::new(), "ast.rs", &parents).join("\n");

        assert!(body.contains("`entity_id`"), "{body}");
        assert!(body.contains("`from_keyword`"), "{body}");
        assert!(
            !body.contains("`DefStmt`"),
            "the shared type is not exclusive to anyone:\n{body}"
        );
        // The honest caveat: `DefStmt` keeps the edge alive, so this split
        // is worth making and will not move the number. An agent promised
        // a tier here will revert a good change when it does not arrive.
        assert!(body.contains("expect `branching` not to move"), "{body}");
    }

    /// The case the first end-to-end run got wrong: when the dependents
    /// share *nothing*, the split does not merely tidy the folder, it
    /// removes the edge. Telling an agent the edge stays would talk it out
    /// of the one split that clears the gate.
    #[test]
    fn a_complete_division_is_reported_as_dissolving_the_edge() {
        let mut graph = DependencyGraph::default();
        let span = entity(&mut graph, "file_info.rs", "Span", EntityKind::Struct);
        let lang = entity(&mut graph, "file_info.rs", "Language", EntityKind::Enum);
        let e = entity(&mut graph, "entity.rs", "CodeEntity", EntityKind::Struct);
        let r = entity(
            &mut graph,
            "relationship.rs",
            "Relationship",
            EntityKind::Struct,
        );
        uses(&mut graph, &e, &span);
        uses(&mut graph, &r, &lang);

        let parents = vec!["entity.rs".to_string(), "relationship.rs".to_string()];
        let body = split_lines(&graph, Path::new(""), &BTreeMap::new(), "file_info.rs", &parents).join("\n");

        assert!(body.contains("divide `file_info.rs` completely"), "{body}");
        assert!(body.contains("dissolves this merge"), "{body}");
        assert!(
            !body.contains("expect `branching` not to move"),
            "a complete split does move it:\n{body}"
        );
    }

    fn folder_child(path: &str, level: u32) -> PictureChild {
        PictureChild {
            path: path.to_string(),
            kind: ChildKind::Folder,
            level,
            inbound: 0,
            is_door: false,
        }
    }

    /// The `src/parser/rust` case as seen on the canvas: one arrow from
    /// `declarations` to `bodies` standing for three separate reaches into
    /// three different files. The collapsed drawing is what makes the
    /// folder readable and is also what lets it look tidy while a sibling
    /// walks past its door.
    #[test]
    fn an_edge_into_a_subfolder_admits_how_many_files_it_lands_on() {
        let mut graph = DependencyGraph::default();
        let calls = entity(
            &mut graph,
            "bodies/calls.rs",
            "extract_body_calls",
            EntityKind::Function,
        );
        let cx = entity(
            &mut graph,
            "bodies/complexity.rs",
            "compute",
            EntityKind::Function,
        );
        let infer = entity(
            &mut graph,
            "bodies/inference.rs",
            "TypeEnv",
            EntityKind::Struct,
        );
        let f = entity(
            &mut graph,
            "declarations/functions.rs",
            "parse_fn",
            EntityKind::Function,
        );
        for target in [&calls, &cx, &infer] {
            uses(&mut graph, &f, target);
        }

        let p = picture(
            vec![folder_child("declarations", 0), folder_child("bodies", 1)],
            vec![edge("declarations", "bodies")],
        );
        let found = landings(&graph, Path::new(""), &p);
        // No door known for `bodies` here, so nothing may be claimed to be
        // one — the note says exactly that rather than implying it.
        let note = landing_note(&found, &BTreeMap::new(), "declarations", "bodies");

        assert!(note.contains("reaches past the door"), "{note}");
        assert!(note.contains("landing on 3 of its files"), "{note}");
        assert!(note.contains("none of them its door"), "{note}");
        assert!(note.contains("bodies/calls.rs"), "{note}");
    }

    /// The case that shipped wrong: `declarations` lands on two files of
    /// `bodies`, and one of them *is* the door. Calling the whole edge a
    /// piercing was false about half of it, and the agent reading it wrote
    /// back "rather than on bodies' own entry point".
    #[test]
    fn a_landing_on_the_door_is_not_called_a_piercing() {
        let mut graph = DependencyGraph::default();
        let calls = entity(
            &mut graph,
            "bodies/calls.rs",
            "extract",
            EntityKind::Function,
        );
        let cx = entity(
            &mut graph,
            "bodies/complexity.rs",
            "compute",
            EntityKind::Function,
        );
        let f = entity(
            &mut graph,
            "declarations/functions.rs",
            "parse_fn",
            EntityKind::Function,
        );
        uses(&mut graph, &f, &calls);
        uses(&mut graph, &f, &cx);

        let p = picture(
            vec![folder_child("declarations", 0), folder_child("bodies", 1)],
            vec![edge("declarations", "bodies")],
        );
        let doors = BTreeMap::from([(
            "bodies".to_string(),
            BTreeSet::from(["bodies/calls.rs".to_string()]),
        )]);
        let note = landing_note(
            &landings(&graph, Path::new(""), &p),
            &doors,
            "declarations",
            "bodies",
        );

        assert!(note.contains("`bodies/calls.rs` is its door"), "{note}");
        assert!(note.contains("`bodies/complexity.rs` is not"), "{note}");
        assert!(!note.contains("none of them its door"), "{note}");
    }

    /// Every landing on a door is still worth reporting — that subfolder
    /// has several doors — but it is not a piercing and must not be bolded
    /// as one.
    #[test]
    fn landings_that_are_all_doors_are_reported_without_the_accusation() {
        let mut graph = DependencyGraph::default();
        let a = entity(&mut graph, "bodies/a.rs", "one", EntityKind::Function);
        let b = entity(&mut graph, "bodies/b.rs", "two", EntityKind::Function);
        let f = entity(
            &mut graph,
            "declarations/f.rs",
            "call",
            EntityKind::Function,
        );
        uses(&mut graph, &f, &a);
        uses(&mut graph, &f, &b);

        let p = picture(
            vec![folder_child("declarations", 0), folder_child("bodies", 1)],
            vec![edge("declarations", "bodies")],
        );
        let doors = BTreeMap::from([(
            "bodies".to_string(),
            BTreeSet::from(["bodies/a.rs".to_string(), "bodies/b.rs".to_string()]),
        )]);
        let note = landing_note(
            &landings(&graph, Path::new(""), &p),
            &doors,
            "declarations",
            "bodies",
        );

        assert!(note.contains("every one of them is a door"), "{note}");
        assert!(!note.contains("reaches past"), "{note}");
    }

    /// A subfolder entered at exactly one point has a door, and saying so
    /// on every such line would bury the ones that do not.
    #[test]
    fn a_single_landing_point_is_not_flagged() {
        let mut graph = DependencyGraph::default();
        let door = entity(&mut graph, "bodies/mod.rs", "run", EntityKind::Function);
        let f = entity(
            &mut graph,
            "declarations/functions.rs",
            "parse_fn",
            EntityKind::Function,
        );
        uses(&mut graph, &f, &door);

        let p = picture(
            vec![folder_child("declarations", 0), folder_child("bodies", 1)],
            vec![edge("declarations", "bodies")],
        );
        let found = landings(&graph, Path::new(""), &p);

        assert_eq!(
            landing_note(&found, &BTreeMap::new(), "declarations", "bodies"),
            ""
        );
    }

    /// An edge between two plain files has no door to walk past, and the
    /// note must stay out of the ordinary case.
    #[test]
    fn an_edge_between_files_carries_no_landing_note() {
        let mut graph = DependencyGraph::default();
        let t = entity(&mut graph, "b.rs", "Thing", EntityKind::Struct);
        let s = entity(&mut graph, "a.rs", "use_it", EntityKind::Function);
        uses(&mut graph, &s, &t);

        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 1)],
            vec![edge("a.rs", "b.rs")],
        );
        let found = landings(&graph, Path::new(""), &p);

        assert!(found.is_empty(), "no subfolders, so nothing to report");
        assert_eq!(landing_note(&found, &BTreeMap::new(), "a.rs", "b.rs"), "");
    }

    fn door(path: &str, inbound: u32) -> PictureChild {
        PictureChild {
            path: path.to_string(),
            kind: ChildKind::File,
            level: 0,
            inbound,
            is_door: true,
        }
    }

    /// Two dependencies arriving on two files can only score 0.50 or 1.00 —
    /// the 0.60 bar has no reachable value between them, so collapsing them
    /// is the only route, and a re-export is forbidden on the same page. A
    /// gate that can only be cleared by a forbidden move is wrong about the
    /// folder, and has to say so (ADR 0014 §2).
    ///
    /// The fixture is `src/parser/rust` as it stood when this shipped. That
    /// folder has since been cleared honestly — its second door was pure and
    /// left the folder (ADR 0020) — which is why the instruction now asks
    /// about purity before conceding, and why this asserts the concession is
    /// still reachable rather than that it is the first thing said.
    #[test]
    fn a_bar_reachable_only_by_one_door_is_reported_as_unclearable() {
        let mut p = picture(
            vec![door("mod.rs", 1), door("type_names.rs", 1)],
            Vec::new(),
        );
        p.doors = vec!["mod.rs".to_string(), "type_names.rs".to_string()];
        p.outside = vec![
            OutsideEdge {
                outside: "src/main.rs".to_string(),
                inside: "mod.rs".to_string(),
                child: "mod.rs".to_string(),
                verdict: OutsideVerdict::Entry,
            },
            OutsideEdge {
                outside: "src/analyzer/receiver_index.rs".to_string(),
                inside: "type_names.rs".to_string(),
                child: "type_names.rs".to_string(),
                verdict: OutsideVerdict::Entry,
            },
        ];
        let body = entry_lines(&p, &Thresholds::default(), 0.5).join("\n");

        assert!(
            body.contains("cannot be cleared here without collapsing to a single door"),
            "{body}"
        );
        assert!(
            body.contains("the gate is wrong about this \nfolder")
                || body.contains("the gate is wrong about this folder"),
            "the waiver is not offered:\n{body}"
        );
        // And it must not ask for the forbidden move.
        assert!(
            !body.contains("would have to land on one file to clear"),
            "it asked for the collapse it just called impossible:\n{body}"
        );
    }

    /// Tied doors mean every arriving dependency lands on a door, so there
    /// are no breaches. The old wording printed "outsiders pierce it at 0
    /// other points:" above an empty list.
    #[test]
    fn tied_doors_are_explained_rather_than_listed_as_nothing() {
        let mut p = picture(vec![door("a.rs", 2), door("b.rs", 2)], Vec::new());
        p.doors = vec!["a.rs".to_string(), "b.rs".to_string()];
        let body = entry_lines(&p, &Thresholds::default(), 0.5).join("\n");

        assert!(body.contains("no single breach to close"), "{body}");
        assert!(body.contains("2 files"), "{body}");
        assert!(!body.contains("pierce it at 0"), "{body}");
    }

    /// A folder with enough arriving traffic to have a reachable bar still
    /// gets the ordinary instruction, and is told how much has to move.
    #[test]
    fn a_reachable_bar_still_asks_for_the_front_door() {
        let mut p = picture(vec![door("mod.rs", 3), door("guts.rs", 2)], Vec::new());
        p.doors = vec!["mod.rs".to_string()];
        p.outside = vec![OutsideEdge {
            outside: "src/main.rs".to_string(),
            inside: "guts.rs".to_string(),
            child: "guts.rs".to_string(),
            verdict: OutsideVerdict::Breach,
        }];
        let body = entry_lines(&p, &Thresholds::default(), 0.6).join("\n");

        assert!(!body.contains("cannot be cleared"), "{body}");
        assert!(body.contains("3 of the 5 arriving dependencies"), "{body}");
        assert!(body.contains("src/main.rs → guts.rs"), "{body}");
    }

    fn back(from: &str, to: &str) -> PictureEdge {
        PictureEdge {
            from: from.to_string(),
            to: to.to_string(),
            verdict: EdgeVerdict::Back,
        }
    }

    /// The src/educator case. Two loops that share no edge: one through
    /// `mod.rs`, one through `java`/`predicate`/`rules`/`scan`. Cutting the
    /// first leaves the second, and an agent told only "21 edges are in a
    /// loop" will fix one and report the folder clean.
    #[test]
    fn separate_loops_are_reported_separately_and_counted() {
        let p = picture(
            vec![
                child("mod.rs", 1),
                child("position.rs", 1),
                child("java.rs", 1),
                child("predicate.rs", 1),
                child("rules.rs", 1),
                child("scan.rs", 1),
            ],
            vec![
                back("mod.rs", "position.rs"),
                back("position.rs", "mod.rs"),
                back("java.rs", "predicate.rs"),
                back("predicate.rs", "rules.rs"),
                back("rules.rs", "scan.rs"),
                back("scan.rs", "java.rs"),
            ],
        );
        let graph = DependencyGraph::default();
        let body = cycle_lines(&p, &graph, Path::new("")).join("\n");

        assert!(
            body.contains("There are 2 separate loops"),
            "the loops were not separated:\n{body}"
        );
        assert!(
            body.contains("clearing one does not clear the others"),
            "nothing warns that one cut is not enough:\n{body}"
        );
        assert!(
            body.contains("Loop 1 of 2") && body.contains("Loop 2 of 2"),
            "{body}"
        );
        // The four-child loop is the one a reader is struggling with, so
        // it leads.
        let (first, second) = (
            body.find("Loop 1 of 2").unwrap(),
            body.find("Loop 2 of 2").unwrap(),
        );
        assert!(body[first..second].contains("4 children"), "{body}");
    }

    /// A single loop must not be described as one of several, and must not
    /// carry the plan-for-N-changes warning.
    #[test]
    fn one_loop_is_not_dressed_up_as_many() {
        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 0)],
            vec![back("a.rs", "b.rs"), back("b.rs", "a.rs")],
        );
        let graph = DependencyGraph::default();
        let body = cycle_lines(&p, &graph, Path::new("")).join("\n");

        assert!(body.contains("**One loop**"), "{body}");
        assert!(!body.contains("separate loops"), "{body}");
    }

    /// The reason this recipe exists: one collapsed arrow can stand for a
    /// single `use` or for a whole module's worth of calls, and the drawing
    /// shows them identically. Ranking by what each carries is what turns
    /// a flat list into an instruction.
    #[test]
    fn the_cheapest_edge_to_cut_is_named_first_with_what_it_carries() {
        let mut graph = DependencyGraph::default();
        // `position.rs → mod.rs` carries one type — the `Educator` case.
        let educator = entity(&mut graph, "mod.rs", "Educator", EntityKind::Struct);
        let q = entity(&mut graph, "position.rs", "query", EntityKind::Function);
        uses(&mut graph, &q, &educator);
        // `mod.rs → position.rs` carries four — the real structure.
        for name in ["query", "Located", "Hit", "Span"] {
            let e = entity(&mut graph, "position.rs", name, EntityKind::Struct);
            let caller = entity(
                &mut graph,
                "mod.rs",
                &format!("use_{name}"),
                EntityKind::Function,
            );
            uses(&mut graph, &caller, &e);
        }

        let p = picture(
            vec![child("mod.rs", 1), child("position.rs", 1)],
            vec![back("mod.rs", "position.rs"), back("position.rs", "mod.rs")],
        );
        let body = cycle_lines(&p, &graph, Path::new("")).join("\n");

        let thin = body.find("`position.rs → mod.rs`").expect(&body);
        let thick = body.find("`mod.rs → position.rs`").expect(&body);
        assert!(
            thin < thick,
            "the expensive edge was offered first:\n{body}"
        );
        assert!(body.contains("1 entity: `Educator`"), "{body}");
        assert!(
            body.contains("carrying one entity is usually a definition sitting in the wrong file"),
            "the reason the thin edge is the cheap one is unstated:\n{body}"
        );
    }

    /// Overlapping dependents have no split in them, and inventing one is
    /// the wrapper-per-caller move the tool forbids.
    #[test]
    fn overlapping_dependents_are_told_there_is_no_split() {
        let mut graph = DependencyGraph::default();
        let shared = entity(&mut graph, "util.rs", "Config", EntityKind::Struct);
        let a = entity(&mut graph, "a.rs", "run", EntityKind::Function);
        let b = entity(&mut graph, "b.rs", "run", EntityKind::Function);
        uses(&mut graph, &a, &shared);
        uses(&mut graph, &b, &shared);

        let parents = vec!["a.rs".to_string(), "b.rs".to_string()];
        let body = split_lines(&graph, Path::new(""), &BTreeMap::new(), "util.rs", &parents).join("\n");

        assert!(body.contains("Do not manufacture one."), "{body}");
    }

    // --------------------------------------------------------------
    //  Where the arrow was written (AN-024)
    // --------------------------------------------------------------

    fn site(from: &str, to: &str, line: usize, reexport: bool) -> ImportSite {
        ImportSite {
            from: std::path::PathBuf::from(from),
            to: std::path::PathBuf::from(to),
            line,
            is_reexport: reexport,
            is_type_only: false,
        }
    }

    /// The same statement, written `import type { … }`.
    fn type_site(from: &str, to: &str, line: usize) -> ImportSite {
        ImportSite {
            is_type_only: true,
            ..site(from, to, line, false)
        }
    }

    /// A graph carrying nothing but import sites — enough to ask where an
    /// arrow was written, and built through `from_analysis` so the test
    /// runs the same join the real path does.
    fn graph_with(sites: Vec<ImportSite>) -> DependencyGraph {
        DependencyGraph::from_analysis(&crate::analyzer::AnalysisResult {
            entities: Vec::new(),
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: sites,
            warnings: Vec::new(),
        })
    }

    fn note_for(sites: Vec<ImportSite>, children: &[&str], from: &str, to: &str) -> String {
        let graph = graph_with(sites);
        let p = picture(
            children.iter().map(|c| child(c, 0)).collect(),
            vec![edge(from, to)],
        );
        let written = written(&graph, Path::new(""), &p);
        written_note(&written, from, to)
    }

    /// The line the statement was typed on, 1-based for a reader, and not
    /// the line anything it names was declared on.
    #[test]
    fn an_edge_cites_the_line_its_import_was_written_on() {
        let note = note_for(
            vec![site("main.ts", "vocab.ts", 13, false)],
            &["main.ts", "vocab.ts"],
            "main.ts",
            "vocab.ts",
        );
        assert_eq!(note, " — written at `main.ts:14`");
    }

    /// Three imports of one module are three places to change. The note
    /// keeps them rather than collapsing to the pair.
    #[test]
    fn several_imports_of_one_module_cite_several_lines() {
        let note = note_for(
            vec![
                site("main.ts", "vocab.ts", 0, false),
                site("main.ts", "vocab.ts", 4, false),
                site("main.ts", "vocab.ts", 9, false),
            ],
            &["main.ts", "vocab.ts"],
            "main.ts",
            "vocab.ts",
        );
        assert_eq!(
            note,
            " — written at `main.ts:1`, `main.ts:5`, `main.ts:10`"
        );
    }

    #[test]
    fn past_the_first_few_the_rest_become_a_count() {
        let note = note_for(
            (0..6)
                .map(|l| site("main.ts", "vocab.ts", l, false))
                .collect(),
            &["main.ts", "vocab.ts"],
            "main.ts",
            "vocab.ts",
        );
        assert!(note.ends_with(", and 3 more"), "{note}");
    }

    /// An edge made by a call has no import behind it. Silence is the
    /// honest answer; a line that did not create the edge is the defect
    /// this exists to end.
    #[test]
    fn an_edge_no_import_created_cites_nothing() {
        let note = note_for(Vec::new(), &["main.ts", "vocab.ts"], "main.ts", "vocab.ts");
        assert_eq!(note, "");
    }

    /// A statement inside a subfolder is not where the arrow into that
    /// subfolder was written.
    #[test]
    fn a_statement_inside_the_target_is_not_the_edges_own_line() {
        let note = note_for(
            vec![site("settings/settings.ts", "settings/defaults.ts", 10, true)],
            &["main.ts", "settings"],
            "main.ts",
            "settings",
        );
        assert_eq!(note, "");
    }

    /// An import landing anywhere in a collapsed subfolder still cites
    /// the line in the source file that put it there.
    #[test]
    fn an_edge_into_a_subfolder_cites_the_source_files_line() {
        let note = note_for(
            vec![site("main.ts", "settings/settings.ts", 2, false)],
            &["main.ts", "settings"],
            "main.ts",
            "settings",
        );
        assert_eq!(note, " — written at `main.ts:3`");
    }

    #[test]
    fn a_cited_re_export_is_marked_as_one() {
        let note = note_for(
            vec![site("main.ts", "vocab.ts", 3, true)],
            &["main.ts", "vocab.ts"],
            "main.ts",
            "vocab.ts",
        );
        assert_eq!(note, " — written at `main.ts:4` (re-export)");
    }

    // --------------------------------------------------------------
    //  Type-only edges (AN-022)
    // --------------------------------------------------------------

    #[test]
    fn a_cited_type_only_import_is_marked_as_one() {
        let note = note_for(
            vec![type_site("main.ts", "vocab.ts", 3)],
            &["main.ts", "vocab.ts"],
            "main.ts",
            "vocab.ts",
        );
        assert_eq!(note, " — written at `main.ts:4` (type-only)");
    }

    /// The shim case: `settings.ts` forwards a name it never uses, so no
    /// entity in it depends on anything and the drawing has no arrow to
    /// hang the citation off. The statement is still there and is still
    /// on the forbidden list.
    #[test]
    fn a_re_export_is_listed_though_the_drawing_draws_no_arrow_for_it() {
        let graph = graph_with(vec![site("settings.ts", "defaults.ts", 10, true)]);
        let p = picture(
            vec![child("settings.ts", 0), child("defaults.ts", 0)],
            Vec::new(),
        );
        assert_eq!(
            reexports(&graph, Path::new(""), &p),
            vec!["`settings.ts:11` → `defaults.ts`".to_string()]
        );
    }

    #[test]
    fn an_ordinary_import_is_not_listed_as_a_shim() {
        let graph = graph_with(vec![site("main.ts", "vocab.ts", 0, false)]);
        let p = picture(
            vec![child("main.ts", 0), child("vocab.ts", 1)],
            vec![edge("main.ts", "vocab.ts")],
        );
        assert!(reexports(&graph, Path::new(""), &p).is_empty());
    }

    /// A re-export between two files of the same subfolder is that
    /// folder's business, and this drawing does not show either of them.
    #[test]
    fn a_re_export_inside_one_child_is_not_this_folders_shim() {
        let graph = graph_with(vec![site(
            "settings/settings.ts",
            "settings/defaults.ts",
            3,
            true,
        )]);
        let p = picture(vec![child("main.ts", 0), child("settings", 1)], Vec::new());
        assert!(reexports(&graph, Path::new(""), &p).is_empty());
    }
}
