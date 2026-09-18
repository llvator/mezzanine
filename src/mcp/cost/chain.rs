//! What a call chain costs, composed frame by frame (MCP-047).
//!
//! [`super`] answers about one body. Almost no real cost lives in one body.
//! The shape that actually bites is a cheap-looking function calling a
//! cheap-looking helper from inside a loop, three frames down:
//!
//! ```text
//! run()            → loops over files          O(f)
//!   └ analyze()    → loops over entities       O(e)
//!       └ lookup() → linear scan of a Vec      O(n)
//! ```
//!
//! Each frame reads as fine. The composition is `O(f·e·n)`, and before this
//! no tool in the set would say so: `impact` walks the call graph and reports
//! *who breaks*, `trace` walks it and reports *how A reaches B*, and neither
//! carries a cost along the walk.
//!
//! ## How a frame is charged
//!
//! `here = own + carried`, where `own` is [`Estimate`]'s reading of that
//! body and `carried` is the number of loops the chain passed through to
//! reach it. The multiplier comes from the body scope the call edge actually
//! *leaves* — the thing every other tool lifts away
//! ([`crate::mcp::tools::lifted`]) because for every other question a call in
//! a loop counts exactly like one at the top. [`crate::mcp::chains`] reads the
//! edge before that lift; this module prices what it found.
//!
//! The reported bound is the largest `here` over the reached frames, and the
//! report names the frame that produced it. A total nobody can locate is not
//! actionable.
//!
//! ## Where it refuses to be confident
//!
//! Four things turn the product into a floor, and each is stated in as many
//! words rather than folded silently into a number:
//!
//! * **A ring.** Mutual recursion stops the chain at the re-entry and is
//!   reported as recursive-unsolved. No closed form is claimed.
//! * **A call mezz could not bind.** The walk cannot follow it, so anything
//!   behind it is uncounted — the rule MCP-039 already imposes on `impact`'s
//!   counts.
//! * **A caller whose loops cannot be placed.** Rust and Groovy emit no loop
//!   nodes (RS-001), so a call site there reads as "at the top of the body"
//!   whether or not it is. The chain is charged the floor — zero — and the
//!   report names the exponent it would be if those calls sit in the loops.
//! * **A frame mezz cannot measure at all.** A language with no loop table
//!   makes that factor unknown, and an unknown factor makes the whole product
//!   unknown-at-least, never a confident product.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::graph::DependencyGraph;
use crate::mcp::chains::{Plan, Row, Site, Walk};
use crate::mcp::tools::{cap_lines, find_by_name, rel_path, Found};
use crate::mcp::McpServer;
use crate::models::CodeEntity;

use super::{Bound, Estimate};

/// Frames whose cost is spelled out on a row.
///
/// Stricter than `impact`'s forty, because a chain is wide: forty rows of
/// chain is the whole answer eaten by the body cap, and the frames below the
/// worst dozen are where the reader was never going to look.
const MAX_FRAMES: usize = 12;

/// What the walk found, priced.
pub(super) struct Chains<'g> {
    walk: Walk<'g>,
    /// Parallel to `walk.nodes`.
    frames: Vec<Frame>,
    book: Book<'g>,
    /// Frames kept in the rendering, worst-first. The first is the frame the
    /// headline is charged to.
    ranked: Vec<usize>,
    depth: usize,
    /// Ranked towards one named destination (`from`/`to`) rather than over
    /// everything reached. The rows are then a route, and the frames walked
    /// to find it are not an answer anybody asked for.
    route: bool,
}

/// One frame's arithmetic.
struct Frame {
    /// Index into [`Book::estimates`].
    est: usize,
    /// What this body costs on its own — [`super::Estimate`]'s answer.
    own: Bound,
    /// Loop levels the callers above it are charging it.
    carried: u32,
    /// Loop levels that *might* apply on top: a caller measures loops but
    /// emits no loop nodes, so mezz cannot tell whether the call sits inside
    /// them. Counted separately so the floor stays a floor.
    unplaced: u32,
}

impl<'g> Chains<'g> {
    /// Everything `root` reaches to `depth` hops, priced — or `None` when
    /// the walk was not asked for or reached nothing, where the chain
    /// sections would be a heading over a zero.
    pub(super) fn of(
        graph: &'g DependencyGraph,
        root: &'g CodeEntity,
        depth: usize,
    ) -> Option<Self> {
        if depth == 0 {
            return None;
        }
        let mut chains = Self::walked(graph, root, depth);
        chains.rank(None);
        chains.reached().then_some(chains)
    }

