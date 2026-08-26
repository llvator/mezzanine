//! Which of a folder's children belong behind which door.
//!
//! `reshape` can tell a folder its level is too wide and that grouping is
//! the fix. It cannot say *what the groups are*, and that is the step
//! agents have been getting wrong: asked to group twenty files, they group
//! them by name — `utils/`, `types/`, `helpers/` — which cuts across the
//! drawing and leaves every edge crossing a boundary it did not cross
//! before. The folder is narrower and no more readable, which is the
//! failure `reshape`'s forbidden list already names and has had nothing to
//! offer instead.
//!
//! This computes the grouping from the drawing itself, by one rule:
//!
//! > A child that the rest of the folder can only reach *through* one
//! > other child is that child's private business, and belongs inside it.
//!
//! That is the definition of a **dominator**, so the grouping is a
//! dominator tree over the folder's own child graph, rooted at everything
//! the folder is entered by. Nothing is invented and nothing is weighted:
//! either every path to a file goes through one sibling, or it does not.
//!
//! ## Why this shape and not a better-clustered one
//!
//! Modularity clustering would put more edges inside groups. It would also
//! produce groups with several doors, because nothing in it is about
//! reachability — and a folder with several doors is the thing the whole
//! measure is against. Dominance buys the property directly: if `h`
//! dominates everything in its group, then by definition no path from
//! outside the group reaches a member without passing `h`. **Every group
//! this proposes has exactly one door, and that is a theorem about the
//! construction rather than a hope about the score.**
//!
//! The cost is that it groups less. A file two siblings both reach is
//! dominated by neither and stays where it is — which is the right answer
//! and the one `reshape` already gives in prose: a shared contract is
//! honest to leave alone.

use std::collections::{HashMap, HashSet};

use petgraph::algo::dominators::simple_fast;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::models::{ChildKind, FolderPicture, WIRING_FILES};

use super::relayout::{basename, join, Move};

/// One proposed subfolder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The child every path into this group passes through — the folder's
    /// single door once the group exists.
    pub head: String,
    /// The children moving in behind it. Never empty: a group of one is
    /// a folder around a file, which is not a grouping.
    pub behind: Vec<String>,
    /// Where they land. An existing subfolder when `head` is already one,
    /// a new folder named after `head` otherwise.
    pub into: String,
    /// Whether `head` moves too. False only when it is already the folder
    /// they are moving into.
    pub head_moves: bool,
}

impl Group {
    /// The moves that would build it.
    pub fn moves(&self) -> Vec<Move> {
        let head = self.head_moves.then(|| Move {
            what: self.head.clone(),
            into: self.into.clone(),
        });
        head.into_iter()
            .chain(self.behind.iter().map(|b| Move {
                what: b.clone(),
                into: self.into.clone(),
            }))
            .collect()
    }
}

/// Why a folder got no subfolders proposed.
///
/// Carried rather than re-inferred by the reporting layer, which was
/// getting it wrong. `layout` used to guess the reason from the picture —
/// "no edges at all" or, failing that, "everything here is reached from
/// more than one place" — and on `plugin/settings` printed the second over
/// a folder whose children each had exactly one in-folder dependent. Right
/// answer, wrong explanation, and the wrong one sends a reader looking for
/// a shared-contract problem they do not have.
///
/// The pass that made the decision is the only thing that knows which of
/// these it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoLayout {
    /// The children never mention each other, so there is no dependency
    /// structure to group along.
    NothingDrawn,
    /// Every candidate head was the folder itself — its wiring file, or
    /// its single door, which dominates everything in it by being the way
    /// in. Grouping behind one of those is the folder again with a segment
    /// added.
    OnlyTheDoor,
    /// Heads exist that are neither wiring nor the whole level, and none
    /// of them dominates anything. Every child is reached from more than
    /// one place — a folder of shared contracts.
    NothingOwned,
}

/// What one folder's drawing implies: the subfolders, largest first, or
/// why there are none.
pub struct Proposal {
    pub groups: Vec<Group>,
    /// Set exactly when `groups` is empty.
    pub why_none: Option<NoLayout>,
}

