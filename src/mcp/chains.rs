//! One walk, two directions, one outline (MCP-047).
//!
//! Two tools want the same thing and neither could have it. `cost` needs the
//! call tree *under* an entry point, because almost no real cost lives in one
//! body — the shape that bites is a cheap-looking function calling a
//! cheap-looking helper from inside a loop, three frames down. `impact` needs
//! the caller tree *above* a target, because "at depth 3" tells a reader how
//! far a change reaches and not *through what*, which is the part that names
//! the frames they have to open.
//!
//! Those are the same walk with the arrows reversed, so this is one walker
//! rather than two. What it produces is a **tree**, not a set: every node
//! remembers the node it was reached from, which is the fact the existing
//! `transitive_dependents` throws away as it walks.
//!
//! ## What a hop carries
//!
//! [`Site`] is the piece no other walk in the tool set keeps. Call edges are
//! re-sourced onto the enclosing `Branch`/`Loop` node wherever the parser
//! emits body scopes, so the graph already knows a call happens inside a
//! loop — and every tool then lifts that away
//! ([`super::tools::lifted`]), because for every other question a call in a
//! loop counts exactly like one at the top. For a cost chain it does not: the
//! loops a call sits inside are the multiplier. This walk reads the edge
//! *before* the lift and records the loops it left.
//!
//! A caveat that rides along with it: an empty [`Site::loops`] means "at the
//! top of the body" only where the parser emits loop nodes at all. Rust and
//! Groovy emit none, so every site there reads as empty whether or not it is
//! (RS-001). Nothing here can tell those apart — the consumer has to, which
//! is what [`super::cost`]'s `placed` gate is for.

use std::collections::HashSet;

use crate::graph::DependencyGraph;
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind};

use super::tools::{is_body_scope, is_listed, lifted, parent_of};

/// How deep a scope tree is followed before giving up. A body cut into
/// scopes is never this deep, and a chain that points at itself is cheaper
/// to cap than to prove impossible — the bound [`super::tools`] uses for
/// the same walk.
const MAX_SCOPE_DEPTH: usize = 64;

/// Nodes one walk may emit before it stops expanding.
///
/// A call tree is exponential in the fan-out: twenty callees each calling
/// twenty is four hundred frames at two hops, and a consumer that prices
/// every frame pays per node. The walk stops rather than finishing, and says
/// it stopped — a truncation a reader is told about is a narrower answer, and
/// one they are not told about is a wrong one.
const BUDGET: usize = 240;

/// Which way the arrows point.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    /// Along callees — the call tree under an entry point.
    Out,
    /// Along callers — the blast radius above a target.
    In,
}

/// Which edges a walk is allowed to cross, and what it may land on.
///
/// The two questions disagree about what a hop *is*. A cost composes
/// through frames that run, so only a `Calls` edge onto something with a
/// body carries it — a function merely named (`UsesFn`) never runs, and a
/// type has no body to charge. A blast radius is about what breaks, and a
/// contract change breaks the function that takes the target as a parameter
/// type (`UsesType`) and the module that imports it every bit as much as the
/// body that calls it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// `Calls` only, onto functions and methods.
    Calls,
    /// Every dependency edge, onto anything a listing may name.
    Any,
}

impl Reach {
    fn crosses(self, kind: RelationshipKind) -> bool {
        match self {
            Reach::Calls => kind == RelationshipKind::Calls,
            Reach::Any => true,
        }
    }

    /// A cost or a call chain composes through bodies, so a call edge
    /// landing on a type or a module is a frame with nothing to compose. A
    /// radius lands on whatever a reader can be sent to open.
    fn lands_on(self, e: &CodeEntity) -> bool {
        match self {
            Reach::Calls => matches!(e.kind, EntityKind::Function | EntityKind::Method),
            Reach::Any => is_listed(e),
        }
    }
}

/// Whether an entity reached twice is drawn twice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Revisit {
    /// Once, the first time it is reached, so the chain shown is *a*
    /// shortest route to it and the tree stays small.
    Once,
    /// On every branch that reaches it. Costs more and is what a question
    /// about cost needs: a second route to the same helper can run through
    /// two more loops than the first, and first-visit-wins would report the
    /// cheap one.
    PerPath,
}