    /// One route: the worst chain from `from` that arrives at `to`, or
    /// `None` when nothing reaches it inside `depth` hops.
    pub(super) fn of_pair(
        graph: &'g DependencyGraph,
        from: &'g CodeEntity,
        to: &CodeEntity,
        depth: usize,
    ) -> Option<Self> {
        let mut chains = Self::walked(graph, from, depth);
        chains.rank(Some(&to.id));
        chains.route = true;
        (!chains.ranked.is_empty()).then_some(chains)
    }

    fn walked(graph: &'g DependencyGraph, root: &'g CodeEntity, depth: usize) -> Self {
        let walk = Walk::of(graph, root, Plan::cost_chain(depth));
        let mut book = Book::default();
        let mut frames: Vec<Frame> = Vec::with_capacity(walk.nodes.len());

        for node in &walk.nodes {
            let est = book.price(graph, node.entity);
            let (carried, unplaced) = match node.parent {
                None => (0, 0),
                Some(at) => charge(&frames[at], &book.estimates[frames[at].est], &node.site),
            };
            let own = book.estimates[est].bound().0;
            frames.push(Frame { est, own, carried, unplaced });
        }
        Chains { walk, frames, book, ranked: Vec::new(), depth, route: false }
    }

    /// Keep the worst [`MAX_FRAMES`], and order what survives worst-first so
    /// the dominant chain is the first branch of the outline.
    ///
    /// `only` narrows the ranking to one entity — `from`/`to`'s question,
    /// which wants the worst route to one named frame rather than the worst
    /// frames overall.
    fn rank(&mut self, only: Option<&str>) {
        let mut order: Vec<usize> = (1..self.walk.nodes.len())
            .filter(|at| match only {
                Some(id) => self.walk.nodes[*at].entity.id == id,
                None => true,
            })
            .collect();
        order.sort_by_key(|at| (std::cmp::Reverse(self.here(*at)), self.walk.nodes[*at].depth, *at));
        order.truncate(match only {
            Some(_) => 1,
            None => MAX_FRAMES,
        });

        let mut keep: HashSet<usize> = HashSet::from([0]);
        for at in &order {
            keep.extend(self.walk.ancestors(*at));
        }
        // Worst-in-subtree, propagated to the parents. A child's index is
        // always greater than its parent's, so one reverse pass is enough.
        let mut worst: Vec<Bound> = (0..self.walk.nodes.len()).map(|at| self.here(at)).collect();
        for at in (1..self.walk.nodes.len()).rev() {
            if let Some(parent) = self.walk.nodes[at].parent {
                worst[parent] = worst[parent].max(worst[at]);
            }
        }
        let key: Vec<(std::cmp::Reverse<Bound>, usize)> = (0..self.walk.nodes.len())
            .map(|at| (std::cmp::Reverse(worst[at]), self.walk.nodes[at].site.line))
            .collect();

        self.walk.prune(&keep);
        self.walk.sort_children_by_key(|at| key[at]);
        self.ranked = order;
    }

    /// What one frame costs from the entry point: its own body, times the
    /// loops the chain passed through to reach it.
    fn here(&self, at: usize) -> Bound {
        Bound {
            exponent: self.frames[at].own.exponent + self.frames[at].carried,
            log: self.frames[at].own.log,
        }
    }

    /// The frames the answer is built from.
    ///
    /// Everything walked, normally: a cheap frame that was ranked off the
    /// rows still calls something mezz could not bind, and that still makes
    /// the total a floor. For a route, only the route — the frames walked to
    /// *find* it were never the question.
    fn considered(&self) -> Vec<usize> {
        match self.route {
            true => self.walk.rows().iter().map(|row| row.at).collect(),
            false => (0..self.walk.nodes.len()).collect(),
        }
    }

    /// The dominating frame, or `None` when there is nothing to charge.
    ///
    /// Includes the entry point itself: a chain whose callees are all cheap
    /// still costs whatever its own body costs, and a headline that came out
    /// *below* the body's own bound would be worse than no headline.
    ///
    /// A frame that re-enters a ring is excluded: its `here` is the bound of
    /// one more trip round a loop mezz cannot count, and printing it as a
    /// total would be claiming the closed form this tool refuses to claim.
    fn dominant(&self) -> Option<usize> {
        self.considered()
            .into_iter()
            .filter(|at| !self.walk.nodes[*at].cycle)
            .max_by_key(|at| (self.here(*at), std::cmp::Reverse(*at)))
    }

