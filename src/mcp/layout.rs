//! `layout` — where the files in one folder would sit if the drawing
//! decided, and what that would measure out at.
//!
//! `reshape` answers "what is wrong here and what is the one change". It
//! stops one step short of the question an agent restructuring a folder
//! actually has to answer, which is *what does the folder look like
//! afterwards*. Field reports from agents driving `reshape` all describe
//! the same loop: guess a grouping, edit the tree, re-measure, find the
//! number went the wrong way, undo. Two or three rounds of that per
//! folder, and the layout that survives is the one that happened to score,
//! not the one that was reasoned about.
//!
//! Both halves of that loop are things mezz already holds the data for, so
//! this tool closes it:
//!
//! 1. **Proposing.** [`grouping::propose`] reads the folder's own drawing
//!    and returns the subfolders it implies — each headed by the child
//!    every path into the group passes through, which is what makes each
//!    proposed folder have exactly one door.
//! 2. **Scoring.** [`relayout::relaid_out`] rewrites the tree's paths and
//!    the ordinary folder pass scores the result. The number reported for
//!    a hypothetical is the number the folder will get once the move is
//!    made, because it is computed by the same code over the same inputs.
//!
//! Nothing is written. A layout is a claim about paths, and the caller
//! makes it true with `git mv`.
//!
//! ## The honest limit, which the output states every time
//!
//! Moving files changes no dependency. That is why this is safe to
//! propose, and it is also the ceiling on what it can buy: breadth,
//! branching and layering are properties of the drawing and a relayout can
//! genuinely fix them; coupling is not, and a folder whose files are
//! tangled with each other stays tangled in whatever arrangement. When the
//! measured "after" says so, the tool says so.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::Value;

use crate::analyzer::folder_shape;
use crate::analyzer::grouping::{self, Group, NoLayout};
use crate::analyzer::relayout::{self, Move};
use crate::graph::FileGraph;
use crate::models::{FolderPicture, FolderShape, Thresholds};

use super::format::{listed, num};
use super::reshape::{rel, target_folder};
use super::tools::cap_lines;
use super::McpServer;

/// `layout` — the folder rearranged the way its own drawing says, scored
/// against the arrangement it has.
pub fn layout(server: &McpServer, args: &Value) -> Result<String> {
    let folder = target_folder(server, args)?;
    let absolute = folder.display().to_string();
    // The whole repo, for the reason `reshape` analyses the whole repo:
    // half of what a folder's shape says is a fact about who reaches in
    // from outside it.
    let graph = super::tools::analyze(server, &server.root)?;
    let before = Reading::take(&graph.file_graph(), &absolute).ok_or_else(|| {
        anyhow::anyhow!(
            "No analysed folder at {}. Folders come from the files in scope, so a \
             directory holding nothing mezz parsed has no shape to report.",
            rel(&folder, &server.root),
        )
    })?;

    let plan = Plan::of(args, &before, server, &folder)?;
    let after = Reading::take(&relayout::relaid_out(&before.tree, &plan.moves), &absolute);

    let root = &server.root;
    let mut body = vec![format!("# Layout for {}", rel(&folder, root)), String::new()];
    body.extend(now_section(&before, root));
    body.extend(plan.describe(&before, root));
    body.extend(after_section(&before, after.as_ref(), &plan, root));
    let spell_out = !server
        .layout_caveat_spelled_out
        .swap(true, std::sync::atomic::Ordering::Relaxed);
    body.extend(limits_section(&plan, spell_out));
    Ok(cap_lines(
        body,
        "Pass fewer `moves`, or call `layout` on a subfolder.",
    ))
}

// ------------------------------------------------------------------
//  One reading of the tree
// ------------------------------------------------------------------

/// Everything this tool needs to say about one arrangement of the files.
///
/// The tree travels with the reading so the "after" can be derived from
/// the "before" by one rewrite, and so both readings are demonstrably
/// taken over the same pass rather than two that agree.
struct Reading {
    tree: FileGraph,
    shape: FolderShape,
    picture: FolderPicture,
    /// Folder → every file in it something outside depends on. The count
    /// this tool reports as "ways in", which is
    /// [`folder_shape::entered_by_folder`] and not the door ratio: a
    /// caller asking whether a folder has one entry point wants the
    /// number, not the share the busiest of them takes.
    ways_in: HashMap<String, Vec<String>>,
}

impl Reading {
    fn take(tree: &FileGraph, folder: &str) -> Option<Reading> {
        let shape = folder_shape::compute(
            tree.files.iter().map(String::as_str),
            &tree.pairs,
            &tree.imports,
            &tree.folders,
            &tree.declaration_only,
        )
        .remove(folder)?;
        let picture = folder_shape::picture(
            folder,
            tree.files.iter().map(String::as_str),
            &tree.pairs,
            &tree.imports,
            &tree.folders,
            &tree.declaration_only,
        )?;
        Some(Reading {
            ways_in: folder_shape::entered_by_folder(&tree.pairs, &tree.folders),
            tree: clone_tree(tree),
            shape,
            picture,
        })
    }

    /// How many ways into `folder` there are, or `0` for one nothing
    /// outside depends on.
    fn entries_to(&self, folder: &str) -> usize {
        self.ways_in.get(folder).map_or(0, Vec::len)
    }
}

/// `FileGraph` holds no `Clone`, and a `Reading` has to own its tree —
/// the "after" is derived from the "before"'s, which would otherwise have
/// to outlive a borrow through two more passes.
fn clone_tree(tree: &FileGraph) -> FileGraph {
    FileGraph {
        files: tree.files.clone(),
        pairs: tree.pairs.clone(),
        folders: tree.folders.clone(),
        declaration_only: tree.declaration_only.clone(),
        imports: tree.imports.clone(),
    }
}