/// Where the edge into a node was written, in the body that wrote it.
#[derive(Clone, Default)]
pub(crate) struct Site {
    /// 1-based header lines of the loops enclosing the call site, outermost
    /// first. Empty at the top of a body — **and** empty for every site in a
    /// language whose parser emits no loop nodes. See the module header.
    pub(crate) loops: Vec<usize>,
    /// 1-based line of the body scope the call was written in.
    pub(crate) line: usize,
}

/// One frame of the tree: an entity, and how it was reached.
///
/// Named `Hop` rather than `Node`, which is what it is. Every parser in this
/// tree binds tree-sitter's `Node`, and an unqualified type name resolves to
/// whatever entity happens to share it (AN-039) — so a `Node` here collects
/// eight hundred phantom dependents and reads as the most coupled struct in
/// the repository. The rename is free and the collision is not.
pub(crate) struct Hop<'g> {
    pub(crate) entity: &'g CodeEntity,
    /// Index of the frame this one was reached from. `None` for the root.
    pub(crate) parent: Option<usize>,
    /// Hops from the root.
    pub(crate) depth: usize,
    pub(crate) site: Site,
    /// The edge this frame was reached through, so a renderer can say what
    /// kind of dependency it is rather than only that there is one. `None`
    /// for the root.
    pub(crate) edge: Option<&'g Relationship>,
    /// The member of the previous frame the edge actually landed on, when
    /// it was not the frame itself. A caller of `Foo::bar` depends on
    /// `Foo`, and *through `bar`* is the half of that a reader can open.
    pub(crate) through: Option<&'g CodeEntity>,
    /// This frame re-enters a body already open on its own chain, so the
    /// walk stopped here rather than going round the ring.
    pub(crate) cycle: bool,
    /// Call targets leaving this body that mezz could not bind. The walk
    /// cannot follow them, so any answer built on it is a floor.
    pub(crate) unbound: usize,
}

/// One neighbour, and everything the walk learned on the way to it.
struct Lead<'g> {
    entity: &'g CodeEntity,
    site: Site,
    edge: Option<&'g Relationship>,
    through: Option<&'g CodeEntity>,
}

/// How far to walk and how, carried as one value rather than four
/// parameters threaded through the recursion.
///
/// Built through the named constructors rather than assembled at the call
/// site: the combinations are questions, not settings, and a walk that
/// crosses every edge *and* revisits per path is nobody's question.
#[derive(Clone, Copy)]
pub(crate) struct Plan {
    direction: Direction,
    max_depth: usize,
    revisit: Revisit,
    reach: Reach,
}

impl Plan {
    /// Every route to every frame under an entry point — what a cost
    /// composes over, where the second route to a helper can be the
    /// expensive one.
    pub(crate) fn cost_chain(max_depth: usize) -> Self {
        Plan {
            direction: Direction::Out,
            max_depth,
            revisit: Revisit::PerPath,
            reach: Reach::Calls,
        }
    }

    /// The call tree under an entity, each callable named once.
    pub(crate) fn call_tree(max_depth: usize) -> Self {
        Plan {
            direction: Direction::Out,
            max_depth,
            revisit: Revisit::Once,
            reach: Reach::Calls,
        }
    }

    /// What breaks above a target: one shortest route per reached entity,
    /// across every kind of dependency edge.
    pub(crate) fn blast_radius(max_depth: usize) -> Self {
        Plan {
            direction: Direction::In,
            max_depth,
            revisit: Revisit::Once,
            reach: Reach::Any,
        }
    }
}

/// The tree, flat: `nodes[0]` is the root, and a child's index is always
/// greater than its parent's, so index order is a topological order.
pub(crate) struct Walk<'g> {
    pub(crate) nodes: Vec<Hop<'g>>,
    pub(crate) children: Vec<Vec<usize>>,
    /// The walk hit [`BUDGET`] and stopped expanding.
    pub(crate) truncated: bool,
}

