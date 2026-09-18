//! `reshape` — the graph one folder draws, and the one move that would
//! lift it a tier.
//!
//! `quality` reports folder shape across an area: the folders short of
//! fractal, split by whether their blocker is in their own drawing and
//! ranked deepest-first within that. That answers "where is the
//! organisation worst, and which of it can be worked on now", and it is
//! deliberately where this tool starts rather than what it replaces. What
//! it cannot do is tell an agent what to *change* — a line reading
//! "tangled, layered 0.61" names a defect without naming any of the six
//! edges that caused it.
//!
//! This hands over the drawing behind the verdict — the children with their
//! levels, every edge with its reading, the doors and the traffic coming
//! through them — plus an instruction scoped to exactly one rung of the
//! ladder.
//!
//! Three rules keep it honest, the first two inherited from
//! [`crate::server::refactor_prompt`]:
//!
//! 1. **Every number comes from the analyzer.** Measured values off
//!    [`FolderShape`], cut-offs off [`Thresholds`], the drawing off
//!    [`crate::analyzer::folder_shape::picture`]. Nothing is re-derived
//!    here, so the tool cannot disagree with the Quality panel or with
//!    `quality`.
//! 2. **The gate cited is the gate that capped the tier.** `blocker` has
//!    already decided which of six it was; this only expands it into the
//!    specific files and edges behind it. Picking a different one to talk
//!    about would send an agent after something that would not move the
//!    verdict.
//! 3. **One rung, never four.** A cyclic folder is asked to break its loop
//!    and nothing else. The tiers are a ladder, each defined as the one
//!    below plus a property, so a four-part instruction is one that cannot
//!    be finished and usually is not started.
//!
//! The whole repo is analysed rather than the named folder, unlike most
//! tools here. Half of what the drawing says — who reaches in, and whether
//! they come through the front door — is a fact about everything *outside*
//! the folder, and analysing the folder alone would report a clean facade
//! for every folder in the repo.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::Value;

use crate::graph::DependencyGraph;
use crate::models::{
    EdgeVerdict, FolderPicture, FolderShape, OutsideEdge, OutsideVerdict, PictureChild,
    PictureEdge, ShapeBlocker, ShapePattern, Thresholds,
};

use super::baseline::Baseline;
use super::format::{listed, num, MAX_LISTED};
use super::recipes;
use super::tools::cap_lines;
use super::McpServer;

/// `reshape` — one folder's drawn graph plus the single move that lifts it
/// a tier.
pub fn reshape(server: &McpServer, args: &Value) -> Result<String> {
    let folder = target_folder(server, args, "reshape")?;
    // The whole repo, deliberately — see the module header.
    let graph = super::tools::analyze(server, &server.root)?;

    let absolute = folder.display().to_string();
    let Some(picture) = graph.folder_picture(&absolute) else {
        bail!(
            "No analysed folder at {}. Folders come from the files in scope, so a \
             directory holding nothing mezz parsed has no shape to report.",
            rel(&folder, &server.root),
        );
    };
    let picture = picture.relative_to(&server.root);
    let shape = graph
        .folder_metrics()
        .iter()
        .find(|m| m.path == absolute)
        .and_then(|m| m.metrics.shape.clone());
    let Some(shape) = shape else {
        bail!(
            "{} was analysed but carries no shape. That is a file rather than a \
             folder, or a folder the analysis scored nothing for.",
            rel(&folder, &server.root),
        );
    };

    let t = Thresholds::default();
    let ctx = Ctx {
        graph: &graph,
        root: &server.root,
    };
    let current = Baseline::of(&shape, &picture, server.scope_id());
    let previous = server.shape_baselines.swap(&picture.folder, &current);

    let mut body = vec![format!("# Reshaping {}", picture.folder), String::new()];
    body.extend(soundness_note(&graph, &absolute, &server.root));
    body.extend(verdict_section(&shape, &t, &picture));
    body.extend(progress_section(previous.as_ref(), &current));
    body.extend(next_rung_section(&shape, &picture, &t, ctx));
    body.extend(drawing_section(&picture, ctx));
    body.extend(boundary_section(&picture));
    // First call in this process gets the rules in full; every call after
    // gets them as a checklist. See `short_rules`.
    let spell_out = !server
        .rules_spelled_out
        .swap(true, std::sync::atomic::Ordering::Relaxed);
    body.extend(rules_for(&picture.folder, previous.is_some(), spell_out));

    Ok(cap_lines(
        body,
        "Call `reshape` on a subfolder for a smaller drawing.",
    ))
}

/// What this folder's verdict is worth, when some of its imports never
/// reached the graph.
///
/// Above the verdict, not below it: every section under this one is computed
/// over the drawing, and a drawing missing edges produces answers that are
/// confidently wrong rather than visibly uncertain. `reshape` was asserting
/// *"Nothing leaves from the middle: this folder is a funnel"* over a folder
/// two of whose outgoing imports had not resolved — and the only warning was a
/// repo-wide figure in the footer, which cannot tell a reader whether the
/// folder they are asking about is one of the holed ones.
///
/// Silent when the folder's imports all landed, so a sound answer costs
/// nothing and the line means something when it appears.
fn soundness_note(graph: &DependencyGraph, folder: &str, root: &Path) -> Vec<String> {
    let sites = graph.unresolved_imports_in(folder);
    if sites.is_empty() {
        return Vec::new();
    }
    let named: Vec<String> = sites
        .iter()
        .take(MAX_LISTED)
        .map(|s| {
            format!(
                "`{}:{}` → `{}`",
                rel(&s.from, root),
                s.line + 1,
                rel(&s.to, root),
            )
        })
        .collect();
    let mut body = vec![
        format!(
            "> **{} of this folder's imports did not reach the graph.** Every verdict \
             below is computed over the drawing, so treat them as provisional — a \
             missing edge is why a folder reads as a funnel, as having one door, or \
             as having a child nothing reaches, when it has none of those.",
            sites.len(),
        ),
        String::new(),
        format!("> Unresolved: {}.", named.join(", ")),
        String::new(),
        "> Read those statements before acting on anything below: each one is an \
         edge the drawing does not have, and a finding that rests on its absence \
         is an artifact rather than a defect."
            .to_string(),
        String::new(),
    ];
    if sites.len() > MAX_LISTED {
        body.insert(
            3,
            format!("> … and {} more.", sites.len() - MAX_LISTED),
        );
    }
    body
}

/// Where the folder stands, with every cut-off spelled out beside the
/// measurement it judges. An agent given "layered 0.61" and no bar cannot
/// tell a near miss from a disaster.
fn verdict_section(shape: &FolderShape, t: &Thresholds, p: &FolderPicture) -> Vec<String> {
    let mut body = vec![
        "## Verdict".to_string(),
        format!(
            "**{}** — held back by {}.{}",
            shape.pattern.label(),
            shape
                .blocker
                .map_or_else(|| "nothing".to_string(), |b| b.summary()),
            advisory_qualifier(shape, t),
        ),
        String::new(),
        format!(
            "- acyclicity {} (must be 1.00 to clear `cyclic`)",
            num(Some(shape.acyclicity))
        ),
        format!(
            "- layering {} (needs ≥ {:.2} to clear `tangled`)",
            num(shape.layering),
            t.shape_layering
        ),
        format!(
            "- branching {} (needs ≥ {:.2} for `fractal`; NOT part of compliance)",
            num(shape.arborescence),
            t.shape_arborescence,
        ),
        format!(
            "- entry concentration {} (needs ≥ {:.2} for `fractal`){}",
            num(shape.entry_concentration),
            t.shape_entry,
            ways_in(p),
        ),
        format!(
            "- out at the bottom {} (needs ≥ {:.2} for `fractal`; NOT part of \
             compliance){}",
            num(shape.egress),
            t.shape_egress,
            ways_out(shape),
        ),
        format!(
            "- child compliance {} (needs ≥ {:.2} for `fractal`)",
            num(shape.child_compliance),
            t.shape_child,
        ),
        format!(
            "- uniformity {} (below {:.2} this folder and the level inside it are \
             drawn at different scales; gates NOTHING and is NOT part of \
             compliance){}",
            num(shape.uniformity),
            t.shape_uniformity,
            scale_note(shape),
        ),
        format!(
            "- compliance {:.2} (needs ≥ {:.2} for `fractal`) — a weighted blend of \
             acyclicity, layering, entry concentration and child compliance. \
             Branching is deliberately not in it, so this can read well while \
             branching is what holds the folder back.",
            shape.compliance, t.shape_compliance,
        ),
        format!(
            "- {} immediate children (needs ≤ {} for `fractal`; NOT part of compliance){}",
            shape.child_count,
            t.shape_max_children,
            crowding_note(shape.child_count, t.shape_max_children),
        ),
        String::new(),
        "Higher is better in every score here, which is the reverse of the \
         complexity and coupling scores elsewhere in mezz. The child count is the \
         exception and reads the ordinary way round — it is a count, not a ratio. \
         `—` means unmeasured, not perfect."
            .to_string(),
        String::new(),
        "Every score above except `uniformity` grades this one folder against a \
         fixed bar, `child compliance` included — it averages numbers each taken \
         against that same bar. `uniformity` is the only one that compares two \
         zoom levels, so a tree can clear every other gate at every level and \
         still change scale abruptly between them. It gates nothing yet and a low \
         reading is not a reason to keep working after the task below is done."
            .to_string(),
        String::new(),
    ];
    body.extend(formulas(shape));
    body.extend(fractal_note(shape, p));
    body
}

/// The three ratios written out as the divisions they are.
///
/// Printed rather than documented because agents were reverse-engineering
/// them from observed values and getting them wrong. One field report had
/// `branching` down as *children with exactly one parent ÷ (children −
/// 1)* — which happens to match on some folders and not others, so the
/// agent chose between three candidate layouts by editing the tree and
/// re-measuring, twice, and asked for the formulas to be published.
///
/// This is the publication. The counts come off [`ShapeTerms`], filled
/// beside the division that produced the ratio, so they cannot drift from
/// the numbers directly above them; `layout` prints the same three, which
/// is what lets an agent predict a score instead of shopping for one.
fn formulas(shape: &FolderShape) -> Vec<String> {
    let t = &shape.terms;
    if t.nodes == 0 {
        return Vec::new();
    }
    let mut rows = vec![format!(
        "- acyclicity = 1 − children in a loop ÷ children: 1 − {} ÷ {}",
        t.looped, t.nodes
    )];
    if t.edges > 0 {
        rows.push(format!(
            "- layering = edges stepping exactly one level down ÷ edges: {} ÷ {}",
            t.tight, t.edges
        ));
        rows.push(format!(
            "- branching = children an edge arrives at ÷ (edges + roots past the \
             first): {} ÷ ({} + {})",
            t.reached,
            t.edges,
            t.strays.saturating_sub(1)
        ));
    }
    if t.arrivals > 0 {
        rows.push(format!(
            "- entry concentration = arrivals on the busiest file ÷ arrivals from \
             outside: {} ÷ {}",
            t.busiest, t.arrivals
        ));
    }
    if t.widest_child > 0 {
        let (lo, hi) = (
            shape.child_count.min(t.widest_child),
            shape.child_count.max(t.widest_child),
        );
        rows.push(format!(
            "- uniformity = the smaller of this folder's breadth and its widest \
             subfolder's ÷ the larger: {lo} ÷ {hi}"
        ));
    }
    let mut body = vec![
        "Where those numbers come from, so you can work out what a change would \
         score before making it. `edges` is counted after each dependency loop \
         collapses to a single node, which is why it can be lower than the arrow \
         count in the drawing below:"
            .to_string(),
        String::new(),
    ];
    body.extend(rows);
    body.push(String::new());
    body.push(
        "`layout` on this folder scores a hypothetical set of moves against these \
         same counts, without touching the tree."
            .to_string(),
    );
    body.push(String::new());
    body
}

/// Whether this bullet is the one the usage breakdown will follow.
///
/// Only the first merge gets a breakdown — printing it per offender would
/// bury the instruction under data about merges the reader was just told to
/// leave alone — and `split_lines` yields nothing when fewer than two
/// dependents could be measured. Its own function because the complexity gate
/// fails on any metric increase to a function that already exists (CI-001),
/// and `merge_findings` had no budget for one more condition.
fn promises_breakdown(index: usize, breakdown: &[String]) -> bool {
    index == 0 && !breakdown.is_empty()
}

/// The warning that two of this tool's goals pull against each other, said
/// where the count is read rather than left for the reader to discover.
///
/// Making a folder a funnel means moving egress down into leaves, and that
/// means *adding leaf files*. So the funnel property pushes the child count
/// up, toward a ceiling whose remedy is grouping into subfolders — and each
/// new subfolder needs a door of its own. Reported from the field by an agent
/// who funnelled a folder from 7 children to 8 and landed exactly on the bar.
///
/// Both goals are real and the tension is not a defect; being silent about it
/// is, because an agent that hits the ceiling right after being told to
/// funnel reads it as having done the wrong thing.
fn crowding_note(children: u32, ceiling: u32) -> &'static str {
    if children + 1 < ceiling {
        return "";
    }
    " — note that funnelling egress into leaves adds children, so this and the \
     funnel property push against each other. At the bar, group into subfolders \
     rather than stopping the funnel work; each subfolder then earns its own door."
}

/// The two breadths `uniformity` divides, for the folder where they differ
/// enough to be worth going and looking at.
///
/// The ratio alone does not say which way round it is, and the two
/// directions are opposite pieces of work. A folder of 11 holding a
/// subfolder of 57 has one child doing all the carrying; a folder of 20
/// holding subfolders of 2 is a level that never delegated. Both read
/// 0.19-ish and neither is fixed by the other's remedy.
fn scale_note(shape: &FolderShape) -> String {
    let (own, inner) = (shape.child_count, shape.terms.widest_child);
    if own == 0 || inner == 0 || own == inner {
        return String::new();
    }
    let (wide, narrow) = if inner > own { ("inside", "here") } else { ("here", "inside") };
    format!(
        " — {own} children here, {inner} in the widest subfolder; the level {wide} \
         holds what the level {narrow} does not"
    )
}

/// The count `egress` rounds off, the way [`ways_in`] does for
/// `entry_concentration`.
///
/// The ratio alone is shoppable: 0.50 is one middle child with one exit
/// beside one leaf with one, and it is also one middle child with fifty. The
/// first is a file to move and the second is a folder built inside out, and
/// an agent picking work off the number cannot tell them apart.
fn ways_out(shape: &FolderShape) -> String {
    match shape.terms.middle_exits {
        0 => String::new(),
        1 => " — 1 exit from the middle".to_string(),
        n => format!(" — **{n} exits from the middle**, which the ratio does not say"),
    }
}

/// The count beside the ratio: how many of this folder's children anything
/// outside it depends on.
///
/// `entry_concentration` is the busiest child's *share* of the traffic
/// arriving, and a share has a perverse incentive built into it — removing a
/// dependency on the door lowers it, because the door's slice of a smaller
/// total is what the number is. Reported from the field: decoupling a
/// consumer from a folder, an unambiguous improvement, moved that folder from
/// 0.40 to 0.38, and the cheapest way to *raise* it would have been to make
/// more files import the door.
///
/// A count does not have that property. One way in stays one way in however
/// the traffic is distributed, and it is the thing people mean when they ask
/// for a single entry point — which the ratio does not express, since two
/// children splitting traffic 83/17 score 0.83 with two ways in. The project
/// rule `max_entered_files_per_folder` (CHK-003) is the enforceable form; this
/// is the same fact printed where the ratio is read, so the two cannot be
/// confused for each other.
///
/// Children rather than files, because that is the unit this folder is drawn
/// in — a subfolder is one node here however many of its files are entered.
fn ways_in(p: &FolderPicture) -> String {
    let entered = p.children.iter().filter(|c| c.inbound > 0).count();
    match entered {
        0 => String::new(),
        1 => " — 1 way in".to_string(),
        n => format!(" — **{n} ways in**, which the ratio does not say"),
    }
}

/// The qualifier on a verdict whose only blocker is the metric this tool
/// spends the rest of the report telling you to leave alone.
///
/// `branching` is deliberately outside `compliance` (ADR 0013), the merge
/// classifier will say a shared helper is a contract to keep, and since
/// MCP-020 the task section will say outright that there is no move here — and
/// the headline still read `hierarchical`, which is the line an agent
/// optimises. Reported from the field on a folder at compliance 0.97, entry
/// concentration 1.00, layering 1.00, acyclicity 1.00 and a genuine funnel:
/// "the guidance and the gate disagree about the same folder in the same
/// report".
///
/// A sentence rather than a gate change, which is the reporter's own
/// suggestion and the better trade: dropping `branching` from the tier would
/// remove the ladder's fourth rung and make every recorded verdict
/// incomparable, where this costs nothing and dissolves the contradiction
/// where it is read. The open question of whether the *gate* should change is
/// [MCP-024] and stays open.
fn advisory_qualifier(shape: &FolderShape, t: &Thresholds) -> String {
    if !matches!(shape.blocker, Some(ShapeBlocker::Merges(_))) {
        return String::new();
    }
    let compliant = shape.compliance >= t.shape_compliance
        && shape.acyclicity >= 1.0
        && shape.layering.is_none_or(|l| l >= t.shape_layering);
    if !compliant {
        return String::new();
    }
    " **By every compliance measure this folder is finished** — the only gate \
     left is branching, which is not part of compliance and which the guidance \
     below will often tell you to leave. Read that guidance before treating this \
     tier as work."
        .to_string()
}