// ------------------------------------------------------------------
//  The plan
// ------------------------------------------------------------------

/// The moves being scored, and where they came from.
struct Plan {
    moves: Vec<Move>,
    /// The groups behind them, when mezz proposed the layout. Empty for a
    /// caller-supplied one: a hand-written move list has no head to name
    /// and no dominance claim to make about it.
    groups: Vec<Group>,
    /// Why the proposer found nothing, straight from the pass that
    /// decided it. Reconstructing this from the picture got it wrong —
    /// see [`NoLayout`].
    why_none: Option<NoLayout>,
    proposed: bool,
}

impl Plan {
    fn of(args: &Value, before: &Reading, server: &McpServer, folder: &Path) -> Result<Plan> {
        match args.get("moves").and_then(Value::as_array) {
            Some(raw) => Ok(Plan {
                moves: parse_moves(raw, &before.tree, server, folder)?,
                groups: Vec::new(),
                why_none: None,
                proposed: false,
            }),
            None => {
                let proposal = grouping::propose(&before.picture);
                Ok(Plan {
                    moves: proposal.groups.iter().flat_map(Group::moves).collect(),
                    groups: proposal.groups,
                    why_none: proposal.why_none,
                    proposed: true,
                })
            }
        }
    }
}

/// Read and check the caller's move list.
///
/// Checked rather than trusted because every failure here is silent
/// otherwise: a misspelled path moves nothing, and the tool would report
/// an unchanged score as the honest answer to a question it never asked.
fn parse_moves(
    raw: &[Value],
    tree: &FileGraph,
    server: &McpServer,
    folder: &Path,
) -> Result<Vec<Move>> {
    let known: HashSet<&str> = tree
        .files
        .iter()
        .chain(tree.folders.iter())
        .map(String::as_str)
        .collect();
    let mut moves: Vec<Move> = Vec::new();
    for entry in raw {
        let what = absolute(entry, "what", server)?;
        let into = absolute(entry, "into", server)?;
        if !known.contains(what.as_str()) {
            bail!(
                "`{}` is not an analysed file or folder, so moving it would change \
                 nothing this measures. Call `map` on {} for the paths in scope.",
                rel(Path::new(&what), &server.root),
                rel(folder, &server.root),
            );
        }
        if moves.iter().any(|m| m.what == what) {
            bail!(
                "`{}` is named by two moves and so has no single destination.",
                rel(Path::new(&what), &server.root),
            );
        }
        if into == what || into.starts_with(&format!("{what}{}", std::path::MAIN_SEPARATOR)) {
            bail!(
                "`{}` cannot move inside itself.",
                rel(Path::new(&what), &server.root),
            );
        }
        moves.push(Move { what, into });
    }
    if moves.is_empty() {
        bail!("`moves` was empty. Omit it entirely to have mezz propose the layout.");
    }
    Ok(moves)
}

/// One path field, resolved against the project root.
fn absolute(entry: &Value, field: &str, server: &McpServer) -> Result<String> {
    let raw = entry
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("Every entry in `moves` needs a non-empty `{field}`, relative to the project root.")
        })?;
    let joined: PathBuf = server.root.join(raw);
    if !joined.starts_with(&server.root) {
        bail!("`{raw}` leaves the project root.");
    }
    Ok(joined.display().to_string())
}

// ------------------------------------------------------------------
//  Rendering
// ------------------------------------------------------------------

fn now_section(before: &Reading, root: &Path) -> Vec<String> {
    let s = &before.shape;
    vec![
        format!(
            "**{}** as it stands — {} children, {} edges between them, {} \
             {} in. Layering {}, branching {}, entry concentration {}, out at the \
             bottom {}.",
            s.pattern.label(),
            s.child_count,
            before.picture.edges.len(),
            before.entries_to(&before.picture.folder),
            if before.entries_to(&before.picture.folder) == 1 {
                "way"
            } else {
                "ways"
            },
            num(s.layering),
            num(s.arborescence),
            num(s.entry_concentration),
            num(s.egress),
        ),
        String::new(),
        format!(
            "Every path below is relative to the project root; the folder is `{}`.",
            rel(Path::new(&before.picture.folder), root)
        ),
        String::new(),
    ]
}

impl Plan {
    /// The moves, grouped the way they were derived.
    fn describe(&self, before: &Reading, root: &Path) -> Vec<String> {
        if !self.proposed {
            return caller_moves(&self.moves, root);
        }
        if self.groups.is_empty() {
            return no_grouping(self.why_none);
        }
        let mut body = vec![
            "## The layout its own drawing implies".to_string(),
            String::new(),
            format!(
                "{}, headed by the one child every path into {} passes through. That \
                 is what a dominator is, and it is why {} has exactly one way in — by \
                 construction, not by hoping the score lands well.",
                plural(self.groups.len(), "group"),
                if self.groups.len() == 1 { "it" } else { "each" },
                if self.groups.len() == 1 {
                    "it"
                } else {
                    "each of these folders"
                },
            ),
            String::new(),
        ];
        body.push(
            "The folder names are the head file's, and are a suggestion — rename \
             them, or rename the door to your language's entry-file convention, \
             without changing anything measured here. A folder called `noteUid/` \
             holding `noteUid.ts` reads as a stutter and an `index.ts` does not; \
             both are the same drawing."
                .to_string(),
        );
        body.push(String::new());
        for group in &self.groups {
            body.extend(group_lines(group, root));
        }
        body.extend(left_alone(&self.groups, before, root));
        body
    }
}