/// One rendered row: which frame, and the outline number that says where it
/// hangs.
pub(crate) struct Row {
    pub(crate) at: usize,
    /// `1`, `1.1`, `1.2.3` — assigned by walking the tree depth-first, so
    /// the number says where the row hangs rather than only how far.
    pub(crate) number: String,
    pub(crate) depth: usize,
}

impl<'g> Walk<'g> {
    pub(crate) fn of(graph: &'g DependencyGraph, root: &'g CodeEntity, plan: Plan) -> Self {
        let mut walk = Walk {
            nodes: vec![Hop {
                entity: root,
                parent: None,
                depth: 0,
                site: Site::default(),
                edge: None,
                through: None,
                cycle: false,
                unbound: 0,
            }],
            children: vec![Vec::new()],
            truncated: false,
        };
        let mut seen: HashSet<&str> = HashSet::from([root.id.as_str()]);
        walk.expand(graph, 0, plan, &mut seen);
        walk
    }

    /// One frame's neighbours, recorded and then followed.
    ///
    /// The neighbours are read even at `max_depth`, where none of them is
    /// followed, because [`Hop::unbound`] is a property of the leaf's body
    /// and a leaf that stops beside four unbindable calls is exactly what a
    /// "this total is a floor" note needs to name.
    fn expand(
        &mut self,
        graph: &'g DependencyGraph,
        at: usize,
        plan: Plan,
        seen: &mut HashSet<&'g str>,
    ) {
        let (entity, depth) = (self.nodes[at].entity, self.nodes[at].depth);
        let (neighbours, unbound) = step(graph, entity, plan);
        self.nodes[at].unbound = unbound;
        if depth >= plan.max_depth {
            return;
        }
        for lead in neighbours {
            if self.nodes.len() >= BUDGET {
                self.truncated = true;
                return;
            }
            let cycle = self.on_chain(at, &lead.entity.id);
            if !cycle && plan.revisit == Revisit::Once && !seen.insert(lead.entity.id.as_str()) {
                continue;
            }
            let child = self.push(at, lead, cycle);
            if !cycle {
                self.expand(graph, child, plan, seen);
            }
        }
    }

    fn push(&mut self, at: usize, lead: Lead<'g>, cycle: bool) -> usize {
        let child = self.nodes.len();
        self.nodes.push(Hop {
            entity: lead.entity,
            parent: Some(at),
            depth: self.nodes[at].depth + 1,
            site: lead.site,
            edge: lead.edge,
            through: lead.through,
            cycle,
            unbound: 0,
        });
        self.children.push(Vec::new());
        self.children[at].push(child);
        child
    }

    /// Whether `id` is already open on the chain ending at `at` — mutual
    /// recursion, seen from inside the walk.
    fn on_chain(&self, at: usize, id: &str) -> bool {
        self.ancestors(at).iter().any(|i| self.nodes[*i].entity.id == id)
    }

    /// `at` and every frame above it, nearest first.
    pub(crate) fn ancestors(&self, at: usize) -> Vec<usize> {
        let mut out = vec![at];
        let mut cur = at;
        while let Some(parent) = self.nodes[cur].parent {
            out.push(parent);
            cur = parent;
        }
        out
    }

    /// Order siblings by a key the caller supplies.
    ///
    /// A knob rather than a rule, because the two questions want opposite
    /// orders: a cost chain reads worst-first, and a blast radius reads in
    /// the order the call sites appear.
    pub(crate) fn sort_children_by_key<K: Ord>(&mut self, key: impl Fn(usize) -> K) {
        for kids in &mut self.children {
            kids.sort_by_key(|i| key(*i));
        }
    }

    /// Drop every frame outside `keep` from the rendering, leaving the tree
    /// itself intact. Ancestors of a kept frame must be kept too, or the
    /// branch is unreachable from the root and simply vanishes.
    pub(crate) fn prune(&mut self, keep: &HashSet<usize>) {
        for kids in &mut self.children {
            kids.retain(|i| keep.contains(i));
        }
    }

