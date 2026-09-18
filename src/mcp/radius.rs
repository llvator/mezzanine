//! The blast radius as routes, not levels (MCP-048).
//!
//! `impact` used to answer "what breaks if I change this" with a flat list
//! of entities carrying a depth tag: `- [depth 3] function `watch``. A
//! reader who learns that `watch` is three hops out cannot tell **through
//! what** — and through what is the whole reason they asked, because it
//! names the frames they have to open, in order, to decide whether the
//! change is safe.
//!
//! So the walk keeps its predecessors ([`super::chains`], which was built
//! for the outgoing direction and is the same walk with the arrows
//! reversed), and this module renders what it found as an outline:
//!
//! ```text
//! - `1`   function `createApi` (src/api.ts:79) — calls ·heuristic
//!   - `1.1` function `runWatch` (src/watch.ts:45) — calls, in loop L47
//!   - `1.2` function `startServer` (src/serve.ts:12) — calls
//! - `2`   function `createClient` (src/client.ts:22) — uses type
//! ```
//!
//! A row's number says where it hangs, which `[depth N]` could only say how
//! far.
//!
//! ## Both directions, because it is one walk
//!
//! `direction: out` turns the same machinery on the callees and draws the
//! call tree under an entity — the shape nothing else in the tool set
//! renders, and the one [`super::cost`] prices. The two differ in what they
//! may cross: what *breaks* breaks through any dependency edge, and what
//! *runs* runs only through a call ([`super::chains::Reach`]).

use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::Value;

use crate::graph::DependencyGraph;
use crate::models::CodeEntity;

use super::chains::{Direction, Plan, Row, Walk};
use super::tools::{edge_label, render_node};

/// Rows a radius spends on routes.
///
/// Stricter than the forty flat rows it replaces, because a chain is wide:
/// a row carries a number, a node, a `file:line`, an edge kind and often a
/// loop, so forty of them and the 400-line body cap eats the rest of the
/// report. Ancestors count against it — a route is only readable whole.
const MAX_ROWS: usize = 25;

/// What the walk reached, ranked, pruned and ready to print.
pub(super) struct Radius<'g> {
    walk: Walk<'g>,
    direction: Direction,
    depth: usize,
    /// Entities reached before the cap, which is what the heading counts.
    reached: usize,
}

/// Which way `impact` walks, from the argument.
///
/// Default `in`: the tool's own question is what breaks. An unknown value
/// is refused rather than defaulted, because silently answering the other
/// question is worse than saying which two words are allowed.
pub(super) fn direction_of(args: &Value) -> Result<Direction> {
    match args.get("direction").and_then(|v| v.as_str()) {
        None | Some("") | Some("in") => Ok(Direction::In),
        Some("out") => Ok(Direction::Out),
        Some(other) => bail!(
            "`direction` is `in` (what breaks if this changes — the default) or \
             `out` (the call tree under this entity). Got `{other}`."
        ),
    }
}

impl<'g> Radius<'g> {
    pub(super) fn of(
        graph: &'g DependencyGraph,
        target: &'g CodeEntity,
        direction: Direction,
        depth: usize,
    ) -> Self {
        let plan = match direction {
            Direction::In => Plan::blast_radius(depth),
            Direction::Out => Plan::call_tree(depth),
        };
        let mut walk = Walk::of(graph, target, plan);
        let reached = walk.nodes.len() - 1;

        walk.prune(&keep(&walk));
        let order: Vec<Written> =
            (0..walk.nodes.len()).map(|at| written_at(&walk, direction, at)).collect();
        walk.sort_children_by_key(|at| order[at].clone());

        Radius { walk, direction, depth, reached }
    }

    pub(super) fn section(&self, root: &Path) -> Vec<String> {
        let rows = self.walk.rows();
        let mut body = vec![String::new(), self.heading(), self.explainer()];
        if self.reached == 0 {
            body.push(self.nothing_reached());
            return body;
        }
        body.push(self.furthest(&rows, root));
        body.extend(rows.iter().skip(1).map(|row| self.row(row, root)));
        body.extend(self.dropped_note(rows.len() - 1));
        body
    }