fn group_lines(group: &Group, root: &Path) -> Vec<String> {
    let held = group.behind.len() + usize::from(group.head_moves);
    let mut body = vec![
        format!(
            "**`{}/`** — {}, entered at `{}`",
            rel(Path::new(&group.into), root),
            plural(held, "file"),
            rel(Path::new(&group.head), root),
        ),
        String::new(),
    ];
    if group.head_moves {
        body.push(format!(
            "- `git mv {} {}/` — the door",
            rel(Path::new(&group.head), root),
            rel(Path::new(&group.into), root),
        ));
    }
    body.extend(listed(group.behind.iter().map(|b| {
        format!(
            "`git mv {} {}/`",
            rel(Path::new(b), root),
            rel(Path::new(&group.into), root)
        )
    })));
    body.push(String::new());
    body
}

/// The children no group claimed, and why that is the right answer.
fn left_alone(groups: &[Group], before: &Reading, root: &Path) -> Vec<String> {
    let claimed: BTreeSet<&str> = groups
        .iter()
        .flat_map(|g| {
            g.behind
                .iter()
                .map(String::as_str)
                .chain(g.head_moves.then_some(g.head.as_str()))
        })
        .collect();
    let staying: Vec<String> = before
        .picture
        .children
        .iter()
        .filter(|c| !claimed.contains(c.path.as_str()))
        .map(|c| format!("`{}`", rel(Path::new(&c.path), root)))
        .collect();
    if staying.is_empty() {
        return Vec::new();
    }
    vec![
        format!(
            "Staying put: {}. More than one entry reaches each of these, so no single \
             sibling owns them — filing them under one anyway is the move `reshape` \
             warns about, and it would show up below as branching that did not \
             improve.",
            staying.join(", ")
        ),
        String::new(),
    ]
}

fn no_grouping(why: Option<NoLayout>) -> Vec<String> {
    vec![
        "## No layout to propose".to_string(),
        String::new(),
        format!(
            "The drawing implies no subfolders. That is a finding rather than a \
             failure: {}",
            why_nothing(why)
        ),
        String::new(),
        "Pass `moves` explicitly to score a layout of your own against this one."
            .to_string(),
        String::new(),
    ]
}

/// Which of the three ways a folder can have no grouping this one is.
///
/// Read off the pass that decided rather than re-inferred here. The
/// re-inferred version printed "every child here is reached from more than
/// one place, which is what a folder of shared contracts looks like" over
/// `plugin/settings`, a folder whose children each had exactly one
/// in-folder dependent — the door. Right answer, wrong reason, and the
/// wrong reason sends a reader hunting a shared-contract problem that is
/// not there.
///
/// The three want opposite next moves, which is why they are three.
fn why_nothing(why: Option<NoLayout>) -> &'static str {
    match why {
        Some(NoLayout::NothingDrawn) => {
            "these children never mention each other, so there is no dependency \
             structure to group along. Any grouping would be by topic, which is a \
             judgement call this tool has no data for."
        }
        Some(NoLayout::OnlyTheDoor) => {
            "the only thing that dominates anything here is the folder's own way in \
             — its wiring file, or the single door every path enters through. A \
             folder built behind that is this folder again with a segment added, so \
             the pass looks one level past it, and one level past it these children \
             are siblings that do not own each other. Nothing is wrong with this \
             level; it is already a fan-out from one entry."
        }
        _ => {
            "every child here is reached from more than one place, which is what a \
             folder of shared contracts looks like. Grouping it would file shared \
             things under one of their users."
        }
    }
}

fn caller_moves(moves: &[Move], root: &Path) -> Vec<String> {
    let mut body = vec![
        "## Your moves".to_string(),
        String::new(),
        format!("{}, scored below against the tree as it stands.", plural(moves.len(), "move")),
        String::new(),
    ];
    body.extend(listed(moves.iter().map(|m| {
        format!(
            "`{}` → `{}/`",
            rel(Path::new(&m.what), root),
            rel(Path::new(&m.into), root)
        )
    })));
    body.push(String::new());
    body
}

/// The measured result — the point of the tool.
fn after_section(
    before: &Reading,
    after: Option<&Reading>,
    plan: &Plan,
    root: &Path,
) -> Vec<String> {
    if plan.moves.is_empty() {
        return Vec::new();
    }
    let Some(after) = after else {
        return vec![
            "## What it measures out at".to_string(),
            String::new(),
            "These moves empty the folder, so there is no drawing left to score. \
             That is a real answer — but it is a deletion of the level rather than a \
             layout for it."
                .to_string(),
            String::new(),
        ];
    };
    let mut body = vec![
        "## What it measures out at".to_string(),
        String::new(),
        "Measured, not estimated: the paths were rewritten and the same folder pass \
         that produced the numbers above was run again over the result."
            .to_string(),
        String::new(),
    ];
    body.extend(comparison_table(before, after));
    body.push(String::new());
    body.extend(gate_crossings(before, after));
    body.extend(arithmetic(before, after));
    body.extend(new_folder_lines(plan, after, root));
    body.push(verdict_line(before, after));
    body.push(String::new());
    body
}