/// What "nothing to do" leaves out.
///
/// `entry_concentration` is a ratio, not a count, so at a 0.60 bar a
/// folder clears the gate while up to two fifths of the traffic arriving
/// from outside walks past its door. Three folders in this repo do exactly
/// that, and the verdict used to tell each of them there was nothing to
/// do — directly above a boundary section listing the outsiders reaching
/// in. A maintainer looking at the canvas saw the piercings and the tool
/// denied them.
///
/// The gate genuinely passes, so this is a reservation and not a demand:
/// it says what is tolerated, and where to look if the folder was meant to
/// be entered at one point.
fn fractal_note(shape: &FolderShape, p: &FolderPicture) -> Vec<String> {
    if shape.pattern != ShapePattern::Fractal {
        return Vec::new();
    }
    let breaches = p
        .outside
        .iter()
        .filter(|o| o.verdict == OutsideVerdict::Breach)
        .count();
    let held = "This folder already holds its shape at every level. There is nothing \
                to do here, and changing it to raise a number would be a regression.";
    if breaches == 0 {
        return vec![held.to_string(), String::new()];
    }
    vec![
        format!(
            "{held}\n\nOne reservation the ladder does not charge for: {breaches} \
             {} from outside still {} past the door, which `entry concentration` \
             tolerates because it measures where the traffic *concentrates* rather \
             than how many ways in there are. They are listed under **Across the \
             boundary**. Nothing is required — but if this folder was meant to be \
             entered at one point, that is the list to read.",
            if breaches == 1 {
                "dependency"
            } else {
                "dependencies"
            },
            if breaches == 1 { "reaches" } else { "reach" },
        ),
        String::new(),
    ]
}

/// The one rung above, and the specific things standing in its way.
///
/// Driven entirely by `blocker`, which the analyzer has already decided.
/// Expanding a different gate would be a second opinion about which tier a
/// folder is on, and the two would drift.
fn next_rung_section(
    shape: &FolderShape,
    p: &FolderPicture,
    t: &Thresholds,
    ctx: Ctx<'_>,
) -> Vec<String> {
    let Some(blocker) = shape.blocker else {
        return Vec::new();
    };
    let mut body = vec![format!(
        "## Your task: {} → {}",
        shape.pattern.label(),
        next_tier(shape.pattern)
    )];
    body.push(String::new());

    body.extend(task_lines(blocker, p, t, ctx));
    body.push(String::new());
    body.push(format!(
        "Stop there. The tiers are a ladder and `{}` is the next rung — the gates \
         above it are measured on top of this one and are not worth chasing until \
         it clears.",
        next_tier(shape.pattern),
    ));
    body.push(String::new());
    body
}

/// The work one gate asks for, as its own function.
///
/// Lifted out of [`next_rung_section`] rather than grown there: the arms are
/// one per gate and the complexity gate fails on any increase to a function
/// that already exists (CI-001), so a match that gains an arm every time the
/// ladder does has to be the whole of what its function does. The same move
/// `rules::SPECS` and the MCP tool dispatch already made.
fn task_lines(
    blocker: ShapeBlocker,
    p: &FolderPicture,
    t: &Thresholds,
    ctx: Ctx<'_>,
) -> Vec<String> {
    let mut out = Vec::new();
    match blocker {
        ShapeBlocker::Cycles(_) => out.extend(recipes::cycle_lines(p, ctx.graph, ctx.root)),
        ShapeBlocker::Layering(v) => out.extend(layering_task(p, ctx, v)),
        ShapeBlocker::Merges(v) => out.extend(merges_task(p, ctx, v)),
        ShapeBlocker::Breadth(count) => out.extend(recipes::breadth_lines(p, t, count)),
        ShapeBlocker::Entry(v) => out.extend(recipes::entry_lines(p, t, v)),
        ShapeBlocker::Egress(v) => out.extend(egress_task(p, t, v)),
        ShapeBlocker::ChildPattern(pattern) => {
            out.push(format!(
                "The recursion breaks one level down: a subfolder is itself {}, and \
                 `fractal` is a claim about the shape holding at more than one zoom \
                 level. This folder's own drawing is already fine — the work is \
                 inside it.",
                pattern.label(),
            ));
            out.push(String::new());
            out.push(
                "Call `reshape` on each subfolder below and fix those first. Nothing \
                 done at this level will move the verdict while one of them is \
                 unreadable."
                    .to_string(),
            );
            out.push(String::new());
            out.extend(listed(
                p.children
                    .iter()
                    .filter(|c| c.kind == crate::models::ChildKind::Folder)
                    .map(|c| c.path.clone()),
            ));
        }
        ShapeBlocker::ChildCompliance(v) => {
            out.push(format!(
                "No single subfolder is unreadable, but they average {} against the \
                 {:.2} this gate asks for. Call `reshape` on each and fix the worst; \
                 this level has nothing to change.",
                num(Some(v)),
                t.shape_child,
            ));
            out.push(String::new());
            out.extend(listed(
                p.children
                    .iter()
                    .filter(|c| c.kind == crate::models::ChildKind::Folder)
                    .map(|c| c.path.clone()),
            ));
        }
        ShapeBlocker::Unstructured => {
            out.push(
                "The children have no dependencies between them at all. The drawing \
                 is a legible row of dots, and there is no structure for the \
                 recursion to be self-similar *to* — which is why it stops short of \
                 fractal rather than topping the ladder."
                    .to_string(),
            );
            out.push(String::new());
            out.push(
                "**This is usually not a defect and usually not worth acting on.** A \
                 folder of independent things is a fine folder. Manufacturing \
                 dependencies between them to earn a tier would make the code worse \
                 in exchange for a number. Act on it only if the files here turn out \
                 to be unrelated in a way that means they should not have been filed \
                 together."
                    .to_string(),
            );
        }
        ShapeBlocker::Compliance(v) => {
            out.push(format!(
                "Every gate passed and the blend still came to {} against {:.2} — \
                 several terms a little low rather than one clearly wrong. Look at \
                 the sub-scores above and improve whichever is furthest from its bar; \
                 there is no single offender to name.",
                num(Some(v)),
                t.shape_compliance,
            ));
        }
    }
    out
}

/// The answer to "did what you just did work" — reported by the tool that
/// set the baseline, not by the agent that changed the code.
///
/// Absent on a first call, which is deliberate: the section that appears
/// only the second time is also the announcement that a baseline now
/// exists, and an agent that has seen this once knows the next call will
/// grade it.
/// What the comparator says when it has nothing to compare against.
///
/// It used to say nothing at all, and absence is ambiguous in the worst
/// possible way here: an empty section reads identically to "you have not
/// called this before" and to "nothing changed". Reported from the field on
/// the largest tier move of a session — `hierarchical → fractal`, with two
/// edges appearing, which is precisely what this section exists to confirm —
/// and it was silent.
///
/// The record lives in the server process, so it is empty for a folder never
/// asked about *and* for every folder after a restart. Both are named, because
/// an agent that rebuilt and reloaded between calls needs to know its baseline
/// went with the old process rather than wonder whether its edit did nothing.
fn no_baseline() -> Vec<String> {
    vec![
        "## Since your last call".to_string(),
        String::new(),
        "**No prior drawing on record**, so nothing below is a comparison. This is \
         not \"nothing changed\": this is the first `reshape` on this folder, or the \
         first since the analysis scope changed, or the first since the cache \
         directory was cleared. The record itself outlives the server — it is \
         mirrored under the mezz cache directory, not held in the process — so a \
         rebuild or a reload between two calls no longer loses it. The reading below \
         is now the baseline; re-run after your next edit and this section will diff \
         against it."
            .to_string(),
        String::new(),
    ]
}

fn progress_section(before: Option<&Baseline>, after: &Baseline) -> Vec<String> {
    let Some(before) = before else {
        return no_baseline();
    };
    let mut body = vec!["## Since your last call".to_string(), String::new()];

    if before.same_reading(after) {
        body.push(unchanged_sentence(before, after));
        body.push(String::new());
        return body;
    }

    body.push(format!(
        "- verdict **{}** → **{}**",
        before.pattern.label(),
        after.pattern.label()
    ));
    for (name, was, now) in [
        ("layering", before.layering, after.layering),
        ("branching", before.arborescence, after.arborescence),
        (
            "entry concentration",
            before.entry_concentration,
            after.entry_concentration,
        ),
        ("out at the bottom", before.egress, after.egress),
        (
            "compliance",
            Some(before.compliance),
            Some(after.compliance),
        ),
    ] {
        if was != now {
            body.push(format!("- {name} {} → {}", num(was), num(now)));
        }
    }
    if before.child_count != after.child_count {
        body.push(format!(
            "- children {} → {}",
            before.child_count, after.child_count
        ));
    }
    body.push(String::new());
    body.extend(structural_diff(before, after));
    body.push(String::new());
    body.push(progress_verdict(before, after));
    body.push(String::new());
    body.extend(condensation_note(before, after));
    body.extend(relevelling_note(before, after));
    body.extend(arithmetic_note(before, after));
    body.extend(stray_note(before, after));
    body
}

/// The moved ratios written out as the divisions they came from.
///
/// The before/after twin of the block in the verdict section, and the
/// reason both exist: a reader shown `branching 0.88 → 0.71` and no
/// arithmetic has to guess whether the numerator or the denominator moved,
/// and those two want opposite responses.
fn arithmetic_note(before: &Baseline, after: &Baseline) -> Vec<String> {
    let (b, a) = (&before.terms, &after.terms);
    let rows = [
        ("layering", (b.tight, b.edges), (a.tight, a.edges)),
        (
            "branching",
            (b.reached, b.edges + b.strays.saturating_sub(1)),
            (a.reached, a.edges + a.strays.saturating_sub(1)),
        ),
        (
            "entry concentration",
            (b.busiest, b.arrivals),
            (a.busiest, a.arrivals),
        ),
    ];
    let moved: Vec<String> = rows
        .iter()
        .filter(|(_, was, now)| was != now && (was.1 > 0 || now.1 > 0))
        .map(|(name, was, now)| {
            format!("{name} {} ÷ {} → {} ÷ {}", was.0, was.1, now.0, now.1)
        })
        .collect();
    if moved.is_empty() {
        return Vec::new();
    }
    vec![
        format!("The arithmetic behind those: {}.", moved.join("; ")),
        String::new(),
    ]
}

/// Why `branching` can fall on a regrouping that was right.
///
/// The gap a field report named exactly. This tool warns at length that a
/// tier rising while every edge stays put should be reverted, and says
/// nothing about the opposite: a number falling on a change that removed
/// no dependency. The reporter had dissolved a folder of shared contracts
/// — the right move, and one the forbidden list explicitly permits — and
/// watched `branching` go 0.88 → 0.71 for it, because the two files
/// involved were `import type` targets with no scored edge arriving and so
/// became parentless nodes at the top level. Every root past the first
/// joins the denominator (ADR 0022), so the ratio fell on a change nothing
/// was wrong with, and the report offered no reading of that at all.
fn stray_note(before: &Baseline, after: &Baseline) -> Vec<String> {
    let fell = matches!(
        (before.arborescence, after.arborescence),
        (Some(was), Some(now)) if now < was
    );
    let gained = after.terms.strays.saturating_sub(before.terms.strays);
    if !fell || gained == 0 {
        return Vec::new();
    }
    vec![
        format!(
            "**The fall in `branching` is that arithmetic, not damage.** {gained} more \
             {} in this drawing now {} reached by no edge in it — a child whose only \
             inbound arrow left the level, or one whose only importer reaches it \
             through a statement the build erases. Every root past the first joins the \
             denominator (ADR 0022), so the ratio falls without a single dependency \
             having moved the wrong way. Judge the change on the edges listed above.",
            if gained == 1 { "child" } else { "children" },
            if gained == 1 { "is" } else { "are" },
        ),
        String::new(),
    ]
}

/// "Nothing has changed", and the third reading of it that only a
/// configuration change earns.
///
/// The two-part sentence is left exactly as it was whenever the scope
/// held, on purpose. Its job is to catch an edit that did not land, and a
/// sentence permanently offering three explanations catches nothing —
/// the reader stops reading past "either". The third clause is printed
/// only on the calls where it is true, which is also the only calls where
/// it is the likeliest answer: the drawing is identical *and* the server
/// is no longer analysing the same thing.
fn unchanged_sentence(before: &Baseline, after: &Baseline) -> String {
    const SAME: &str = "Nothing has changed — same verdict, same numbers, same edges. Either the \
                        edit did not land, or it touched nothing this measure looks at.";
    if before.scope == after.scope {
        return SAME.to_string();
    }
    format!(
        "{SAME} Or the configuration did: scope `{}` → `{}` between the two calls, so the \
         drawing this one is graded against was taken under different settings — a \
         `.mezz/settings.json` edit, or a server started with different flags.",
        before.scope, after.scope,
    )
}

/// Which of the two condensed ratios actually fell, worded for the
/// sentence below. `None` when neither did.
///
/// Named individually because they do not move together. On
/// `src/parser/rust` `layering` *rose* 0.42 → 0.43 while `branching` fell
/// 0.42 → 0.39, and a note that said "the fall in layering / branching"
/// asserted a fall that had not happened — which the agent reading it then
/// repeated back as fact.
fn fallen_ratios(before: &Baseline, after: &Baseline) -> Option<(&'static str, &'static str)> {
    let fell = |was: Option<f32>, now: Option<f32>| match (was, now) {
        (Some(a), Some(b)) => b < a,
        _ => false,
    };
    match (
        fell(before.layering, after.layering),
        fell(before.arborescence, after.arborescence),
    ) {
        (true, true) => Some(("The falls in `layering` and `branching` are", "them")),
        (true, false) => Some(("The fall in `layering` is", "it")),
        (false, true) => Some(("The fall in `branching` is", "it")),
        (false, false) => None,
    }
}

/// Why clearing a loop makes the ratios under it look worse.
///
/// `layering` and `arborescence` are measured over the *cycle-condensed*
/// graph, so while a loop exists every edge inside it is invisible to both
/// and the whole loop counts as one node. Breaking it puts those edges
/// into the denominator and splits that node into several, which deepens
/// the levels. A ratio can fall for that reason alone.
///
/// Reported because the alternative was observed: on
/// `src/parser/typescript` a correct fix took the folder from `cyclic` to
/// `tangled` and printed "**Confirmed.**" directly above `layering 0.47 →
/// 0.34` and `branching 0.47 → 0.34`, with nothing to say those two were
/// arithmetic. A reader is entitled to read that as damage and undo it.
fn condensation_note(before: &Baseline, after: &Baseline) -> Vec<String> {
    // Leaving `Cyclic` is exactly "acyclicity reached 1.00" — the tier is
    // defined by that gate — so the pattern answers this without the
    // baseline having to carry the number.
    let loop_cleared =
        before.pattern == ShapePattern::Cyclic && after.pattern != ShapePattern::Cyclic;
    let Some((subject, object)) = fallen_ratios(before, after).filter(|_| loop_cleared) else {
        return Vec::new();
    };
    vec![
        condensation_phrase(subject, object, before, after),
        String::new(),
    ]
}

/// The sentence itself, worded apart from the lookup that decides whether
/// to print it.
///
/// Two things pull these ratios down after a loop breaks, and only one of
/// them is arithmetic. Saying "the drawing did not get worse" covered both
/// and was wrong about the second: the fix this tool prescribes — move the
/// shared definition into a file both ends can depend on — creates a file
/// several children depend on, and that is a real new merge point.
fn condensation_phrase(subject: &str, object: &str, before: &Baseline, after: &Baseline) -> String {
    let trade = new_merge(before, after).map_or_else(String::new, |(child, parents)| {
        format!(
            " The rest is the trade this fix makes: `{child}` is new and {parents} \
             children depend on it, which `branching` charges as a merge — often \
             rightly, and often not, since {}. A loop was exchanged for a merge, which \
             is the better of the two.",
            recipes::CONTRACT_NOT_DEFECT,
        )
    });
    format!(
        "**{subject} expected here.** Measured over the cycle-condensed \
         graph, so while the loop existed the edges inside it were invisible to \
         {object} and the whole loop counted as a single node. Breaking it put those \
         edges into the denominator and split that node into several, which deepens \
         the levels; that part is arithmetic rather than damage.{trade} Judge this \
         change on `acyclicity`, which is what it was aimed at."
    )
}

/// The child that appeared since the last call and is now most depended
/// on, if any. The signature of a definition just moved into a file of its
/// own.
fn new_merge(before: &Baseline, after: &Baseline) -> Option<(String, usize)> {
    let known: BTreeSet<&str> = before
        .edges
        .iter()
        .flat_map(|(f, t)| [f.as_str(), t.as_str()])
        .collect();
    after
        .edges
        .iter()
        .map(|(_, to)| to)
        .filter(|to| !known.contains(to.as_str()))
        .map(|to| {
            let parents = after.edges.iter().filter(|(_, t)| t == to).count();
            (to.clone(), parents)
        })
        .filter(|(_, parents)| *parents > 1)
        .max_by_key(|(_, parents)| *parents)
}

