//! How a callable scales, and on what (MCP-046).
//!
//! Every complexity number mezz carries answers "how hard is this to read".
//! `cyclomatic` counts decisions, `cognitive` weights them by nesting,
//! `working_set` counts names in view. An agent asking "is this a problem on
//! a large input" gets one of those and is answered about something else: a
//! flat forty-arm `match` scores far worse than a doubly-nested loop, and the
//! doubly-nested loop is the one that falls over at 10k rows.
//!
//! This is the tool that answers the question actually asked. Two inputs,
//! both already in the graph:
//!
//! * **The loops the body writes** — [`crate::parser::loops`] measures the
//!   deepest loop-inside-loop chain as `loop_nesting`, counting loops and not
//!   branches, which is what makes it an exponent rather than a nesting
//!   score.
//! * **The loops the body calls** — [`crate::parser::costs`] recognises the
//!   library operations that walk, so one written loop around one `indexOf`
//!   reads as the O(n²) it is rather than the O(n) a loop count alone sees.
//!
//! ## Three tiers of answer, and the report says which it gave
//!
//! | Tier | Languages | What it can say |
//! |------|-----------|-----------------|
//! | Measured + evidenced | Python, TypeScript/JavaScript/Svelte, Go, Java, Kotlin, Dart, C/C++ | The depth, the loop at each level with its `file:line` and its header, and which calls sit inside which loop. |
//! | Measured only | Rust, Groovy | The depth, because the parser measures it. No loop entities exist in the graph for these two, so no evidence line and no per-call loop depth: a library call can be named but not placed. |
//! | Not measured | everything else | No loop table, so `max_nesting` is all there is — and it counts branches as well as loops, so the figure is an upper bound on the *shape* and is reported as one. |
//!
//! ## What it will not do
//!
//! **Recursion is classified, not solved.** Self-recursive, mutually
//! recursive, self-call inside a loop — the shape is reported and the closed
//! form is not derived. A Master-theorem answer would need the recurrence's
//! branching factor and subproblem size, neither of which an
//! entity/relationship graph holds, and printing one mezz cannot justify is
//! worse than printing none.
//!
//! **It is a structural worst case.** `break`, an early return, a
//! data-dependent bound, a loop that runs twice in practice — none is
//! modelled. It fails in both directions and the report names both: it
//! over-reports a loop whose bound is a constant it cannot see, and
//! under-reports a linear operation its tables have no rule for.

mod chain;

use std::path::Path;

use anyhow::{bail, Result};
use serde_json::Value;

use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use crate::parser::costs::{self, Cost};
use crate::parser::loops;

use super::externals::split;
use super::tools::{analyze, cap_lines, find_target, is_body_scope, rel_path, Found};
use super::McpServer;

use chain::Chains;

/// How deep a `parent_id` chain is followed before giving up. A body cut
/// into scopes is never this deep, and a chain that points at itself is
/// cheaper to cap than to prove impossible — the bound [`super::tools`] uses
/// for the same walk.
const MAX_SCOPE_DEPTH: usize = 64;

/// Evidence rows printed before the rest is counted.
const MAX_ROWS: usize = 12;

/// A structural bound: `n^exponent`, optionally times `log n`.
///
/// Deliberately not a number. Two loops over different collections are
/// `n × m`, not `n²`, and the whole report is careful to say the exponent
/// counts *levels* rather than claiming one shared size.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Bound {
    exponent: u32,
    log: bool,
}

impl Bound {
    fn render(self) -> String {
        let poly = match self.exponent {
            0 => None,
            1 => Some("n".to_string()),
            2 => Some("n²".to_string()),
            3 => Some("n³".to_string()),
            k => Some(format!("n^{k}")),
        };
        match (poly, self.log) {
            (None, false) => "O(1)".to_string(),
            (None, true) => "O(log n)".to_string(),
            (Some(p), false) => format!("O({p})"),
            (Some(p), true) => format!("O({p} log n)"),
        }
    }
}

/// One loop in the body, as the graph holds it.
struct LoopSite {
    /// 1-based line of the loop's header.
    line: usize,
    /// 1-based last line of the loop's body, so a call site can be placed
    /// inside it.
    end: usize,
    /// How many loops enclose it, itself included. The outermost is 1.
    depth: u32,
    /// The source line, so the report names the iterated expression in the
    /// author's own words rather than inventing an `n`.
    header: String,
    /// Whether that header shows a bound mezz can see is a literal.
    literal: bool,
}

/// One call the body makes back to itself.
struct SelfCall {
    line: usize,
    depth: u32,
}

/// One recognised library call, where it was found.
struct OpSite {
    name: String,
    cost: Cost,
    certain: bool,
    /// Loops enclosing the call. `0` is the top of the body — and is also
    /// what a language with no loop entities always reports, which is why
    /// [`Estimate::placed`] gates what may be said about it.
    depth: u32,
    /// 1-based line of the scope holding the call.
    line: usize,
}

/// One body scope, and how many loops enclose it — counting only the
/// levels whose bound mezz could not see is a literal, since that is the
/// count a call inside it is charged.
struct Scope {
    id: String,
    effective: u32,
}

/// Everything the report is built from, gathered once.
pub(crate) struct Estimate<'g> {
    target: &'g CodeEntity,
    language: Language,
    /// Parser-measured loop depth. `None` for a language with no loop table.
    measured: Option<u32>,
    loops: Vec<LoopSite>,
    ops: Vec<OpSite>,
    self_calls: Vec<SelfCall>,
    in_cycle: bool,
}