fn comparison_table(before: &Reading, after: &Reading) -> Vec<String> {
    let (b, a) = (&before.shape, &after.shape);
    let folder = &before.picture.folder;
    let rows = [
        ("verdict", b.pattern.label().to_string(), a.pattern.label().to_string()),
        ("layering", num(b.layering), num(a.layering)),
        ("branching", num(b.arborescence), num(a.arborescence)),
        (
            "entry concentration",
            num(b.entry_concentration),
            num(a.entry_concentration),
        ),
        ("out at the bottom", num(b.egress), num(a.egress)),
        ("compliance", num(Some(b.compliance)), num(Some(a.compliance))),
        ("children", b.child_count.to_string(), a.child_count.to_string()),
        (
            "edges in this drawing",
            before.picture.edges.len().to_string(),
            after.picture.edges.len().to_string(),
        ),
        (
            "ways into this folder",
            before.entries_to(folder).to_string(),
            after.entries_to(folder).to_string(),
        ),
    ];
    let mut table = vec![
        "| | now | after |".to_string(),
        "| --- | --- | --- |".to_string(),
    ];
    table.extend(rows.iter().map(|(name, was, now)| {
        let mark = if was == now { "" } else { " ←" };
        format!("| {name} | {was} | {now}{mark} |")
    }));
    table
}

/// Which bars the moved numbers crossed, and which gate binds afterwards.
///
/// The table shows the digits and nothing marks a crossing, so a forecast
/// of `branching 0.71 → 0.67` reads as a small fall rather than as the
/// move that puts the folder the wrong side of the 0.70 gate. Reported
/// from the field on exactly that: the layout was right and worth keeping,
/// but it changed which gate `reshape` names next, and "worth doing, it
/// does not finish the job" reads identically either way.
///
/// The binding gate comes off `FolderShape::blocker`, which the classifier
/// already decided for both readings — so this cannot name a different
/// gate from the one `reshape` will name on the next call.
fn gate_crossings(before: &Reading, after: &Reading) -> Vec<String> {
    let t = Thresholds::default();
    let (b, a) = (&before.shape, &after.shape);
    let mut lines: Vec<String> = [
        ("layering", b.layering, a.layering, t.shape_layering),
        ("branching", b.arborescence, a.arborescence, t.shape_arborescence),
        (
            "entry concentration",
            b.entry_concentration,
            a.entry_concentration,
            t.shape_entry,
        ),
        ("out at the bottom", b.egress, a.egress, t.shape_egress),
        (
            "compliance",
            Some(b.compliance),
            Some(a.compliance),
            t.shape_compliance,
        ),
    ]
    .iter()
    .filter_map(|&(name, was, now, bar)| crossing(name, was?, now?, bar))
    .collect();
    if let Some(line) = breadth_crossing(b.child_count, a.child_count, t.shape_max_children) {
        lines.push(line);
    }
    if let Some(line) = binding_gate(b, a) {
        lines.push(line);
    }
    if lines.is_empty() {
        return Vec::new();
    }
    let mut body = vec!["What that does to the gates:".to_string(), String::new()];
    body.extend(listed(lines));
    body.push(String::new());
    body
}

/// One ratio's bar, when it changed sides. `None` when it did not — a
/// number that moved within a band it was already on the wrong (or right)
/// side of has crossed nothing.
fn crossing(name: &str, was: f32, now: f32, bar: f32) -> Option<String> {
    match (was >= bar, now >= bar) {
        (true, false) => Some(format!(
            "**`{name}` falls below the {bar:.2} gate it was clearing** — {was:.2} \
             → {now:.2}. This is now a gate the folder fails."
        )),
        (false, true) => Some(format!(
            "`{name}` clears the {bar:.2} gate — {was:.2} → {now:.2}."
        )),
        _ => None,
    }
}

/// The child bar, which reads the other way round from every ratio here.
fn breadth_crossing(was: u32, now: u32, bar: u32) -> Option<String> {
    match (was <= bar, now <= bar) {
        (false, true) => Some(format!(
            "the level comes under the {bar}-child bar — {was} → {now}."
        )),
        (true, false) => Some(format!(
            "**the level goes over the {bar}-child bar** — {was} → {now}."
        )),
        _ => None,
    }
}

/// Which gate holds the folder back, when the answer changes.
///
/// Compared by kind rather than by value: a blocker whose measurement
/// moved is the same gate still binding, and reporting that as a change
/// would fire on almost every call.
fn binding_gate(before: &FolderShape, after: &FolderShape) -> Option<String> {
    let kind = |b: &Option<crate::models::ShapeBlocker>| {
        b.as_ref().map(std::mem::discriminant)
    };
    if kind(&before.blocker) == kind(&after.blocker) {
        return None;
    }
    Some(match (before.blocker, after.blocker) {
        (Some(was), Some(now)) => format!(
            "**The gate that binds changes** — from {} to {}. `reshape` will name \
             the second one next, and ask for a different thing than it asked for \
             before.",
            was.summary(),
            now.summary(),
        ),
        (Some(was), None) => format!(
            "**Every gate passes.** {} was the last one holding it.",
            was.summary()
        ),
        (None, Some(now)) => format!(
            "**A gate that was passing now fails** — {}. Read this beside the verdict              below before applying.",
            now.summary()
        ),
        (None, None) => unreachable!("kinds differ, so at least one is Some"),
    })
}