    /// Whether the walk reached any frame at all.
    fn reached(&self) -> bool {
        self.walk.nodes.len() > 1
    }

    pub(super) fn depth(&self) -> usize {
        self.depth
    }

    /// The total, and whether it is a floor rather than an estimate.
    fn total(&self) -> (Bound, bool) {
        let bound = self.dominant().map_or(Bound::default(), |at| self.here(at));
        (bound, self.is_a_floor())
    }

    /// Every reason the product cannot be stated as an answer.
    fn is_a_floor(&self) -> bool {
        self.walk.truncated
            || self.considered().into_iter().any(|at| {
                let node = &self.walk.nodes[at];
                node.cycle
                    || node.unbound > 0
                    || self.frames[at].unplaced > 0
                    || self.book.estimates[self.frames[at].est].measured.is_none()
            })
    }

    /// `at least O(n³)` — what the header prints after the body's own bound.
    pub(super) fn headline(&self) -> String {
        let (bound, floor) = self.total();
        match floor {
            true => format!("at least {}", bound.render()),
            false => bound.render(),
        }
    }

    pub(super) fn section(&self, root: &Path) -> Vec<String> {
        let rows = self.walk.rows();
        let mut out = vec![
            String::new(),
            match self.route {
                true => format!("## The route — {} hops", rows.len() - 1),
                false => format!(
                    "## Along the chain — {} frames reached to depth {}",
                    self.walk.nodes.len() - 1,
                    self.depth,
                ),
            },
            "A callee reached from inside k loops is charged its own cost times those k \
             levels. `own` is that body alone; **here** is what it costs from this entry \
             point. The exponent counts loop *levels* along the chain, not one shared size."
                .to_string(),
        ];
        out.push(self.charged_to(root));
        out.extend(rows.iter().map(|row| self.row(row, root)));
        out.extend(self.dropped_note(rows.len() - 1));
        out.extend(self.stops_section(root));
        out
    }

    /// What the rows are not showing, and the argument that would show it.
    fn dropped_note(&self, shown: usize) -> Vec<String> {
        let dropped = (self.walk.nodes.len() - 1).saturating_sub(shown);
        if dropped == 0 {
            return Vec::new();
        }
        let frames = match dropped {
            1 => "1 frame".to_string(),
            k => format!("{k} frames"),
        };
        vec![match self.route {
            true => format!(
                "{frames} elsewhere under `{}` were walked to find this route and are not on \
                 it. Drop `from`/`to` to rank everything this entry point reaches.",
                self.walk.nodes[0].entity.name,
            ),
            false => format!(
                "… and {frames} cheaper than these, not shown. Worst first; ask `cost` about \
                 one of the rows above to see its own body in full."
            ),
        }]
    }

    /// The headline bound, and the frame it is charged to — a total nobody
    /// can locate is not actionable.
    fn charged_to(&self, root: &Path) -> String {
        let Some(at) = self.dominant() else {
            return "- Nothing to charge: the walk reached no frame it could price."
                .to_string();
        };
        if at == 0 {
            return format!(
                "- **{}** — the entry point's own body, which nothing it reaches beats. The \
                 frames below are what they cost *from here*, not what they cost alone.",
                self.headline(),
            );
        }
        let through: Vec<String> = self
            .walk
            .ancestors(at)
            .iter()
            .rev()
            .map(|i| format!("`{}`", self.walk.nodes[*i].entity.name))
            .collect();
        format!(
            "- **{}** — charged to {} `{}` at {}, reached through {}.",
            self.headline(),
            self.walk.nodes[at].entity.kind.display_name(),
            self.walk.nodes[at].entity.name,
            self.place(at, root),
            through.join(" → "),
        )
    }

    fn place(&self, at: usize, root: &Path) -> String {
        let e = self.walk.nodes[at].entity;
        format!("{}:{}", rel_path(&e.file_path, root), e.span.start.line + 1)
    }

    fn row(&self, row: &Row, root: &Path) -> String {
        let node = &self.walk.nodes[row.at];
        let frame = &self.frames[row.at];
        // A bound printed beside "unsolved" is the claim this tool refuses,
        // so a re-entry row carries the word and not a number.
        let tail = match node.cycle {
            true => " — here **unsolved**: this body is already open on the chain, so the \
                     walk stops at the re-entry and no closed form is claimed for the ring"
                .to_string(),
            false => format!(
                " — here **{}**{}",
                self.here(row.at).render(),
                self.ceiling_phrase(row.at),
            ),
        };
        format!(
            "{}- `{}` {} `{}` — {} — own {}{}{}",
            // Two spaces a level: an outline in plain text, a nested list
            // once the markdown is rendered, and the same tree either way.
            "  ".repeat(row.depth),
            row.number,
            node.entity.kind.display_name(),
            node.entity.name,
            self.place(row.at, root),
            frame.own.render(),
            self.site_phrase(row.at),
            tail,
        )
    }