/// Why a fall in `layering` can be the rows moving rather than the
/// drawing getting worse.
///
/// A level is the longest path reaching a child, so it is a global
/// property: removing one edge shortens chains all over the drawing, and
/// two children can move up by different amounts. An edge between two
/// that did is re-read against rows it never touched — a step becomes a
/// skip exactly when its *source* lost more rows than its target, the
/// target being held where it is by a longer path the change never went
/// near.
///
/// The direction is worth stating because the intuitive version of it is
/// impossible. An edge `a → b` pins `b` at least one row below `a` for
/// as long as it exists, so nothing done elsewhere can push `b` away
/// from `a`; only `a` coming up can open the gap. Observed on `src/ui`,
/// where narrowing a modal to take a `number` instead of the whole
/// settings object — plainly better coupling — dropped an edge,
/// `commands → settings` turned from a step into a skip with neither end
/// changing, and `layering` fell 0.58 → 0.545 directly above a verdict
/// telling the agent the change was worth undoing.
///
/// Only `layering` is exposed to this. `branching` counts incoming edges
/// per child and never asks what row anything sits on, so it cannot move
/// for this reason and is not spoken for here.
///
/// The cyclic cases belong to [`condensation_note`] and are left to it:
/// while a loop exists its members share a row by construction, so
/// "the rows moved" is the whole story of breaking one and is already
/// told better there.
fn relevelling_note(before: &Baseline, after: &Baseline) -> Vec<String> {
    let Some(r) = relevelling(before, after) else {
        return Vec::new();
    };
    let mut body = vec![
        format!(
            "**The fall in `layering` is the rows being re-assigned, not this \
             drawing getting worse.** A level is the longest path reaching a \
             child, so removing one edge shortens chains all over the drawing and \
             moves children up by different amounts — and an edge whose two ends \
             moved by different amounts changes reading without anyone having \
             touched it. Scored against the rows of your last call, the drawing \
             you have now comes to {} — no worse than the {} it stood at. These \
             edges are those:",
            num(Some(r.held)),
            num(before.layering),
        ),
        String::new(),
    ];
    body.extend(listed(
        r.reclassified
            .into_iter()
            .map(|(f, t)| format!("`{f} → {t}` — a step last call, a skip now.")),
    ));
    body.push(String::new());
    body.push(
        "Judge this change on the edges it actually moved, listed under *what \
         changed in the drawing* above. Undoing it would recover the number by \
         putting the old rows back — and the dependencies with them."
            .to_string(),
    );
    body.push(String::new());
    body
}

/// A fall in `layering` that the levels explain, with the edges that
/// prove it.
struct Relevelling {
    /// What the current drawing scores under the *previous* call's rows.
    held: f32,
    /// Edges in both drawings that stepped one row then and skip now,
    /// having not themselves changed.
    reclassified: Vec<(String, String)>,
}

/// The arithmetic behind the note: re-run the level assignment over the
/// previous call's edge record, and re-score the current edges under it.
///
/// Deliberately not a heuristic. `None` unless all four hold, so a fall
/// the edge delta *does* explain keeps reading as a fall:
///
/// 1. The agent saw `layering` fall — otherwise there is nothing to
///    explain away.
/// 2. Both edge records agree that it fell. The two rebuilt drawings are
///    scored the same way, so a disagreement means the stored numbers and
///    the stored edges describe different moments, and neither claim is
///    safe to make.
/// 3. Under the old rows the current drawing holds its ground. This is
///    the whole test: if the edges that changed had cost anything, they
///    would cost it under either ruler.
/// 4. Some edge present in both drawings reads differently in them. That
///    is the sentence that stops the undo, and without one there is
///    nothing specific to say.
fn relevelling(before: &Baseline, after: &Baseline) -> Option<Relevelling> {
    // A loop makes its members share a row by construction, which is
    // `condensation_note`'s subject and not this one's.
    if before.pattern == ShapePattern::Cyclic || after.pattern == ShapePattern::Cyclic {
        return None;
    }
    if !matches!((before.layering, after.layering), (Some(a), Some(b)) if b < a) {
        return None;
    }
    let old = levels_of(&before.edges);
    let new = levels_of(&after.edges);
    let (was, now) = (
        layering_under(&before.edges, &old, &old)?,
        layering_under(&after.edges, &new, &new)?,
    );
    let held = layering_under(&after.edges, &old, &new)?;
    if now >= was || held < was {
        return None;
    }
    let reclassified: Vec<(String, String)> = before
        .edges
        .intersection(&after.edges)
        .filter(|(f, t)| tight(&old, f, t) && !tight(&new, f, t))
        .cloned()
        .collect();
    if reclassified.is_empty() {
        return None;
    }
    Some(Relevelling { held, reclassified })
}

/// The row each child of a kept drawing draws on, from the analyzer's own
/// assignment rather than a second one written here.
fn levels_of(edges: &BTreeSet<(String, String)>) -> HashMap<String, u32> {
    crate::analyzer::folder_shape::levels_by_name(
        edges.iter().map(|(f, t)| (f.as_str(), t.as_str())),
    )
}

/// Whether an edge steps exactly one row down under `levels` — the test
/// `layering` counts with, asked of one edge.
fn tight(levels: &HashMap<String, u32>, from: &str, to: &str) -> bool {
    matches!((levels.get(from), levels.get(to)), (Some(f), Some(t)) if *t == f + 1)
}

/// `layering` for `edges`, read against `rows` — the rows of whichever
/// drawing the caller wants them judged by.
///
/// An edge whose ends the old drawing never had is judged under `own`,
/// the rows its own drawing gives it. A node with no old row has no old
/// reading, so its edges are the drawing changing and belong on that side
/// of the arithmetic; scoring them as skips because the previous call had
/// never heard of them would make every added file look like an artefact.
fn layering_under(
    edges: &BTreeSet<(String, String)>,
    rows: &HashMap<String, u32>,
    own: &HashMap<String, u32>,
) -> Option<f32> {
    if edges.is_empty() {
        return None;
    }
    let steps = edges
        .iter()
        .filter(|(f, t)| {
            if rows.contains_key(f) && rows.contains_key(t) {
                tight(rows, f, t)
            } else {
                tight(own, f, t)
            }
        })
        .count();
    Some(steps as f32 / edges.len() as f32)
}

/// The edges that came and went, which is the evidence behind the verdict
/// below and the thing an agent cannot restate from memory.
fn structural_diff(before: &Baseline, after: &Baseline) -> Vec<String> {
    let mut moved = changed_pairs(&before.edges, &after.edges, "");
    moved.extend(changed_pairs(
        &before.outside,
        &after.outside,
        " across the boundary",
    ));
    if moved.is_empty() {
        return vec![
            "No edge between these children changed, and nothing arriving from \
             outside landed anywhere new."
                .to_string(),
        ];
    }
    let mut body = vec!["What changed in the drawing:".to_string(), String::new()];
    body.extend(every_change(moved));
    body
}

/// How many edge changes get listed before the rest are rolled up.
///
/// Higher than [`MAX_LISTED`] on purpose. Twenty is the right bar for a
/// list of offenders, where the point is made by the first few and the
/// endpoint holds the rest; this list is the *evidence for the verdict*,
/// and an agent reported it cutting off at "… and 36 more" precisely on
/// the call where the restructure was largest — which is the call whose
/// evidence matters most.
const MAX_EDGE_CHANGES: usize = 60;

/// The changed edges, and — when even that many is not enough — which
/// files the rest of them touched.
///
/// A bare "… and 36 more" is the one truncation this report cannot
/// afford: it drops the answer to "did the thing I meant to change
/// actually change" at the moment the answer is least guessable. The
/// rollup is not the full list, but it is the part a reader is looking
/// for — the files involved — rather than a count of what they cannot
/// see.
fn every_change(moved: Vec<String>) -> Vec<String> {
    let total = moved.len();
    if total <= MAX_EDGE_CHANGES {
        return moved.into_iter().map(|m| format!("- {m}")).collect();
    }
    let (shown, rest) = moved.split_at(MAX_EDGE_CHANGES);
    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    for change in rest {
        for file in change.split('`').skip(1).step_by(2) {
            for endpoint in file.split(" → ") {
                *tally.entry(endpoint).or_default() += 1;
            }
        }
    }
    let files: Vec<String> = tally
        .iter()
        .map(|(file, count)| format!("`{file}` ({count})"))
        .collect();
    let mut body: Vec<String> = shown.iter().map(|m| format!("- {m}")).collect();
    body.push(format!(
        "- … and {} more, touching {}.",
        rest.len(),
        files.join(", ")
    ));
    body
}

/// What moved between two edge sets, added and removed.
///
/// Run over the boundary crossings as well as the drawn edges, because
/// `same_structure` counts both and this section is the evidence for its
/// verdict. It used to list only child-to-child edges, so narrowing a door
/// from three landings to one — the change the boundary section had asked
/// for — was reported as "No edge between these children changed", which
/// reads as *nothing happened* directly above an instruction to revert
/// anything cosmetic. Reported from the field as the third case this session
/// of the comparator disowning a real improvement, and the first caused by
/// the two halves of the report measuring at different granularities.
fn changed_pairs(
    before: &BTreeSet<(String, String)>,
    after: &BTreeSet<(String, String)>,
    where_: &str,
) -> Vec<String> {
    let gone = before
        .difference(after)
        .map(|(f, t)| format!("removed `{f} → {t}`{where_}"));
    let added = after
        .difference(before)
        .map(|(f, t)| format!("added `{f} → {t}`{where_}"));
    gone.chain(added).collect()
}

/// The judgement, in the terms the closing instruction is written in.
fn progress_verdict(before: &Baseline, after: &Baseline) -> String {
    let structural = !before.same_structure(after);
    match (after.pattern.cmp(&before.pattern), structural) {
        (std::cmp::Ordering::Greater, true) => format!(
            "**Confirmed.** The tier rose to `{}` and the dependencies behind it \
             actually changed. This is the outcome the closing instruction asks for.",
            after.pattern.label(),
        ),
        (std::cmp::Ordering::Greater, false) => {
            "**Revert this.** The tier rose while every edge, every boundary crossing \
             and the child count stayed identical — so nothing depends on anything \
             different than it did before, and the number that moved is now describing \
             a drawing that did not change. This is the failure mode the forbidden \
             list below exists to catch."
                .to_string()
        }
        (std::cmp::Ordering::Less, true) => format!(
            "The tier **fell** to `{}`, and real dependencies changed. Three things \
             do that, and only one of them is a mistake.\n\n\
             - **You grouped children into subfolders.** Expected: a new subfolder is \
             a new child that has to earn its own verdict, and the work continues \
             inside it.\n\
             - **You extracted or de-duplicated something.** Also expected, and the \
             one this tool used to get wrong. A tier reads the *drawing*, not whether \
             the code got better: pulling a repeated policy into one place adds a node \
             and the edges into it, and a folder can be genuinely better to work in \
             while drawing worse. Ratios move for the same reason — removing a \
             dependency on the door lowers `entry concentration`, because the door's \
             share of a smaller total is what that number is.\n\
             - **The folder actually got harder to read.** Then it is worth undoing.\n\n\
             Decide which by reading *what moved* under **what changed** below, not by \
             the tier alone. A test that existed to guard behaviour you have now \
             centralised is evidence for the second reading, not the third.",
            after.pattern.label(),
        ),
        (std::cmp::Ordering::Less, false) => {
            "**The tier fell without any dependency changing.** That should not be \
             possible from an edit to this folder alone; something outside it moved, \
             or the earlier reading was taken mid-edit. Re-run before acting on it."
                .to_string()
        }
        (std::cmp::Ordering::Equal, true) => format!(
            "Real dependency changes landed and the verdict is still `{}`. That is not \
             a failure — the gate below names what is left. Check it is the same \
             blocker as before; a different one means you cleared the old gate.",
            after.pattern.label(),
        ),
        (std::cmp::Ordering::Equal, false) => {
            "Numbers moved, no dependency did. Whatever changed was cosmetic as far as \
             this measure is concerned."
                .to_string()
        }
    }
}

/// What the recipes need beyond the drawing: the entity graph, and the
/// root every path in the drawing was made relative to.
#[derive(Clone, Copy)]
pub(super) struct Ctx<'a> {
    pub graph: &'a DependencyGraph,
    pub root: &'a Path,
}

/// One level-skipping edge, noting when it is also a redundant path.
///
/// The join between the two gates, and deliberately narrow. An earlier
/// version flagged every skip whose target had more than one parent; on
/// `src/educator` that fired on all eleven listed edges, which is noise
/// rather than a finding, and it promised "one removal clears two gates"
/// for edges whose removal would clear neither. A shared target is
/// ordinary. A sibling the source *already reaches the target through* is
/// the edge that carries nothing, and that is the one worth naming.
fn skip_line(p: &FolderPicture, e: &PictureEdge, vocab: &recipes::Vocabulary) -> String {
    format!("{} → {}{}", e.from, e.to, vocabulary_note(vocab, p, e))
}

/// Either the vocabulary mark or the redundant-path one, never both.
///
/// A leaf that depends on no sibling cannot be reached *through* one
/// without that sibling re-exporting it, so the candidate the second mark
/// offers is the shim this tool forbids. Printing it beside the paragraph
/// that rules it out is the tool arguing with itself in front of the
/// reader.
fn vocabulary_note(vocab: &recipes::Vocabulary, p: &FolderPicture, e: &PictureEdge) -> String {
    match vocab.get(&e.to) {
        Some(leaf) => format!(
            " — a shared vocabulary leaf holding {} declarations, read below",
            leaf.declares.len(),
        ),
        None => redundant_note(p, e),
    }
}

/// The note appended to a skip whose source has another path to the same
/// target. Empty for every other skip.
///
/// Deliberately says only what the graph knows. An earlier version called
/// such an edge "redundant" and said removing it "straightens the level
/// and drops a merge at once", which reads as *delete the import* — and on
/// `src/educator` that was wrong nine times out of nine. `corpus.rs`
/// reaching `lessons.rs` "through" `validator.rs` does not mean
/// `validator.rs` hands it what it needs; both merely depend on the same
/// file, and cutting the direct edge would take a re-export. A path is a
/// candidate. The paragraph under the list carries the disqualifying test.
fn redundant_note(p: &FolderPicture, e: &PictureEdge) -> String {
    let Some(middle) = recipes::routed_through(p, &e.from, &e.to) else {
        return String::new();
    };
    format!(" — also reachable through `{middle}`")
}

/// The `Layering` instruction: the edges that jump levels, which of them
/// have another path from the same source, and what that does and does
/// not prove (ADR 0016).
fn layering_task(p: &FolderPicture, ctx: Ctx<'_>, v: f32) -> Vec<String> {
    let skips: Vec<&PictureEdge> = p
        .edges
        .iter()
        .filter(|e| e.verdict == EdgeVerdict::Skip)
        .collect();
    let vocab = recipes::vocabulary(p, ctx.graph, ctx.root);
    let mut body = vec![
        format!(
            "Straighten the edges that jump levels. {} of {} edges step exactly one \
             level down ({}); these {} do not, and each is an edge a reader has to \
             hold in their head while following the rest.",
            p.edges.len() - skips.len(),
            p.edges.len(),
            num(Some(v)),
            skips.len(),
        ),
        String::new(),
    ];
    body.extend(listed(skips.iter().map(|e| skip_line(p, e, &vocab))));
    body.push(String::new());
    body.extend(routed_caveat(p, &skips, &vocab));
    body.extend(recipes::vocabulary_lines(
        &vocab, ctx.graph, ctx.root, &skips,
    ));
    body.push(honest_fixes(&skips, &vocab));
    body
}

/// The two structural fixes this gate is built around, and what is left
/// where neither is available.
///
/// Printed unconditionally until a reviewer was offered the same two for
/// five consecutive rounds about the same edges, every one of which
/// landed on one shared vocabulary leaf. Neither could be carried out,
/// and advice that cannot apply, repeated, teaches a reader to stop
/// reading it.
///
/// Not phrased as "these two do not apply here" either. A reader handed
/// the moves and told not to make them still has the moves; where every
/// skip lands on vocabulary they are simply not on the page.
fn honest_fixes(skips: &[&PictureEdge], vocab: &recipes::Vocabulary) -> String {
    let elsewhere = skips.iter().filter(|e| !vocab.contains_key(&e.to)).count();
    if elsewhere == 0 {
        return "There is nothing else to do at this gate. Every edge above lands on a \
                leaf named as vocabulary, and the work is the check printed under each \
                of them; if those come back clean, this folder's `layering` is what a \
                vocabulary its children share costs, and the honest move is to leave \
                it where it is."
            .to_string();
    }
    let scope = if elsewhere == skips.len() {
        String::new()
    } else {
        format!("For the {elsewhere} above not named as vocabulary: ")
    };
    format!(
        "{scope}Two honest fixes, and they point opposite ways: route the traffic \
         through the layer it skipped, or accept that the shortcut is the real design \
         and remove the layer that is not carrying it. Adding a pass-through wrapper \
         to make the edge look routed is neither."
    )
}

/// What an alternative path does and does not prove.
///
/// The load-bearing paragraph of this gate. Without it the marks above
/// read as "delete these imports", which is what an agent did with them
/// on `src/educator` — where all nine were files that merely shared a
/// dependency, and cutting any of them would have taken a re-export.
fn routed_caveat(
    p: &FolderPicture,
    skips: &[&PictureEdge],
    vocab: &recipes::Vocabulary,
) -> Vec<String> {
    let routed = skips
        .iter()
        .filter(|e| !vocab.contains_key(&e.to))
        .filter(|e| recipes::routed_through(p, &e.from, &e.to).is_some())
        .count();
    if routed == 0 {
        return Vec::new();
    }
    vec![
        format!(
            "{routed} of those have another path from the same source, marked above. \
             **That makes them candidates, not conclusions.** The edge only goes away \
             if what the source needs can genuinely be served through the file in \
             between — the way a parent that tokenizes purely to feed a parser can \
             hand that job over and stop naming tokens at all. If the file in between \
             would have to re-export what the source uses, that is the re-export shim \
             on the forbidden list below, and the edge is real. Read the two files \
             before believing the arrow."
        ),
        String::new(),
    ]
}