/// The subfolders this folder's own drawing says it has, largest first.
///
/// Empty when the drawing supports none — a folder of children that never
/// mention each other has no dominance to read, and the honest answer is
/// that grouping it is a judgement call this cannot make. Which kind of
/// none it was travels with the answer; see [`NoLayout`].
pub fn propose(p: &FolderPicture) -> Proposal {
    let nodes = node_list(p);
    let Some(dom) = dominance(p, &nodes) else {
        return Proposal {
            groups: Vec::new(),
            why_none: Some(NoLayout::NothingDrawn),
        };
    };
    let kinds: HashMap<&str, ChildKind> =
        p.children.iter().map(|c| (c.path.as_str(), c.kind)).collect();
    let considered = worth_grouping(&dom, &nodes);
    let mut groups: Vec<Group> = considered
        .accepted
        .iter()
        .filter_map(|&head| group_at(head, &nodes, &dom.tree, &kinds))
        .collect();
    // Largest first: a caller taking only some of these wants the ones
    // that narrow the level most, and a caller taking all of them is
    // unaffected by the order.
    groups.sort_by(|a, b| {
        b.behind
            .len()
            .cmp(&a.behind.len())
            .then_with(|| a.head.cmp(&b.head))
    });
    // Whether a head was *refused* is the discriminator, not whether any
    // survived. On a folder whose door fans out to four sections, the door
    // is refused for swallowing the level and all four sections are then
    // accepted as heads — a non-empty list, none of which owns anything.
    // Reading the surviving list alone called that a folder of shared
    // contracts, which is the opposite of what it is.
    let why_none = groups.is_empty().then_some(if considered.refused {
        NoLayout::OnlyTheDoor
    } else {
        NoLayout::NothingOwned
    });
    Proposal { groups, why_none }
}

/// The smallest number of children a folder can be left with and still
/// have a drawing worth reading: the new subfolder, and at least two
/// things beside it.
///
/// Below that, the "group" is the folder. A parent left holding one node
/// has been renamed, and a parent left holding two has had a wrapper put
/// round it — neither is a split, and both would score as one.
const MIN_CHILDREN_LEFT: usize = 3;

/// The heads that should actually become folders, descending past the
/// ones that should not.
///
/// Dominance alone proposes bad folders in two specific shapes, and both
/// are the same mistake: **the head is the folder itself rather than a
/// part of it.**
///
/// 1. **A wiring file.** `mod.rs`, `index.ts` and `__init__.py` reach
///    everything and so dominate everything — that is what wiring is. On
///    `src/parser` the untreated pass proposed `git mv src/parser/mod.rs
///    src/parser/mod/`, a rename dressed as a restructure that Rust's
///    module system will not even allow.
/// 2. **A head that swallows the level.** A folder with one door has a
///    door that dominates every file in it, so the group is the whole
///    folder. Grouping there buys a path segment and nothing else.
///
/// Descending rather than dropping is what makes the answer useful. The
/// subfolders worth having are one level further down the dominator tree
/// — what the door *fans out to* — so a rejected head is replaced by its
/// own dominator-children, which are judged by the same two rules. On a
/// folder entered at `api.ts` that fans out to `core.ts` and `util.ts`,
/// that turns "move all ten files into `api/`" into "`core/` and
/// `util/`", which is the layout somebody would have drawn by hand.
fn worth_grouping(dom: &Dominance, nodes: &[&str]) -> Considered {
    let mut accepted = Vec::new();
    let mut refused = false;
    let mut queue = dom.heads.clone();
    // Terminates because every rejection replaces a head with strictly
    // deeper nodes of a finite tree, and every node has one parent.
    while let Some(head) = queue.pop() {
        if is_wiring(nodes[head]) || swallows_the_level(head, dom, nodes.len()) {
            refused = true;
            queue.extend(dom.tree.get(&head).into_iter().flatten());
        } else {
            accepted.push(head);
        }
    }
    accepted.sort_unstable();
    Considered { accepted, refused }
}