    /// The tree depth-first, numbered.
    pub(crate) fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        let mut stack = vec![(0usize, "1".to_string())];
        while let Some((at, number)) = stack.pop() {
            out.push(Row { at, number: number.clone(), depth: self.nodes[at].depth });
            for (i, child) in self.children[at].iter().enumerate().rev() {
                stack.push((*child, format!("{number}.{}", i + 1)));
            }
        }
        out
    }
}

/// One frame's neighbours in the asked-for direction, and how many of its
/// call targets mezz could not bind.
fn step<'g>(
    graph: &'g DependencyGraph,
    entity: &'g CodeEntity,
    plan: Plan,
) -> (Vec<Lead<'g>>, usize) {
    match plan.direction {
        Direction::Out => callees(graph, entity, plan.reach),
        Direction::In => (callers(graph, entity, plan.reach), 0),
    }
}

/// The callables this body calls, each with the loops its call site sits in.
///
/// Read from the entity *and* the branch and loop nodes its body is cut
/// into: a call written inside an `if` leaves from the branch and never
/// touches the callable's own id, so gathering is the only way to see it
/// (see [`super::tools::body_scope_ids`], which is this walk without the
/// depths).
///
/// `Calls` only. `UsesFn` is a function named and not called at that site,
/// so charging it the enclosing loops would be fiction — the loop runs, the
/// named function does not.
fn callees<'g>(
    graph: &'g DependencyGraph,
    entity: &'g CodeEntity,
    reach: Reach,
) -> (Vec<Lead<'g>>, usize) {
    let mut found = Vec::new();
    let mut unbound = 0;
    for (id, site) in scopes_of(graph, entity) {
        for (dep, rel) in graph.dependencies(&id) {
            if !reach.crosses(rel.kind) {
                continue;
            }
            match dep.tags.contains("ghost") {
                true => unbound += 1,
                false if reach.lands_on(dep) && dep.id != entity.id => found.push(Lead {
                    entity: dep,
                    site: site.clone(),
                    edge: Some(rel),
                    through: None,
                }),
                false => {}
            }
        }
    }
    (found, unbound)
}

/// What depends on this entity, each with the loops the call site sits in
/// *within the dependent*.
///
/// A dependent whose call is inside a branch or a loop arrives as that scope
/// node and is lifted to the callable a reader can open — the same rule
/// `impact` applies, with the lifted-away fact kept beside it rather than
/// discarded.
///
/// Members are read as well as the entity itself, because parsers emit no
/// type-usage edge for most languages: what depends on a class is mostly the
/// callers of its methods, and a walk that reads only the class's own
/// dependents reports a type nothing uses. Which member was landed on rides
/// along on the lead — it is the part a reader can open.
fn callers<'g>(graph: &'g DependencyGraph, entity: &'g CodeEntity, reach: Reach) -> Vec<Lead<'g>> {
    let mut found: Vec<Lead<'g>> = Vec::new();
    let members = match reach {
        Reach::Any => graph.children(&entity.id),
        Reach::Calls => Vec::new(),
    };
    for landing in std::iter::once(entity).chain(members) {
        for (src, rel) in graph.dependents(&landing.id) {
            if !reach.crosses(rel.kind) {
                continue;
            }
            let Some(dependent) = lifted(graph, src) else { continue };
            // The entity's own members are not its dependents: a method
            // calling a sibling method is the type using itself, and a row
            // saying so is one nobody can act on.
            if !reach.lands_on(dependent) || dependent.id == entity.id || owns(graph, entity, dependent) {
                continue;
            }
            found.push(Lead {
                entity: dependent,
                site: site_under(graph, src, &dependent.id),
                edge: Some(rel),
                through: (landing.id != entity.id).then_some(landing),
            });
        }
    }
    one_lead_per_dependent(found)
}

/// Whether `e` is declared inside `owner`.
fn owns(graph: &DependencyGraph, owner: &CodeEntity, e: &CodeEntity) -> bool {
    parent_of(graph, e).is_some_and(|p| p.id == owner.id)
}