    /// Where the call into this frame was written, in the caller's own words.
    fn site_phrase(&self, at: usize) -> String {
        let Some(parent) = self.walk.nodes[at].parent else {
            return String::new();
        };
        let caller = &self.book.estimates[self.frames[parent].est];
        let site = &self.walk.nodes[at].site;
        if !caller.placed() {
            // Deliberately no line. Rust and Groovy attach every call to the
            // callable itself, so the only line available here is the
            // caller's signature — a number that reads as the call site and
            // is not one.
            return format!(
                ", **unplaced**: mezz emits no loop nodes for {}, so it cannot tell whether \
                 this call sits inside the {} `{}` measures",
                caller.language.display_name(),
                match caller.measured.unwrap_or(0) {
                    1 => "1 loop".to_string(),
                    k => format!("{k} nested loops"),
                },
                caller.target.name,
            );
        }
        let charged: Vec<usize> = site
            .loops
            .iter()
            .copied()
            .filter(|line| !caller.literal_at(*line))
            .collect();
        match (charged.is_empty(), site.loops.is_empty()) {
            (true, true) => format!(", called at the top of `{}`", caller.target.name),
            (true, false) => format!(
                ", called inside {} loops whose bounds mezz reads as literals, so they cost \
                 nothing",
                site.loops.len()
            ),
            (false, _) => format!(
                ", called inside {} ({})",
                match charged.len() {
                    1 => "1 loop".to_string(),
                    k => format!("{k} nested loops"),
                },
                self.loop_words(caller, &charged),
            ),
        }
    }

    /// The loops naming themselves, innermost last, in the header the author
    /// wrote — so the collections are named in their own words rather than
    /// collapsed into an invented `n`.
    fn loop_words(&self, caller: &Estimate, lines: &[usize]) -> String {
        lines
            .iter()
            .map(|line| match caller.header_at(*line) {
                Some(header) => format!("`{header}` L{line}"),
                None => format!("L{line}"),
            })
            .collect::<Vec<_>>()
            .join(" ⊃ ")
    }

    /// The exponent this frame would carry if the calls mezz could not place
    /// do sit inside their callers' loops.
    fn ceiling_phrase(&self, at: usize) -> String {
        let frame = &self.frames[at];
        if frame.unplaced == 0 {
            return String::new();
        }
        let ceiling = Bound {
            exponent: frame.own.exponent + frame.carried + frame.unplaced,
            log: frame.own.log,
        };
        format!(" (up to **{}** — see *Where the chain stops*)", ceiling.render())
    }

    /// Everything that makes the total a floor, named.
    fn stops_section(&self, root: &Path) -> Vec<String> {
        let mut out = vec![String::new(), "## Where the chain stops".to_string()];
        let rings: Vec<usize> = (1..self.walk.nodes.len())
            .filter(|at| self.walk.nodes[*at].cycle)
            .collect();
        if let Some(at) = rings.first() {
            out.push(format!(
                "- **Recursive ({} re-entry point{})** — `{}` is already open on its own chain \
                 at {}. The walk stops at the re-entry rather than going round; the bound for \
                 the ring is **unsolved**, not `O(1)`. Solving a recurrence needs its branching \
                 factor and subproblem size, and an entity/relationship graph holds neither.",
                rings.len(),
                match rings.len() { 1 => "", _ => "s" },
                self.walk.nodes[*at].entity.name,
                self.place(*at, root),
            ));
        }
        out.extend(self.unplaced_note());
        let unbound: usize = self.considered().iter().map(|at| self.walk.nodes[*at].unbound).sum();
        if unbound > 0 {
            out.push(format!(
                "- **{unbound} call targets could not be bound** across the frames walked, so \
                 whatever they cost is uncounted. The total above is a floor. `impact` on a \
                 frame names them and says which are library calls and which are calls mezz \
                 should have resolved and did not."
            ));
        }
        out.extend(self.unmeasured_note());
        if self.walk.truncated {
            out.push(
                "- **The walk stopped early.** The call tree exceeded the frame budget, so \
                 branches are missing. Lower `depth`, or ask `cost` about a narrower entry \
                 point."
                    .to_string(),
            );
        }
        if out.len() == 2 {
            out.push(
                "- Nothing. No ring, no unbindable call, no unmeasured frame: every frame on \
                 these chains was read."
                    .to_string(),
            );
        }
        out
    }

