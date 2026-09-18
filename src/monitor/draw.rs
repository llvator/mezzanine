//! The drawing, and nothing else.
//!
//! Every number this file renders was decided by [`super::sample`],
//! [`super::series`] or [`super::movers`]. Nothing is computed here beyond
//! how to fit it on the screen — which is what keeps the dashboard testable
//! without a terminal.
//!
//! ## Colour
//!
//! Only foreground colours, only from the sixteen the terminal already has,
//! and never a background. A dashboard that paints its own panel is legible
//! on the theme it was written against and unreadable on the other one, and
//! the operator running it in a tmux split beside their agents did not
//! choose their theme for this.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::movers::Mover;
use super::series::{Anchor, Series};
use super::sample::{Commit, Metrics, Verdict};

/// Below this the second column of tiles is dropped, then the sparklines.
const TWO_COLUMN_WIDTH: u16 = 78;
const SPARK_WIDTH: usize = 12;

/// Everything the screen shows, gathered by the caller.
pub struct View<'a> {
    pub repo: &'a str,
    /// `None` until the first reading lands.
    pub series: Option<&'a Series>,
    pub movers: &'a [Mover],
    /// What the engine is doing, from the activity sink, or `None` when idle.
    pub running: Option<&'a str>,
    pub paused: bool,
    /// Attempts that failed since the last good reading.
    pub failed: u32,
    /// The commit the head side is pinned to. `Some` means this screen is a
    /// comparison of two commits and cannot change again: no sparklines, since
    /// two states are not a series, and no keys that would move a zero the
    /// operator named on the command line (MON-008).
    pub against: Option<&'a Commit>,
}

/// One figure on the dashboard.
struct Tile {
    label: &'static str,
    value: String,
    /// `None` for a figure with no baseline to move from.
    delta: Option<f64>,
    /// How to word the delta: `2` prints `+0.25`, `0` prints `+4`.
    places: usize,
    spark: String,
    /// Whether an increase is the bad direction. Cohesion rises when things
    /// improve; smells do not.
    up_is_bad: bool,
}

pub fn dashboard(frame: &mut Frame, view: &View, movers_state: &mut ListState) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(tile_height(view, frame.area().width)),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    metrics(frame, areas[0], view);
    moved(frame, areas[1], view, movers_state);
    frame.render_widget(Paragraph::new(footer(view)), areas[2]);
}

/// Whether the tiles are drawn two-up, given the width they are laid out in.
///
/// One answer, asked by the two places that must not differ: [`tile_height`]
/// sizes the panel and [`rows`] fills it. When they disagreed the panel was
/// sized for one layout and drawn in the other, and a `Paragraph` does not
/// report the lines it could not fit — the tiles past the bottom border simply
/// were not there.
pub(super) fn paired(width: u16) -> bool {
    width >= TWO_COLUMN_WIDTH
}

/// The width the tile grid is laid out in: the panel's, less the border down
/// each side.
///
/// The whole of the bug this exists to close. `dashboard` knows the panel's
/// outer width, `metrics` lays the tiles out in `block.inner`, and two columns
/// of tiles do not stop fitting at the same number in both — so at exactly 78
/// and 79 the height was computed two-up and the grid drawn one-up, and the
/// bottom four tiles (`cycles`, `cohesion`, `unreadable`, `rules`) were
/// silently clipped.
pub(super) fn inner_width(outer: u16) -> u16 {
    outer.saturating_sub(2)
}

/// How many lines `tiles` many tiles occupy in a panel `outer` columns wide.
///
/// Split out of [`tile_height`] so the reserved height can be checked against
/// the drawn one without a terminal or a `View` to build — the two are only
/// equal because both resolve the layout through [`paired`] and
/// [`inner_width`], and that is the property worth a test rather than the
/// arithmetic on either side of it.
pub(super) fn grid_rows(tiles: u16, outer: u16) -> u16 {
    match paired(inner_width(outer)) {
        true => tiles.div_ceil(2),
        false => tiles,
    }
}