/// One lead per dependent, picking the edge worth printing.
///
/// The graph is a multigraph and a body calls the same target more than
/// once: a pair joined by a `Calls` and a `UsesFn` edge, or by two call
/// sites, arrives here two or three times. A blast radius wants one route
/// per reached entity, so the duplicates are collapsed here — where the
/// choice can be made deliberately — rather than left to whichever edge
/// petgraph happened to yield first. A call outranks a mention, and among
/// call sites the one inside the most loops wins, because that is the one
/// worth telling a reader about.
///
/// Deliberately not done in [`callees`]: two call sites from one body are
/// two different costs there, and collapsing them would report the cheap
/// one.
fn one_lead_per_dependent(mut leads: Vec<Lead<'_>>) -> Vec<Lead<'_>> {
    leads.sort_by(|a, b| {
        a.entity
            .id
            .cmp(&b.entity.id)
            .then(rank(a).cmp(&rank(b)))
            .then(b.site.loops.len().cmp(&a.site.loops.len()))
    });
    leads.dedup_by(|a, b| a.entity.id == b.entity.id);
    leads
}

fn rank(lead: &Lead<'_>) -> u8 {
    match lead.edge.map(|r| r.kind) {
        Some(RelationshipKind::Calls) => 0,
        _ => 1,
    }
}

/// Every body scope under `entity`, with the loops enclosing each.
fn scopes_of(graph: &DependencyGraph, entity: &CodeEntity) -> Vec<(String, Site)> {
    let top = Site { loops: Vec::new(), line: entity.span.start.line + 1 };
    let mut out = vec![(entity.id.clone(), top)];
    let mut frontier = vec![(entity.id.clone(), Vec::new())];

    for _ in 0..MAX_SCOPE_DEPTH {
        let mut next = Vec::new();
        for (id, loops) in &frontier {
            for child in graph.children(id).into_iter().filter(|c| is_body_scope(c)) {
                let line = child.span.start.line + 1;
                let mut inner = loops.clone();
                if child.kind == EntityKind::Loop {
                    inner.push(line);
                }
                out.push((child.id.clone(), Site { loops: inner.clone(), line }));
                next.push((child.id.clone(), inner));
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    out
}

/// The loops between a scope node and the callable it belongs to, outermost
/// first — the same fact [`scopes_of`] records, read from the other end.
fn site_under(graph: &DependencyGraph, from: &CodeEntity, upto: &str) -> Site {
    let mut loops = Vec::new();
    let mut cur = from;
    for _ in 0..MAX_SCOPE_DEPTH {
        if cur.id == upto {
            break;
        }
        if cur.kind == EntityKind::Loop {
            loops.push(cur.span.start.line + 1);
        }
        match parent_of(graph, cur) {
            Some(parent) => cur = parent,
            None => break,
        }
    }
    loops.reverse();
    Site { loops, line: from.span.start.line + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{EntityMetrics, Relationship, Span};

    fn callable(name: &str) -> CodeEntity {
        let mut e = CodeEntity::new(name, EntityKind::Function, "a.ts", Span::default());
        e.metrics = EntityMetrics { cyclomatic: Some(1), ..Default::default() };
        e
    }

    fn loop_scope(parent: &CodeEntity, name: &str, line: usize) -> CodeEntity {
        let mut span = Span::default();
        span.start.line = line;
        span.end.line = line + 2;
        let mut e = CodeEntity::new(name, EntityKind::Loop, &parent.file_path, span);
        e.id = format!("{}::loop::{name}", parent.id);
        e.parent_id = Some(parent.id.clone());
        e
    }

    fn graph_of(entities: Vec<CodeEntity>, calls: &[(usize, usize)]) -> DependencyGraph {
        let mut relationships: Vec<Relationship> = calls
            .iter()
            .map(|(from, to)| {
                Relationship::new(&entities[*from].id, &entities[*to].id, RelationshipKind::Calls)
            })
            .collect();
        for e in &entities {
            if let Some(parent) = &e.parent_id {
                relationships.push(Relationship::new(parent, &e.id, RelationshipKind::Contains));
            }
        }
        DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn walk_out<'g>(graph: &'g DependencyGraph, name: &str, depth: usize) -> Walk<'g> {
        let root = graph.entities().find(|e| e.name == name).expect("fixture");
        Walk::of(graph, root, Plan::cost_chain(depth))
    }

    /// The fact no other walk keeps: a callee reached from inside two
    /// nested loops arrives carrying both of their header lines.
    #[test]
    fn a_hop_carries_the_loops_its_call_site_sits_in() {
        let run = callable("run");
        let outer = loop_scope(&run, "l1", 17);
        let mut inner = loop_scope(&run, "l2", 22);
        inner.parent_id = Some(outer.id.clone());
        let analyze = callable("analyze");
        let graph = graph_of(vec![run, outer, inner, analyze], &[(2, 3)]);

        let walk = walk_out(&graph, "run", 2);
        assert_eq!(walk.nodes.len(), 2, "the callee is reached through the loop nodes");
        assert_eq!(walk.nodes[1].entity.name, "analyze");
        assert_eq!(walk.nodes[1].site.loops, vec![18, 23], "outermost first, 1-based");
    }

    /// Mutual recursion stops the walk at the re-entry and labels it,
    /// rather than spinning or quietly dropping the edge.
    #[test]
    fn a_ring_stops_at_the_re_entry_and_is_labelled() {
        let a = callable("a");
        let b = callable("b");
        let graph = graph_of(vec![a, b], &[(0, 1), (1, 0)]);

        let walk = walk_out(&graph, "a", 4);
        assert_eq!(walk.nodes.len(), 3, "a → b → a, and no further");
        assert!(walk.nodes[2].cycle, "the re-entry is marked");
        assert!(walk.children[2].is_empty(), "and not expanded");
    }

    /// `PerPath` is the whole reason cost cannot use the existing BFS: the
    /// second route to a helper can be the expensive one.
    #[test]
    fn per_path_keeps_the_second_route_and_once_drops_it() {
        let run = callable("run");
        let mid = callable("mid");
        let leaf = callable("leaf");
        let entities = vec![run, mid, leaf];
        let graph = graph_of(entities, &[(0, 1), (0, 2), (1, 2)]);
        let root = graph.entities().find(|e| e.name == "run").expect("fixture");

        let per_path = Walk::of(&graph, root, Plan::cost_chain(3));
        let reached = |w: &Walk| w.nodes.iter().filter(|n| n.entity.name == "leaf").count();
        assert_eq!(reached(&per_path), 2, "both routes to `leaf`");

        let once = Walk::of(&graph, root, Plan::call_tree(3));
        assert_eq!(reached(&once), 1, "the first route only");
    }

    /// The incoming direction is the same walk with the arrows reversed,
    /// and the caller's loops survive the lift that `impact` applies.
    #[test]
    fn the_incoming_direction_lifts_the_caller_and_keeps_its_loops() {
        let watch = callable("watch");
        let l = loop_scope(&watch, "l1", 44);
        let target = callable("createApi");
        let graph = graph_of(vec![watch, l, target], &[(1, 2)]);
        let root = graph.entities().find(|e| e.name == "createApi").expect("fixture");

        let walk = Walk::of(&graph, root, Plan::blast_radius(2));
        assert_eq!(walk.nodes.len(), 2);
        assert_eq!(walk.nodes[1].entity.name, "watch", "lifted past the loop node");
        assert_eq!(walk.nodes[1].site.loops, vec![45], "and the loop is still named");
    }

    /// What a blast radius crosses that a cost chain must not. A function
    /// taking the target as a parameter type never calls it, so it is no
    /// frame of a cost chain — and it is exactly what breaks when the
    /// target's shape changes.
    #[test]
    fn the_radius_crosses_every_edge_kind_and_the_cost_chain_only_calls() {
        let mut config = CodeEntity::new("Config", EntityKind::Struct, "c.rs", Span::default());
        config.id = "c.rs::Config".to_string();
        let load = callable("load");
        let entities = vec![config, load];
        let relationships = vec![Relationship::new(
            &entities[1].id,
            &entities[0].id,
            RelationshipKind::UsesType,
        )];
        let graph = DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        });
        let root = graph.entities().find(|e| e.name == "Config").expect("fixture");

        let radius = Walk::of(&graph, root, Plan::blast_radius(2));
        assert_eq!(radius.nodes.len(), 2, "the type's user is in the radius");
        assert_eq!(radius.nodes[1].entity.name, "load");

        let calls_only = Walk::of(&graph, root, Plan::call_tree(2));
        assert_eq!(calls_only.nodes.len(), 1, "and is no frame of a call tree");
    }

    /// A caller of a method depends on the type that declares it, and
    /// *through which method* is the half of that a reader can open.
    #[test]
    fn a_callers_route_into_a_type_names_the_member_it_landed_on() {
        let mut store = CodeEntity::new("Store", EntityKind::Class, "s.ts", Span::default());
        store.id = "s.ts::Store".to_string();
        let mut load = callable("load");
        load.kind = EntityKind::Method;
        load.id = "s.ts::Store::load".to_string();
        load.parent_id = Some(store.id.clone());
        let caller = callable("run");
        let graph = graph_of(vec![store, load, caller], &[(2, 1)]);
        let root = graph.entities().find(|e| e.name == "Store").expect("fixture");

        let walk = Walk::of(&graph, root, Plan::blast_radius(2));
        assert_eq!(walk.nodes.len(), 2, "the method's caller is a dependent of the type");
        assert_eq!(walk.nodes[1].entity.name, "run");
        assert_eq!(
            walk.nodes[1].through.map(|m| m.name.as_str()),
            Some("load"),
            "the member the edge landed on was thrown away"
        );
    }

    /// The multigraph collapses to one row per dependent, and the call
    /// outranks the mention — not whichever edge petgraph yielded first.
    #[test]
    fn two_edges_between_the_same_pair_are_one_route() {
        let target = callable("parse");
        let caller = callable("run");
        let entities = vec![target, caller];
        let relationships = vec![
            Relationship::new(&entities[1].id, &entities[0].id, RelationshipKind::UsesFn),
            Relationship::new(&entities[1].id, &entities[0].id, RelationshipKind::Calls),
        ];
        let graph = DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        });
        let root = graph.entities().find(|e| e.name == "parse").expect("fixture");

        let walk = Walk::of(&graph, root, Plan::blast_radius(2));
        assert_eq!(walk.nodes.len(), 2, "one dependent, one row");
        assert_eq!(
            walk.nodes[1].edge.map(|r| r.kind),
            Some(RelationshipKind::Calls),
            "the mention won over the call"
        );
    }

    /// A call target mezz could not bind is counted rather than followed,
    /// so a consumer can say its answer is a floor.
    #[test]
    fn an_unbindable_call_is_counted_on_the_frame_it_leaves() {
        let run = callable("run");
        let mut ghost = callable("fetch");
        ghost.tags.insert("ghost".to_string());
        let graph = graph_of(vec![run, ghost], &[(0, 1)]);

        let walk = walk_out(&graph, "run", 2);
        assert_eq!(walk.nodes.len(), 1, "a ghost is not a frame");
        assert_eq!(walk.nodes[0].unbound, 1);
    }

    /// The outline number says where a row hangs, not only how far — and
    /// sibling order is the caller's to fix, because `graph.dependencies`
    /// yields edges in no order a reader could rely on.
    #[test]
    fn outline_numbers_follow_the_tree() {
        let run = callable("run");
        let a = callable("a");
        let b = callable("b");
        let c = callable("c");
        let graph = graph_of(vec![run, a, b, c], &[(0, 1), (0, 2), (1, 3)]);

        let mut walk = walk_out(&graph, "run", 3);
        let names: Vec<String> = walk.nodes.iter().map(|n| n.entity.name.clone()).collect();
        walk.sort_children_by_key(|at| names[at].clone());
        let rows = walk.rows();
        let numbered: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (walk.nodes[r.at].entity.name.as_str(), r.number.as_str()))
            .collect();
        assert_eq!(numbered, vec![("run", "1"), ("a", "1.1"), ("c", "1.1.1"), ("b", "1.2")]);
    }
}