/// Every moved ratio decomposed into the counts it was divided from.
///
/// Printed because agents cannot otherwise predict what a layout will
/// score, and an agent that cannot predict a score chooses between
/// candidate layouts by editing the tree and re-measuring — which is the
/// loop this tool exists to end. One field report had `branching`
/// reverse-engineered as *children with one parent ÷ (children − 1)*,
/// which is not the formula and agrees with it on some folders.
///
/// The counts come off [`ShapeTerms`], filled beside the division that
/// produced the ratio, so this cannot drift from the number above it.
fn arithmetic(before: &Reading, after: &Reading) -> Vec<String> {
    let (b, a) = (&before.shape.terms, &after.shape.terms);
    let rows = [
        (
            "layering",
            "edges stepping exactly one level down ÷ edges",
            (b.tight, b.edges),
            (a.tight, a.edges),
        ),
        (
            "branching",
            "children an edge arrives at ÷ (edges + roots past the first)",
            (b.reached, b.edges + b.strays.saturating_sub(1)),
            (a.reached, a.edges + a.strays.saturating_sub(1)),
        ),
        (
            "entry concentration",
            "arrivals landing on the busiest file ÷ arrivals from outside",
            (b.busiest, b.arrivals),
            (a.busiest, a.arrivals),
        ),
    ];
    let moved: Vec<String> = rows
        .iter()
        .filter(|(_, _, was, now)| was != now)
        .map(|(name, formula, was, now)| {
            format!(
                "**{name}** = {formula} — was {}, now {}",
                division(*was),
                division(*now)
            )
        })
        .collect();
    if moved.is_empty() {
        return Vec::new();
    }
    let mut body = vec![
        "Where those came from. The counts are the scoring pass's own, so you can \
         work out what a different set of moves would score without making them:"
            .to_string(),
        String::new(),
    ];
    body.extend(listed(moved));
    body.push(String::new());
    body.extend(condensation_caveat(before, after));
    body.extend(fallen_note(before, after));
    body
}

/// Why the `edges` in those formulas is smaller than the arrow count in
/// the table above it.
///
/// Both numbers are right and they count different things: the table
/// counts the arrows a reader sees, and the ratios are taken after each
/// dependency loop collapses to a single node, so a loop's internal
/// arrows are charged to `acyclicity` and not to `layering` twice. Two
/// numbers under one word is exactly the kind of thing that gets a
/// formula written down wrong, so it is said rather than left to be
/// noticed.
fn condensation_caveat(before: &Reading, after: &Reading) -> Vec<String> {
    let hidden = before.shape.terms.edges as usize != before.picture.edges.len()
        || after.shape.terms.edges as usize != after.picture.edges.len();
    if !hidden {
        return Vec::new();
    }
    vec![
        "`edges` there is lower than the arrow count in the table: the ratios are \
         taken after each dependency loop collapses to one node, so a loop is \
         charged to `acyclicity` rather than to `layering` a second time."
            .to_string(),
        String::new(),
    ]
}

/// One side of a ratio, or what it means to have no side at all.
///
/// A zero denominator is not `0 ÷ 0`, which reads as a catastrophe: it is
/// a ratio with nothing left to measure, and the table prints it as an em
/// dash for exactly that reason. Grouping a folder down to a single child
/// takes every edge out of its drawing and reaches this legitimately.
fn division((top, bottom): (u32, u32)) -> String {
    if bottom == 0 {
        return "not measured — nothing left in this drawing to divide".to_string();
    }
    format!("{top} ÷ {bottom}")
}

/// Why `branching` can fall on a regrouping that was right.
///
/// The gap in the tooling a field report named exactly: the docs warn that
/// a tier rising while every edge stays put should be reverted, and say
/// nothing about the converse — a number falling on a change that removed
/// no dependency. That is the case that makes an agent undo good work, so
/// it gets the same treatment in the other direction.
fn fallen_note(before: &Reading, after: &Reading) -> Vec<String> {
    let (b, a) = (&before.shape.terms, &after.shape.terms);
    let fell = match (before.shape.arborescence, after.shape.arborescence) {
        (Some(was), Some(now)) => now < was,
        _ => false,
    };
    if !fell {
        return Vec::new();
    }
    let cause = if a.strays > b.strays {
        format!(
            "{} more of these children are now reached by no edge in *this* drawing — \
             the sibling that depended on them went into a subfolder and took the edge \
             with it. Every root past the first joins the denominator (ADR 0022), so \
             the ratio falls",
            a.strays - b.strays
        )
    } else {
        format!(
            "the drawing lost {} and {} of the children they arrived at, and \
             `branching` is a ratio over what is left",
            plural(b.edges.saturating_sub(a.edges) as usize, "edge"),
            b.reached.saturating_sub(a.reached),
        )
    };
    vec![
        format!(
            "**`branching` fell, and that is not evidence the move was wrong.** It fell \
             because {cause}. No dependency changed — a relayout cannot change one. Judge this on the \
             verdict and on the child count, and if the fall bothers you, the honest \
             reading is that `branching` measures the parent's drawing and the parent's \
             drawing now has less in it."
        ),
        String::new(),
    ]
}

/// What each folder the layout creates comes out at — the claim about one
/// door, checked rather than asserted.
fn new_folder_lines(plan: &Plan, after: &Reading, root: &Path) -> Vec<String> {
    let created: BTreeSet<&str> = plan.moves.iter().map(|m| m.into.as_str()).collect();
    let t = Thresholds::default();
    let lines: Vec<String> = created
        .iter()
        .map(|folder| {
            let crowded = after
                .tree
                .files
                .iter()
                .filter(|f| f.starts_with(&format!("{folder}{}", std::path::MAIN_SEPARATOR)))
                .count();
            format!(
                "`{}/` — {}, {}{}",
                rel(Path::new(folder), root),
                plural(crowded, "file"),
                ways_in(after.entries_to(folder)),
                crowding(crowded, t.shape_max_children),
            )
        })
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut body = vec!["The folders this creates:".to_string(), String::new()];
    body.extend(listed(lines));
    body.push(String::new());
    body
}