/// Hops the chain walk follows by default, and the hard ceiling on it.
///
/// Two is the depth at which the shape MCP-047 exists for — a loop calling a
/// helper that scans — becomes visible, and `impact`'s ceiling is five
/// because a walk wider than that answers with a wall of text rather than an
/// answer.
const DEFAULT_DEPTH: u64 = 2;
const MAX_DEPTH: u64 = 5;

/// Two questions, one tool: what does this entry point cost, and what does
/// this named route cost. Which one is being asked is the whole of this
/// function — the answering is [`entry_point`] and [`chain::between`].
pub fn cost(server: &McpServer, args: &Value) -> Result<String> {
    let depth = depth_arg(args);
    // Read before the analysis, so a malformed pair costs no parse.
    let route = route_args(args)?;
    let graph = analyze(server, &server.root)?;
    match route {
        Some((from, to)) => chain::between(server, &graph, from, to, depth.max(1)),
        None => entry_point(server, &graph, args, depth),
    }
}

/// What one callable costs: its own body, then the frames it reaches.
fn entry_point(
    server: &McpServer,
    graph: &DependencyGraph,
    args: &Value,
    depth: usize,
) -> Result<String> {
    let target = match find_target(server, graph, args)? {
        Found::One(e) => e,
        Found::Ambiguous(text) => return Ok(text),
    };
    match is_callable(target) {
        false => Ok(refusal(target)),
        true => Ok(cap_lines(
            Estimate::of(graph, target)
                .report(&server.root, Chains::of(graph, target, depth).as_ref()),
            "Lower `depth`, or target a narrower entity.",
        )),
    }
}

fn depth_arg(args: &Value) -> usize {
    args.get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_DEPTH)
        .min(MAX_DEPTH) as usize
}

/// The two ends of one named route, or `None` when the call is asking the
/// other question.
///
/// `from` + `to` is `trace`'s targeting: not "what does this entry point
/// cost" but "what does *this particular route* cost, frame by frame". Half
/// a pair is rejected rather than read as either — a `from` with no `to`
/// silently answered as an entry point would price something nobody asked
/// about.
fn route_args(args: &Value) -> Result<Option<(&str, &str)>> {
    match (named(args, "from"), named(args, "to")) {
        (Some(from), Some(to)) => Ok(Some((from, to))),
        (None, None) => Ok(None),
        _ => bail!(
            "`from` and `to` go together — they name the two ends of one chain. \
             For the cost of everything an entry point reaches, pass `entity` \
             (or `path` + `line`) and `depth` instead."
        ),
    }
}

fn named<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

/// A body is what scales, so anything without one is refused rather than
/// answered with zeros. A callable with no branches still measures
/// `cyclomatic 1`, so the presence of that metric is the test.
fn is_callable(e: &CodeEntity) -> bool {
    e.metrics.cyclomatic.is_some() && matches!(e.kind, EntityKind::Function | EntityKind::Method)
}

fn refusal(target: &CodeEntity) -> String {
    format!(
        "`{}` is a {}, and `cost` answers for a callable — a function or \
         a method. Scaling is a property of a body, and a {} has none. \
         Call `map` on it to list the callables it holds, or `impact` for \
         what depends on it.",
        target.name,
        target.kind.display_name(),
        target.kind.display_name(),
    )
}

impl<'g> Estimate<'g> {
    pub(crate) fn of(graph: &'g DependencyGraph, target: &'g CodeEntity) -> Self {
        let source = std::fs::read_to_string(&target.file_path).ok();
        Self::read(graph, target, source.as_deref())
    }

    /// The same, with the file already in hand.
    ///
    /// A chain walk prices the same helper on branch after branch and every
    /// estimate reads its file, so the caller keeps one read per file rather
    /// than one per frame.
    fn read(graph: &'g DependencyGraph, target: &'g CodeEntity, source: Option<&str>) -> Self {
        let language = Language::from_path(&target.file_path);
        let (scopes, loops) = walk_scopes(graph, target, source);
        let mut estimate = Estimate {
            target,
            language,
            measured: target.metrics.loop_nesting,
            loops,
            ops: Vec::new(),
            self_calls: Vec::new(),
            in_cycle: target.metrics.in_cycle,
        };
        for scope in &scopes {
            estimate.harvest(graph, scope);
        }
        estimate.self_calls = estimate.find_self_calls(source);
        estimate.ops.sort_by(|a, b| {
            (b.depth, b.cost, a.line)
                .cmp(&(a.depth, a.cost, b.line))
        });
        estimate
    }

    /// Classify every unbound call one scope makes, and note the ones that
    /// come back here.
    fn harvest(&mut self, graph: &DependencyGraph, scope: &Scope) {
        let line = graph
            .entities()
            .find(|e| e.id == scope.id)
            .map_or(0, |e| e.span.start.line + 1);
        for (dep, rel) in graph.dependencies(&scope.id) {
            if rel.kind == RelationshipKind::Contains {
                continue;
            }
            if !dep.tags.contains("ghost") {
                continue;
            }
            if let Some(op) = self.recognise(dep) {
                self.ops.push(OpSite {
                    name: one_line(qualified(dep)),
                    cost: op.cost,
                    certain: op.certain,
                    depth: scope.effective,
                    line,
                });
            }
        }
    }