/// The outcome of [`worth_grouping`]: the heads that stand, and whether
/// any candidate was turned away for being the folder rather than a part
/// of it.
///
/// The flag is not derivable from the list. A refused head is replaced by
/// its own dominator-children, so refusing one can *grow* the accepted
/// list — which is exactly the case that was being misreported.
struct Considered {
    accepted: Vec<usize>,
    refused: bool,
}

/// Whether a group headed here would leave the parent with too little to
/// draw.
///
/// The count is the same whether or not the head moves: a moving head
/// takes itself and its subtree out and leaves one folder node behind, and
/// a head that is already a folder stays put and absorbs its subtree.
/// Either way the parent ends up with `children − subtree` of them.
fn swallows_the_level(head: usize, dom: &Dominance, children: usize) -> bool {
    children.saturating_sub(subtree(head, &dom.tree).len()) < MIN_CHILDREN_LEFT
}

/// Whether this child is a folder's wiring rather than one of its parts.
///
/// A subfolder is never wiring however it is named: `src/index` is a
/// directory, and directories are exactly what this proposes to create.
fn is_wiring(path: &str) -> bool {
    WIRING_FILES.contains(&basename(path))
}

/// One group, or `None` when `head` dominates nothing and a folder around
/// it would hold one file.
fn group_at(
    head: usize,
    nodes: &[&str],
    tree: &HashMap<usize, Vec<usize>>,
    kinds: &HashMap<&str, ChildKind>,
) -> Option<Group> {
    let behind: Vec<String> = subtree(head, tree)
        .into_iter()
        .map(|n| nodes[n].to_string())
        .collect();
    if behind.is_empty() {
        return None;
    }
    let head = nodes[head];
    // A subfolder that already dominates its dependents is the folder the
    // grouping is asking for; proposing `uid/model/model/` around it would
    // be a rename dressed as a restructure.
    let head_moves = kinds.get(head) != Some(&ChildKind::Folder);
    let into = if head_moves {
        join(parent_of(head), stem(head))
    } else {
        head.to_string()
    };
    Some(Group {
        head: head.to_string(),
        behind,
        into,
        head_moves,
    })
}

/// The dominator tree of the folder's drawing, and the children of its
/// root — the candidate group heads.
struct Dominance {
    /// Node → the nodes it immediately dominates.
    tree: HashMap<usize, Vec<usize>>,
    /// The nodes the virtual root immediately dominates: everything the
    /// folder is entered by, plus everything reachable from more than one
    /// of them.
    heads: Vec<usize>,
}

/// Run the dominator pass, or `None` when there is no drawing to run it
/// over.
fn dominance(p: &FolderPicture, nodes: &[&str]) -> Option<Dominance> {
    if p.edges.is_empty() || nodes.len() < 2 {
        return None;
    }
    let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut graph: DiGraph<usize, ()> = DiGraph::new();
    let root = graph.add_node(usize::MAX);
    let ids: Vec<NodeIndex> = (0..nodes.len()).map(|i| graph.add_node(i)).collect();
    for e in &p.edges {
        if let (Some(&a), Some(&b)) = (index.get(e.from.as_str()), index.get(e.to.as_str())) {
            graph.add_edge(ids[a], ids[b], ());
        }
    }
    for entry in entries(p, nodes, &index) {
        graph.add_edge(root, ids[entry], ());
    }

    let doms = simple_fast(&graph, root);
    let mut tree: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut heads = Vec::new();
    for (i, &id) in ids.iter().enumerate() {
        match doms.immediate_dominator(id) {
            Some(idom) if idom == root => heads.push(i),
            Some(idom) => tree.entry(graph[idom]).or_default().push(i),
            // Unreachable from every entry, which only a cycle nothing
            // outside points at can be. It has no dominator to sit under
            // and becomes a head of its own rather than being dropped.
            None => heads.push(i),
        }
    }
    Some(Dominance { tree, heads })
}