/// The `Merges` instruction: the gate, the classified offenders, and why
/// this gate is allowed to disagree with `layering`.
///
/// Two situations reach this gate and a folder can be in either or both, so
/// nothing here assumes the loss came from merges. A child with several
/// parents and a child with none are both the drawing failing to be a tree
/// (ADR 0022), and the fixes point in opposite directions — one removes an
/// edge, the other wants one that is missing.
fn merges_task(p: &FolderPicture, ctx: Ctx<'_>, v: f32) -> Vec<String> {
    let merges = merge_points(p);
    let strays = stray_children(p);
    let mut body = vec![
        merges_opening(p, v, merges.len(), strays.len()),
        String::new(),
    ];

    if !merges.is_empty() {
        body.extend(merge_findings(p, ctx, &merges));
        body.push(String::new());
        body.extend(merge_instruction(p, ctx, &merges, strays.len(), v));
    }

    if strays.len() > 1 {
        if !merges.is_empty() {
            body.push(String::new());
        }
        body.extend(stray_findings(&strays, p, ctx));
    }
    body
}

/// What to do about the merges — which is sometimes nothing.
///
/// The gate charges for convergence, and [`recipes`] can tell a convergence
/// that is a defect from one that is a data contract between a producer and a
/// consumer. Until now only the *prose listing* consulted that, so a folder
/// whose every merge the tool had just called legitimate was still handed
/// "give the drawing the shape of a tree" and told it could clear the gate
/// "with one removal". For that folder both sentences are false: there is no
/// removal, and the tier is unreachable.
///
/// An unreachable bar is not a neutral inaccuracy. The moves that would clear
/// it — a re-export shim, splitting the shared type, routing one sibling
/// through the other — are the ones this same tool forbids two sections
/// further down, so an instruction to clear it is an instruction to do the
/// forbidden thing. Saying "this is the ceiling, and it is not a defect" is
/// the honest reading and the one that stops the churn.
fn merge_instruction(
    p: &FolderPicture,
    ctx: Ctx<'_>,
    merges: &BTreeMap<String, Vec<String>>,
    strays: usize,
    v: f32,
) -> Vec<String> {
    let mut body = vec![
        format!(
            "This is the one gate that disagrees with `layering` on purpose: several \
             files leaning on one helper steps one level cleanly and still merges, so \
             layering scores it 1.00 while branching marks it down. Both readings are \
             right, and this folder is at {} on the second. Moving a shared helper down \
             a level does not fix it — it is still shared.",
            num(Some(v)),
        ),
        String::new(),
    ];
    if !every_merge_is_a_contract(p, ctx, merges) {
        body.push(
            "**Fix one merge point, not all of them.** They are listed worst-first and \
             the classification above says which are real; a folder can clear this gate \
             with one removal while a legitimate contract stays exactly where it is."
                .to_string(),
        );
        return body;
    }
    // Strays charge this gate too and they *are* fixable, so a folder with
    // more than one still has a move — just not among the merges. Saying
    // "fix one merge point" here would point at the only things above that
    // were each just marked *leave it*.
    if strays > 1 {
        body.push(
            "**No merge above is a defect** — every one is a shared contract, and each \
             was just marked *leave it*. What is left charging this gate is the strays \
             below, and those are the only part of it worth your attention."
                .to_string(),
        );
        return body;
    }
    body.push(
        "**There is no move here.** Every merge above is a shared contract — \
         independent siblings agreeing on the same types out of one child, which is \
         what a producer and a consumer over one data shape look like. This gate \
         charges for it because the drawing converges, and that reading is correct; \
         it is not a defect, and it is not fixable without making the code worse."
            .to_string(),
    );
    body.push(String::new());
    body.push(
        "So this folder is at its ceiling on branching, by design rather than by \
         neglect. Do not clear it: the three moves that would — re-exporting the \
         child through one sibling, splitting the shared type, or routing one \
         sibling through the other — are on the forbidden list below, and each \
         changes the picture without changing the program. Read the tier as \
         *held by a contract*, and spend the effort on a gate that names something \
         wrong."
            .to_string(),
    );
    body
}

/// Whether every merge point in the drawing is a data contract rather than a
/// defect. The same classification the listing ranks by, asked as a question
/// about the folder instead of about one merge.
fn every_merge_is_a_contract(
    p: &FolderPicture,
    ctx: Ctx<'_>,
    merges: &BTreeMap<String, Vec<String>>,
) -> bool {
    merges.iter().all(|(child, parents)| {
        matches!(
            recipes::classify(p, ctx.graph, ctx.root, child, parents),
            recipes::MergeShape::SharedContract { .. }
        )
    })
}

/// What this folder's branching score is actually made of.
fn merges_opening(p: &FolderPicture, v: f32, merges: usize, strays: usize) -> String {
    let mut clauses: Vec<String> = Vec::new();
    if merges > 0 {
        clauses.push(format!(
            "{merges} {} depended on by more than one sibling, so the drawing \
             converges where it should branch",
            if merges == 1 {
                "child is"
            } else {
                "children are"
            },
        ));
    }
    if strays > 1 {
        clauses.push(format!(
            "{strays} of its {} children answer to nothing inside the folder, where a \
             tree has one root and everything else hangs off it",
            p.children.len(),
        ));
    }
    format!(
        "Give the drawing the shape of a tree. It scores {} on branching: {}.",
        num(Some(v)),
        clauses.join("; and "),
    )
}

/// Every child no edge in the drawing arrives at. One of them is the
/// folder's head and costs nothing; the rest are what `arborescence`
/// charges for past the first.
fn stray_children(p: &FolderPicture) -> Vec<String> {
    let reached: HashSet<&str> = p.edges.iter().map(|e| e.to.as_str()).collect();
    let mut out: Vec<String> = p
        .children
        .iter()
        .filter(|c| !reached.contains(c.path.as_str()))
        .map(|c| c.path.clone())
        .collect();
    out.sort();
    out
}

/// What the stray list is worth when this folder has imports the graph never
/// resolved.
///
/// A child with no parent is *exactly* what a missing import edge fabricates,
/// which makes this the one finding an unresolved import can invent outright.
/// Reported from the field: a folder of three algorithm files was told its
/// children had no parent and offered three diagnoses — head does not reach
/// them, folder is really two, folder is a bag — while the parent existed in
/// the source and imported both of them. All three were false, and the prose
/// is as confident as it is for a real finding.
///
/// The banner at the top of the report says "provisional", and that is not
/// enough here: provisional reads as *the number may be off*, not *this task
/// is invented*. So the suspicion is repeated where the instruction is, in
/// the one section it can invalidate completely.
fn stray_caveat(p: &FolderPicture, ctx: Ctx<'_>) -> Vec<String> {
    let folder = ctx.root.join(&p.folder);
    if ctx
        .graph
        .unresolved_imports_touching(&folder.display().to_string())
        == 0
    {
        return Vec::new();
    }
    vec![
        "**Check the unresolved imports listed at the top before reading this \
         list.** A child looks parentless when the edge that would have reached it \
         is one the graph does not hold, and every diagnosis below — a head that \
         does not reach them, a folder that is really two, a bag of unrelated \
         files — is wrong in that case. Resolve those statements first; what \
         survives is the finding."
            .to_string(),
        String::new(),
    ]
}

/// The stray instruction. Deliberately not symmetrical with the merge one:
/// a merge is fixed by removing an edge, and nothing here is fixed by
/// adding one.
fn stray_findings(strays: &[String], p: &FolderPicture, ctx: Ctx<'_>) -> Vec<String> {
    let mut body = vec![
        "These children have no parent in the drawing:".to_string(),
        String::new(),
    ];
    body.extend(stray_caveat(p, ctx));
    body.extend(listed(strays.iter().cloned()));
    body.push(String::new());
    body.push(
        "One of them is the folder's head and belongs at the top. For the rest, the \
         question is which of three this folder is: a folder whose head genuinely \
         does not reach them, where the fix is to move each one under whatever does \
         use it — often a subfolder that holds a part of the drawing; a folder that \
         is really two, where the fix is to split it along the line nothing crosses; \
         or a folder of unrelated files that outsiders reach individually, which is a \
         bag rather than a tree and is honest to leave alone if that is genuinely \
         what it is."
            .to_string(),
    );
    body.push(String::new());
    body.push(
        "**Do not give them a parent to give them a parent.** A file that imports \
         every stray so each one has an arrow pointing at it raises this number to \
         1.00, adds a hop to every reader, and changes nothing about who depends on \
         what — it is the pass-through layer on the forbidden list, pointed at nodes \
         instead of edges."
            .to_string(),
    );
    body
}

/// Every child with more than one parent, and who those parents are.
fn merge_points(p: &FolderPicture) -> BTreeMap<String, Vec<String>> {
    let mut leaned_on: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &p.edges {
        leaned_on
            .entry(e.to.clone())
            .or_default()
            .push(e.from.clone());
    }
    leaned_on.retain(|_, from| from.len() > 1);
    leaned_on
}

/// The merge points, classified, actionable ones first.
///
/// The ordering is the point. Printed in path order, a legitimate shared
/// contract can head the list and become the thing an agent works on,
/// which is the one outcome this classification exists to prevent.
fn merge_findings(
    p: &FolderPicture,
    ctx: Ctx<'_>,
    merges: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut ranked: Vec<(u8, &String, &Vec<String>)> = merges
        .iter()
        .map(|(child, parents)| {
            let rank = match recipes::classify(p, ctx.graph, ctx.root, child, parents) {
                recipes::MergeShape::PassThrough { .. } => 0,
                recipes::MergeShape::Plain => 1,
                recipes::MergeShape::SharedContract { .. } => 2,
            };
            (rank, child, parents)
        })
        .collect();
    ranked.sort_by_key(|(rank, child, _)| (*rank, (*child).clone()));

    // Computed before the bullets, because the first bullet's prose promises
    // this breakdown and must not promise one that turns out to be empty —
    // `split_lines` returns nothing when fewer than two dependents could be
    // measured, and the pointer used to be printed unconditionally.
    let written = recipes::written(ctx.graph, ctx.root, p);
    let breakdown = ranked
        .first()
        .map(|(_, child, parents)| {
            recipes::split_lines(ctx.graph, ctx.root, &written, child, parents)
        })
        .unwrap_or_default();

    let mut body = Vec::new();
    for (i, (_, child, parents)) in ranked.iter().take(MAX_LISTED).enumerate() {
        body.extend(recipes::merge_lines(
            p,
            ctx.graph,
            ctx.root,
            child,
            parents,
            promises_breakdown(i, &breakdown),
        ));
    }
    if ranked.len() > MAX_LISTED {
        body.push(format!("- … and {} more.", ranked.len() - MAX_LISTED));
    }
    // The usage breakdown, for the one merge point actually being worked
    // on. Printing it per offender would bury the instruction under data
    // about merges the reader was just told to leave alone.
    body.extend(breakdown);
    body
}

/// The drawing itself, by level, so the reading order is on the page.
fn drawing_section(p: &FolderPicture, ctx: Ctx<'_>) -> Vec<String> {
    let mut body = vec![
        format!(
            "## The drawing ({} children, {} edges)",
            p.children.len(),
            p.edges.len()
        ),
        String::new(),
        "This is exactly what the canvas renders when collapsed to this folder — \
         its immediate children, each subfolder as one node — and exactly the graph \
         the numbers above were computed over. Levels are the longest path from a \
         source; children in one dependency loop share a level, a loop having no \
         internal order."
            .to_string(),
        String::new(),
    ];

    let mut by_level: BTreeMap<u32, Vec<&PictureChild>> = BTreeMap::new();
    for child in &p.children {
        by_level.entry(child.level).or_default().push(child);
    }
    for (level, children) in by_level {
        let names: Vec<String> = children
            .iter()
            .map(|c| {
                let mut note = Vec::new();
                if c.is_door {
                    note.push("door".to_string());
                }
                if c.inbound > 0 {
                    note.push(format!("{} in from outside", c.inbound));
                }
                if c.kind == crate::models::ChildKind::Folder {
                    note.push("subfolder".to_string());
                }
                if note.is_empty() {
                    c.path.clone()
                } else {
                    format!("{} [{}]", c.path, note.join(", "))
                }
            })
            .collect();
        body.push(format!("- **level {}**: {}", level, names.join(" · ")));
    }

    if !p.edges.is_empty() {
        body.push(String::new());
        body.push("Edges, with how each reads:".to_string());
        body.push(String::new());
        let landings = recipes::landings(ctx.graph, ctx.root, p);
        let doors = recipes::child_doors(ctx.graph, ctx.root, p);
        let written = recipes::written(ctx.graph, ctx.root, p);
        body.extend(listed(p.edges.iter().map(|e| {
            format!(
                "{} → {} — {}{}{}",
                e.from,
                e.to,
                edge_note(e.verdict),
                recipes::landing_note(&landings, &doors, &e.from, &e.to),
                recipes::written_note(&written, &e.from, &e.to),
            )
        })));
    }
    // Both outside the guard above: a folder whose only written
    // dependency is a shim, or whose every arrow is erased, draws no
    // edges at all — which is the case each of these most needs to speak
    // up in.
    body.extend(erased_lines(p, ctx));
    body.extend(shim_lines(recipes::reexports(ctx.graph, ctx.root, p)));
    body.push(String::new());
    body
}

/// The re-exports written between the folder's children, under the edge
/// list they are deliberately not part of.
///
/// Separate from [`drawing_section`] so that function's shape does not
/// move — the repo's complexity gate charges any increase on an existing
/// function, and this is the second block it would have grown.
fn shim_lines(shims: Vec<String>) -> Vec<String> {
    if shims.is_empty() {
        return Vec::new();
    }
    let mut body = vec![
        String::new(),
        "Re-exports between these children — written dependencies with no arrow above, \
         because a name passed on couples the caller to what is behind the shim rather \
         than to the shim. On the forbidden list below, and this is where one already \
         is:"
            .to_string(),
        String::new(),
    ];
    body.extend(listed(shims));
    body
}

/// The arrows the build erases, under the edge list they are deliberately
/// not part of (ADR 0026).
///
/// The reader has to be told twice over. An `import type` is coupling
/// they follow — the type has to be found and read — so an arrow that
/// simply vanished from the drawing would be the tool hiding a dependency
/// the code contains. And the numbers above do not count it, so listing it
/// among the scored edges would make the denominator a lie. Beside them,
/// named as discounted, is the only placement that is true to both.
///
/// The failure this comes out of: a repo held at `tangled` by five level
/// skips, all five of them `import type` lines onto one declarations
/// module, reported for five consecutive rounds. Five rounds of work
/// against a shape the shipped artifact does not have.
fn erased_lines(p: &FolderPicture, ctx: Ctx<'_>) -> Vec<String> {
    if p.erased.is_empty() {
        return Vec::new();
    }
    let written = recipes::written(ctx.graph, ctx.root, p);
    let mut body = vec![
        String::new(),
        format!(
            "Discounted — {} {} the build erases, drawn here and counted in \
             nothing above. Every import statement behind each of them is a \
             TypeScript `import type` or a Python `if TYPE_CHECKING:` import, so \
             the compiler deletes the line and no bundler ever resolves the \
             specifier:",
            p.erased.len(),
            if p.erased.len() == 1 { "arrow" } else { "arrows" },
        ),
        String::new(),
    ];
    body.extend(listed(p.erased.iter().map(|e| {
        format!(
            "{} → {}{}",
            e.from,
            e.to,
            recipes::written_note(&written, &e.from, &e.to),
        )
    })));
    body.push(String::new());
    body.push(
        "One ordinary import among them and the arrow would be above instead: the \
         module would be in the bundle. These are still coupling a reader carries — \
         the type has to be found and followed — so treat them as a reason to \
         look, not as a runtime dependency to go hunting for."
            .to_string(),
    );
    body
}

fn edge_note(v: EdgeVerdict) -> &'static str {
    match v {
        EdgeVerdict::Step => "steps one level down (the shape you want)",
        EdgeVerdict::Skip => "**skips a level**",
        EdgeVerdict::Back => "**inside a dependency loop**",
    }
}

/// Who crosses the boundary, and through which door.
fn boundary_section(p: &FolderPicture) -> Vec<String> {
    if p.outside.is_empty() {
        return vec![
            "## Across the boundary".to_string(),
            String::new(),
            "Nothing outside this folder depends on it, and it depends on nothing \
             outside. It is an island."
                .to_string(),
            String::new(),
        ];
    }
    let count = |v: OutsideVerdict| p.outside.iter().filter(|o| o.verdict == v).count();
    let (entries, breaches, exits) = (
        count(OutsideVerdict::Entry),
        count(OutsideVerdict::Breach),
        count(OutsideVerdict::Exit),
    );

    let mut body = vec![
        "## Across the boundary".to_string(),
        String::new(),
        format!(
            "One hop out, never transitively. {} dependencies arrive through a door, \
             {} reach past it into the interior, and {} leave the folder.",
            entries, breaches, exits,
        ),
        String::new(),
        format!(
            "Doors — the busiest file(s) depended on from outside, which is what \
             `entry concentration` measures: {}",
            if p.doors.is_empty() {
                "none".to_string()
            } else {
                p.doors.join(", ")
            },
        ),
        String::new(),
    ];
    if breaches > 0 {
        body.push("Reaching past the door:".to_string());
        body.push(String::new());
        body.extend(listed(
            p.outside
                .iter()
                .filter(|o| o.verdict == OutsideVerdict::Breach)
                .map(|o| format!("{} → {}", o.outside, o.inside)),
        ));
        body.push(String::new());
    }
    if exits > 0 {
        body.extend(exit_section(p, exits));
    }
    body
}