    /// The tier-2 caveat, said once rather than on every row: a caller that
    /// measures loops but emits no loop nodes leaves the chain charging a
    /// floor of zero for calls that may sit inside all of them (RS-001).
    fn unplaced_note(&self) -> Vec<String> {
        // Read from the *edges*, not the frames: a body whose calls all sit
        // beyond `depth` charges nobody, and naming it here would send the
        // reader to a caller that cost this chain nothing.
        let mut names: Vec<&str> = self
            .considered()
            .into_iter()
            .filter_map(|at| self.walk.nodes[at].parent)
            .filter(|p| {
                let est = &self.book.estimates[self.frames[*p].est];
                !est.placed() && est.measured.unwrap_or(0) > 0
            })
            .map(|p| self.walk.nodes[p].entity.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        if names.is_empty() {
            return Vec::new();
        }
        vec![format!(
            "- **{} caller{} whose loops mezz cannot place a call in** — {}. Those parsers \
             emit no loop nodes, so every call in them reads as \"at the top of the body\" \
             whether or not it is. The chain is charged the **floor** — zero — and each \
             affected row carries the exponent it would be instead if those calls do sit \
             inside the loops. Open the caller to decide which.",
            names.len(),
            match names.len() { 1 => "", _ => "s" },
            names.iter().take(6).map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", "),
        )]
    }

    fn unmeasured_note(&self) -> Vec<String> {
        let mut names: Vec<&str> = self
            .considered()
            .into_iter()
            .filter(|at| self.book.estimates[self.frames[*at].est].measured.is_none())
            .map(|at| self.walk.nodes[at].entity.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        if names.is_empty() {
            return Vec::new();
        }
        vec![format!(
            "- **{} frame{} mezz carries no loop table for** — {}. Those factors are unknown, \
             and an unknown factor makes the product unknown-at-least rather than a confident \
             number. `cost` on one of them says which language and what it can still see.",
            names.len(),
            match names.len() { 1 => "", _ => "s" },
            names.iter().take(6).map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", "),
        )]
    }

    /// The modelling gaps a chain adds on top of the ones one body already
    /// has — stated here rather than left to be discovered.
    fn gaps(&self) -> Vec<String> {
        vec![
            "- **The same collection twice.** Two frames looping over the *same* collection \
             multiply in this model and should not. The loop headers are printed on each row \
             in the author's own words so the reader can spot it and collapse them; mezz does \
             not name iteration domains yet, so it cannot."
                .to_string(),
            "- **Memoisation, amortisation, early exit.** A cached recursive function is \
             reported at its uncached shape. Nothing here detects a cache, amortises a cost \
             over a run, or models a `break` that makes the inner frame run once."
                .to_string(),
            "- **Only calls compose.** A function *named* and not called (`UsesFn`) is not a \
             frame, because the loop runs and the named function does not."
                .to_string(),
        ]
    }
}

/// What a report's *What this does not model* section owes the reader about
/// callees — which is a different debt depending on whether a chain ran.
///
/// A free function taking the `Option` rather than a method on `Chains`, so
/// the caller extends one list and does not branch: the body report and the
/// chain report differ here by a bullet, not by a shape.
pub(super) fn caveats(chains: Option<&Chains>) -> Vec<String> {
    let Some(chains) = chains else {
        return vec![
            "- **Callees.** This is the target's own body. A constant-looking call to a \
             function that sorts is the callee's cost, not this one's — ask `cost` about \
             that callee with `depth`, or `impact` for what the body reaches."
                .to_string(),
        ];
    };
    let mut out = vec![
        "- **Callees past `depth`.** The chain above stops where `depth` does; a sort one \
         hop further out is uncounted."
            .to_string(),
    ];
    out.extend(chains.gaps());
    out
}