/// Where the folder is entered: every child something outside it depends
/// on, and every child nothing inside it depends on.
///
/// The union rather than either alone. A door with siblings pointing at it
/// is still a door — traffic arrives there from outside whatever else is
/// true — and a child with no inbound edge at all is an entry even when
/// the world has never heard of it, because the drawing has to be rooted
/// somewhere or half of it is unreachable and ungroupable.
fn entries(p: &FolderPicture, nodes: &[&str], index: &HashMap<&str, usize>) -> Vec<usize> {
    let mut reached: HashSet<usize> = HashSet::new();
    for e in &p.edges {
        if let Some(&to) = index.get(e.to.as_str()) {
            reached.insert(to);
        }
    }
    let doors: HashSet<&str> = p
        .children
        .iter()
        .filter(|c| c.inbound > 0)
        .map(|c| c.path.as_str())
        .collect();
    (0..nodes.len())
        .filter(|i| !reached.contains(i) || doors.contains(nodes[*i]))
        .collect()
}

/// Everything under `head` in the dominator tree, `head` excluded.
///
/// Iterative because the tree is as deep as the folder's longest
/// dependency chain, which on a tangled folder is every child in a row.
fn subtree(head: usize, tree: &HashMap<usize, Vec<usize>>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack: Vec<usize> = tree.get(&head).cloned().unwrap_or_default();
    while let Some(node) = stack.pop() {
        out.push(node);
        if let Some(kids) = tree.get(&node) {
            stack.extend(kids);
        }
    }
    out.sort_unstable();
    out
}

/// The children in the drawing, in a fixed order so two runs over one
/// folder propose the same layout.
fn node_list(p: &FolderPicture) -> Vec<&str> {
    let mut nodes: Vec<&str> = p.children.iter().map(|c| c.path.as_str()).collect();
    nodes.sort_unstable();
    nodes
}

/// The folder holding `path`.
fn parent_of(path: &str) -> &str {
    let cut = path.len() - basename(path).len();
    path[..cut].trim_end_matches(std::path::MAIN_SEPARATOR)
}