    /// What one unbound target costs, or `None` when the tables do not
    /// recognise it.
    ///
    /// Deliberately *without* MCP-039's "an owner this repo declares is a
    /// hole, not a dependency" rule, which [`super::effects`] applies and
    /// this cannot. There the rule tests the *owner* — a repo with its own
    /// `Command` gets silence. Here the classification hangs on the
    /// **member**, because a cost is a property of the operation rather than
    /// of the module it lives in, and members are short common words. Run
    /// against this repo, the same rule suppressed every `.map()` in the
    /// TypeScript UI because a Rust tool two directories away is called
    /// `map`. A call mezz did bind is already excluded — only ghosts reach
    /// here — and that is the whole guard this question supports.
    fn recognise(&self, ghost: &CodeEntity) -> Option<costs::Op> {
        let (owner, member) = split(self.language, qualified(ghost));
        costs::classify(self.language, owner, member)
    }

    /// Where the body calls itself, read from the source rather than the
    /// graph.
    ///
    /// The graph has no such edge. A Python `fact` whose body calls `fact`
    /// reports `Uses (0)` in `impact` — the resolver drops the self-loop —
    /// so a recursion check built on edges would report every directly
    /// recursive function as straight-line, which is the one shape it most
    /// needs to catch.
    ///
    /// Textual, and in the same class of claim as the cost tables: the name
    /// inside a comment or a string will be counted, and a call written
    /// through an alias or a function pointer will not. The declaration line
    /// is skipped so the signature is not read as a call to itself.
    fn find_self_calls(&self, source: Option<&str>) -> Vec<SelfCall> {
        let Some(source) = source else {
            return Vec::new();
        };
        let (first, last) = (self.target.span.start.line, self.target.span.end.line);
        source
            .lines()
            .enumerate()
            .skip(first + 1)
            .take(last.saturating_sub(first))
            .filter(|(_, text)| calls_name(text, &self.target.name))
            .map(|(i, _)| SelfCall {
                line: i + 1,
                depth: self.loop_depth_at(i + 1),
            })
            .collect()
    }

    /// How many loops enclose a line, from the loop spans already gathered.
    fn loop_depth_at(&self, line: usize) -> u32 {
        self.loops
            .iter()
            .filter(|l| l.line <= line && line <= l.end)
            .map(|l| l.depth)
            .max()
            .unwrap_or(0)
    }

    /// Whether the graph can place a call inside a loop for this language.
    ///
    /// The tier-2 gate. Rust and Groovy emit no loop entities, so every call
    /// reports depth 0 whether or not it sits in a loop — and a `0` that
    /// means "not known" must never be printed as "at the top of the body".
    fn placed(&self) -> bool {
        !self.loops.is_empty() || self.measured == Some(0)
    }

    /// The exponent the loops contribute, and whether it was discounted.
    ///
    /// The parser's measurement is authoritative: it walks the AST and sees
    /// every loop. The graph's loop entities are evidence, and the literal
    /// discount rides on them — so it is applied only where the two agree on
    /// the depth, which is the one case where dropping a level is provably
    /// dropping *that* level.
    fn loop_exponent(&self) -> (u32, Option<u32>) {
        let measured = self.measured.unwrap_or(0);
        let seen = self.loops.iter().map(|l| l.depth).max().unwrap_or(0);
        let literal = self.loops.iter().filter(|l| l.literal).count() as u32;
        match seen == measured && literal > 0 && measured > 0 {
            true => (measured.saturating_sub(literal), Some(measured)),
            false => (measured, None),
        }
    }

    /// The dominating term, and the operation that set it if a call did.
    fn bound(&self) -> (Bound, Option<&OpSite>) {
        let (exponent, _) = self.loop_exponent();
        let mut best = Bound { exponent, log: false };
        let mut via = None;
        for op in self.ops.iter().filter(|o| o.certain) {
            let term = Bound {
                exponent: op.depth + op.cost.exponent(),
                log: op.cost.has_log(),
            };
            if term > best {
                best = term;
                via = Some(op);
            }
        }
        (best, via)
    }

    /// Whether the loop whose header sits on `line` shows a bound mezz reads
    /// as a literal — the discount, asked from outside the body, so a chain
    /// charges the same levels this body's own exponent does.
    fn literal_at(&self, line: usize) -> bool {
        self.loops.iter().any(|l| l.line == line && l.literal)
    }

    /// The header the author wrote at `line`, so a chain row names the
    /// collection in their words rather than an invented `n`.
    fn header_at(&self, line: usize) -> Option<&str> {
        self.loops
            .iter()
            .find(|l| l.line == line)
            .map(|l| l.header.as_str())
    }

    fn report(&self, root: &Path, chains: Option<&Chains>) -> Vec<String> {
        let (bound, via) = self.bound();
        let mut body = vec![self.title(root, bound, chains), preamble(chains)];
        body.extend(self.estimate_section(bound, via));
        body.extend(self.loops_section());
        body.extend(self.ops_section());
        body.extend(self.recursion_section());
        if let Some(chains) = chains {
            body.extend(chains.section(root));
        }
        body.extend(self.caveats_section(chains));
        body
    }

    /// The `# Cost of …` line: the body's own bound, and — when a chain was
    /// walked — what it comes to over the frames it calls. Both, because
    /// they answer different questions and a reader optimising the wrong one
    /// is the failure this tool exists to prevent.
    fn title(&self, root: &Path, bound: Bound, chains: Option<&Chains>) -> String {
        let over = chains.map_or(String::new(), |c| {
            format!(" in its own body, {} over {} hops", c.headline(), c.depth())
        });
        format!(
            "# Cost of {} `{}` — {}:{} — {}{}",
            self.target.kind.display_name(),
            self.target.name,
            rel_path(&self.target.file_path, root),
            self.target.span.start.line + 1,
            self.headline(bound),
            over,
        )
    }