/// What to say about how much a created folder ends up holding.
///
/// A folder around one file earns a warning of its own. It reads as a
/// grouping and is not one: it narrows no level, buries the file one path
/// segment deeper, and can still move a ratio, which makes it the folder
/// form of the wrapper on `reshape`'s forbidden list.
fn crowding(held: usize, ceiling: u32) -> &'static str {
    if held <= 1 {
        return ". A folder around one file is not a grouping — it narrows nothing \
                and buries the file a segment deeper. Drop this one";
    }
    if held as u32 > ceiling {
        return ". Over the child bar itself — call `layout` on it next";
    }
    ""
}

/// Whether to do it, in one sentence.
fn verdict_line(before: &Reading, after: &Reading) -> String {
    let (b, a) = (&before.shape, &after.shape);
    // A folder already at the top tier has no rung to climb, and the
    // numbers below it will still wobble a point either way on any
    // regrouping. Selling a move on that wobble is how a measure gets
    // gamed; the tool has to be willing to say "leave it".
    if b.pattern == crate::models::ShapePattern::Fractal {
        return "**Leave it alone.** This folder is already `fractal` — the top tier, \
                and the one the whole ladder is climbing towards. Whatever these \
                moves do to the second decimal, there is no rung above to reach."
            .to_string();
    }
    match a.pattern.cmp(&b.pattern) {
        std::cmp::Ordering::Greater => format!(
            "**Worth doing.** The tier rises to `{}` on a change that moves files and \
             nothing else. Apply the moves, then re-run `reshape` on the folder to \
             see it against the drawing it had.",
            a.pattern.label()
        ),
        std::cmp::Ordering::Less => format!(
            "**Do not.** The tier falls to `{}`. A relayout that scores worse has cut \
             across the drawing rather than along it.",
            a.pattern.label()
        ),
        std::cmp::Ordering::Equal => within_tier(before, after),
    }
}

/// The verdict when the tier held, which is most of the time — a folder
/// two rungs down does not reach the top on one regrouping.
///
/// Judged on the blend and on the two counts a relayout can honestly
/// move, rather than on the tier alone. An earlier version answered
/// "nothing measurable moved" over a change that lifted `compliance` by
/// six points, which is the tool disowning its own reading.
fn within_tier(before: &Reading, after: &Reading) -> String {
    let (b, a) = (&before.shape, &after.shape);
    // A blend of four ratios moves in the third decimal on rounding
    // alone; a claim of "better" wants more than that behind it.
    const NOISE: f32 = 0.005;
    let narrower = a.child_count < b.child_count;
    let sparser = after.picture.edges.len() < before.picture.edges.len();
    if a.compliance < b.compliance - NOISE {
        return format!(
            "**Do not, on these numbers.** {}, but compliance falls {:.2} → {:.2}: \
             what this arrangement takes out of the parent's drawing it charges to \
             the folder it puts it in. Every count is above if you want to overrule \
             it.",
            if narrower || sparser {
                improvement(narrower, sparser, b.compliance, a.compliance)
            } else {
                "The tier holds".to_string()
            },
            b.compliance,
            a.compliance
        );
    }
    if narrower || sparser || a.compliance > b.compliance + NOISE {
        return format!(
            "**Worth doing, but it does not finish the job.** {} without the tier \
             moving, so a different gate is holding it — run `reshape` on the folder \
             after applying these to see which one.",
            improvement(narrower, sparser, b.compliance, a.compliance)
        );
    }
    "**No.** Nothing measurable moved. These moves rename paths and leave the \
     drawing as it was, which is the cosmetic change the measure exists to catch."
        .to_string()
}

/// What actually got better, named rather than summarised — a reader
/// deciding whether to spend a `git mv` on it wants the specific thing.
fn improvement(narrower: bool, sparser: bool, was: f32, now: f32) -> String {
    let mut parts: Vec<String> = Vec::new();
    if narrower {
        parts.push("the level gets narrower".to_string());
    }
    if sparser {
        parts.push("the drawing loses edges".to_string());
    }
    if now > was {
        parts.push(format!("compliance rises {was:.2} → {now:.2}"));
    }
    let mut sentence = match parts.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, front)) => format!("{}, and {last}", front.join(", ")),
        None => String::new(),
    };
    if let Some(first) = sentence.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    sentence
}

fn limits_section(plan: &Plan, spell_out: bool) -> Vec<String> {
    let mut body = caveat(spell_out);
    body.extend([
        "## Next".to_string(),
        String::new(),
        if plan.moves.is_empty() {
            "There is nothing here to apply. `reshape` on this folder names the gate \
             that is actually holding it, and on a folder this tool has no layout for \
             that gate is about coupling rather than arrangement."
                .to_string()
        } else if plan.proposed {
            "Apply the `git mv` lines above, fix the import specifiers your compiler \
             now complains about, and re-run `reshape` on the folder. Nothing was \
             written by this call."
                .to_string()
        } else {
            "Nothing was written by this call. Omit `moves` to see the layout the \
             folder's own drawing implies, and score yours against it."
                .to_string()
        },
    ]);
    body
}