/// A file's name without its extension — the folder it would name.
fn stem(path: &str) -> &str {
    let name = basename(path);
    name.split_once('.').map_or(name, |(front, _)| front)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EdgeVerdict, PictureChild, PictureEdge};

    fn p(s: &str) -> String {
        s.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    fn child(path: &str, inbound: u32, kind: ChildKind) -> PictureChild {
        PictureChild {
            path: p(path),
            kind,
            level: 0,
            inbound,
            is_door: inbound > 0,
        }
    }

    fn edge(from: &str, to: &str) -> PictureEdge {
        PictureEdge {
            from: p(from),
            to: p(to),
            verdict: EdgeVerdict::Step,
        }
    }

    fn picture(children: Vec<PictureChild>, edges: Vec<PictureEdge>) -> FolderPicture {
        FolderPicture {
            folder: p("src"),
            children,
            edges,
            erased: Vec::new(),
            outside: Vec::new(),
            doors: Vec::new(),
        }
    }

    /// `note.ts` is the only way to `noteStore.ts` and `noteText.ts`, so
    /// both are its private business. The folder is deliberately wider
    /// than the group: a grouping that leaves the parent with nothing
    /// beside it is a rename, and is refused by `MIN_CHILDREN_LEFT`.
    #[test]
    fn a_child_reached_only_through_one_sibling_goes_inside_it() {
        let pic = picture(
            vec![
                child("src/api.ts", 3, ChildKind::File),
                child("src/note.ts", 0, ChildKind::File),
                child("src/noteStore.ts", 0, ChildKind::File),
                child("src/noteText.ts", 0, ChildKind::File),
                child("src/log.ts", 0, ChildKind::File),
                child("src/clock.ts", 0, ChildKind::File),
            ],
            vec![
                edge("src/api.ts", "src/note.ts"),
                edge("src/api.ts", "src/log.ts"),
                edge("src/api.ts", "src/clock.ts"),
                edge("src/note.ts", "src/noteStore.ts"),
                edge("src/note.ts", "src/noteText.ts"),
            ],
        );
        let groups = propose(&pic).groups;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].head, p("src/note.ts"));
        assert_eq!(
            groups[0].behind,
            vec![p("src/noteStore.ts"), p("src/noteText.ts")]
        );
        assert_eq!(groups[0].into, p("src/note"));
        assert!(groups[0].head_moves);
    }

    /// Dominance is transitive: a chain collapses into one group, not a
    /// nest of one-file folders.
    #[test]
    fn a_chain_behind_one_door_becomes_one_group() {
        let pic = picture(
            vec![
                child("src/api.ts", 2, ChildKind::File),
                child("src/a.ts", 0, ChildKind::File),
                child("src/b.ts", 0, ChildKind::File),
                child("src/c.ts", 0, ChildKind::File),
                child("src/log.ts", 0, ChildKind::File),
                child("src/clock.ts", 0, ChildKind::File),
            ],
            vec![
                edge("src/api.ts", "src/a.ts"),
                edge("src/api.ts", "src/log.ts"),
                edge("src/api.ts", "src/clock.ts"),
                edge("src/a.ts", "src/b.ts"),
                edge("src/b.ts", "src/c.ts"),
            ],
        );
        let groups = propose(&pic).groups;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].head, p("src/a.ts"));
        assert_eq!(groups[0].behind, vec![p("src/b.ts"), p("src/c.ts")]);
    }

    /// A subfolder that already dominates its dependents is the folder
    /// being asked for; the members move into it rather than into a new
    /// one named after it.
    #[test]
    fn an_existing_subfolder_head_absorbs_rather_than_being_wrapped() {
        let pic = picture(
            vec![
                child("src/api.ts", 4, ChildKind::File),
                child("src/store", 0, ChildKind::Folder),
                child("src/keys.ts", 0, ChildKind::File),
                child("src/log.ts", 0, ChildKind::File),
                child("src/clock.ts", 0, ChildKind::File),
            ],
            vec![
                edge("src/api.ts", "src/store"),
                edge("src/api.ts", "src/log.ts"),
                edge("src/api.ts", "src/clock.ts"),
                edge("src/store", "src/keys.ts"),
            ],
        );
        let groups = propose(&pic).groups;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].into, p("src/store"));
        assert!(!groups[0].head_moves);
        assert_eq!(groups[0].moves().len(), 1);
    }

    /// A folder with one door has a door that dominates everything in it.
    /// Grouping there produces the folder again with a segment added, so
    /// the pass descends past it — and when there is nothing left below
    /// that is worth grouping, it proposes nothing rather than a wrapper.
    #[test]
    fn a_group_that_would_be_the_whole_folder_is_refused() {
        let pic = picture(
            vec![
                child("src/api.ts", 2, ChildKind::File),
                child("src/a.ts", 0, ChildKind::File),
                child("src/b.ts", 0, ChildKind::File),
            ],
            vec![edge("src/api.ts", "src/a.ts"), edge("src/a.ts", "src/b.ts")],
        );
        assert!(propose(&pic).groups.is_empty());
    }

    /// `mod.rs` reaches everything by being the folder's wiring, so it
    /// dominates everything. `git mv src/mod.rs src/mod/` is a rename
    /// dressed as a restructure — and in Rust, not even legal.
    #[test]
    fn a_wiring_file_never_heads_a_group() {
        let pic = picture(
            vec![
                child("src/mod.rs", 2, ChildKind::File),
                child("src/a.rs", 0, ChildKind::File),
                child("src/b.rs", 0, ChildKind::File),
                child("src/c.rs", 0, ChildKind::File),
                child("src/d.rs", 0, ChildKind::File),
                child("src/e.rs", 0, ChildKind::File),
            ],
            vec![
                edge("src/mod.rs", "src/a.rs"),
                edge("src/mod.rs", "src/b.rs"),
                edge("src/mod.rs", "src/e.rs"),
                edge("src/a.rs", "src/c.rs"),
                edge("src/a.rs", "src/d.rs"),
            ],
        );
        let groups = propose(&pic).groups;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].head, p("src/a.rs"));
        assert!(groups
            .iter()
            .flat_map(Group::moves)
            .all(|m| m.what != p("src/mod.rs")));
    }

    #[test]
    fn a_child_two_entries_reach_stays_where_it_is() {
        let pic = picture(
            vec![
                child("src/a.ts", 1, ChildKind::File),
                child("src/b.ts", 1, ChildKind::File),
                child("src/types.ts", 0, ChildKind::File),
            ],
            vec![
                edge("src/a.ts", "src/types.ts"),
                edge("src/b.ts", "src/types.ts"),
            ],
        );
        assert!(propose(&pic).groups.is_empty());
    }

    /// The reason a door-shaped folder gets no layout, which the reporting
    /// layer was getting wrong. `SettingsTab.ts` fans out to four sections
    /// and each has exactly one in-folder dependent — so the door is
    /// refused for swallowing the level, the four sections are accepted as
    /// heads, and none of them owns anything. A non-empty accepted list
    /// over a folder that is the opposite of shared contracts.
    #[test]
    fn a_door_that_fans_out_is_not_a_folder_of_shared_contracts() {
        let pic = picture(
            vec![
                child("src/SettingsTab.ts", 2, ChildKind::File),
                child("src/generator.ts", 0, ChildKind::File),
                child("src/clearing.ts", 0, ChildKind::File),
                child("src/autoGeneration.ts", 0, ChildKind::File),
                child("src/copyFormatSection.ts", 0, ChildKind::File),
            ],
            vec![
                edge("src/SettingsTab.ts", "src/generator.ts"),
                edge("src/SettingsTab.ts", "src/clearing.ts"),
                edge("src/SettingsTab.ts", "src/autoGeneration.ts"),
                edge("src/SettingsTab.ts", "src/copyFormatSection.ts"),
            ],
        );
        let out = propose(&pic);
        assert!(out.groups.is_empty());
        assert_eq!(out.why_none, Some(NoLayout::OnlyTheDoor));
    }

    /// And the case that really is shared contracts still reads as one:
    /// two entries, both reaching one file neither owns.
    #[test]
    fn two_entries_sharing_a_leaf_reads_as_shared_contracts() {
        let pic = picture(
            vec![
                child("src/a.ts", 1, ChildKind::File),
                child("src/b.ts", 1, ChildKind::File),
                child("src/types.ts", 0, ChildKind::File),
            ],
            vec![
                edge("src/a.ts", "src/types.ts"),
                edge("src/b.ts", "src/types.ts"),
            ],
        );
        let out = propose(&pic);
        assert!(out.groups.is_empty());
        assert_eq!(out.why_none, Some(NoLayout::NothingOwned));
    }

    #[test]
    fn a_folder_whose_children_never_mention_each_other_is_left_alone() {
        let pic = picture(
            vec![
                child("src/a.ts", 1, ChildKind::File),
                child("src/b.ts", 1, ChildKind::File),
            ],
            Vec::new(),
        );
        assert!(propose(&pic).groups.is_empty());
    }

    /// Two files in a loop that nothing enters are unreachable from every
    /// entry. They must survive as heads rather than vanish from the
    /// layout.
    #[test]
    fn a_closed_loop_nothing_enters_is_still_accounted_for() {
        let pic = picture(
            vec![
                child("src/a.ts", 0, ChildKind::File),
                child("src/b.ts", 0, ChildKind::File),
                child("src/c.ts", 1, ChildKind::File),
                child("src/d.ts", 0, ChildKind::File),
            ],
            vec![
                edge("src/a.ts", "src/b.ts"),
                edge("src/b.ts", "src/a.ts"),
                edge("src/c.ts", "src/d.ts"),
            ],
        );
        let named: HashSet<String> = propose(&pic)
            .groups
            .iter()
            .flat_map(|g| g.moves())
            .map(|m| m.what)
            .collect();
        assert!(named.contains(&p("src/d.ts")));
    }
}