    /// The bound, hedged where the substrate makes it a guess at the shape
    /// rather than a reading of the loops.
    fn headline(&self, bound: Bound) -> String {
        match self.measured {
            Some(_) => bound.render(),
            None => format!(
                "{} (upper bound on the shape)",
                Bound {
                    exponent: self.target.metrics.max_nesting.unwrap_or(0),
                    log: false,
                }
                .render()
            ),
        }
    }

    fn estimate_section(&self, bound: Bound, via: Option<&OpSite>) -> Vec<String> {
        let mut out = vec![String::new(), "## Estimate".to_string()];
        let Some(measured) = self.measured else {
            out.extend(self.unmeasured());
            return out;
        };
        let (exponent, before) = self.loop_exponent();
        out.push(format!(
            "- **{}** — the exponent counts loop *levels*, not one shared size. Two nested \
             loops over different collections are `n × m`; mezz writes that `{}` and does \
             not claim the two are equal.",
            bound.render(),
            Bound { exponent: 2, log: false }.render(),
        ));
        out.push(match measured {
            0 => "- No loop in this body (`loop_nesting 0`), measured from the source.".to_string(),
            1 => "- 1 loop (`loop_nesting 1`), measured from the source.".to_string(),
            k => format!("- {k} nested loops (`loop_nesting {k}`), measured from the source."),
        });
        if let Some(before) = before {
            out.push(format!(
                "- Discounted from `{before}` to `{exponent}`: a loop below has a bound mezz \
                 can see is a literal, so that level is constant. Every other bound is \
                 unknown — mezz does not read loop bounds it cannot prove."
            ));
        }
        if let Some(op) = via {
            out.push(format!(
                "- Set by a library call, not by the loops: `{}` costs `{}`{}.",
                op.name,
                op.cost.label(),
                depth_phrase(op.depth, self.placed()),
            ));
        }
        out
    }

    /// The sentence a language with no loop table prints in place of a
    /// measurement.
    ///
    /// The failure this guards against is the one the ticket is named for: a
    /// reader turning `max_nesting 3` into O(n³) when the three are `if`s.
    fn unmeasured(&self) -> Vec<String> {
        vec![
            format!(
                "- mezz carries no loop table for {}, so this is **not** a reading of the \
                 loops. It carries one for {}.",
                self.language.display_name(),
                loops::COVERED,
            ),
            format!(
                "- The figure above is `max_nesting {}` raised to an exponent, and \
                 `max_nesting` counts every nesting construct — `if`, `switch`, a match \
                 arm, a closure — not only loops. Three nested `if`s and no loop score 3. \
                 Treat it as an upper bound on the shape and open the file.",
                self.target.metrics.max_nesting.unwrap_or(0),
            ),
        ]
    }

    fn loops_section(&self) -> Vec<String> {
        let mut out = vec![String::new()];
        if self.loops.is_empty() {
            out.push("## Loops — no evidence lines".to_string());
            out.push(match self.measured {
                Some(0) => "This body has no loop, which is a measurement and not a gap."
                    .to_string(),
                _ => format!(
                    "mezz emits no loop entities for {}, so the depth above is measured but \
                     cannot be pointed at, and no call below can be placed inside a loop. \
                     The depth is still real; only the `file:line` evidence is missing.",
                    self.language.display_name(),
                ),
            });
            return out;
        }
        out.push(format!("## Loops ({}) — where the exponent comes from", self.loops.len()));
        let mut sites: Vec<&LoopSite> = self.loops.iter().collect();
        sites.sort_by_key(|l| (l.line, l.depth));
        for site in sites.iter().take(MAX_ROWS) {
            out.push(format!(
                "- depth {} — line {} — `{}`{}",
                site.depth,
                site.line,
                site.header,
                match site.literal {
                    true => "  ← literal bound, so this level is constant",
                    false => "",
                }
            ));
        }
        if sites.len() > MAX_ROWS {
            out.push(format!("… and {} more.", sites.len() - MAX_ROWS));
        }
        out
    }

    fn ops_section(&self) -> Vec<String> {
        let mut out = vec![String::new()];
        if !costs::has_table(self.language) {
            out.push(format!(
                "## Library operations — not classified for {}",
                self.language.display_name()
            ));
            out.push(format!(
                "mezz carries cost tables for {}. For any other language this section is \
                 silence, not a finding that the body calls nothing that walks.",
                costs::COVERED
            ));
            return out;
        }
        out.push(format!(
            "## Library operations ({}) — the loops this body does not write",
            match self.ops.is_empty() {
                true => "none recognised".to_string(),
                false => self.ops.len().to_string(),
            }
        ));
        out.push(
            "Classified from call target names, not from types. A name absent from the \
             tables is a name mezz has no rule for — not a claim that it is constant."
                .to_string(),
        );
        if self.ops.is_empty() {
            return out;
        }
        out.extend(self.ops.iter().take(MAX_ROWS).map(|op| self.op_row(op)));
        if self.ops.len() > MAX_ROWS {
            out.push(format!("… and {} more.", self.ops.len() - MAX_ROWS));
        }
        out
    }

    fn op_row(&self, op: &OpSite) -> String {
        format!(
            "- `{}` ({}){} — line {}{}",
            op.name,
            op.cost.label(),
            depth_phrase(op.depth, self.placed()),
            op.line,
            match op.certain {
                true => String::new(),
                false =>
                    " — **receiver-dependent**: this walks a sequence and probes a set or \
                     map, and mezz could not bind the receiver's type. It does not raise \
                     the bound above."
                        .to_string(),
            }
        )
    }