/// How tall the metric panel has to be: two borders plus a row per pair of
/// tiles, or per tile when the terminal is too narrow to pair them.
///
/// `width` is the panel's *outer* width, which is what the caller has before
/// the block exists to be asked for its inner one.
fn tile_height(view: &View, width: u16) -> u16 {
    // Two more for the shape-ladder and smell-kind lines under the grid.
    grid_rows(tiles(view).len() as u16, width) + 4
}

fn metrics(frame: &mut Frame, area: Rect, view: &View) {
    let block = Block::bordered()
        .title(format!(" mezz monitor · {} ", view.repo))
        .title_top(Line::from(status(view)).right_aligned())
        .title_bottom(Line::from(anchor(view, area.width)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = rows(&tiles(view), inner.width);
    lines.push(Line::from(ladder(view)));
    lines.push(Line::from(kinds(view)));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The figures, in reading order: the four the operator watches on the left,
/// their context on the right.
fn tiles(view: &View) -> Vec<Tile> {
    let Some(series) = view.series else {
        return Vec::new();
    };
    let width = spark_width(view);
    let tile = |label, value: String, pick: fn(&Metrics) -> f64, places, up_is_bad| Tile {
        label,
        value,
        delta: Some(series.delta(pick)),
        places,
        spark: series.spark(pick, width),
        up_is_bad,
    };
    let m = &series.latest().metrics;
    vec![
        // First, and deliberately: it is the only figure on the panel that is
        // about the whole tree rather than about one of the tree's problems,
        // so it is the one an operator glancing at a tmux split reads.
        tile("score", format!("{:.2}", m.score), |m| m.score as f64, 2, true),
        tile("smells", count(m.smells), |m| m.smells as f64, 0, true),
        tile(
            "entities",
            count(m.entities),
            |m| m.entities as f64,
            0,
            false,
        ),
        tile(
            "avg cplx",
            format!("{:.1}", m.mean_cyclomatic),
            |m| m.mean_cyclomatic as f64,
            1,
            true,
        ),
        tile("loc", count(m.loc), |m| m.loc as f64, 0, false),
        tile(
            "over ceiling",
            count(m.over_ceiling),
            |m| m.over_ceiling as f64,
            0,
            true,
        ),
        tile(
            "red entities",
            count(m.red_entities),
            |m| m.red_entities as f64,
            0,
            true,
        ),
        tile("cycles", count(m.cycles), |m| m.cycles as f64, 0, true),
        tile(
            "cohesion",
            format!("{:.2}", m.mean_cohesion),
            |m| m.mean_cohesion as f64,
            2,
            false,
        ),
        // The one tile drawn as a share, because it is the one figure whose
        // bare count says nothing: twelve unreadable folders is a different
        // tree at twenty folders and at four hundred. The noun the value used
        // to carry is gone to pay for the denominator — `12 / 87 folders` is
        // fifteen characters against the thirteen `tile_spans` gives every
        // value, and a tile that overflows its column costs the whole grid the
        // alignment it exists for. The label carries the sense instead, beside
        // `cycles` and `cohesion`, which are folder figures too.
        tile(
            "unreadable",
            format!("{} / {}", count(m.shape.unreadable()), count(m.shape.total())),
            |m| m.shape.unreadable() as f64,
            0,
            true,
        ),
        rules_tile(series, width),
    ]
}

/// How many notches of history to draw beside each figure.
///
/// None at all in a comparison: two states are not a series, and one notch
/// scaled against itself is a flat block that says "nothing is moving" about a
/// screen whose whole subject is what moved (MON-008).
fn spark_width(view: &View) -> usize {
    match view.against {
        Some(_) => 0,
        None => SPARK_WIDTH,
    }
}

/// The rules figure, the one that can have no answer.
///
/// Two of its three states are words rather than numbers, and they are said
/// rather than rounded to zero: a dashboard that cannot tell "passing" from
/// "ungraded" from "your rules file is broken" is reporting on a measurement
/// it never made. See [`Verdict`].
fn rules_tile(series: &Series, width: usize) -> Tile {
    let pick = |m: &Metrics| m.rules.count().unwrap_or(0) as f64;
    let said = |value: &str| Tile {
        label: "rules",
        value: value.to_string(),
        delta: None,
        places: 0,
        spark: String::new(),
        up_is_bad: true,
    };
    match series.latest().metrics.rules {
        Verdict::Undeclared => said("none declared"),
        Verdict::Unreadable => said("unreadable"),
        Verdict::Broken(n) => Tile {
            label: "rules",
            value: format!("{n} broken"),
            delta: Some(series.delta(pick)),
            places: 0,
            spark: series.spark(pick, width),
            up_is_bad: true,
        },
    }
}

/// Lay the tiles out two-up, or one-up on a narrow terminal.
fn rows(tiles: &[Tile], width: u16) -> Vec<Line<'static>> {
    if tiles.is_empty() {
        return vec![Line::from(Span::styled(
            "measuring…",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let spark = width >= 40;
    match paired(width) {
        true => tiles
            .chunks(2)
            .map(|pair| {
                let mut spans = tile_spans(&pair[0], spark);
                if let Some(second) = pair.get(1) {
                    spans.push(Span::raw("  "));
                    spans.extend(tile_spans(second, spark));
                }
                Line::from(spans)
            })
            .collect(),
        false => tiles
            .iter()
            .map(|t| Line::from(tile_spans(t, spark)))
            .collect(),
    }
}

/// One tile: label, value, delta, sparkline — in fixed columns, so the eye
/// can travel down a column instead of hunting along each row.
fn tile_spans(tile: &Tile, spark: bool) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(
            format!("{:<13}", tile.label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{:>13}", tile.value),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        delta_span(tile),
    ];
    if spark {
        spans.push(Span::styled(
            format!(" {:<width$}", tile.spark, width = SPARK_WIDTH),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans
}

/// `▲+4`, `▼-0.2`, or a dim `=` when nothing moved.
fn delta_span(tile: &Tile) -> Span<'static> {
    let Some(delta) = tile.delta else {
        return Span::raw(format!("{:<9}", ""));
    };
    // A float that rounds to nothing at the shown precision is not a move —
    // printing `▲+0.0` would have the operator looking for a change that the
    // number itself says is not there.
    let shown = format!("{:+.*}", tile.places, delta);
    if shown.parse::<f64>().unwrap_or(delta) == 0.0 {
        return Span::styled(format!("{:<9}", "  ="), Style::default().fg(Color::DarkGray));
    }
    let up = delta > 0.0;
    let arrow = match up {
        true => '▲',
        false => '▼',
    };
    let colour = match up == tile.up_is_bad {
        true => Color::Red,
        false => Color::Green,
    };
    Span::styled(
        format!("{:<9}", format!("{arrow}{shown}")),
        Style::default().fg(colour),
    )
}

/// The line along the bottom border: what the deltas are measured from, and
/// whether that is still the commit in force.
///
/// On the border rather than in a row of its own, because the panel's height
/// is counted in tiles ([`tile_height`]) and a sentence given a row of its own
/// is a row the movers list does not get.
///
/// Nothing at all until the first reading lands. The baseline is measured
/// before the tree is, so there is a window where a commit is known and the
/// tiles are still empty — naming a zero over a panel with no numbers in it
/// tells the reader something they cannot use yet.
pub fn anchor(view: &View, width: u16) -> Vec<Span<'static>> {
    let Some(series) = view.series else {
        return Vec::new();
    };
    // A comparison names both of its states and never the HEAD notice: the
    // head side is a commit the operator pinned, so "HEAD has moved" would be
    // true of a tree this screen is not about, and `B` cannot answer it.
    if let Some(to) = view.against {
        return vec![Span::styled(
            between_text(series.anchor(), to, width),
            Style::default().fg(Color::DarkGray),
        )];
    }
    let moved = series.behind_head().is_some();
    let text = match series.anchor() {
        Anchor::Commit(commit) => commit_text(commit, subject_width(width, moved)),
        // A moment can only be described by how it was taken — there is no
        // name for it to carry.
        Anchor::Reading => " baseline: a reading of the working tree ".to_string(),
    };
    let mut spans = vec![Span::styled(text, Style::default().fg(Color::DarkGray))];
    if moved {
        // Yellow, not red: the deltas are exactly as true as they were a
        // commit ago. What has changed is that "since the baseline" and
        // "uncommitted" have stopped being the same sentence.
        spans.push(Span::styled(
            "· HEAD has moved · B re-baselines ",
            Style::default().fg(Color::Yellow),
        ));
    }
    spans
}

/// The commit half, with as much of the subject as there is room for. Seven
/// hex digits identify a commit to git and to nobody else, so the subject goes
/// wherever it fits.
fn commit_text(commit: &Commit, budget: usize) -> String {
    match budget >= 8 && !commit.subject.is_empty() {
        true => format!(
            " baseline {} · {} ",
            commit.sha,
            clip(&commit.subject, budget)
        ),
        false => format!(" baseline {} ", commit.sha),
    }
}

/// The border of a comparison: both states, and as much of the head commit's
/// subject as there is room for.
///
/// The head subject rather than the baseline's, because the baseline is the
/// state the reader already chose to measure from and the head is the one they
/// are asking about.
fn between_text(from: &Anchor, to: &Commit, width: u16) -> String {
    /// ` 0123456 → 89abcde · ` plus the two corners.
    const FIXED: usize = 24;
    let Anchor::Commit(from) = from else {
        // Refused by `Ends::pair` before the screen is taken, so this is here
        // to be honest rather than to be reached.
        return format!(" the working tree → {} ", to.sha);
    };
    let budget = (width as usize).saturating_sub(FIXED);
    match budget >= 8 && !to.subject.is_empty() {
        true => format!(
            " {} → {} · {} ",
            from.sha,
            to.sha,
            clip(&to.subject, budget)
        ),
        false => format!(" {} → {} ", from.sha, to.sha),
    }
}

/// How much of the subject fits, once the SHA and the HEAD notice have taken
/// their share of the border.
fn subject_width(width: u16, moved: bool) -> usize {
    /// ` baseline 0123456 · ` plus the two corners.
    const FIXED: usize = 22;
    /// The yellow half, when it is there.
    const NOTICE: usize = 35;
    (width as usize).saturating_sub(FIXED + if moved { NOTICE } else { 0 })
}

/// Text that fits, keeping its head — the opposite of [`elide`], because the
/// first words of a commit subject are the ones that say what it did.
fn clip(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let head: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{head}…")
}

/// One tier of the shape ladder, as [`ladder`] draws it.
struct Tier {
    label: &'static str,
    colour: Color,
    /// How to read a rise. `None` for the two middle tiers: a folder arrives
    /// there by climbing out of `tangled` or by falling out of `fractal`, and
    /// the count alone cannot say which — so it is reported without a verdict
    /// rather than given the wrong one.
    up_is_bad: Option<bool>,
    pick: fn(&Metrics) -> u32,
}

/// The four tiers, best first — the opposite of [`super::sample::ShapeCounts`],
/// whose fields run worst-first. Read left to right this is a tree improving,
/// which is the direction an operator wants the weight to move in.
const LADDER: [Tier; 4] = [
    Tier {
        label: "fractal",
        colour: Color::Green,
        up_is_bad: Some(false),
        pick: |m| m.shape.fractal,
    },
    Tier {
        label: "hierarchical",
        colour: Color::Cyan,
        up_is_bad: None,
        pick: |m| m.shape.hierarchical,
    },
    Tier {
        label: "tangled",
        colour: Color::Yellow,
        up_is_bad: None,
        pick: |m| m.shape.tangled,
    },
    Tier {
        label: "cyclic",
        colour: Color::Red,
        up_is_bad: Some(true),
        pick: |m| m.shape.cyclic,
    },
];

/// The ladder's verdict policy, for the test that pins it. The table itself
/// stays private: nothing outside the drawing has a use for the colours.
#[cfg(test)]
pub(super) fn ladder_tiers() -> Vec<(&'static str, Option<bool>)> {
    LADDER.iter().map(|t| (t.label, t.up_is_bad)).collect()
}

/// How many folders there are, and how they are spread across the ladder.
///
/// A line rather than five more tiles. Five tiles is three more rows of a
/// panel whose height comes straight out of the movers list below it, and the
/// four counts are one distribution rather than four figures: read across,
/// they say what shape the tree is in; read a row apart, each one is a number
/// with no denominator. The `unreadable` tile above is this same population's
/// headline — both come from one [`super::sample::ShapeCounts`], counted in
/// one pass, so the line and the tile cannot drift.
fn ladder(view: &View) -> Vec<Span<'static>> {
    let Some(series) = view.series else {
        return Vec::new();
    };
    let mut spans = vec![
        Span::styled(
            format!("{:<13}", "folders"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            count(series.latest().metrics.shape.total()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    for tier in &LADDER {
        spans.extend(tier_spans(series, tier));
    }
    spans
}

/// One tier: its count, its name, and how far it has moved from the baseline.
fn tier_spans(series: &Series, tier: &Tier) -> Vec<Span<'static>> {
    let pick = tier.pick;
    let dim = Style::default().fg(Color::DarkGray);
    let mut spans = vec![
        Span::styled(" · ", dim),
        Span::styled(
            count(pick(&series.latest().metrics)),
            Style::default().fg(tier.colour),
        ),
        Span::styled(format!(" {}", tier.label), dim),
    ];
    let moved = series.delta(|m| pick(m) as f64);
    if moved != 0.0 {
        // Dim wherever the tier itself has no direction, so a green `+3` never
        // congratulates a reader for folders that fell into `hierarchical`.
        let colour = match tier.up_is_bad {
            Some(bad) if (moved > 0.0) == bad => Color::Red,
            Some(_) => Color::Green,
            None => Color::DarkGray,
        };
        spans.push(Span::styled(
            format!(" {moved:+.0}"),
            Style::default().fg(colour),
        ));
    }
    spans
}

/// The smell kinds behind the count, or the test-code aside when there are
/// no smells to break down.
fn kinds(view: &View) -> Vec<Span<'static>> {
    let Some(series) = view.series else {
        return Vec::new();
    };
    let latest = series.latest();
    let mut text: Vec<String> = latest
        .top_smell_kinds(4)
        .into_iter()
        .map(|(kind, n)| format!("{kind} {n}"))
        .collect();
    if latest.metrics.smells_in_tests > 0 {
        text.push(format!(
            "{} in test code, not counted",
            latest.metrics.smells_in_tests
        ));
    }
    vec![Span::styled(
        text.join(" · "),
        Style::default().fg(Color::DarkGray),
    )]
}

fn moved(frame: &mut Frame, area: Rect, view: &View, state: &mut ListState) {
    let ticks = view.series.map_or(0, |s| s.since_baseline());
    // A comparison has exactly one reading standing against its baseline, and
    // saying so as `1 ticks` would offer a count that cannot go up as if it
    // could.
    let block = Block::bordered().title(match view.against {
        Some(_) => " moved between the two commits ".to_string(),
        None => format!(" moved since baseline · {ticks} ticks "),
    });
    if view.movers.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(Span::styled(
                match ticks {
                    0 => "nothing measured since the baseline yet.",
                    _ => "nothing has moved.",
                },
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }
    let items: Vec<ListItem> = view.movers.iter().map(mover_item).collect();
    frame.render_stateful_widget(List::new(items).block(block), area, state);
}

fn mover_item(m: &Mover) -> ListItem<'static> {
    let (mark, colour) = match m.worse {
        true => ("▲", Color::Red),
        false => ("▼", Color::Green),
    };
    ListItem::new(Line::from(vec![
        Span::styled(format!(" {mark} "), Style::default().fg(colour)),
        Span::raw(format!("{:<44}", elide(&m.path, 44))),
        Span::styled(m.what.clone(), Style::default().fg(Color::DarkGray)),
    ]))
}

/// A path that fits, keeping its tail — the file name is what identifies it.
fn elide(path: &str, width: usize) -> String {
    let count = path.chars().count();
    if count <= width {
        return path.to_string();
    }
    let tail: String = path.chars().skip(count - width + 1).collect();
    format!("…{tail}")
}

/// The word in the top-right corner, and the one figure on screen that is
/// about the dashboard rather than the tree.
///
/// `stale` is the case MON-006 added. An analysis that could not complete
/// leaves every number below unchanged and still correct *about an older
/// tree*; saying `watching` over them is the frozen-but-live failure
/// [`super::Dashboard::drive`] breaks the loop over, and a reader has no other
/// way to tell. A retry in flight shows the running phase instead, because
/// then the answer is already being recomputed.
fn status(view: &View) -> Vec<Span<'static>> {
    let (text, colour) = match (view.paused, view.running, view.failed) {
        (true, _, _) => ("paused".to_string(), Color::Yellow),
        (false, Some(phase), _) => (phase.to_string(), Color::Cyan),
        // Not green: green here means "these figures are as new as the tree",
        // and a comparison makes no such claim — it is a still of two commits,
        // and the tree may have moved on since either of them.
        (false, None, 0) if view.against.is_some() => ("comparison".to_string(), Color::Cyan),
        (false, None, 0) => ("watching".to_string(), Color::Green),
        (false, None, 1) => ("stale · last reading failed".to_string(), Color::Red),
        (false, None, n) => (format!("stale · {n} readings failed"), Color::Red),
    };
    vec![
        Span::styled(format!(" {text} "), Style::default().fg(colour)),
        Span::raw(""),
    ]
}

/// The key hints, and what the session has measured so far.
///
/// A comparison offers only the two keys that still mean something. The three
/// it drops are not disabled quietly: an operator who reads `b re-baseline` and
/// presses it is owed a moved zero, and there is none to move to when both ends
/// were named on the command line (MON-008).
fn footer(view: &View) -> Line<'static> {
    let keys = match view.against {
        Some(_) => " q quit · ↑↓ scroll",
        None => " q quit · b re-baseline · B at HEAD · p pause · ↑↓ scroll",
    };
    let read = view.series.map(|s| measured(view, s)).unwrap_or_default();
    Line::from(Span::styled(
        format!("{keys}{read}"),
        Style::default().fg(Color::DarkGray),
    ))
}

/// The figure at the end of the footer: how much has been read, and how long
/// the last of it took.
fn measured(view: &View, series: &Series) -> String {
    let ms = series.latest().analysis_ms;
    // Counting readings on a screen that will only ever hold two of them
    // measures nothing; what the operator wants to know there is what the
    // second checkout cost.
    if view.against.is_some() {
        return format!(" · the head side in {ms}ms");
    }
    let n = series.len();
    let noun = match n {
        1 => "reading",
        _ => "readings",
    };
    format!(" · {n} {noun} · last in {ms}ms")
}

/// `4,812` — thousands separated, because a five-digit entity count changing
/// in its hundreds is unreadable without it.
fn count(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
