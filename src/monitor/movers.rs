//! What moved, and where — the half of the dashboard a number cannot give.
//!
//! `smells 33 → 37` tells an operator that four arrived and nothing about
//! where to look. This module answers the second question by diffing the
//! per-scope rows of two readings: the baseline and the latest.
//!
//! ## Improvements are listed too
//!
//! A regressions-only list would make a swarm look like it is only ever
//! doing damage, and would hide the case the operator most wants to see —
//! an agent that was asked to reduce complexity and did. Worse first, better
//! after, because the first is what needs a decision.
//!
//! ## Both readings are walked, not just the latest (MON-004)
//!
//! The first version walked `now` alone, which made two whole classes of
//! change invisible. A file the baseline had and the tree no longer does
//! produced no row — so a swarm deleting the repo's worst file dropped the
//! `smells` tile by six under a list that said *nothing has moved*, which is
//! the exact question this module exists to answer, and deletion is the single
//! largest improvement anyone can make to a bad file. And a folder the
//! baseline never saw produced no row whatever tier it landed in — so a new
//! `cyclic` folder, the loudest thing that can happen to a tree's shape and
//! precisely what a swarm asked to extract a module produces, was silent.
//!
//! Both halves now walk the union of the two readings.

use super::sample::{FileRow, Scopes};
use crate::models::ShapePattern;

/// One scope that reads differently than it did at the baseline.
#[derive(Clone, Debug, PartialEq)]
pub struct Mover {
    pub path: String,
    /// True when the change is in the wrong direction.
    pub worse: bool,
    /// What moved, already worded: `cplx 22→31`, `+1 smell`,
    /// `shape fractal→tangled`.
    pub what: String,
    /// How far it moved, for ranking. Unitless and only ever compared with
    /// other movers of the same kind of subject.
    weight: u32,
}

/// A file arriving or leaving is only worth a row once it is carrying
/// something. Every file a swarm adds would otherwise appear here, which is a
/// file list, not a findings list — and the mirror argument holds for a swarm
/// clearing two hundred generated files.
const NEW_FILE_FLOOR: u32 = 10;

/// Every scope that moved between two readings, worse first, capped.
pub fn between(base: &Scopes, now: &Scopes, limit: usize) -> Vec<Mover> {
    let mut movers = files(base, now);
    movers.extend(folders(base, now));
    // Worse before better; within each, the largest move first; then path,
    // so two equal moves do not swap places between ticks that measured the
    // same tree.
    movers.sort_by(|a, b| {
        b.worse
            .cmp(&a.worse)
            .then(b.weight.cmp(&a.weight))
            .then(a.path.cmp(&b.path))
    });
    movers.truncate(limit);
    movers
}

/// Files whose worst complexity or smell count changed, arrived, or left.
fn files(base: &Scopes, now: &Scopes) -> Vec<Mover> {
    let present = now
        .files
        .iter()
        .filter_map(|(path, row)| match base.files.get(path) {
            Some(was) => moved(path, was, row),
            None => arrived(path, row),
        });
    let gone = base
        .files
        .iter()
        .filter(|(path, _)| !now.files.contains_key(*path))
        .filter_map(|(path, was)| departed(path, was));
    present.chain(gone).collect()
}

/// A file that was already there at the baseline.
fn moved(path: &str, was: &FileRow, now: &FileRow) -> Option<Mover> {
    let mut parts = Vec::new();
    let mut weight = 0;
    let mut worse = false;

    if now.max_cyclomatic != was.max_cyclomatic {
        parts.push(format!(
            "cplx {}→{}",
            was.max_cyclomatic, now.max_cyclomatic
        ));
        weight += now.max_cyclomatic.abs_diff(was.max_cyclomatic);
        worse |= now.max_cyclomatic > was.max_cyclomatic;
    }
    if now.smells != was.smells {
        let delta = now.smells as i64 - was.smells as i64;
        parts.push(format!("{delta:+} smell"));
        // A smell is a coarser event than a complexity point, and a reader
        // scanning the list wants the file that grew one above the file that
        // grew three cyclomatic points in a function that was already gnarly.
        weight += now.smells.abs_diff(was.smells) * 10;
        worse |= delta > 0;
    }

    match parts.is_empty() {
        true => None,
        false => Some(Mover {
            path: path.to_string(),
            worse,
            what: parts.join("  "),
            weight,
        }),
    }
}