    fn recursion_section(&self) -> Vec<String> {
        if self.self_calls.is_empty() && !self.in_cycle {
            return Vec::new();
        }
        let mut out = vec![String::new(), "## Recursion — shape only".to_string()];
        if !self.self_calls.is_empty() {
            let lines: Vec<String> = self
                .self_calls
                .iter()
                .take(MAX_ROWS)
                .map(|c| match c.depth {
                    0 => format!("line {}", c.line),
                    1 => format!("line {} (inside 1 loop)", c.line),
                    d => format!("line {} (inside {d} nested loops)", c.line),
                })
                .collect();
            out.push(format!(
                "- **Self-recursive** — {}. Read from the source, not the graph: mezz \
                 records no self-edge, so a name in a comment counts and a call through an \
                 alias does not.",
                lines.join(", ")
            ));
            if self.self_calls.iter().any(|c| c.depth > 0) {
                out.push(
                    "- At least one self-call is **inside a loop**, which multiplies rather \
                     than deepens. That is the shape worth looking at first."
                        .to_string(),
                );
            }
        }
        if self.in_cycle {
            out.push(
                "- **In a dependency cycle** (`in_cycle`), so the recursion may be mutual \
                 rather than direct. `trace` from this entity back to itself shows the ring."
                    .to_string(),
            );
        }
        out.push(
            "- The closed form is **not derived**. Solving a recurrence needs its branching \
             factor and subproblem size, and an entity/relationship graph holds neither. \
             mezz reports the shape rather than a Master-theorem answer it cannot justify, \
             and the bound above describes one invocation's own body."
                .to_string(),
        );
        out
    }

    fn caveats_section(&self, chains: Option<&Chains>) -> Vec<String> {
        let mut out = vec![String::new(), "## What this does not model".to_string()];
        out.push(
            "- **Loop bounds.** `for i in 0..3` is a loop and counts as one, except where \
             the header shows a literal mezz can read. Every other bound is unknown, so the \
             estimate over-reports a loop that runs a fixed number of times."
                .to_string(),
        );
        out.push(
            "- **Early exit.** `break`, `return`, a short-circuit, a guard that makes the \
             inner loop run once — none is modelled. This is the worst case of the shape, \
             not of a run."
                .to_string(),
        );
        out.push(
            "- **Operators.** Python's `x in xs`, a C++ `map[k]`, an index into a linked \
             structure: none is a call, so no cost table can see it. This is the commonest \
             way the estimate under-reports."
                .to_string(),
        );
        out.push(
            "- **Iteration written as a call.** `.iter().map(…)`, `forEach`, `stream()` and \
             a recursive descent are not loop nodes. The recognised ones are listed above; \
             the rest are invisible."
                .to_string(),
        );
        out.extend(chain::caveats(chains));
        out
    }
}

/// The sentence under the `# Cost of …` line, which says which of the two
/// questions the report below answers.
fn preamble(chains: Option<&Chains>) -> String {
    match chains {
        Some(_) => "How this callable scales in the size of what it is handed — its own \
                    loops and library calls first, then what the frames it calls cost from \
                    here. A structural worst case, not a proof — see *What this does not \
                    model*."
            .to_string(),
        None => "How this callable scales in the size of what it is handed, read from the \
                 loops in its body and the names of the library calls it makes. A structural \
                 worst case, not a proof — see *What this does not model*."
            .to_string(),
    }
}

/// `inside 2 loops`, or nothing when the language cannot place a call.
fn depth_phrase(depth: u32, placed: bool) -> String {
    match (placed, depth) {
        (false, _) => " (mezz cannot tell whether it is inside a loop here)".to_string(),
        (true, 0) => " at the top of the body".to_string(),
        (true, 1) => " inside 1 loop".to_string(),
        (true, d) => format!(" inside {d} nested loops"),
    }
}

fn qualified(ghost: &CodeEntity) -> &str {
    match ghost.qualified_name.is_empty() {
        true => ghost.name.as_str(),
        false => ghost.qualified_name.as_str(),
    }
}

/// A call target fit to print on one row.
///
/// A ghost's qualified name is its receiver expression plus the member, and
/// a receiver can be a whole formatted method chain — four lines of it, in
/// this repo's own code. Whitespace is collapsed and the head of a long
/// receiver dropped, keeping the tail, because the tail is the part that
/// names the operation.
fn one_line(qualified: &str) -> String {
    const MAX: usize = 56;
    let flat = qualified.split_whitespace().collect::<Vec<_>>().join(" ");
    let width = flat.chars().count();
    match width > MAX {
        true => format!("…{}", flat.chars().skip(width - MAX).collect::<String>()),
        false => flat,
    }
}