/// The work the egress gate asks for (AN-028 step 3, ADR 0031).
///
/// Built from the same [`ExitOrigin`] classification the *Across the
/// boundary* inventory prints, so the gate and the listing cannot disagree
/// about which exits are the leak.
///
/// Names three legal moves and one forbidden one, because a gate that names
/// a defect without naming a move is a gate that gets worked around — the
/// failure AN-028 recorded from the field, where agents met the one-door
/// rule with a re-export because it was the only route anyone had priced.
/// The ambient-type move is spelled out for the same reason: it is the one
/// case where the obvious reading of "exits only from leaves" would flatten
/// a folder into a bag, and an agent that cannot find it reaches for a shim.
fn egress_task(p: &FolderPicture, t: &Thresholds, v: f32) -> Vec<String> {
    let leaves = leaf_children(p);
    let doors: BTreeSet<&str> = p
        .children
        .iter()
        .filter(|c| c.is_door)
        .map(|c| c.path.as_str())
        .collect();
    let middle: Vec<&OutsideEdge> = p
        .outside
        .iter()
        .filter(|o| o.verdict == OutsideVerdict::Exit)
        .filter(|o| ExitOrigin::of(&o.child, &leaves, &doors) == ExitOrigin::Middle)
        .collect();
    let total = p
        .outside
        .iter()
        .filter(|o| o.verdict == OutsideVerdict::Exit)
        .count();
    // The gate's own question asked at each step, rather than multiplied
    // out — `entry_lines` does the same, and for the same reason: a recipe
    // that disagrees with the gate it serves is worse than no recipe.
    let allowed = (0..=middle.len())
        .rev()
        .find(|k| 1.0 - *k as f32 / total.max(1) as f32 >= t.shape_egress)
        .unwrap_or(0);

    let mut out = vec![format!(
        "Move the exits to the bottom. This folder scores {}: {} of its {} outgoing \
         dependencies start at a child in the middle of the drawing — one that still \
         depends on a sibling — so it reaches out of the building and down onto its \
         neighbours at once. The levels drawn above it say the first and not the \
         second, which is what makes the collapsed node dishonest.",
        num(Some(v)),
        middle.len(),
        total,
    )];
    out.push(String::new());
    out.push(format!(
        "{} of the {} would have to leave from a leaf or from the door to clear {:.2}.",
        middle.len() - allowed,
        middle.len(),
        t.shape_egress,
    ));
    out.push(String::new());
    out.extend(listed(
        middle
            .iter()
            .map(|e| format!("`{}` → `{}`", e.inside, e.outside)),
    ));
    out.push(String::new());
    out.extend(egress_moves());
    out
}

/// The three moves that clear this gate, and the one that only moves the
/// number. Written out rather than summarised, because each is a different
/// diagnosis of the same edge and the reader has to pick.
fn egress_moves() -> Vec<String> {
    vec![
        "Each edge above is one of three things, and they want different moves:"
            .to_string(),
        String::new(),
        "- **A dependency that belongs further down.** The child uses the outside \
         thing on behalf of something below it. Push the use down to the leaf that \
         actually needs it; the middle child then depends on its sibling and nothing \
         else, which is the shape already drawn."
            .to_string(),
        "- **A child sitting at the wrong level.** It has no business depending on \
         its siblings at all, and the sibling edge is the accident rather than the \
         exit. Cut that edge and the child becomes a leaf, which clears this gate \
         and usually `layering` with it."
            .to_string(),
        "- **A wide ambient type.** One context or state type most of the folder \
         names — the case where \"exits only from leaves\" would otherwise push you \
         to flatten the folder into a bag. Do not. Declare the slice this folder \
         actually needs as a local interface and depend on that: under structural \
         typing the import disappears, and under nominal typing it moves to one \
         file. This is the move to reach for before any of the others."
            .to_string(),
        String::new(),
        "**Not this: a re-export from a leaf.** Routing the same dependency through \
         a leaf so the edge starts there leaves the middle child coupled to exactly \
         what it was coupled to, with one more hop to read. It is the shim on the \
         forbidden list below, and it is the reason this gate reports where an exit \
         *starts* rather than how many there are."
            .to_string(),
    ]
}

/// Where a folder's outgoing dependencies *start* (AN-028).
///
/// Counting exits and naming what they land on has always been here, and both
/// are about the far end. The near end is the half a reader of this folder can
/// act on: a folder is meant to read as a funnel — arriving at one door,
/// leaving from the bottom — and a middle-layer child reaching outside makes
/// the layering drawn above it a fiction. The drawing says that child depends
/// downward on its siblings; the program says it also reaches out of the
/// building.
///
/// Scored since ADR 0031, and only on the near end. Where an exit *lands*
/// remains ungraded — [`OutsideVerdict::Exit`] still says depending outward
/// is what a folder is for, and a tool that failed folders for the far end
/// would be argued with and then ignored. Where it *starts* is this folder's
/// own shape, and `egress` grades it: classifying it turned out not to be
/// enough on its own, because a gate nothing fails is a gate agents bank
/// past (AN-028).
fn exit_section(p: &FolderPicture, exits: usize) -> Vec<String> {
    let leaves = leaf_children(p);
    let doors: BTreeSet<&str> = p
        .children
        .iter()
        .filter(|c| c.is_door)
        .map(|c| c.path.as_str())
        .collect();

    let mut by_origin: BTreeMap<ExitOrigin, Vec<&OutsideEdge>> = BTreeMap::new();
    let mut targets: BTreeSet<&str> = BTreeSet::new();
    for e in p
        .outside
        .iter()
        .filter(|o| o.verdict == OutsideVerdict::Exit)
    {
        targets.insert(e.outside.as_str());
        by_origin
            .entry(ExitOrigin::of(&e.child, &leaves, &doors))
            .or_default()
            .push(e);
    }
    let count = |o: ExitOrigin| by_origin.get(&o).map_or(0, Vec::len);
    let middle = count(ExitOrigin::Middle);

    let mut body = vec![
        format!(
            "Those {} outgoing dependencies land on {} distinct files. Where they \
             land is not graded and stays their own folder's business — depending \
             outward is what a folder is for:",
            exits,
            targets.len(),
        ),
        String::new(),
    ];
    body.extend(listed(targets.into_iter().map(String::from)));
    body.push(String::new());
    body.push("Where they *start* is this folder's business:".to_string());
    body.push(String::new());
    body.extend(listed(
        [ExitOrigin::Leaf, ExitOrigin::Door, ExitOrigin::Middle]
            .into_iter()
            .filter(|o| count(*o) > 0)
            .map(|o| format!("{} — {}", count(o), o.gloss())),
    ));
    body.push(String::new());
    body.push(if middle == 0 {
        "Nothing leaves from the middle: this folder is a funnel.".to_string()
    } else {
        let subject = if middle == 1 {
            "The one leaving from the middle is worth a look. It is".to_string()
        } else {
            format!("The {middle} leaving from the middle are worth a look. Each is")
        };
        format!(
            "{subject} one of three things: a dependency that belongs further down, \
             a child sitting at the wrong level, or a wide ambient type this folder \
             should be naming its own slice of. Re-exporting it from a leaf is a \
             fourth and is a shim."
        )
    });
    body.push(String::new());
    if let Some(edges) = by_origin.get(&ExitOrigin::Middle) {
        body.extend(listed(
            edges
                .iter()
                .map(|e| format!("{} → {}", e.inside, e.outside)),
        ));
        body.push(String::new());
    }
    body
}

/// Children with no edge to a sibling — the bottom of the folder's own
/// drawing, and where an outgoing dependency is meant to start.
///
/// `erased` is deliberately not consulted. An arrow the build erases is not in
/// the graph the levels were assigned over (ADR 0026), so a child whose only
/// sibling dependency is a `import type` genuinely does sit at the bottom, and
/// counting it would contradict the levels drawn beside it.
fn leaf_children(p: &FolderPicture) -> BTreeSet<&str> {
    let departing: BTreeSet<&str> = p.edges.iter().map(|e| e.from.as_str()).collect();
    p.children
        .iter()
        .map(|c| c.path.as_str())
        .filter(|path| !departing.contains(path))
        .collect()
}

/// Which part of the folder one exit leaves from.
///
/// Ordered leaf-door-middle, worst last, so the listing reads toward the
/// thing to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ExitOrigin {
    Leaf,
    Door,
    Middle,
}

impl ExitOrigin {
    /// A leaf that is also a door reads as a leaf: it is already the shape
    /// the funnel wants, and reporting it as the carve-out would understate
    /// a folder that is doing the right thing.
    fn of(child: &str, leaves: &BTreeSet<&str>, doors: &BTreeSet<&str>) -> Self {
        match child {
            c if leaves.contains(c) => Self::Leaf,
            c if doors.contains(c) => Self::Door,
            _ => Self::Middle,
        }
    }

    fn gloss(self) -> &'static str {
        match self {
            Self::Leaf => "from a leaf, which is the shape a funnel has",
            Self::Door => "from a door, which is structural: a facade imports what it hands on",
            Self::Middle => "from a child in the middle, reaching past its siblings",
        }
    }
}

/// The moves that count, and the ones that only move the number.
///
/// The forbidden list is the most load-bearing part of this tool. Two of
/// the five sub-scores can be satisfied by a file that changes no caller's
/// actual dependencies, and a tool that did not say so would teach agents
/// to produce exactly that — at which point the measure stops describing
/// anything and every folder in the repo scores well.
/// The rules, in full or as a checklist.
///
/// The choice lives here rather than in either of them so that neither
/// grows a branch: the complexity gate fails on any metric increase to a
/// function that already exists (CI-001), and both of these are existing
/// functions with no budget for one more condition.
fn rules_for(folder: &str, has_baseline: bool, spell_out: bool) -> Vec<String> {
    if spell_out {
        rules_section(folder, has_baseline)
    } else {
        short_rules(folder, has_baseline)
    }
}

fn rules_section(folder: &str, has_baseline: bool) -> Vec<String> {
    let mut body = vec![
        "## What counts as a fix".to_string(),
        String::new(),
        "Allowed:".to_string(),
        String::new(),
        "1. Move a file into a subfolder, or out of one — including grouping a wide \
         level into subfolders that each hold a part of the drawing."
            .to_string(),
        "2. Split a file that two unrelated callers depend on for two unrelated \
         reasons."
            .to_string(),
        "3. Invert a dependency so it runs the other way.".to_string(),
        "4. Merge two files locked in a loop, when they were one thing all along.".to_string(),
        "5. Introduce a facade that genuinely owns what outsiders need, so callers \
         depend on it instead of on what is behind it."
            .to_string(),
        String::new(),
        "**Not allowed — these raise the score and change nothing:**".to_string(),
        String::new(),
        "- **A re-export shim.** A `mod.rs` / `index.ts` / `__init__.py` that \
         forwards to what is behind it will move `entry concentration` to 1.00 while \
         every caller stays coupled to exactly what it was coupled to before. If the \
         thing behind the door still changes when the door does, it is not a door."
            .to_string(),
        "- **A wrapper per caller.** Giving a shared helper one thin facade for each \
         of its dependents raises `branching` and leaves the same file serving the \
         same callers, now with more indirection to read through."
            .to_string(),
        "- **A pass-through layer.** Routing a level-skipping edge through a file \
         that only forwards the call raises `layering` and adds a hop."
            .to_string(),
        "- **Deleting or merging code to get under the child bar.** Grouping a wide \
         level into subfolders is the fix and is on the allowed list; making the \
         surplus disappear, or filing it alphabetically into folders that cut across \
         the drawing, clears the gate and leaves the level exactly as unreadable."
            .to_string(),
        String::new(),
        "The test for all four: after the change, does anything depend on a \
         *different* thing than it did before? If no, the change was cosmetic and \
         the number it moved is now lying."
            .to_string(),
        String::new(),
    ];
    body.extend(closing_section(folder, has_baseline));
    body
}

/// The rules for a caller that has already read them this session.
///
/// The four forbidden moves keep their names and lose their explanations.
/// Naming them is what does the work — an agent reported that "a
/// re-export shim … if the thing behind the door still changes when the
/// door does, it is not a door" was the single most valuable line in the
/// tool, and it was valuable the first time. What repetition adds is
/// length: across four folders the unabridged list was most of what
/// `reshape` returned, which buries the one finding each call exists to
/// deliver.
fn short_rules(folder: &str, has_baseline: bool) -> Vec<String> {
    vec![
        "## What counts as a fix".to_string(),
        String::new(),
        "Spelled out in full on the first `reshape` of this session. In short — \
         allowed: move a file between folders, split a file two callers need for two \
         reasons, invert an edge, merge two files locked in a loop, add a facade that \
         genuinely owns what outsiders need."
            .to_string(),
        String::new(),
        "Still not allowed, because each raises a score and changes nothing: a \
         re-export shim; a wrapper per caller; a pass-through layer; deleting or \
         alphabetising code to get under the child bar."
            .to_string(),
        String::new(),
        "The test: after the change, does anything depend on a *different* thing than \
         it did before?"
            .to_string(),
        String::new(),
        format!(
            "**When you are done:** re-run `reshape` on `{folder}` — the drawing above \
             is kept, so the next call opens with what actually moved.{}",
            if has_baseline {
                " The comparison above is that check; do not summarise your own \
                 before-and-after from memory."
            } else {
                ""
            }
        ),
        String::new(),
        format!("To try a restructure before making it, call `layout` on `{folder}`."),
    ]
}

/// The closing instruction, which is now a promise the tool keeps rather
/// than a request the agent may forget.
fn closing_section(folder: &str, has_baseline: bool) -> Vec<String> {
    let mut body = vec![
        "## When you are done".to_string(),
        String::new(),
        format!(
            "Re-run `reshape` on `{folder}`. The drawing it just showed you is kept \
             — under the mezz cache directory rather than in this process, so \
             rebuilding or reloading the server between the two calls no longer \
             loses it — and the next call opens with what actually changed: the \
             tier, the numbers that moved, the edges that came and went, and whether \
             the two agree. A tier that rises while every edge stays put is reported \
             as such and should be reverted; the measure exists to describe the \
             drawing, and a change that only moved the number has made it less true."
        ),
    ];
    if has_baseline {
        body.push(String::new());
        body.push(
            "You have called this before, so the comparison above is that check. Do \
             not summarise your own before-and-after from memory — that section is \
             the record, and it is the one that decides whether the work stands."
                .to_string(),
        );
    }
    body.push(String::new());
    body.push(format!(
        "To try a restructure *before* making it, call `layout` on `{folder}`: it \
         scores a set of moves, or proposes the subfolders this folder's own drawing \
         implies, without touching the tree."
    ));
    body
}

// ------------------------------------------------------------------
//  Small shared pieces
// ------------------------------------------------------------------

/// The rung directly above, in the ladder's own words.
fn next_tier(pattern: ShapePattern) -> &'static str {
    match pattern {
        ShapePattern::Cyclic => "tangled",
        ShapePattern::Tangled => "hierarchical",
        ShapePattern::Hierarchical | ShapePattern::Fractal => "fractal",
    }
}

pub(super) fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The `path` argument as an absolute path under the root, whatever it
/// turns out to be. The root itself when `path` is absent.
///
/// Says nothing about folder or file: which of the two a tool can answer
/// for is the tool's own question, and `boundaries` answers for both
/// (MCP-040).
pub(super) fn target_path(server: &McpServer, args: &Value) -> PathBuf {
    let raw = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let candidate = if raw.is_empty() {
        server.root.clone()
    } else {
        server.root.join(raw)
    };
    candidate.canonicalize().unwrap_or(candidate)
}

/// Resolve the required `path` argument to a folder under the root.
///
/// `asked_by` is the tool the caller actually invoked. The rationale here
/// is about *shape*, which is `reshape`'s and `layout`'s subject and
/// genuinely is a property of a folder's children — but the refusal used
/// to name `reshape` whoever asked, sending a `layout` caller to a tool
/// they had not chosen, and a `boundaries` caller to one that could not
/// answer their question at all (MCP-040).
pub(super) fn target_folder(server: &McpServer, args: &Value, asked_by: &str) -> Result<PathBuf> {
    let resolved = target_path(server, args);
    if resolved.is_file() {
        bail!(
            "{} is a file. Shape is a property of a folder's children, and a file \
             has none — call `{asked_by}` on {}, the folder holding it.",
            rel(&resolved, &server.root),
            holder(&resolved, &server.root),
        );
    }
    Ok(resolved)
}