/// A file the baseline had never seen.
fn arrived(path: &str, now: &FileRow) -> Option<Mover> {
    if now.smells == 0 && now.max_cyclomatic < NEW_FILE_FLOOR {
        return None;
    }
    let smells = match now.smells {
        0 => String::new(),
        n => format!("  {n} smell"),
    };
    Some(Mover {
        path: path.to_string(),
        worse: true,
        what: format!("new  cplx {}{}", now.max_cyclomatic, smells),
        weight: now.max_cyclomatic + now.smells * 10,
    })
}

/// A file the tree no longer has.
///
/// The mirror of [`arrived`], down to the floor and the weight, so a deletion
/// ranks against an arrival on the same scale rather than on a second one
/// invented here. Never a regression: whatever the file was carrying left with
/// it, and every figure it contributed to can only fall.
///
/// What it was carrying is named rather than left at a bare `deleted`, because
/// that is why the tile above moved.
fn departed(path: &str, was: &FileRow) -> Option<Mover> {
    if was.smells == 0 && was.max_cyclomatic < NEW_FILE_FLOOR {
        return None;
    }
    let smells = match was.smells {
        0 => String::new(),
        n => format!("  -{n} smell"),
    };
    Some(Mover {
        path: path.to_string(),
        worse: false,
        what: format!("deleted  was cplx {}{}", was.max_cyclomatic, smells),
        weight: was.max_cyclomatic + was.smells * 10,
    })
}

/// Folders whose shape tier changed, appeared, or left.
///
/// A tier is the whole signal here — the underlying compliance score moves
/// on every edit and would fill the list with rows nobody can act on, which
/// is the complaint ADR 0029 settles for `reshape`.
fn folders(base: &Scopes, now: &Scopes) -> Vec<Mover> {
    let present = now
        .folders
        .iter()
        .filter_map(|(path, pattern)| match base.folders.get(path) {
            Some(was) if was == pattern => None,
            Some(was) => Some(reshaped(path, *was, *pattern)),
            None => Some(appeared(path, *pattern)),
        });
    let gone = base
        .folders
        .iter()
        .filter(|(path, _)| !now.folders.contains_key(*path))
        .map(|(path, was)| vanished(path, *was));
    present.chain(gone).collect()
}

/// A folder that was there at the baseline and reads differently now.
fn reshaped(path: &str, was: ShapePattern, now: ShapePattern) -> Mover {
    Mover {
        path: path.to_string(),
        // The ladder is ordered worst-first, so a fall is a decrease.
        worse: now < was,
        what: format!("shape {}→{}", was.label(), now.label()),
        weight: rungs(was, now) * 10,
    }
}

/// A folder the baseline never saw.
///
/// No floor, unlike [`arrived`]: there are orders of magnitude fewer folders
/// than files, so the noise argument the floor answers does not arise, and the
/// tier alone is actionable either way. A new folder below `hierarchical` is a
/// regression the `unreadable` tile will show; a new one at or above it is a
/// module somebody extracted cleanly, and worth seeing for the reason this
/// module lists improvements at all.
fn appeared(path: &str, now: ShapePattern) -> Mover {
    Mover {
        path: path.to_string(),
        worse: now < ShapePattern::Hierarchical,
        what: format!("new  {}", now.label()),
        weight: rungs(ShapePattern::Hierarchical, now) * 10,
    }
}

/// A folder the tree no longer has.
///
/// Never a regression, and that is a statement about what is on screen rather
/// than a judgement: the only shape figure the dashboard draws is `unreadable`
/// — folders below `hierarchical` — and a folder leaving can lower it or leave
/// it alone. It cannot raise it.
fn vanished(path: &str, was: ShapePattern) -> Mover {
    Mover {
        path: path.to_string(),
        worse: false,
        what: format!("deleted  was {}", was.label()),
        weight: rungs(ShapePattern::Hierarchical, was) * 10,
    }
}

/// How many rungs of the shape ladder a folder moved.
fn rungs(was: ShapePattern, now: ShapePattern) -> u32 {
    let rung = |p: ShapePattern| -> u32 {
        match p {
            ShapePattern::Cyclic => 0,
            ShapePattern::Tangled => 1,
            ShapePattern::Hierarchical => 2,
            ShapePattern::Fractal => 3,
        }
    };
    rung(was).abs_diff(rung(now))
}
