//! The measuring thread's other job: two commits, measured once each
//! (MON-008).
//!
//! `--against <REF>` pins the head side of the comparison to a commit, and a
//! pinned head is a head that cannot move. So there is nothing to watch, no
//! floor to keep and no time axis to draw — what is left is the two things the
//! dashboard was already the right shape for: a grid of figures with a delta on
//! each, and the list of what moved between the two states. Both are answered
//! by the same [`super::baseline::measure`] the live session opens with; the
//! only difference is that here it is called twice and nothing follows.
//!
//! ## Failing is not skipping
//!
//! A watching session that cannot read the tree skips the tick and says the
//! figures are stale, because another tick is coming. Nothing is coming here,
//! so a checkout that cannot be measured ends the session with the reason —
//! reported by [`super::run`] once the terminal is the operator's again, which
//! is the only place they could read it.

use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result};

use super::baseline::{self, Plan, Role, Site};
use super::watch::{Measure, Reading};

/// How long the parked thread sleeps between looks at the stop flag. The same
/// 200ms the watcher polls on, and for the same reason: it is the cost of
/// quitting, not of measuring.
const PARK_MS: u64 = 200;

/// Measure both ends, send them, and hold the channel open until the operator
/// quits.
///
/// The zero goes first, exactly as it does when a live session opens: the
/// dashboard holds a baseline that arrives before any reading, and draws
/// nothing until the head side lands beside it. That ordering is what keeps
/// the tiles from ever carrying a commit's numbers under a delta of `=`.
pub fn run(m: Measure, from: Plan, to: Plan) -> Result<()> {
    for (plan, role) in [(from, Role::Zero), (to, Role::Head)] {
        let Some(reading) = taken(&m, &plan, role)? else {
            return Ok(());
        };
        if m.tx.send(reading).is_err() {
            return Ok(());
        }
    }
    park(&m);
    Ok(())
}

/// One end of the comparison, logged like any other reading and wrapped as
/// whichever kind of reading its role makes it.
///
/// The role decides the variant here rather than at the two call sites, which
/// is the same reason it exists at all: "the zero" and "the state measured
/// against it" is one distinction, and a second place to spell it is a second
/// place to spell it the wrong way round.
///
/// `None` is the operator quitting mid-analysis, which the analyzer reports as
/// a cancelled walk and which must not be worded as a failure — they are
/// already looking at their shell prompt.
fn taken(m: &Measure, plan: &Plan, role: Role) -> Result<Option<Reading>> {
    let site = Site {
        config: &m.config,
        root: &m.root,
        repo_root: &m.repo_root,
        stop: &m.stop,
    };
    match baseline::measure(&site, plan, super::watch::now_ms(), role) {
        Ok(sample) => {
            super::watch::logged(m, &sample);
            Ok(Some(match role {
                Role::Zero => Reading::Base(Box::new(sample)),
                Role::Head => Reading::Took(Box::new(sample)),
            }))
        }
        Err(_) if m.stop.load(Ordering::Relaxed) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot measure {}", plan.reference)),
    }
}

/// Do nothing until the operator quits.
///
/// Returning instead would drop `tx`, and the dashboard reads a disconnected
/// channel as a measuring thread that died — correctly, for a session that
/// promised more readings. Here it would tear the screen down at the very
/// moment the comparison finished being drawn on it.
fn park(m: &Measure) {
    while !m.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(PARK_MS));
    }
}