/// What a relayout cannot buy — in full the first time this session, as
/// one sentence afterwards.
///
/// The caveat is load-bearing: without it a caller can read a rise in
/// `branching` as having decoupled something, which is the claim the
/// re-export shim makes on `reshape`'s forbidden list. It is also the same
/// four lines every call, arriving beside `reshape`'s own unabridged
/// rules — an agent working through several folders reported that fixing
/// one and adding the other left the total higher than before.
fn caveat(spell_out: bool) -> Vec<String> {
    if !spell_out {
        return vec![
            "_A relayout changes no dependency — it changes which folder's drawing \
             each edge lands in. Spelled out in full on the first `layout` of this \
             session._"
                .to_string(),
            String::new(),
        ];
    }
    vec![
        "## What a layout cannot do".to_string(),
        String::new(),
        "Moving a file changes no dependency. Every file still imports exactly what \
         it imported, and both ends of every edge survived the rewrite — what moved \
         is which folder's drawing each edge lands in. So `branching`, `layering` and \
         the child count above are real improvements to a picture, and nothing here \
         is a decoupling."
            .to_string(),
        String::new(),
        "In particular: if the numbers barely moved, the folder's problem is coupling \
         rather than arrangement, and no layout will fix it. `reshape` names the \
         edge to invert or the file to split."
            .to_string(),
        String::new(),
    ]
}