    fn heading(&self) -> String {
        match self.direction {
            Direction::In => format!(
                "## Blast radius to depth {} — {} entities reach this, one route each",
                self.depth, self.reached
            ),
            Direction::Out => format!(
                "## Call tree to depth {} — {} callables run under this, one route each",
                self.depth, self.reached
            ),
        }
    }

    /// How to read the outline, including the two things it is *not*.
    fn explainer(&self) -> String {
        let ordering = match self.direction {
            Direction::In =>
                "Siblings are ordered by where the dependency was written — file, then line \
                 — falling back to the dependent's own declaration position on the edges \
                 that do not record their site (AN-024), which is most of them. Stable and \
                 readable, but **not** execution order: nothing here says which caller runs \
                 first.",
            Direction::Out =>
                "Siblings are ordered by where the call was written, which on an edge that \
                 records its site (AN-024) is the order the calls appear in the body. On \
                 one that does not — most of them today — it falls back to the callee's \
                 declaration position, which is reading order and not call order.",
        };
        format!(
            "One row per entity reached, nested under the one it was reached through: `1.2` \
             hangs under `1`, and the indent is the hop count. Each row shows *a* shortest \
             route — first visit wins, so another route may exist. {ordering} A hop written \
             inside a loop says so; silence is not proof of the opposite, because some \
             parsers emit no loop nodes at all (RS-001)."
        )
    }

    fn nothing_reached(&self) -> String {
        match self.direction {
            Direction::In => format!(
                "Nothing reaches this within {} hops. That is not the same as unused — see \
                 the caveat under _Used by_.",
                self.depth
            ),
            Direction::Out => format!(
                "This calls nothing mezz could bind within {} hops. Calls it could not bind \
                 are in _Unresolved_ above; library calls are in _External_.",
                self.depth
            ),
        }
    }

    /// The far end, as a chain — the one route a reader most wants spelled
    /// out, and the form `trace` prints between two entities they already
    /// had to name.
    ///
    /// Picked from the rows rather than from the walk: a headline naming a
    /// route the outline below does not contain sends the reader looking
    /// for a row that is not there.
    fn furthest(&self, rows: &[Row], root: &Path) -> String {
        let Some(at) = rows
            .iter()
            .map(|row| row.at)
            .filter(|at| *at != 0 && !self.walk.nodes[*at].cycle)
            .max_by_key(|at| (self.walk.nodes[*at].depth, pressure(self.walk.nodes[*at].entity)))
        else {
            return "- Every route stops where it started: the only entities reached re-enter \
                    this one's own ring."
                .to_string();
        };
        let chain: Vec<String> = self
            .walk
            .ancestors(at)
            .iter()
            .rev()
            .map(|i| render_node(self.walk.nodes[*i].entity, root))
            .collect();
        format!(
            "- **Furthest** — {} hops: {}",
            self.walk.nodes[at].depth,
            chain.join(&format!(" {} ", self.arrow()))
        )
    }