/// `cost --from A --to B`: what one named route costs, frame by frame.
///
/// `trace`'s targeting, priced. `trace` answers *how* A reaches B and every
/// hop of its answer reads the same whether it sits at the top of a body or
/// inside a triple loop; this answers what that route costs, and the loops
/// are the whole point.
///
/// Where the two differ: `trace` searches to thirty hops and this one to the
/// five `impact` allows, because a hop here is a priced frame rather than a
/// name. A route longer than that is `trace`'s to find and this tool's to
/// decline, which is what the miss message says.
pub(super) fn between(
    server: &McpServer,
    graph: &DependencyGraph,
    from_name: &str,
    to_name: &str,
    depth: usize,
) -> Result<String> {
    let from = match find_by_name(server, graph, from_name)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };
    let to = match find_by_name(server, graph, to_name)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };
    for end in [from, to] {
        if !super::is_callable(end) {
            return Ok(super::refusal(end));
        }
    }

    let root = &server.root;
    let Some(chains) = Chains::of_pair(graph, from, to, depth) else {
        return Ok(format!(
            "# Cost of the chain `{}` → `{}` — no route within {} hops\n\n\
             `cost` follows **bound calls only**, and only to `depth` hops (max {}). Either \
             nothing calls `{}` along this route, the route is longer than {} hops, or it \
             runs through a call mezz could not bind. `trace --from {} --to {}` searches \
             thirty hops and every edge kind, so it will find a route this cannot price — \
             then ask `cost` about a frame on it.",
            from.name,
            to.name,
            depth,
            super::MAX_DEPTH,
            to.name,
            depth,
            from_name,
            to_name,
        ));
    };

    let mut body = vec![
        format!(
            "# Cost of the chain `{}` → `{}` — {}",
            from.name,
            to.name,
            chains.headline(),
        ),
        "What this particular route costs, frame by frame. Where several routes reach the \
         destination the **worst** one is shown — not the shortest, which is `trace`'s \
         answer to a different question."
            .to_string(),
    ];
    body.extend(chains.section(root));
    body.push(String::new());
    body.push("## What this does not model".to_string());
    body.extend(chains.gaps());
    body.push(
        "- **One route.** Another chain into the same frame may cost more than this one; \
         `cost` on the entry point with `depth` ranks them all."
            .to_string(),
    );
    Ok(cap_lines(body, "Lower `depth`."))
}

/// The carried and unplaceable loop levels a callee inherits from its caller.
fn charge(parent: &Frame, caller: &Estimate, site: &Site) -> (u32, u32) {
    // A level whose bound mezz reads as a literal is constant, so it is not
    // a multiplier — the same discount [`Estimate::loop_exponent`] applies
    // inside one body, applied to the same loops from the other side.
    let counted = site.loops.iter().filter(|line| !caller.literal_at(**line)).count() as u32;
    // A caller with no loop nodes reports every site as "at the top of the
    // body", so its measured loops are neither charged nor forgotten.
    let blind = match caller.placed() {
        true => 0,
        false => caller.measured.unwrap_or(0),
    };
    (parent.carried + counted, parent.unplaced + blind)
}

/// One [`Estimate`] per entity, and one read per file.
///
/// A call tree revisits the same helper on branch after branch, and pricing
/// a frame reads its file off disk. Without this the walk pays for both per
/// node instead of per distinct body.
#[derive(Default)]
struct Book<'g> {
    estimates: Vec<Estimate<'g>>,
    by_id: HashMap<String, usize>,
    sources: HashMap<PathBuf, Option<String>>,
}