/// `1 file` / `3 files`, for a count that reads badly as a bare number.
fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// How many ways into a folder there are, in words.
///
/// Its own sentence at zero, because "0 ways in" reads as a defect and is
/// the opposite: nothing outside the folder depends on anything in it,
/// which is the most self-contained a folder gets.
fn ways_in(n: usize) -> String {
    match n {
        0 => "nothing outside depends on it".to_string(),
        1 => "1 way in".to_string(),
        _ => format!("{n} ways in"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap as Map;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    /// A throwaway project the real analyzer can be pointed at.
    ///
    /// The prefix carries neither "test" nor "spec", for the reason
    /// `reshape`'s fixture gives: `is_test_path` matches on the whole
    /// path, and a fixture under a directory named for the test has every
    /// one of its entities filtered out.
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "mezz-layout-{}-{}-{}",
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
            graph_cache: Mutex::new(Map::new()),
            base_cache: Mutex::new(Map::new()),
            generation: Arc::new(AtomicU64::new(0)),
            shape_baselines: Default::default(),
            rules_spelled_out: Default::default(),
            layout_caveat_spelled_out: Default::default(),
        }
    }

    /// A folder wide enough that grouping it is a real split.
    ///
    /// `api.ts` is the door. Behind it `core.ts` fans out to three
    /// parsers nothing else reaches, and `fmt.ts` sits behind `util.ts`;
    /// `shared.ts` is reached from both sides and belongs to neither. The
    /// door itself dominates the whole folder, which is exactly the shape
    /// the proposer has to descend past rather than wrap.
    fn wide_tree(name: &str) -> TmpDir {
        let dir = TmpDir::new(name);
        dir.write(
            "app.ts",
            "import { serve } from './pack/api';\nexport const go = () => serve();\n",
        );
        dir.write(
            "pack/api.ts",
            "import { run } from './core';\nimport { helper } from './util';\n\
             export function serve() {\n  return run() + helper();\n}\n",
        );
        dir.write(
            "pack/core.ts",
            "import { readOne } from './parseOne';\nimport { readTwo } from './parseTwo';\n\
             import { readThree } from './parseThree';\nimport { common } from './shared';\n\
             export function run() {\n  return readOne() + readTwo() + readThree() + common();\n}\n",
        );
        dir.write(
            "pack/util.ts",
            "import { render } from './fmt';\nimport { common } from './shared';\n\
             export function helper() {\n  return render() + common();\n}\n",
        );
        for (file, func) in [
            ("parseOne", "readOne"),
            ("parseTwo", "readTwo"),
            ("parseThree", "readThree"),
            ("fmt", "render"),
            ("shared", "common"),
        ] {
            dir.write(
                &format!("pack/{file}.ts"),
                &format!("export function {func}() {{\n  return 1;\n}}\n"),
            );
        }
        dir
    }

    #[test]
    fn the_proposal_names_the_dominator_as_the_door() {
        let dir = wide_tree("propose");
        let out = layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        assert!(out.contains("The layout its own drawing implies"), "{out}");
        assert!(out.contains("entered at `pack/core.ts`"), "{out}");
        assert!(out.contains("git mv pack/parseOne.ts pack/core/"), "{out}");
        assert!(out.contains("git mv pack/parseTwo.ts pack/core/"), "{out}");
    }

    /// The folder's own door dominates every file in it, so dominance
    /// alone would propose wrapping the whole folder in `api/`. That is a
    /// rename, and the pass has to descend past it.
    #[test]
    fn the_folders_own_door_is_not_proposed_as_a_group() {
        let dir = wide_tree("no-wrap");
        let out = layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        assert!(!out.contains("git mv pack/api.ts"), "{out}");
    }

    /// A file two entries reach is nobody's private business. This is the
    /// case `reshape` describes in prose as a bag that is honest to leave
    /// alone, made mechanical.
    #[test]
    fn a_shared_file_is_left_where_it_is_and_said_so() {
        let dir = wide_tree("shared");
        let out = layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        assert!(out.contains("Staying put:"), "{out}");
        assert!(out.contains("`pack/shared.ts`"), "{out}");
        assert!(!out.contains("git mv pack/shared.ts"), "{out}");
    }

    /// The property the whole construction exists for.
    #[test]
    fn every_folder_the_proposal_creates_has_one_way_in() {
        let dir = wide_tree("one-door");
        let out = layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        assert!(out.contains("`pack/core/` — 4 files, 1 way in"), "{out}");
    }

    /// Nothing is written. A layout is a claim about paths.
    #[test]
    fn nothing_is_written_to_disk() {
        let dir = wide_tree("read-only");
        let before: Vec<_> = std::fs::read_dir(dir.0.join("pack"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        let after: Vec<_> = std::fs::read_dir(dir.0.join("pack"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(before.len(), after.len());
        assert!(!dir.0.join("pack/core").exists());
    }

    /// The formulas, published where an agent reading a moved score needs
    /// them — the fix for reverse-engineering `branching` from observed
    /// values and getting it wrong.
    #[test]
    fn a_moved_ratio_is_shown_as_the_division_behind_it() {
        let dir = wide_tree("arithmetic");
        let out = layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        assert!(
            out.contains("children an edge arrives at ÷ (edges + roots past the first)"),
            "{out}"
        );
        assert!(out.contains(" ÷ "), "{out}");
    }

    #[test]
    fn a_caller_supplied_move_is_scored_rather_than_proposed() {
        let dir = wide_tree("what-if");
        let out = layout(
            &server_for(&dir),
            &json!({
                "path": "pack",
                "moves": [{ "what": "pack/parseOne.ts", "into": "pack/core" }],
            }),
        )
        .unwrap();
        assert!(out.contains("## Your moves"), "{out}");
        assert!(!out.contains("The layout its own drawing implies"), "{out}");
        assert!(out.contains("What it measures out at"), "{out}");
    }

    /// A misspelled path would otherwise move nothing, and an unchanged
    /// score would be reported as the honest answer to a question that
    /// was never asked.
    #[test]
    fn a_path_that_was_never_analysed_is_refused() {
        let dir = wide_tree("unknown");
        let err = layout(
            &server_for(&dir),
            &json!({ "path": "pack", "moves": [{ "what": "pack/ghost.ts", "into": "pack/x" }] }),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("not an analysed file or folder"), "{err}");
    }

    /// A folder of files nothing links has no dominance to read, and the
    /// tool has to say so rather than invent a grouping by name.
    #[test]
    fn an_unlinked_folder_gets_a_finding_not_a_guess() {
        let dir = TmpDir::new("unlinked");
        dir.write("pack/a.ts", "export const a = 1;\n");
        dir.write("pack/b.ts", "export const b = 2;\n");
        dir.write("app.ts", "import { a } from './pack/a';\nimport { b } from './pack/b';\nexport const go = () => a + b;\n");
        let out = layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        assert!(out.contains("No layout to propose"), "{out}");
        assert!(out.contains("no dependency structure to group along"), "{out}");
        assert!(out.contains("There is nothing here to apply"), "{out}");
    }

    /// A bar is only crossed when the sides differ. A number that moved
    /// within the band it was already in has crossed nothing, and a line
    /// saying otherwise on every call is a line nobody reads.
    #[test]
    fn only_a_number_that_changes_sides_is_a_crossing() {
        assert!(crossing("branching", 0.71, 0.67, 0.70).is_some());
        assert!(crossing("branching", 0.90, 0.75, 0.70).is_none());
        assert!(crossing("branching", 0.40, 0.55, 0.70).is_none());
        let cleared = crossing("layering", 0.45, 0.86, 0.70).unwrap();
        assert!(cleared.contains("clears the 0.70 gate"), "{cleared}");
        let lost = crossing("branching", 0.71, 0.67, 0.70).unwrap();
        assert!(lost.contains("falls below the 0.70 gate"), "{lost}");
    }

    /// The child count reads the other way round from every ratio here —
    /// fewer is better — so its test for "crossed" is inverted.
    #[test]
    fn the_child_bar_is_read_the_other_way_round() {
        assert!(breadth_crossing(9, 7, 8).unwrap().contains("comes under"));
        assert!(breadth_crossing(7, 9, 8).unwrap().contains("goes over"));
        assert!(breadth_crossing(9, 10, 8).is_none());
    }

    /// The reason this section exists: a forecast of `0.71 -> 0.67` reads
    /// as a small fall, and is the move that changes what `reshape` will
    /// ask for next.
    #[test]
    fn a_gate_that_starts_binding_is_named_not_just_numbered() {
        let shape = |blocker| FolderShape {
            pattern: crate::models::ShapePattern::Hierarchical,
            compliance: 0.9,
            acyclicity: 1.0,
            layering: Some(0.9),
            arborescence: Some(0.7),
            entry_concentration: Some(0.43),
            egress: None,
            child_compliance: None,
            child_count: 7,
            blocker: Some(blocker),
            terms: crate::models::ShapeTerms::default(),
        };
        let line = binding_gate(
            &shape(crate::models::ShapeBlocker::Entry(0.43)),
            &shape(crate::models::ShapeBlocker::Merges(0.67)),
        )
        .unwrap();
        assert!(line.contains("The gate that binds changes"), "{line}");

        // Same gate, different measurement, is the same gate still
        // binding — and firing on that would fire on almost every call.
        assert!(binding_gate(
            &shape(crate::models::ShapeBlocker::Entry(0.43)),
            &shape(crate::models::ShapeBlocker::Entry(0.51)),
        )
        .is_none());
    }

    /// The limit that keeps this from being sold as decoupling.
    #[test]
    fn the_output_always_states_that_no_dependency_changed() {
        let dir = wide_tree("limits");
        let out = layout(&server_for(&dir), &json!({ "path": "pack" })).unwrap();
        assert!(out.contains("Moving a file changes no dependency"), "{out}");
    }
}