    fn arrow(&self) -> &'static str {
        match self.direction {
            Direction::In => "←",
            Direction::Out => "→",
        }
    }

    fn row(&self, row: &Row, root: &Path) -> String {
        let hop = &self.walk.nodes[row.at];
        format!(
            // Two spaces a level: an outline in plain text, a nested list
            // once the markdown is rendered, and the same tree either way.
            // The root's `1.` is stripped so the first hop reads `1`, not
            // `1.1` — the target is the page, not a row on it.
            "{}- `{}` {}{}{}{}",
            "  ".repeat(row.depth - 1),
            row.number.strip_prefix("1.").unwrap_or(&row.number),
            render_node(hop.entity, root),
            hop.edge.map(|r| format!(" — {}", edge_label(r))).unwrap_or_default(),
            self.site_phrase(row.at),
            match hop.cycle {
                true => ", **a ring**: this is already open on the route above, so the walk \
                        stops here rather than going round"
                    .to_string(),
                false => String::new(),
            },
        )
    }

    /// The facts the lift throws away: which member the edge landed on, and
    /// which loops the call site sits in.
    ///
    /// Both are lifted out of the *rows* on purpose — a call inside an `if`
    /// must not spend a level of the radius on the branch node
    /// ([`super::tools::lifted`]) — and both are exactly what a reader of a
    /// route wants back.
    fn site_phrase(&self, at: usize) -> String {
        let hop = &self.walk.nodes[at];
        let member = hop
            .through
            .map(|m| format!(", via `{}`", m.name))
            .unwrap_or_default();
        let loops = match hop.site.loops.len() {
            0 => String::new(),
            1 => format!(", in loop L{}", hop.site.loops[0]),
            _ => format!(
                ", in loops {}",
                hop.site
                    .loops
                    .iter()
                    .map(|l| format!("L{l}"))
                    .collect::<Vec<_>>()
                    .join(" ⊃ ")
            ),
        };
        format!("{member}{loops}")
    }

    /// What the cap dropped, and the argument that would bring it back.
    fn dropped_note(&self, shown: usize) -> Vec<String> {
        let dropped = self.reached.saturating_sub(shown);
        if dropped == 0 && !self.walk.truncated {
            return Vec::new();
        }
        let mut out = Vec::new();
        if dropped > 0 {
            out.push(format!(
                "… and {dropped} more reached, not shown. The routes kept are the deepest \
                 first, then the highest-pressure; lower `depth` for a narrower radius, or \
                 ask `impact` about one of the rows above."
            ));
        }
        if self.walk.truncated {
            out.push(
                "**The walk stopped early.** The tree outgrew the walk's budget, so branches \
                 are missing entirely rather than merely unlisted. Lower `depth`."
                    .to_string(),
            );
        }
        out
    }
}

/// The frames worth printing, and every frame on the way to them.
///
/// Deepest first, then by pressure: the far end is the surprising part of a
/// radius, and its ancestors come along for free — a route with a hole in it
/// is not a route. A chain that would not fit whole is skipped rather than
/// printed headless, and the next one is tried, so the cap fills up with
/// what does fit.
fn keep(walk: &Walk) -> HashSet<usize> {
    let mut order: Vec<usize> = (1..walk.nodes.len()).collect();
    order.sort_by_key(|at| {
        (
            Reverse(walk.nodes[*at].depth),
            Reverse(pressure(walk.nodes[*at].entity)),
            *at,
        )
    });

    let mut keep: HashSet<usize> = HashSet::from([0]);
    for at in order {
        let chain = walk.ancestors(at);
        let fresh = chain.iter().filter(|i| !keep.contains(i)).count();
        if keep.len() + fresh > MAX_ROWS + 1 {
            continue;
        }
        keep.extend(chain);
    }
    keep
}

/// Quality pressure, as an integer so it can be sorted with a depth.
fn pressure(e: &CodeEntity) -> i64 {
    (e.metrics.composite_score * 1000.0) as i64
}

/// Where the edge into a frame was **written** — the key siblings are
/// ordered by, and the reason the report can state an order at all.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Written {
    /// The file the rows are grouped by before anything else. The
    /// dependent's own file walking inwards, where "who reaches this"
    /// reads file by file; nothing walking outwards, where the siblings all
    /// sit in one body and grouping by the callee's file would scatter the
    /// order the calls were written in.
    group: PathBuf,
    /// Where the edge was written. `Relationship::span` (AN-024) when the
    /// edge carries its site, else the line of the body scope it left —
    /// which is the caller's own declaration line for a language whose
    /// parser emits no body scopes.
    line: usize,
    /// The declaration the row points at, as the stated fallback: the line
    /// above is shared by every sibling when no edge carries its site, and
    /// falling back to reading order beats falling back to whatever order
    /// petgraph held the edges in.
    declared: (PathBuf, usize),
    name: String,
}

fn written_at(walk: &Walk, direction: Direction, at: usize) -> Written {
    let hop = &walk.nodes[at];
    let line = hop
        .edge
        .and_then(|r| r.span.as_ref())
        .map(|s| s.start.line + 1)
        .unwrap_or(hop.site.line);
    Written {
        group: match direction {
            Direction::In => hop.entity.file_path.clone(),
            Direction::Out => PathBuf::new(),
        },
        line,
        declared: (hop.entity.file_path.clone(), hop.entity.span.start.line),
        name: hop.entity.name.clone(),
    }
}