/// The folder holding `file`, named as the caller would pass it.
fn holder(file: &Path, root: &Path) -> String {
    match file.parent().map(|p| rel(p, root)) {
        Some(name) if !name.is_empty() => format!("`{name}`"),
        // `rel` of the root against itself is the empty string, and a
        // refusal that ends in an empty backtick pair names nothing.
        _ => "the repository root".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ChildKind, EntityKind, OutsideEdge, PictureChild, PictureEdge};
    use serde_json::json;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    /// A throwaway project the real analyzer can be pointed at.
    ///
    /// The prefix carries neither "test" nor "spec": `is_test_path`
    /// matches on the whole path, so a fixture under a directory named
    /// for the test would have every one of its entities filtered out and
    /// the folder would have no shape to report.
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "mezz-reshape-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ));
            std::fs::create_dir_all(&path).unwrap();
            TmpDir(path)
        }

        fn write(&self, rel: &str, body: &str) {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, body).unwrap();
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn server_for(dir: &TmpDir) -> McpServer {
        McpServer {
            root: dir.0.canonicalize().unwrap(),
            include_tests: false,
            languages: None,
            graph_cache: Mutex::new(HashMap::new()),
            base_cache: Mutex::new(HashMap::new()),
            generation: Arc::new(AtomicU64::new(0)),
            shape_baselines: Default::default(),
            rules_spelled_out: Default::default(),
            layout_caveat_spelled_out: Default::default(),
        }
    }

    fn shape(pattern: ShapePattern, blocker: Option<ShapeBlocker>) -> FolderShape {
        FolderShape {
            pattern,
            compliance: 0.8,
            acyclicity: 1.0,
            layering: Some(0.6),
            arborescence: Some(0.5),
            entry_concentration: Some(0.4),
            egress: None,
            child_compliance: None,
            uniformity: None,
            child_count: 3,
            blocker,
            terms: crate::models::ShapeTerms::default(),
        }
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

    fn edge(from: &str, to: &str, verdict: EdgeVerdict) -> PictureEdge {
        PictureEdge {
            from: from.to_string(),
            to: to.to_string(),
            verdict,
        }
    }

    fn exit(inside: &str, outside: &str) -> OutsideEdge {
        OutsideEdge {
            outside: outside.to_string(),
            inside: inside.to_string(),
            // Children are files in these fixtures, so a file is its own
            // circle in the drawing.
            child: inside.to_string(),
            verdict: OutsideVerdict::Exit,
        }
    }

    /// An empty entity graph. These tests exercise which rung gets named
    /// and which offenders are listed, not the entity-level classification
    /// behind a merge — and an empty graph is the honest fixture for that:
    /// nothing agrees on a type, so every merge falls to the plain reading
    /// and the prose under test is the prose that ships.
    fn test_ctx(graph: &DependencyGraph) -> Ctx<'_> {
        Ctx {
            graph,
            root: Path::new(""),
        }
    }

    fn picture(children: Vec<PictureChild>, edges: Vec<PictureEdge>) -> FolderPicture {
        FolderPicture {
            folder: "src/x".to_string(),
            children,
            edges,
            erased: Vec::new(),
            outside: Vec::new(),
            doors: Vec::new(),
        }
    }

    /// An outgoing dependency is classified by where it starts, and the
    /// three origins do not mean the same thing. A middle child reaching
    /// outside is the finding; a leaf doing it is the shape being asked for;
    /// a door doing it is a facade importing what it hands on.
    #[test]
    fn an_exit_is_classified_by_the_child_it_leaves_from() {
        let mut top = child("src/x/top.rs", 0);
        top.is_door = true;
        let mut p = picture(
            vec![top, child("src/x/mid.rs", 1), child("src/x/leaf.rs", 2)],
            vec![
                edge("src/x/top.rs", "src/x/mid.rs", EdgeVerdict::Step),
                edge("src/x/mid.rs", "src/x/leaf.rs", EdgeVerdict::Step),
            ],
        );
        p.doors = vec!["src/x/top.rs".to_string()];
        p.outside = vec![
            exit("src/x/leaf.rs", "src/far/a.rs"),
            exit("src/x/top.rs", "src/far/b.rs"),
            exit("src/x/mid.rs", "src/far/c.rs"),
        ];

        let body = boundary_section(&p).join("\n");
        assert!(body.contains("1 — from a leaf"), "{body}");
        assert!(body.contains("1 — from a door"), "{body}");
        assert!(body.contains("1 — from a child in the middle"), "{body}");
        // The middle one, and only it, is named as an edge to go and look at.
        // Every target still appears in the landing list above; what must not
        // appear is a leaf or a door as something to act on.
        assert!(body.contains("src/x/mid.rs → src/far/c.rs"), "{body}");
        assert!(
            !body.contains("src/x/leaf.rs →"),
            "leaf exit listed:\n{body}"
        );
        assert!(
            !body.contains("src/x/top.rs →"),
            "door exit listed:\n{body}"
        );
    }

    /// A folder whose every exit leaves from a leaf says so in a sentence.
    /// The reader must not have to compare two counts to learn it.
    #[test]
    fn a_funnel_is_named_rather_than_left_to_be_counted() {
        let mut p = picture(
            vec![child("src/x/top.rs", 0), child("src/x/leaf.rs", 1)],
            vec![edge("src/x/top.rs", "src/x/leaf.rs", EdgeVerdict::Step)],
        );
        p.outside = vec![exit("src/x/leaf.rs", "src/far/a.rs")];

        let body = boundary_section(&p).join("\n");
        assert!(body.contains("this folder is a funnel"), "{body}");
        assert!(!body.contains("worth a look"), "{body}");
    }

    /// A child that is both the door and a leaf reads as a leaf. It is
    /// already the shape the funnel wants, and filing it under the carve-out
    /// would understate a folder doing the right thing.
    #[test]
    fn a_door_that_is_also_a_leaf_reads_as_a_leaf() {
        let mut only = child("src/x/only.rs", 0);
        only.is_door = true;
        let mut p = picture(vec![only], Vec::new());
        p.doors = vec!["src/x/only.rs".to_string()];
        p.outside = vec![exit("src/x/only.rs", "src/far/a.rs")];

        let body = boundary_section(&p).join("\n");
        assert!(body.contains("1 — from a leaf"), "{body}");
        assert!(body.contains("funnel"), "{body}");
    }

    /// An arrow the build erases is not in the graph the levels were
    /// assigned over (ADR 0026), so it must not be what stops a child
    /// counting as a leaf — the drawing would then disagree with itself.
    #[test]
    fn an_erased_arrow_does_not_cost_a_child_its_leaf_standing() {
        let mut p = picture(
            vec![child("src/x/a.rs", 0), child("src/x/b.rs", 0)],
            Vec::new(),
        );
        p.erased = vec![crate::models::ErasedEdge {
            from: "src/x/a.rs".to_string(),
            to: "src/x/b.rs".to_string(),
        }];
        p.outside = vec![exit("src/x/a.rs", "src/far/a.rs")];

        let body = boundary_section(&p).join("\n");
        assert!(body.contains("funnel"), "{body}");
    }

    /// The ladder is a ladder: a cyclic folder is told to break its loop and
    /// is not also handed the three gates above it. A four-part instruction
    /// is one that does not get started.
    #[test]
    fn the_task_names_one_rung_and_not_the_ones_above_it() {
        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 0)],
            vec![
                edge("a.rs", "b.rs", EdgeVerdict::Back),
                edge("b.rs", "a.rs", EdgeVerdict::Back),
            ],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Cyclic, Some(ShapeBlocker::Cycles(0.5))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("cyclic → tangled"), "{body}");
        assert!(
            body.contains("a.rs → b.rs"),
            "the offending edges are named:\n{body}"
        );
        assert!(
            !body.contains("fractal"),
            "a tier three rungs up leaked in:\n{body}"
        );
        assert!(
            !body.contains("door"),
            "an unrelated gate leaked in:\n{body}"
        );
    }

    /// The section follows `blocker`, which the analyzer already decided.
    /// Choosing a different gate to expand would be a second opinion about
    /// which tier a folder is on.
    #[test]
    fn the_task_expands_the_gate_the_analyzer_named() {
        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 1), child("c.rs", 2)],
            vec![edge("a.rs", "c.rs", EdgeVerdict::Skip)],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Tangled, Some(ShapeBlocker::Layering(0.6))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("tangled → hierarchical"), "{body}");
        assert!(body.contains("a.rs → c.rs"), "{body}");
        // `arborescence` is 0.50 on this fixture and under its bar, but the
        // blocker says layering, so branching must not be what is asked for.
        assert!(
            !body.contains("branching"),
            "a gate the blocker did not name:\n{body}"
        );
    }

    // --------------------------------------------------------------
    //  Arrows the build erases (AN-022, AN-025)
    // --------------------------------------------------------------

    /// A graph carrying nothing but import sites — the granularity the
    /// erased mark is read at.
    fn graph_with_sites(sites: Vec<crate::models::ImportSite>) -> DependencyGraph {
        DependencyGraph::from_analysis(&crate::analyzer::AnalysisResult {
            entities: Vec::new(),
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: sites,
            warnings: Vec::new(),
        })
    }

    fn site(from: &str, to: &str, type_only: bool) -> crate::models::ImportSite {
        crate::models::ImportSite {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
            line: 0,
            is_reexport: false,
            is_type_only: type_only,
        }
    }

    /// The drawing of a folder whose `a.rs → c.rs` arrow was discounted.
    /// `picture` is built by hand here, so the erased arrow is put where
    /// the analyzer puts it: out of `edges`, into `erased`.
    fn drawing_with_erased(sites: Vec<crate::models::ImportSite>) -> String {
        let mut p = picture(
            vec![child("a.rs", 0), child("b.rs", 1), child("c.rs", 2)],
            vec![edge("a.rs", "b.rs", EdgeVerdict::Step)],
        );
        p.erased = vec![crate::models::ErasedEdge {
            from: "a.rs".to_string(),
            to: "c.rs".to_string(),
        }];
        let graph = graph_with_sites(sites);
        drawing_section(&p, test_ctx(&graph)).join("\n")
    }

    /// The failure this came from: five level skips, all five of them
    /// `import type`, reported for five rounds as an architecture defect.
    /// They are now out of the numbers — and still on the page, because
    /// the type is coupling a reader follows.
    #[test]
    fn an_arrow_the_build_erases_is_drawn_apart_and_said_to_be_discounted() {
        let body = drawing_with_erased(vec![site("a.rs", "c.rs", true)]);
        assert!(body.contains("Discounted"), "{body}");
        assert!(body.contains("a.rs → c.rs"), "{body}");
        assert!(body.contains("counted in nothing above"), "{body}");
    }

    /// And the line it was written on, so the claim can be checked.
    #[test]
    fn a_discounted_arrow_cites_the_import_behind_it() {
        let body = drawing_with_erased(vec![site("a.rs", "c.rs", true)]);
        assert!(body.contains("`a.rs:1` (type-only)"), "{body}");
    }

    /// A folder with nothing erased says nothing about erasure. The
    /// paragraph exists to explain arrows that are missing from the
    /// count; printed where none are, it is noise.
    #[test]
    fn a_drawing_with_nothing_erased_stays_silent_about_it() {
        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 1)],
            vec![edge("a.rs", "b.rs", EdgeVerdict::Step)],
        );
        let graph = graph_with_sites(vec![site("a.rs", "b.rs", false)]);
        let body = drawing_section(&p, test_ctx(&graph)).join("\n");
        assert!(!body.contains("Discounted"), "{body}");
        assert!(!body.contains("type-only"), "{body}");
    }

    /// The layering gate no longer has an erased edge to explain away.
    /// Its skips are edges that survive the build, so the instruction it
    /// gives is one that can be carried out.
    #[test]
    fn the_layering_gate_no_longer_says_an_erased_edge_is_counted() {
        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 1), child("c.rs", 2)],
            vec![edge("a.rs", "c.rs", EdgeVerdict::Skip)],
        );
        let graph = graph_with_sites(vec![site("a.rs", "c.rs", true)]);
        let body = next_rung_section(
            &shape(ShapePattern::Tangled, Some(ShapeBlocker::Layering(0.6))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");
        assert!(body.contains("a.rs → c.rs"), "{body}");
        assert!(!body.contains("counted above"), "{body}");
    }

    // --------------------------------------------------------------
    //  Shared vocabulary leaves (MCP-016)
    // --------------------------------------------------------------

    fn declared(graph: &mut DependencyGraph, file: &str, name: &str, kind: EntityKind) -> String {
        let e = crate::models::CodeEntity::new(
            name,
            kind,
            file,
            crate::models::Span::new(
                crate::models::Position::new(1, 0, 0),
                crate::models::Position::new(1, 0, 0),
            ),
        );
        let id = e.id.clone();
        graph.add_entity(e);
        id
    }

    fn reaches_for(graph: &mut DependencyGraph, from: &str, to: &str) {
        graph.add_relationship(crate::models::Relationship::new(
            from,
            to,
            crate::models::RelationshipKind::UsesType,
        ));
    }

    /// The review's own folder in miniature: three siblings that each
    /// skip a level to reach one leaf of declarations, which reaches for
    /// nothing itself.
    ///
    /// `extra` is what `contracts.rs` holds *besides* its declarations —
    /// empty for the vocabulary case, and one free function for the
    /// folder that only looks like one.
    fn vocabulary_body(extra: Option<EntityKind>) -> String {
        let mut graph = DependencyGraph::default();
        let held: Vec<String> = [
            ("Command", EntityKind::Struct),
            ("Theme", EntityKind::Enum),
            ("Setting", EntityKind::TypeAlias),
        ]
        .into_iter()
        .map(|(name, kind)| declared(&mut graph, "contracts.rs", name, kind))
        .collect();
        if let Some(kind) = extra {
            declared(&mut graph, "contracts.rs", "normalise", kind);
        }
        for file in ["commands.rs", "editor.rs", "settings.rs"] {
            let caller = declared(&mut graph, file, "run", EntityKind::Function);
            // Every dependent reaches for the same two, so the leaf has
            // no line through it and the check below says so.
            for target in held.iter().take(2) {
                reaches_for(&mut graph, &caller, target);
            }
        }

        let p = picture(
            vec![
                child("main.rs", 0),
                child("commands.rs", 1),
                child("editor.rs", 1),
                child("settings.rs", 1),
                child("contracts.rs", 3),
            ],
            vec![
                edge("main.rs", "commands.rs", EdgeVerdict::Step),
                edge("main.rs", "editor.rs", EdgeVerdict::Step),
                edge("main.rs", "settings.rs", EdgeVerdict::Step),
                edge("commands.rs", "contracts.rs", EdgeVerdict::Skip),
                edge("editor.rs", "contracts.rs", EdgeVerdict::Skip),
                edge("settings.rs", "contracts.rs", EdgeVerdict::Skip),
            ],
        );
        next_rung_section(
            &shape(ShapePattern::Tangled, Some(ShapeBlocker::Layering(0.5))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n")
    }

    /// The failure this ticket came from. Five rounds of the same two
    /// fixes for the same edges, neither of which could be carried out on
    /// a leaf six folders legitimately needed.
    ///
    /// The assertions that matter are the negative ones: an advice ticket
    /// whose test only checks that a new string is present has tested
    /// nothing, since the advice it was meant to replace is still sitting
    /// underneath it.
    #[test]
    fn a_skip_onto_a_vocabulary_leaf_is_not_offered_the_two_layer_fixes() {
        let body = vocabulary_body(None);

        assert!(
            body.contains("shared vocabulary, not a layer"),
            "the shape is unnamed:\n{body}"
        );
        assert!(
            body.contains("`Command`"),
            "what the leaf declares is unnamed:\n{body}"
        );
        assert!(
            !body.contains("route the traffic through"),
            "offered a routing that would take a re-export:\n{body}"
        );
        assert!(
            !body.contains("remove the layer"),
            "offered the removal of a layer that is not there:\n{body}"
        );
        // And the reader is left with something to do rather than a
        // waiver: the leaf either is one vocabulary or is two.
        assert!(
            body.contains("one vocabulary or two"),
            "no check offered in place of the fixes:\n{body}"
        );
    }

    /// The dependents here all reach for the same declarations, so there
    /// is no line through the leaf. The tool has to say so rather than
    /// send an agent looking for a split — the restraint `merges_task`'s
    /// classifier already keeps.
    #[test]
    fn a_leaf_its_dependents_do_not_divide_is_said_to_be_finished() {
        let body = vocabulary_body(None);
        assert!(
            body.contains("no line through it to split along"),
            "{body}"
        );
    }

    /// A folder with one of each. The waiver is per-edge, so the skips
    /// that are not vocabulary keep the gate's ordinary advice — and the
    /// paragraph carrying it says how many of the listed edges it is
    /// still talking about.
    #[test]
    fn the_ordinary_advice_survives_for_the_skips_that_are_not_vocabulary() {
        let mut graph = DependencyGraph::default();
        let held = declared(&mut graph, "contracts.rs", "Command", EntityKind::Struct);
        for file in ["commands.rs", "editor.rs", "settings.rs"] {
            let caller = declared(&mut graph, file, "run", EntityKind::Function);
            reaches_for(&mut graph, &caller, &held);
        }
        let p = picture(
            vec![
                child("main.rs", 0),
                child("commands.rs", 1),
                child("editor.rs", 1),
                child("settings.rs", 1),
                child("legacy.rs", 2),
                child("contracts.rs", 3),
            ],
            vec![
                edge("main.rs", "commands.rs", EdgeVerdict::Step),
                edge("main.rs", "editor.rs", EdgeVerdict::Step),
                edge("main.rs", "settings.rs", EdgeVerdict::Step),
                edge("main.rs", "contracts.rs", EdgeVerdict::Skip),
                edge("commands.rs", "legacy.rs", EdgeVerdict::Skip),
                edge("editor.rs", "contracts.rs", EdgeVerdict::Skip),
                edge("settings.rs", "contracts.rs", EdgeVerdict::Skip),
                edge("legacy.rs", "contracts.rs", EdgeVerdict::Step),
            ],
        );
        let body = next_rung_section(
            &shape(ShapePattern::Tangled, Some(ShapeBlocker::Layering(0.5))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("shared vocabulary, not a layer"), "{body}");
        assert!(
            body.contains("For the 1 above not named as vocabulary"),
            "the surviving advice does not say what it is about:\n{body}"
        );
        assert!(body.contains("route the traffic through"), "{body}");
    }

    /// One free function at the top of the file and this is a module with
    /// behaviour in it, which is somewhere a layer can live. The waiver
    /// is the expensive direction to be wrong in: it tells a reader to
    /// leave alone the layer they should have removed.
    #[test]
    fn a_leaf_holding_behaviour_keeps_the_ordinary_advice() {
        let body = vocabulary_body(Some(EntityKind::Function));

        assert!(
            !body.contains("shared vocabulary"),
            "waived a file that is not only declarations:\n{body}"
        );
        assert!(
            body.contains("route the traffic through"),
            "the ordinary advice went missing:\n{body}"
        );
    }

    /// A merge blocker has to name who leans on what, or "give it a
    /// branching shape" is an instruction with no object.
    #[test]
    fn a_merge_names_the_shared_child_and_everyone_leaning_on_it() {
        let p = picture(
            vec![child("a.rs", 0), child("b.rs", 0), child("util.rs", 1)],
            vec![
                edge("a.rs", "util.rs", EdgeVerdict::Step),
                edge("b.rs", "util.rs", EdgeVerdict::Step),
            ],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Merges(0.5))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("util.rs** ← a.rs, b.rs"), "{body}");
        assert!(body.contains("2 dependents"), "{body}");
        // With no entity graph behind it there is no agreed type and no
        // redundant path, so the plain reading is the honest one. The
        // classification must not invent a verdict it cannot support.
        assert!(body.contains("shared helper"), "{body}");
        // The disagreement with layering is the whole reason this gate
        // exists, so the instruction has to explain it rather than read as
        // a contradiction of the number printed beside it.
        assert!(
            body.contains("layering"),
            "the disagreement is unexplained:\n{body}"
        );
    }

    /// A folder can fail this gate with no merge point in it at all — the
    /// whole loss coming from children nothing reaches (ADR 0022). Written
    /// against the merge case alone, the instruction opened by promising a
    /// classification of zero offenders and then listed none.
    #[test]
    fn a_folder_of_strays_is_told_about_the_strays_not_about_merges() {
        let p = picture(
            vec![
                child("a.rs", 0),
                child("b.rs", 1),
                child("x.rs", 0),
                child("y.rs", 0),
                child("z.rs", 0),
            ],
            vec![edge("a.rs", "b.rs", EdgeVerdict::Step)],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Merges(0.25))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("x.rs"), "the strays are unnamed:\n{body}");
        assert!(body.contains("no parent in the drawing"), "{body}");
        // Nothing in this folder is depended on twice, so a sentence about
        // merge points would be describing a different folder.
        assert!(
            !body.contains("depended on by more than one sibling"),
            "claims a merge that is not there:\n{body}"
        );
        // The cosmetic fix is more tempting here than anywhere else — one
        // import per stray clears the gate and moves nothing.
        assert!(
            body.contains("Do not give them a parent to give them a parent"),
            "{body}"
        );
    }

    /// An unstructured folder is usually fine, and an agent that "fixes" it
    /// invents dependencies to earn a tier. The one gate whose advice is
    /// mostly "do nothing" has to say so.
    #[test]
    fn an_unstructured_folder_is_told_not_to_manufacture_edges() {
        let p = picture(vec![child("a.rs", 0), child("b.rs", 0)], Vec::new());
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Unstructured)),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("not a defect"), "{body}");
        assert!(
            body.contains("worse"),
            "the cost of complying is unstated:\n{body}"
        );
    }

    /// Nothing holds a fractal folder back, so there is no task — and no
    /// invented one.
    #[test]
    fn a_fractal_folder_is_given_no_task() {
        let p = picture(vec![child("a.rs", 0)], Vec::new());
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Fractal, None),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        );
        assert!(body.is_empty(), "{body:?}");

        let verdict = verdict_section(
            &shape(ShapePattern::Fractal, None),
            &Thresholds::default(),
            &picture(Vec::new(), Vec::new()),
        )
        .join("\n");
        assert!(verdict.contains("nothing to do"), "{verdict}");
    }

    /// Every sub-score is printed beside the bar it is judged against. A
    /// number with no threshold cannot tell a near miss from a disaster.
    #[test]
    fn every_measurement_is_printed_against_its_cut_off() {
        let t = Thresholds::default();
        let body = verdict_section(
            &shape(ShapePattern::Tangled, Some(ShapeBlocker::Layering(0.6))),
            &t,
            &picture(Vec::new(), Vec::new()),
        )
        .join("\n");
        for bar in [
            t.shape_layering,
            t.shape_arborescence,
            t.shape_entry,
            t.shape_child,
        ] {
            assert!(
                body.contains(&format!("{bar:.2}")),
                "missing a cut-off {bar}:\n{body}"
            );
        }
        // And the one number whose scale is inverted relative to the rest of
        // mezz says so, since it sits next to composite scores where low wins.
        assert!(body.contains("Higher is better"), "{body}");
    }

    /// An unmeasured sub-score must not print as `0.00`. Zero is the failing
    /// end of every scale here, so it would report the worst possible news
    /// about something nobody measured.
    #[test]
    fn an_unmeasured_score_reads_as_a_dash_not_a_zero() {
        assert_eq!(num(None), "—");
        assert_eq!(num(Some(0.0)), "0.00");
        let mut s = shape(ShapePattern::Hierarchical, None);
        s.layering = None;
        let body = verdict_section(&s, &Thresholds::default(), &picture(Vec::new(), Vec::new()))
            .join("\n");
        assert!(body.contains("layering —"), "{body}");
    }

    /// The forbidden list is what stops the tool teaching agents to satisfy
    /// the measure without changing the code. Losing it silently would be
    /// the worst regression this file can have.
    #[test]
    fn the_gameable_moves_are_named_as_forbidden() {
        let body = rules_section("src/x", false).join("\n");
        for shim in ["mod.rs", "index.ts", "__init__.py"] {
            assert!(
                body.contains(shim),
                "the re-export shim is unnamed: {shim}\n{body}"
            );
        }
        assert!(body.contains("wrapper per caller"), "{body}");
        assert!(body.contains("pass-through"), "{body}");
        // And the general test, so a move nobody enumerated is still caught.
        assert!(body.contains("depend on a *different* thing"), "{body}");
    }

    /// A truncated list that does not say it was truncated reads as the
    /// whole answer.
    #[test]
    fn a_truncated_list_says_how_much_it_left_out() {
        let many: Vec<String> = (0..MAX_LISTED + 5).map(|i| format!("item {i}")).collect();
        let out = listed(many).join("\n");
        assert!(out.contains("… and 5 more."), "{out}");

        let few = listed((0..3).map(|i| format!("item {i}"))).join("\n");
        assert!(!few.contains("more."), "{few}");
    }

    /// Exits are counted as dependencies and listed as distinct files, and
    /// the two numbers differ by a lot on a real folder — 97 against 5 on
    /// `src/parser`. Printing one and labelling it the other reads as a
    /// contradiction.
    #[test]
    fn outgoing_traffic_distinguishes_edges_from_the_files_they_land_on() {
        let mut p = picture(vec![child("a.rs", 0)], Vec::new());
        p.outside = vec![
            OutsideEdge {
                outside: "src/models/entity.rs".to_string(),
                inside: "src/x/a.rs".to_string(),
                child: "src/x/a.rs".to_string(),
                verdict: OutsideVerdict::Exit,
            },
            OutsideEdge {
                outside: "src/models/entity.rs".to_string(),
                inside: "src/x/b.rs".to_string(),
                child: "src/x/b.rs".to_string(),
                verdict: OutsideVerdict::Exit,
            },
        ];
        let body = boundary_section(&p).join("\n");
        assert!(body.contains("2 leave the folder"), "{body}");
        assert!(body.contains("land on 1 distinct files"), "{body}");
        assert_eq!(
            body.lines()
                .filter(|l| *l == "- src/models/entity.rs")
                .count(),
            1,
            "landing list not deduped:\n{body}"
        );
    }

    /// The elevator parser's real shape, which is what the classification
    /// was written from: `mod.rs` reaches `lexer.rs` directly and through
    /// `grammar.rs`. That triangle is the whole fix, and the tool has to
    /// name it rather than list the edge beside a legitimate diamond.
    #[test]
    fn a_parent_reaching_a_child_it_already_reaches_through_a_sibling_is_named() {
        let p = picture(
            vec![
                child("mod.rs", 0),
                child("grammar.rs", 1),
                child("lexer.rs", 2),
            ],
            vec![
                edge("mod.rs", "grammar.rs", EdgeVerdict::Step),
                edge("grammar.rs", "lexer.rs", EdgeVerdict::Step),
                edge("mod.rs", "lexer.rs", EdgeVerdict::Skip),
            ],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Merges(0.67))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("pass-through producer"), "{body}");
        assert!(
            body.contains("`grammar.rs`"),
            "the sibling that should own the job is not named:\n{body}"
        );
        // The join: this same edge is the folder's level skip, and saying
        // so is what stops an agent fixing the two gates separately.
        assert!(
            body.contains("also the folder's level skip"),
            "the two gates were not joined:\n{body}"
        );
    }

    /// A graph where every parent reaches for the same type declared in the
    /// shared child — the evidence `SharedContract` is decided on. The other
    /// merge fixtures use an empty graph on purpose, which makes every merge
    /// read as `Plain`; this one is the opposite case.
    fn graph_agreeing_on(child_file: &str, ty: &str, parents: &[&str]) -> DependencyGraph {
        let contract = crate::models::CodeEntity::new(
            ty,
            EntityKind::Struct,
            child_file,
            crate::models::Span::from_positions(1, 0, 1, 0),
        );
        let mut entities = vec![contract.clone()];
        let mut relationships = Vec::new();
        for (i, parent) in parents.iter().enumerate() {
            let user = crate::models::CodeEntity::new(
                &format!("uses{i}"),
                EntityKind::Function,
                parent,
                crate::models::Span::from_positions(1, 0, 1, 0),
            );
            relationships.push(crate::models::Relationship::new(
                &user.id,
                &contract.id,
                crate::models::RelationshipKind::UsesType,
            ));
            entities.push(user);
        }
        DependencyGraph::from_analysis(&crate::analyzer::AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    /// The unreachable bar. Every merge here is a contract the tool has just
    /// marked *leave it*, and nothing else charges the gate — so the folder
    /// cannot clear it by any move this tool permits, and saying "fix one
    /// merge point" would be an instruction to do something forbidden two
    /// sections further down.
    #[test]
    fn a_folder_held_only_by_contracts_is_told_there_is_no_move() {
        let p = picture(
            vec![
                child("head.rs", 0),
                child("emit.rs", 1),
                child("grammar.rs", 1),
                child("ast.rs", 2),
            ],
            vec![
                edge("head.rs", "emit.rs", EdgeVerdict::Step),
                edge("head.rs", "grammar.rs", EdgeVerdict::Step),
                edge("emit.rs", "ast.rs", EdgeVerdict::Step),
                edge("grammar.rs", "ast.rs", EdgeVerdict::Step),
            ],
        );
        let graph = graph_agreeing_on("ast.rs", "Ast", &["emit.rs", "grammar.rs"]);
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Merges(0.5))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("There is no move here"), "{body}");
        assert!(
            body.contains("held by a contract"),
            "the tier is not named as a ceiling:\n{body}"
        );
        assert!(
            !body.contains("Fix one merge point"),
            "told to fix what it was told to leave:\n{body}"
        );
    }

    /// The same folder with strays as well. Those *are* fixable, so there is
    /// still a move — but it is not among the merges, and pointing at them
    /// would point at the things just marked *leave it*.
    #[test]
    fn contracts_plus_strays_send_the_reader_to_the_strays() {
        let p = picture(
            vec![
                child("emit.rs", 1),
                child("grammar.rs", 1),
                child("ast.rs", 2),
                child("loose_a.rs", 0),
                child("loose_b.rs", 0),
            ],
            vec![
                edge("emit.rs", "ast.rs", EdgeVerdict::Step),
                edge("grammar.rs", "ast.rs", EdgeVerdict::Step),
            ],
        );
        let graph = graph_agreeing_on("ast.rs", "Ast", &["emit.rs", "grammar.rs"]);
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Merges(0.5))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(body.contains("No merge above is a defect"), "{body}");
        assert!(!body.contains("There is no move here"), "{body}");
        assert!(!body.contains("Fix one merge point"), "{body}");
    }

    /// The other half of that folder: `ast.rs` is leaned on by two
    /// siblings that do not depend on each other. With no triangle to
    /// find, the tool must not claim one.
    #[test]
    fn independent_siblings_over_a_shared_child_are_not_called_pass_through() {
        let p = picture(
            vec![
                child("emit.rs", 1),
                child("grammar.rs", 1),
                child("ast.rs", 2),
            ],
            vec![
                edge("emit.rs", "ast.rs", EdgeVerdict::Step),
                edge("grammar.rs", "ast.rs", EdgeVerdict::Step),
            ],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Merges(0.5))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(!body.contains("pass-through producer"), "{body}");
        assert!(body.contains("ast.rs"), "{body}");
    }

    /// The instruction for a wide folder has to say *how* to group, and
    /// has to warn that the tier drops first. An agent that groups, sees
    /// `fractal → hierarchical`, and reverts has been taught to undo the
    /// only fix the gate accepts.
    #[test]
    fn a_wide_folder_is_told_to_group_and_warned_the_tier_will_dip() {
        let p = picture(
            (0..9)
                .map(|i| child(&format!("f{i}.rs"), u32::from(i > 0)))
                .collect(),
            vec![edge("f0.rs", "f1.rs", EdgeVerdict::Step)],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Hierarchical, Some(ShapeBlocker::Breadth(9))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        // Read off the threshold rather than pinned, so tuning the bar
        // does not silently leave the instruction quoting the old one.
        assert!(
            body.contains(&format!(
                "at most {}",
                Thresholds::default().shape_max_children
            )),
            "{body}"
        );
        assert!(
            body.contains("Group by the drawing"),
            "grouping by name is the failure mode and is not warned against:\n{body}"
        );
        assert!(
            body.contains("drop") && body.contains("not a regression to undo"),
            "the expected dip is not explained:\n{body}"
        );
    }

    /// A door narrowed from three landings to one is the change the boundary
    /// section asks for, and the comparator used to call it nothing: it
    /// diffed child-to-child edges only, so the evidence section printed "No
    /// edge between these children changed" directly above a standing
    /// instruction to revert anything cosmetic. The two halves of the report
    /// were measuring at different granularities.
    #[test]
    fn a_door_that_narrowed_is_reported_as_a_change() {
        let mut before = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs")]);
        before.outside = [
            ("far.rs".to_string(), "door.rs".to_string()),
            ("far.rs".to_string(), "inner1.rs".to_string()),
            ("far.rs".to_string(), "inner2.rs".to_string()),
        ]
        .into_iter()
        .collect();
        let mut after = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs")]);
        after.outside = [("far.rs".to_string(), "door.rs".to_string())]
            .into_iter()
            .collect();

        let body = structural_diff(&before, &after).join("\n");
        assert!(
            body.contains("What changed in the drawing"),
            "a boundary-only change read as nothing:\n{body}"
        );
        assert!(body.contains("inner1.rs` across the boundary"), "{body}");
        assert!(body.contains("inner2.rs` across the boundary"), "{body}");
    }

    /// And a drawing where genuinely nothing moved still says so — the
    /// sentence has to stay usable as evidence for "revert this".
    #[test]
    fn an_unchanged_drawing_still_reports_nothing_moved() {
        let before = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs")]);
        let after = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs")]);
        let body = structural_diff(&before, &after).join("\n");
        assert!(
            body.contains("No edge between these children changed"),
            "{body}"
        );
        assert!(body.contains("landed anywhere new"), "{body}");
    }

    fn baseline(pattern: ShapePattern, edges: &[(&str, &str)]) -> Baseline {
        Baseline {
            scope: "a1b2c3".to_string(),
            pattern,
            compliance: 0.9,
            layering: Some(0.9),
            arborescence: Some(0.7),
            egress: Some(1.0),
            entry_concentration: Some(0.9),
            child_count: 3,
            terms: crate::models::ShapeTerms::default(),
            edges: edges
                .iter()
                .map(|(f, t)| ((*f).to_string(), (*t).to_string()))
                .collect(),
            outside: BTreeSet::new(),
        }
    }

    /// A pair of readings where `branching` fell and two children lost the
    /// only edge that reached them — the shape of a dissolved bag.
    fn strayed() -> (Baseline, Baseline) {
        let mut before = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs")]);
        before.arborescence = Some(0.88);
        before.terms = crate::models::ShapeTerms {
            nodes: 4,
            edges: 3,
            reached: 3,
            strays: 1,
            ..Default::default()
        };
        let mut after = baseline(ShapePattern::Hierarchical, &[("a.rs", "c.rs")]);
        after.arborescence = Some(0.71);
        after.terms = crate::models::ShapeTerms {
            nodes: 5,
            edges: 3,
            reached: 3,
            strays: 3,
            ..Default::default()
        };
        (before, after)
    }

    /// The converse of "a tier rose while nothing changed, revert it".
    /// A number falling on a change that removed no dependency is the case
    /// an agent reported as the one that makes you undo good work, and it
    /// had no words anywhere in the tool.
    #[test]
    fn a_ratio_that_fell_because_children_lost_a_parent_is_explained() {
        let (before, after) = strayed();
        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("not damage"), "{body}");
        assert!(body.contains("2 more children"), "{body}");
        assert!(body.contains("ADR 0022"), "{body}");
    }

    /// And the arithmetic, so the reader can check the explanation rather
    /// than take it.
    #[test]
    fn a_moved_ratio_shows_the_division_it_came_from() {
        let (before, after) = strayed();
        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("branching 3 ÷ 5 → 3 ÷ 5") || body.contains("branching"), "{body}");
        assert!(body.contains("The arithmetic behind those"), "{body}");
    }

    /// The sentence that catches an edit which did not land is left
    /// exactly as it was whenever the scope held. Three permanent
    /// readings would be none — a reader stops at "either".
    #[test]
    fn an_unchanged_scope_leaves_the_sentence_at_two_readings() {
        let before = baseline(ShapePattern::Tangled, &[("a.rs", "b.rs")]);
        let after = before.clone();

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("Nothing has changed"), "{body}");
        assert!(
            !body.contains("configuration"),
            "a scope that held must not be offered as an explanation:\n{body}"
        );
    }

    /// The misattribution CFG-014 is about: the drawing really is
    /// identical, but the reason is that the two readings were taken
    /// under different configurations — not that the edit did nothing.
    #[test]
    fn a_scope_that_moved_is_named_beside_the_unchanged_drawing() {
        let before = baseline(ShapePattern::Tangled, &[("a.rs", "b.rs")]);
        let mut after = before.clone();
        after.scope = "d4e5f6".to_string();

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("Nothing has changed"), "{body}");
        assert!(
            body.contains("Or the configuration did")
                && body.contains("`a1b2c3` → `d4e5f6`"),
            "the configuration change is not named:\n{body}"
        );
        assert!(
            !body.contains("**tangled** → **tangled**"),
            "a scope-only change must not print a diff of numbers that did not move:\n{body}"
        );
    }

    /// The whole ticket end to end: two `reshape` calls with a settings
    /// change between them. The second must say the configuration moved
    /// rather than that nothing did — a server that reads its settings
    /// once can only say the latter, and it reads as "your edit did
    /// nothing to the code".
    #[test]
    fn two_calls_across_a_settings_change_report_the_change() {
        let dir = TmpDir::new("settings");
        dir.write("src/door.rs", "pub fn door() { crate::leaf::leaf(); }\n");
        dir.write("src/leaf.rs", "pub fn leaf() -> u32 { 1 }\n");
        dir.write("src/side.rs", "pub fn side() { crate::leaf::leaf(); }\n");
        let server = server_for(&dir);
        let args = json!({ "path": "src" });

        let first = reshape(&server, &args).expect("src has a shape");
        assert!(
            first.contains("No prior drawing on record"),
            "the first call must say it has nothing to compare against:\n{first}"
        );
        assert!(
            !first.contains("verdict **"),
            "the first call invented a comparison:\n{first}"
        );

        // A pattern that excludes nothing: what the analysis includes is
        // untouched, so the drawing is identical and only the scope moves.
        dir.write(
            ".mezz/settings.json",
            "{ \"exclude_patterns\": [\"**/*.never\"] }\n",
        );

        let second = reshape(&server, &args).expect("src still has a shape");
        assert!(second.contains("Nothing has changed"), "{second}");
        assert!(
            second.contains("Or the configuration did"),
            "the settings change is reported as a failed edit:\n{second}"
        );
    }

    /// The check the closing instruction promises. A tier that rose while
    /// every edge stayed put is the cosmetic change the forbidden list
    /// exists to catch, and the tool — not the agent — has to be the one
    /// that catches it.
    #[test]
    fn a_tier_that_rose_without_any_edge_moving_is_called_out() {
        let before = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs")]);
        let mut after = baseline(ShapePattern::Fractal, &[("a.rs", "b.rs")]);
        after.compliance = 0.99;

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("Revert this"), "{body}");
        assert!(body.contains("hierarchical** → **fractal"), "{body}");
        assert!(
            body.contains("No edge between these children changed"),
            "{body}"
        );
    }

    /// The same rise, with the dependency change that earns it.
    #[test]
    fn a_tier_that_rose_with_the_edges_behind_it_is_confirmed() {
        let before = baseline(
            ShapePattern::Hierarchical,
            &[("a.rs", "b.rs"), ("a.rs", "c.rs")],
        );
        let after = baseline(ShapePattern::Fractal, &[("a.rs", "b.rs")]);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("Confirmed"), "{body}");
        assert!(
            body.contains("removed `a.rs → c.rs`"),
            "the edge behind the verdict is not named:\n{body}"
        );
    }

    /// A skip whose source already reaches the target another way is
    /// removable, and saying so joins the two gates. A skip whose target
    /// merely has several parents is not — flagging it fired on all eleven
    /// listed edges of `src/educator` and promised a clearance neither
    /// gate would give.
    #[test]
    fn only_a_genuinely_redundant_skip_is_flagged_as_clearing_two_gates() {
        let p = picture(
            vec![
                child("mod.rs", 0),
                child("grammar.rs", 1),
                child("lexer.rs", 2),
                child("other.rs", 1),
                child("shared.rs", 2),
            ],
            vec![
                edge("mod.rs", "grammar.rs", EdgeVerdict::Step),
                edge("grammar.rs", "lexer.rs", EdgeVerdict::Step),
                // Redundant: mod.rs already reaches lexer.rs via grammar.rs.
                edge("mod.rs", "lexer.rs", EdgeVerdict::Skip),
                // Not redundant: shared.rs has two parents, but mod.rs has
                // no other route to it.
                edge("mod.rs", "shared.rs", EdgeVerdict::Skip),
                edge("other.rs", "shared.rs", EdgeVerdict::Step),
            ],
        );
        let graph = DependencyGraph::default();
        let body = next_rung_section(
            &shape(ShapePattern::Tangled, Some(ShapeBlocker::Layering(0.6))),
            &p,
            &Thresholds::default(),
            test_ctx(&graph),
        )
        .join("\n");

        assert!(
            body.contains("mod.rs → lexer.rs — also reachable through `grammar.rs`"),
            "the alternative path is not named:\n{body}"
        );
        // The shared target is listed, plainly, with no note attached.
        assert!(body.contains("mod.rs → shared.rs"), "{body}");
        assert_eq!(
            body.matches("also reachable through").count(),
            1,
            "a shared target was mistaken for an alternative path:\n{body}"
        );
        // The note is a lead, not a verdict. Said as a verdict it produced
        // "delete the import", which on src/educator was wrong every time.
        assert!(
            body.contains("candidates, not conclusions"),
            "the path is presented as proof:\n{body}"
        );
        assert!(
            body.contains("re-export"),
            "the disqualifying test is missing:\n{body}"
        );
    }

    /// The `src/parser/typescript` case: breaking the loop was correct and
    /// took the folder up a rung, and the two ratios under it fell because
    /// they are measured over the condensed graph. Reported as a bare
    /// "Confirmed" beside two falling numbers, that reads as damage.
    #[test]
    fn a_cleared_loop_explains_why_the_ratios_under_it_dropped() {
        let mut before = baseline(ShapePattern::Cyclic, &[("a.rs", "b.rs")]);
        before.layering = Some(0.47);
        before.arborescence = Some(0.47);
        let mut after = baseline(ShapePattern::Tangled, &[("a.rs", "c.rs")]);
        after.layering = Some(0.34);
        after.arborescence = Some(0.34);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("Confirmed"), "{body}");
        assert!(
            body.contains("expected here"),
            "the drop is left looking like damage:\n{body}"
        );
        assert!(body.contains("Judge this change on `acyclicity`"), "{body}");
        assert!(
            body.contains("**The falls in `layering` and `branching` are expected here.**"),
            "both fell, so both should be named — and plurally:\n{body}"
        );
    }

    /// The cycles recipe's own prescription — move the shared definition
    /// into a file both ends can depend on — creates a file several
    /// children depend on. That is a real new merge, not arithmetic, and
    /// blaming the whole fall on condensation was wrong about it.
    #[test]
    fn a_fix_that_adds_a_shared_file_owns_the_merge_it_created() {
        let mut before = baseline(ShapePattern::Cyclic, &[("mod.rs", "classes.rs")]);
        before.arborescence = Some(0.73);
        let mut after = baseline(
            ShapePattern::Hierarchical,
            &[
                ("mod.rs", "ctx.rs"),
                ("classes.rs", "ctx.rs"),
                ("functions.rs", "ctx.rs"),
            ],
        );
        after.arborescence = Some(0.61);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("expected here"), "{body}");
        assert!(
            body.contains("`ctx.rs` is new and 3 children depend on it"),
            "the merge the fix created is not owned:\n{body}"
        );
        assert!(body.contains("A loop was exchanged for a merge"), "{body}");
        assert!(
            !body.contains("The drawing did not get worse"),
            "the old blanket claim is back:\n{body}"
        );
    }

    /// A cleared loop that added no file has nothing to own, and must not
    /// be handed a sentence about one.
    #[test]
    fn a_fix_that_added_nothing_says_nothing_about_a_trade() {
        let mut before = baseline(ShapePattern::Cyclic, &[("a.rs", "b.rs"), ("b.rs", "a.rs")]);
        before.arborescence = Some(0.7);
        let mut after = baseline(ShapePattern::Tangled, &[("a.rs", "b.rs")]);
        after.arborescence = Some(0.5);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(body.contains("expected here"), "{body}");
        assert!(!body.contains("trade this fix makes"), "{body}");
    }

    /// The `src/parser/rust` case: the two ratios do not move together.
    /// `layering` rose 0.42 → 0.43 while `branching` fell 0.42 → 0.39, and
    /// naming both asserted a fall that had not happened — which the agent
    /// reading it repeated back as fact.
    #[test]
    fn only_the_ratio_that_actually_fell_is_named() {
        let mut before = baseline(ShapePattern::Cyclic, &[("a.rs", "b.rs")]);
        before.layering = Some(0.42);
        before.arborescence = Some(0.42);
        let mut after = baseline(ShapePattern::Tangled, &[("a.rs", "c.rs")]);
        after.layering = Some(0.43);
        after.arborescence = Some(0.39);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(
            body.contains("**The fall in `branching` is expected here.**"),
            "{body}"
        );
        assert!(
            !body.contains("`layering` and `branching`"),
            "layering rose; naming it as fallen is a false statement:\n{body}"
        );
    }

    /// The note is about clearing a loop. A folder that never had one and
    /// whose layering fell has genuinely got worse, and must not be handed
    /// an excuse for it.
    #[test]
    fn a_drop_without_a_cleared_loop_gets_no_excuse() {
        let mut before = baseline(ShapePattern::Tangled, &[("a.rs", "b.rs")]);
        before.layering = Some(0.80);
        let mut after = baseline(ShapePattern::Tangled, &[("a.rs", "c.rs")]);
        after.layering = Some(0.40);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(!body.contains("expected here"), "{body}");
    }

    /// `entry_concentration` is a ratio, so a folder clears the gate while
    /// a minority of its inbound traffic walks past the door. Three
    /// folders in this repo do exactly that, and the verdict used to say
    /// "nothing to do here" directly above a boundary section listing the
    /// outsiders reaching in. A maintainer saw the piercings on the canvas
    /// and the tool denied them.
    #[test]
    fn a_fractal_folder_that_is_still_pierced_says_so() {
        let mut p = picture(vec![child("mod.rs", 0), child("guts.rs", 1)], Vec::new());
        p.outside = vec![
            OutsideEdge {
                outside: "src/main.rs".to_string(),
                inside: "mod.rs".to_string(),
                child: "mod.rs".to_string(),
                verdict: OutsideVerdict::Entry,
            },
            OutsideEdge {
                outside: "src/other.rs".to_string(),
                inside: "guts.rs".to_string(),
                child: "guts.rs".to_string(),
                verdict: OutsideVerdict::Breach,
            },
        ];
        let body = verdict_section(
            &shape(ShapePattern::Fractal, None),
            &Thresholds::default(),
            &p,
        )
        .join("\n");

        assert!(body.contains("holds its shape at every level"), "{body}");
        assert!(
            body.contains("One reservation the ladder does not charge for: 1 dependency"),
            "the piercing the boundary section lists is denied by the verdict:\n{body}"
        );
        assert!(body.contains("Across the boundary"), "{body}");
    }

    /// A folder with a genuine single door gets the clean verdict, with no
    /// reservation attached to soften it.
    #[test]
    fn a_fractal_folder_with_one_door_gets_no_reservation() {
        let mut p = picture(vec![child("mod.rs", 0)], Vec::new());
        p.outside = vec![OutsideEdge {
            outside: "src/main.rs".to_string(),
            inside: "mod.rs".to_string(),
            child: "mod.rs".to_string(),
            verdict: OutsideVerdict::Entry,
        }];
        let body = verdict_section(
            &shape(ShapePattern::Fractal, None),
            &Thresholds::default(),
            &p,
        )
        .join("\n");

        assert!(body.contains("holds its shape at every level"), "{body}");
        assert!(!body.contains("One reservation"), "{body}");
    }

    /// The `src/ui` case, in the smallest drawing that reproduces it.
    /// `commands.rs → settings.rs` steps one row while `commands.rs` sits
    /// a row down; removing the edge that put it there drops it to the
    /// top row, `settings.rs` stays where a longer chain holds it, and an
    /// edge nobody touched is re-read as a skip. `layering` falls, and the
    /// verdict directly above says the change is worth undoing.
    #[test]
    fn a_fall_the_levels_explain_names_the_edges_that_only_look_worse() {
        let mut before = baseline(
            ShapePattern::Hierarchical,
            &[
                ("x.rs", "y.rs"),
                ("y.rs", "settings.rs"),
                ("app.rs", "commands.rs"),
                ("commands.rs", "settings.rs"),
            ],
        );
        before.layering = Some(1.0);
        let mut after = baseline(
            ShapePattern::Tangled,
            &[
                ("x.rs", "y.rs"),
                ("y.rs", "settings.rs"),
                ("commands.rs", "settings.rs"),
            ],
        );
        after.layering = Some(0.67);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(
            body.contains("rows being re-assigned"),
            "the fall is left reading as damage:\n{body}"
        );
        assert!(
            body.contains("`commands.rs → settings.rs` — a step last call, a skip now."),
            "the edge that changed reading without changing is not named:\n{body}"
        );
        // The counterfactual is stated as a number, not as reassurance.
        assert!(
            body.contains("comes to 1.00 — no worse than the 1.00"),
            "{body}"
        );
        // An edge that reads the same under both rulers is not evidence.
        assert!(!body.contains("`x.rs → y.rs`"), "{body}");
    }

    /// The other half of the same test. A skip that was added is a real
    /// skip, and a fall the edge delta accounts for must keep reading as
    /// a fall — otherwise every fall gets a caveat and none of them mean
    /// anything.
    #[test]
    fn a_fall_the_new_edges_account_for_gets_no_excuse() {
        let mut before = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs"), ("b.rs", "c.rs")]);
        before.layering = Some(1.0);
        let mut after = baseline(
            ShapePattern::Tangled,
            &[("a.rs", "b.rs"), ("b.rs", "c.rs"), ("a.rs", "c.rs")],
        );
        after.layering = Some(0.67);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(
            !body.contains("rows being re-assigned"),
            "an added skip was excused as arithmetic:\n{body}"
        );
    }

    /// A file that did not exist last call has no row to have moved, so
    /// its edges are the drawing changing and are read under the rows its
    /// own drawing gives it. Scored the other way — as artefacts of a
    /// ruler that had never heard of them — the skip this new file really
    /// does add would be explained away along with the re-levelling that
    /// happened beside it.
    #[test]
    fn a_new_child_is_the_drawing_changing_and_not_the_ruler_moving() {
        let mut before = baseline(
            ShapePattern::Hierarchical,
            &[
                ("x.rs", "y.rs"),
                ("y.rs", "settings.rs"),
                ("app.rs", "commands.rs"),
                ("commands.rs", "settings.rs"),
            ],
        );
        before.layering = Some(1.0);
        // The same re-levelling as above, and a new file reaching past a
        // row on its way in.
        let mut after = baseline(
            ShapePattern::Tangled,
            &[
                ("x.rs", "y.rs"),
                ("y.rs", "settings.rs"),
                ("commands.rs", "settings.rs"),
                ("new.rs", "settings.rs"),
            ],
        );
        after.layering = Some(0.5);

        let body = progress_section(Some(&before), &after).join("\n");
        assert!(
            !body.contains("rows being re-assigned"),
            "a skip the new file genuinely added was excused as arithmetic:\n{body}"
        );
    }

    /// A first call has nothing to compare against and must not invent a
    /// comparison. It must also not stay *silent* about that, which is what it
    /// used to do: an empty section reads identically to "nothing changed",
    /// and a field report hit exactly that on the largest tier move of a
    /// session — `hierarchical → fractal`, two edges appearing, and the
    /// comparator said nothing. Absence is now stated; the comparison is still
    /// not invented.
    #[test]
    fn the_first_call_says_it_has_nothing_to_compare_against() {
        let after = baseline(ShapePattern::Hierarchical, &[("a.rs", "b.rs")]);
        let body = progress_section(None, &after).join("\n");

        assert!(body.contains("No prior drawing on record"), "{body}");
        // The two readings the old silence was ambiguous between, both ruled
        // out in words.
        assert!(body.contains("not \"nothing changed\""), "{body}");
        // A restart is no longer one of the explanations: the record is
        // mirrored to the cache directory, so it outlives the process.
        assert!(body.contains("outlives the server"), "{body}");
        assert!(body.contains("no longer loses it"), "{body}");
        // Still no comparison: no verdict arrow, no metric diff.
        assert!(!body.contains("verdict **"), "{body}");
        assert!(!body.contains("Nothing has changed"), "{body}");
    }
}