impl<'g> Book<'g> {
    fn price(&mut self, graph: &'g DependencyGraph, entity: &'g CodeEntity) -> usize {
        if let Some(at) = self.by_id.get(&entity.id) {
            return *at;
        }
        let source = self
            .sources
            .entry(entity.file_path.clone())
            .or_insert_with(|| std::fs::read_to_string(&entity.file_path).ok());
        let at = self.estimates.len();
        self.estimates.push(Estimate::read(graph, entity, source.as_deref()));
        self.by_id.insert(entity.id.clone(), at);
        at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{EntityKind, EntityMetrics, Relationship, RelationshipKind, Span};

    fn callable(name: &str, file: &str, loop_nesting: Option<u32>) -> CodeEntity {
        let mut e = CodeEntity::new(name, EntityKind::Function, file, Span::default());
        e.metrics = EntityMetrics {
            cyclomatic: Some(1),
            max_nesting: Some(0),
            loop_nesting,
            ..Default::default()
        };
        e
    }

    /// A loop scope inside `parent`'s body, which is what makes a call site
    /// placeable at all.
    fn loop_scope(parent: &CodeEntity, name: &str, line: usize) -> CodeEntity {
        let mut span = Span::default();
        span.start.line = line;
        span.end.line = line + 4;
        let mut e = CodeEntity::new(name, EntityKind::Loop, &parent.file_path, span);
        e.id = format!("{}::loop::{name}", parent.id);
        e.parent_id = Some(parent.id.clone());
        e
    }

    fn ghost(qualified: &str) -> CodeEntity {
        let mut e = CodeEntity::new(
            qualified.rsplit(['.', ':']).next().unwrap_or(qualified),
            EntityKind::Function,
            "",
            Span::default(),
        );
        e.qualified_name = qualified.to_string();
        e.tags.insert("ghost".to_string());
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

    fn report(graph: &DependencyGraph, name: &str, depth: usize) -> String {
        let root = graph
            .entities()
            .find(|e| e.name == name && e.kind == EntityKind::Function)
            .expect("the fixture declares it");
        match Chains::of(graph, root, depth) {
            Some(chains) => chains.section(Path::new("")).join("\n"),
            None => String::new(),
        }
    }

    /// Three frames looping over three collections. This is the ticket: each
    /// body reads as fine, the composition is `O(n³)`, and before MCP-047 no
    /// tool in the set would say so.
    #[test]
    fn a_loop_calling_a_loop_calling_a_loop_composes() {
        let run = callable("run", "run.ts", Some(1));
        let run_loop = loop_scope(&run, "l", 17);
        let analyze = callable("analyze", "analyze.ts", Some(1));
        let analyze_loop = loop_scope(&analyze, "l", 46);
        let lookup = callable("lookup", "store.ts", Some(1));
        let lookup_loop = loop_scope(&lookup, "l", 90);
        let graph = graph_of(
            vec![run, run_loop, analyze, analyze_loop, lookup, lookup_loop],
            &[(1, 2), (3, 4)],
        );

        let out = report(&graph, "run", 2);
        assert!(out.contains("charged to function `lookup`"), "{out}");
        assert!(out.contains("reached through `run` → `analyze` → `lookup`"), "{out}");
        assert!(out.contains("**O(n³)**"), "the chain did not compose:\n{out}");
        // And it is an estimate, not a floor: nothing here is unbound,
        // unmeasured, unplaceable or recursive.
        assert!(!out.contains("at least"), "{out}");
    }

    /// The other half of the same fact: a callee called at the top of a body
    /// is not multiplied by its caller's loops.
    #[test]
    fn a_callee_outside_the_loop_is_not_charged_for_it() {
        let run = callable("run", "run.ts", Some(1));
        let run_loop = loop_scope(&run, "l", 17);
        let setup = callable("setup", "setup.ts", Some(1));
        let setup_loop = loop_scope(&setup, "l", 3);
        // The call leaves `run` itself, not the loop node inside it.
        let graph = graph_of(vec![run, run_loop, setup, setup_loop], &[(0, 2)]);

        let out = report(&graph, "run", 2);
        assert!(out.contains("called at the top of `run`"), "{out}");
        assert!(out.contains("`1.1` function `setup`"), "{out}");
        assert!(out.contains("here **O(n)**"), "the top-of-body call was multiplied:\n{out}");
    }

    /// A ring terminates the walk, is named, and takes the confident total
    /// with it — a re-entry row carries the word `unsolved`, never a number.
    #[test]
    fn a_ring_stops_the_walk_and_is_reported_unsolved() {
        let a = callable("a", "a.ts", Some(1));
        let a_loop = loop_scope(&a, "l", 5);
        let b = callable("b", "b.ts", Some(0));
        let graph = graph_of(vec![a, a_loop, b], &[(1, 2), (2, 0)]);

        let out = report(&graph, "a", 4);
        assert!(out.contains("here **unsolved**"), "{out}");
        assert!(out.contains("**Recursive (1 re-entry point)**"), "{out}");
        assert!(out.contains("not `O(1)`"), "{out}");
        assert!(out.contains("at least"), "a ring left the total stated as an answer:\n{out}");
    }

    /// Tier 2 (RS-001): a caller that measures loops but emits no loop nodes
    /// cannot place its calls, so the chain is charged the floor and the
    /// report prints the exponent it would be instead.
    #[test]
    fn a_caller_with_no_loop_nodes_is_charged_the_floor_and_says_so() {
        let run = callable("run", "run.rs", Some(2));
        let helper = callable("helper", "helper.rs", Some(1));
        let graph = graph_of(vec![run, helper], &[(0, 1)]);

        let out = report(&graph, "run", 2);
        assert!(out.contains("**unplaced**"), "{out}");
        assert!(out.contains("2 nested loops `run` measures"), "{out}");
        // The floor is O(n) — its own loop — and the ceiling names the two
        // levels it might be sitting in.
        assert!(out.contains("here **O(n)** (up to **O(n³)**"), "{out}");
        assert!(out.contains("caller whose loops mezz cannot place a call in"), "{out}");
    }

    /// A call mezz could not bind stops the walk, so the total is a floor
    /// and the report says which word it means.
    #[test]
    fn an_unbindable_call_turns_the_total_into_a_floor() {
        let run = callable("run", "run.ts", Some(1));
        let run_loop = loop_scope(&run, "l", 4);
        let helper = callable("helper", "helper.ts", Some(0));
        let opaque = ghost("client.send");
        let graph = graph_of(vec![run, run_loop, helper, opaque], &[(1, 2), (2, 3)]);

        let out = report(&graph, "run", 2);
        assert!(out.contains("1 call targets could not be bound"), "{out}");
        assert!(out.contains("The total above is a floor"), "{out}");
        assert!(out.contains("at least"), "{out}");
    }

    /// `from`/`to` prices the **worst** route to a frame, not the shortest —
    /// which is the difference between this and `trace`.
    #[test]
    fn a_route_is_priced_by_its_worst_path_not_its_shortest() {
        let run = callable("run", "run.ts", Some(1));
        let run_loop = loop_scope(&run, "l", 9);
        let mid = callable("mid", "mid.ts", Some(1));
        let mid_loop = loop_scope(&mid, "l", 4);
        let leaf = callable("leaf", "leaf.ts", Some(1));
        let graph = graph_of(
            // `run` → `leaf` directly at the top of the body, and
            // `run` → `mid` → `leaf` through two loops.
            vec![run, run_loop, mid, mid_loop, leaf],
            &[(0, 4), (1, 2), (3, 4)],
        );
        let from = graph.entities().find(|e| e.name == "run").expect("fixture");
        let to = graph.entities().find(|e| e.name == "leaf").expect("fixture");

        let chains = Chains::of_pair(&graph, from, to, 3).expect("a route exists");
        let out = chains.section(Path::new("")).join("\n");
        assert!(out.contains("## The route — 2 hops"), "the one-hop route won:\n{out}");
        assert!(out.contains("reached through `run` → `mid` → `leaf`"), "{out}");
        assert!(out.contains("charged to function `leaf`"), "{out}");
        assert!(out.contains("here **O(n³)**"), "{out}");
    }

    /// A destination nothing reaches is a miss, not an empty report.
    #[test]
    fn a_route_that_does_not_exist_is_none() {
        let run = callable("run", "run.ts", Some(0));
        let stray = callable("stray", "stray.ts", Some(0));
        let graph = graph_of(vec![run, stray], &[]);
        let from = graph.entities().find(|e| e.name == "run").expect("fixture");
        let to = graph.entities().find(|e| e.name == "stray").expect("fixture");

        assert!(Chains::of_pair(&graph, from, to, 5).is_none());
    }

    /// A frame in a language mezz carries no loop table for is an unknown
    /// factor, and an unknown factor makes the product unknown-at-least —
    /// never a confident number, and never a silent zero.
    #[test]
    fn an_unmeasured_frame_makes_the_whole_product_unknown() {
        let run = callable("run", "run.ts", Some(1));
        let run_loop = loop_scope(&run, "l", 4);
        let opaque = callable("render", "view.rb", None);
        let graph = graph_of(vec![run, run_loop, opaque], &[(1, 2)]);

        let out = report(&graph, "run", 2);
        assert!(out.contains("1 frame mezz carries no loop table for"), "{out}");
        assert!(out.contains("`render`"), "{out}");
        assert!(out.contains("unknown-at-least"), "{out}");
        assert!(out.contains("at least"), "{out}");
        // And it must not be reported as a caller whose loops went unplaced:
        // it measures none, which is a different gap with a different fix.
        assert!(!out.contains("cannot place a call in"), "{out}");
    }

    /// `depth 0` is the MCP-046 question, and asking it must not print a
    /// chain heading over a zero.
    #[test]
    fn depth_zero_is_the_body_question_and_grows_no_chain() {
        let run = callable("run", "run.ts", Some(1));
        let helper = callable("helper", "helper.ts", Some(0));
        let graph = graph_of(vec![run, helper], &[(0, 1)]);
        let root = graph.entities().find(|e| e.name == "run").expect("fixture");

        assert!(Chains::of(&graph, root, 0).is_none());
        assert!(Chains::of(&graph, root, 1).is_some());
    }
}