/// Every body scope under `target`, with the loops enclosing it, plus the
/// loops themselves for the evidence section.
///
/// Breadth-first through `Contains`, the way [`super::tools::body_scope_ids`]
/// walks the same tree — but carrying depth, which is the whole point here.
fn walk_scopes<'g>(
    graph: &'g DependencyGraph,
    target: &'g CodeEntity,
    source: Option<&str>,
) -> (Vec<Scope>, Vec<LoopSite>) {
    let mut scopes = vec![Scope { id: target.id.clone(), effective: 0 }];
    let mut loops = Vec::new();
    let mut frontier = vec![(target.id.clone(), 0u32, 0u32)];

    for _ in 0..MAX_SCOPE_DEPTH {
        let mut next = Vec::new();
        for (id, depth, effective) in &frontier {
            let children = graph.children(id);
            for child in children.into_iter().filter(|c| is_body_scope(c)) {
                let step = descend(child, *depth, *effective, source, &mut loops);
                scopes.push(Scope { id: child.id.clone(), effective: step.1 });
                next.push((child.id.clone(), step.0, step.1));
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    (scopes, loops)
}

/// One step down the scope tree: the depths inside `child`, recording the
/// loop if that is what it is.
fn descend(
    child: &CodeEntity,
    depth: u32,
    effective: u32,
    source: Option<&str>,
    loops: &mut Vec<LoopSite>,
) -> (u32, u32) {
    if child.kind != EntityKind::Loop {
        return (depth, effective);
    }
    let header = header_line(source, child.span.start.line);
    let literal = has_literal_bound(&header);
    loops.push(LoopSite {
        line: child.span.start.line + 1,
        end: child.span.end.line + 1,
        depth: depth + 1,
        header,
        literal,
    });
    (depth + 1, effective + u32::from(!literal))
}

/// The source line the loop's header is on, so the report names the
/// iterated expression in the author's own words.
///
/// A loop entity's span is its *body*, which in a braced language starts on
/// the header line and in Python starts on the one after. Rather than
/// special-case the grammar, look at the body's first line and then a few
/// above it for the nearest line that spells a loop — a cheap read that is
/// right in both shapes and degrades to printing the line it was given.
fn header_line(source: Option<&str>, line0: usize) -> String {
    let Some(source) = source else {
        return "source not read".to_string();
    };
    let lines: Vec<&str> = source.lines().collect();
    let at = |i: usize| lines.get(i).map(|l| l.trim()).unwrap_or("");
    for back in 0..=3usize {
        let Some(i) = line0.checked_sub(back) else {
            break;
        };
        if starts_a_loop(at(i)) {
            return truncate(at(i));
        }
    }
    truncate(at(line0))
}

/// Keywords that open a loop in at least one of the nine grammars.
const LOOP_WORDS: &[&str] = &["for", "while", "loop", "do", "forEach", "repeat"];

/// Whether a source line opens a loop in any of the grammars here.
fn starts_a_loop(line: &str) -> bool {
    LOOP_WORDS
        .iter()
        .any(|word| word_ends(line, word).next().is_some())
}

/// Whether a loop header shows a bound mezz can see is a literal.
///
/// Narrow on purpose, in the discipline the cost tables are written to: it
/// claims a constant only for the forms where the bound is *digits in the
/// text*. `i < n`, `0..xs.len()` and `while (true)` are all unknown, and
/// unknown is the default — the estimate over-reporting a fixed-size loop is
/// the safe direction, and discounting one it merely hopes is fixed is not.
///
/// Hand-written rather than four regexes, because `regex` is a test-only
/// dependency here and a scaling estimate is not worth putting it in the
/// binary.
fn has_literal_bound(header: &str) -> bool {
    literal_range(header)
        || literal_keyword_bound(header)
        || literal_call_bound(header)
        || literal_c_bound(header)
}

/// `0..3`, `0..=9` — a Rust or Kotlin range with both ends written out.
fn literal_range(header: &str) -> bool {
    header.match_indices("..").any(|(at, _)| {
        let before = header[..at].trim_end();
        let after = header[at + 2..].trim_start_matches('=').trim_start();
        before.chars().next_back().is_some_and(|c| c.is_ascii_digit())
            && after.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// `until 5`, `downTo 0` — Kotlin's range keywords with a literal end.
fn literal_keyword_bound(header: &str) -> bool {
    ["until", "downTo"].iter().any(|word| {
        word_ends(header, word).any(|end| {
            header[end..]
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit())
        })
    })
}

/// `range(3)`, `range(0, 10, 2)`, `repeat(4)` — every argument a literal.
fn literal_call_bound(header: &str) -> bool {
    ["range", "repeat"]
        .iter()
        .any(|name| word_ends(header, name).any(|end| all_literal_args(&header[end..])))
}

/// The parenthesised argument list at the head of `rest`, if every argument
/// in it is an integer literal.
fn all_literal_args(rest: &str) -> bool {
    let Some(open) = rest.trim_start().strip_prefix('(') else {
        return false;
    };
    let Some(close) = open.find(')') else {
        return false;
    };
    let args = &open[..close];
    !args.trim().is_empty() && args.split(',').all(|arg| is_int_literal(arg.trim()))
}

fn is_int_literal(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// `i < 3;` — a C-family `for` header whose test compares against a literal.
///
/// The trailing `;` is what makes this a loop bound rather than any other
/// comparison on the line: it is the separator between a `for`'s test and
/// its step.
fn literal_c_bound(header: &str) -> bool {
    header
        .char_indices()
        .filter(|(_, c)| *c == '<' || *c == '>')
        .any(|(at, _)| {
            let rest = header[at + 1..].trim_start_matches('=').trim_start();
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            digits > 0 && rest[digits..].trim_start().starts_with(';')
        })
}

/// Byte offsets just past each whole-word occurrence of `word` in `text`.
///
/// Whole-word, so `resort` is not `sort` and `until` inside an identifier is
/// not Kotlin's keyword.
fn word_ends<'t>(text: &'t str, word: &'t str) -> impl Iterator<Item = usize> + 't {
    text.match_indices(word)
        .filter(move |(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
        })
        .map(move |(at, _)| at + word.len())
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether a source line calls `name` — the name as a whole word, with a
/// `(` after it.
///
/// The paren is what separates a call from a mention: `walk(child)` is
/// recursion, `let f = walk;` and `// see walk` are not.
fn calls_name(line: &str, name: &str) -> bool {
    word_ends(line, name).any(|end| line[end..].trim_start().starts_with('('))
}

/// Evidence lines go in a report, not in a file viewer.
fn truncate(line: &str) -> String {
    const MAX: usize = 100;
    match line.chars().count() > MAX {
        true => format!("{}…", line.chars().take(MAX).collect::<String>()),
        false => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{EntityMetrics, Relationship, Span};

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

    fn loop_scope(parent: &CodeEntity, path: &str, line: usize) -> CodeEntity {
        let mut span = Span::default();
        span.start.line = line;
        span.end.line = line + 2;
        let mut e = CodeEntity::new(path, EntityKind::Loop, &parent.file_path, span);
        e.id = format!("{}::branch::{}", parent.id, path);
        e.parent_id = Some(parent.id.clone());
        e.tags.insert("loop_node".to_string());
        e
    }

    fn nested_loop(parent: &CodeEntity, outer: &CodeEntity, path: &str, line: usize) -> CodeEntity {
        let mut e = loop_scope(parent, path, line);
        e.parent_id = Some(outer.id.clone());
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
        // Body scopes reach the walk through `Contains`, which the resolver
        // derives from `parent_id` — built here directly.
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

    /// A real file on disk, because the evidence lines and the recursion
    /// scan both read the source — a fixture graph alone cannot exercise
    /// either.
    fn scratch(dir: &str, file: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mezz-cost-{dir}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(file);
        std::fs::write(&path, body).expect("write");
        path
    }

    fn report_of(graph: &DependencyGraph, name: &str) -> String {
        let target = graph
            .entities()
            .find(|e| e.name == name && e.kind == EntityKind::Function)
            .expect("the fixture declares it");
        Estimate::of(graph, target).report(Path::new(""), None).join("\n")
    }

    /// The ticket's headline case: two nested loops are O(n²), and the
    /// exponent is said to count levels rather than one shared `n`.
    #[test]
    fn two_nested_loops_read_as_a_squared_bound() {
        let f = callable("scan", "a.ts", Some(2));
        let outer = loop_scope(&f, "l1", 3);
        let inner = nested_loop(&f, &outer, "l1.l1", 4);
        let graph = graph_of(vec![f, outer, inner], &[]);
        let out = report_of(&graph, "scan");

        assert!(out.contains("— O(n²)"), "{out}");
        assert!(out.contains("2 nested loops (`loop_nesting 2`)"), "{out}");
        assert!(out.contains("does not claim the two are equal"), "{out}");
    }

    /// The defect MCP-046 was filed for: a body deep in branches and empty
    /// of loops must not read as a high-degree polynomial.
    #[test]
    fn deep_branching_with_no_loop_is_constant() {
        let mut f = callable("classify", "a.ts", Some(0));
        f.metrics.max_nesting = Some(4);
        f.metrics.cyclomatic = Some(12);
        let graph = graph_of(vec![f], &[]);
        let out = report_of(&graph, "classify");

        assert!(out.contains("— O(1)"), "branch depth was raised to an exponent:\n{out}");
        assert!(out.contains("No loop in this body"), "{out}");
    }

    /// The other half of the estimate: one written loop around one linear
    /// library call is the O(n²) a loop count alone cannot see.
    #[test]
    fn a_linear_library_call_inside_a_loop_squares_the_bound() {
        let f = callable("dedupe", "a.ts", Some(1));
        let l = loop_scope(&f, "l1", 3);
        let scan = ghost("ids.indexOf");
        let graph = graph_of(vec![f, l, scan], &[(1, 2)]);
        let out = report_of(&graph, "dedupe");

        assert!(out.contains("— O(n²)"), "{out}");
        assert!(out.contains("Set by a library call"), "{out}");
        assert!(out.contains("`ids.indexOf` (n) inside 1 loop"), "{out}");
    }

    /// …and the honesty that keeps it usable: a name whose cost is its
    /// receiver's type is named and explicitly kept out of the bound.
    /// Every `Set.contains` in a loop is not an O(n²).
    #[test]
    fn a_receiver_dependent_call_is_named_but_does_not_raise_the_bound() {
        let f = callable("check", "a.kt", Some(1));
        let l = loop_scope(&f, "l1", 3);
        let probe = ghost("seen.contains");
        let graph = graph_of(vec![f, l, probe], &[(1, 2)]);
        let out = report_of(&graph, "check");

        assert!(out.contains("— O(n)"), "an unprovable bound was claimed:\n{out}");
        assert!(out.contains("receiver-dependent"), "{out}");
        assert!(out.contains("does not raise the bound"), "{out}");
    }

    /// A sort at the top of a body is `n log n`, and the notation survives
    /// the round trip.
    #[test]
    fn a_sort_reads_as_n_log_n() {
        let f = callable("order", "a.ts", Some(0));
        let sort = ghost("rows.sort");
        let graph = graph_of(vec![f, sort], &[(0, 1)]);
        let out = report_of(&graph, "order");

        assert!(out.contains("— O(n log n)"), "{out}");
        assert!(out.contains("at the top of the body"), "{out}");
    }

    /// A language with no loop table must say so rather than turn
    /// `max_nesting` into a confident exponent — the failure the ticket
    /// names explicitly.
    #[test]
    fn a_language_without_a_loop_table_hedges_instead_of_claiming() {
        let mut f = callable("scan", "a.rb", None);
        f.metrics.max_nesting = Some(3);
        let graph = graph_of(vec![f], &[]);
        let out = report_of(&graph, "scan");

        assert!(out.contains("upper bound on the shape"), "{out}");
        assert!(out.contains("counts every nesting construct"), "{out}");
        assert!(out.contains("Ruby"), "{out}");
        assert!(!out.contains("measured from the source"), "{out}");
    }

    /// Tier 2: the depth is measured but no loop entity exists, so the
    /// report must not place calls it cannot place — a `0` that means "not
    /// known" printed as "at the top of the body" is the wrong answer.
    #[test]
    fn a_language_with_no_loop_entities_says_it_cannot_place_calls() {
        let f = callable("scan", "a.rs", Some(2));
        let sort = ghost("rows::sort");
        let graph = graph_of(vec![f, sort], &[(0, 1)]);
        let out = report_of(&graph, "scan");

        assert!(out.contains("2 nested loops (`loop_nesting 2`)"), "{out}");
        assert!(out.contains("cannot tell whether it is inside a loop"), "{out}");
        assert!(out.contains("only the `file:line` evidence is missing"), "{out}");
        // The measured depth still bounds it, and the unplaceable sort does
        // not get to claim it sits in those loops.
        assert!(out.contains("— O(n²)"), "{out}");
    }

    /// Recursion is reported as a shape and the closed form withheld — and
    /// it is read from the source, because the graph drops the self-edge: a
    /// recursive body reports `Uses (0)` in `impact`, so an edge-based check
    /// would call every one of them straight-line.
    #[test]
    fn recursion_is_read_from_the_source_classified_and_not_solved() {
        let file = scratch(
            "recursion",
            "walk.ts",
            "function walk(node: Node): number {\n  \
             let total = 0;\n  \
             for (const child of node.children) {\n    \
             total += walk(child);\n  }\n  \
             return total;\n}\n",
        );

        let mut f = callable("walk", file.to_str().expect("utf8"), Some(1));
        f.span.end.line = 6;
        let l = loop_scope(&f, "l1", 2);
        let graph = graph_of(vec![f, l], &[]);
        let out = report_of(&graph, "walk");

        assert!(out.contains("## Recursion — shape only"), "{out}");
        assert!(out.contains("**Self-recursive** — line 4 (inside 1 loop)"), "{out}");
        assert!(out.contains("multiplies rather than deepens"), "{out}");
        assert!(out.contains("not derived"), "{out}");
    }

    /// The signature is not a call to itself, and neither is a mention
    /// without parentheses.
    #[test]
    fn a_declaration_or_a_mention_is_not_a_self_call() {
        let file = scratch(
            "no-recursion",
            "plain.ts",
            "function walk(node: Node): number {\n  \
             // walk is called by the caller, not here\n  \
             const f = walk;\n  \
             return 0;\n}\n",
        );
        let mut f = callable("walk", file.to_str().expect("utf8"), Some(0));
        f.span.end.line = 4;
        let graph = graph_of(vec![f], &[]);
        let out = report_of(&graph, "walk");

        assert!(!out.contains("Recursion"), "a mention was read as a call:\n{out}");
    }

    /// `cost` is a question about a body, so anything without one is
    /// refused with the tool that does take it.
    #[test]
    fn a_container_is_refused_rather_than_answered_with_zeros() {
        let mut s = CodeEntity::new("Config", EntityKind::Struct, "a.rs", Span::default());
        s.metrics.field_count = Some(4);
        let graph = graph_of(vec![s], &[]);
        let target = graph.entities().find(|e| e.name == "Config").expect("fixture");
        assert!(!is_callable(target));
    }

    /// The literal-bound reader claims a constant only where both ends are
    /// digits in the text, and calls everything else unknown.
    #[test]
    fn only_a_visible_literal_bound_is_discounted() {
        assert!(has_literal_bound("for i in 0..3 {"));
        assert!(has_literal_bound("for i in 0..=9 {"));
        assert!(has_literal_bound("for (let i = 0; i < 8; i++) {"));
        assert!(has_literal_bound("for row in range(4):"));
        assert!(has_literal_bound("for (i in 0 until 5) {"));

        assert!(!has_literal_bound("for x in xs {"));
        assert!(!has_literal_bound("for i in 0..xs.len() {"));
        assert!(!has_literal_bound("for (let i = 0; i < n; i++) {"));
        assert!(!has_literal_bound("while (true) {"));
        assert!(!has_literal_bound("for row in range(n):"));
    }

    /// The discount is applied to the bound, and both numbers are printed —
    /// a silently lowered exponent is as misleading as a silently raised one.
    #[test]
    fn a_constant_bounded_level_is_discounted_and_said_to_be() {
        let file = scratch(
            "discount",
            "a.ts",
            "function scan(rows) {\n  \
             for (const r of rows) {\n    \
             for (let i = 0; i < 3; i++) {\n      \
             touch(r, i);\n    }\n  }\n}\n",
        );

        let f = callable("scan", file.to_str().expect("utf8"), Some(2));
        let outer = loop_scope(&f, "l1", 1);
        let inner = nested_loop(&f, &outer, "l1.l1", 2);
        let graph = graph_of(vec![f, outer, inner], &[]);
        let out = report_of(&graph, "scan");

        assert!(out.contains("literal bound, so this level is constant"), "{out}");
        assert!(out.contains("Discounted from `2` to `1`"), "{out}");
        assert!(out.contains("— O(n)"), "the discount did not reach the bound:\n{out}");
        // The header the author wrote, not an invented `n`.
        assert!(out.contains("`for (const r of rows) {`"), "{out}");
    }
}
