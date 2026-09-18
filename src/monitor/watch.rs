//! The measuring thread: watch, coalesce, analyze, send.
//!
//! ## Coalesced, never queued
//!
//! A swarm writes continuously, and an analysis takes longer than the gap
//! between two of its bursts. A queue would therefore grow without bound and
//! the dashboard would drift further behind the tree with every tick — the
//! one failure mode that makes a live monitor worse than no monitor, because
//! it still looks live.
//!
//! So there is no queue. Analysis happens on this thread, which means events
//! arriving during a run pile up in the debouncer's channel; when the run
//! ends, [`drain`] takes *all* of them and folds them into one next tick. The
//! dashboard can lag the tree by at most one analysis, and the changed-paths
//! list of a tick names everything that happened since the last one.
//!
//! `min_interval` is the same rule stated as a floor: never measure more
//! often than this, however fast the edits arrive — and it is a floor on the
//! gap *between* runs, stamped when an attempt finishes rather than when it
//! starts. Stamped at the start, an analysis costing more than the floor
//! satisfied the guard the instant it returned, so the loop re-measured back
//! to back with no idle at all: the flag stopped working on exactly the trees
//! whose help text names them (MON-005).
//!
//! ## A failed attempt is an event, not a silence
//!
//! [`measure`] answers `None` for a tree that could not be read — mid-`git
//! checkout` is the case it was written for, and skipping that tick is right.
//! Everything around the skip was not. The paths that woke the attempt go back
//! in the queue rather than being dropped, so the next good reading still
//! names them; the failure is reported into the activity feed; and a
//! [`Reading::Failed`] goes to the dashboard, because the loop only reaches
//! the measuring block when something is pending, and a burst that fails and
//! is then followed by silence — which is what a `git checkout` looks like —
//! would otherwise leave a frozen dashboard under a green `watching` for the
//! rest of the session (MON-006).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, DebouncedEvent};

use crate::analyzer::Analyzer;
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::monitor::sample::{Sample, Stamp};

/// How long the loop waits on the watcher before looking at the stop flag
/// again. The same 200ms `mezz watch` uses, and for the same reason: it is
/// the cost of quitting, not of measuring.
const POLL_MS: u64 = 200;

/// What the measuring thread has to say for itself.
///
/// `Failed` is not a `Sample` with zeros in it — a tree that could not be read
/// has no measurement, and drawing one would be the lie the `None` arm of
/// [`measure`] exists to avoid. It carries no payload because the dashboard
/// only needs to know that the numbers on screen are older than the tree.
///
/// The sample is boxed: it is two orders of magnitude larger than the other
/// variant, and an enum sized for the big one is what travels through the
/// channel on every failure.
pub enum Reading {
    Took(Box<Sample>),
    /// A reading of a *commit*, to be adopted as the baseline: the opening
    /// one, or the answer to `B` (MON-007). Its own variant because it is not
    /// a tick — it does not go in the ring, it does not clear the stale
    /// count, and it does not describe the tree anyone is editing.
    Base(Box<Sample>),
    Failed,
}

/// What the measuring thread needs to do its job.
pub struct Measure {
    pub config: Config,
    pub root: PathBuf,
    pub repo_root: PathBuf,
    pub debounce: Duration,
    pub min_interval: Duration,
    /// Where each reading goes.
    pub tx: Sender<Reading>,
    /// Set when the dashboard is quitting. Polled by the loop *and* handed
    /// to the analyzer, so `q` does not wait out a cold analysis of a large
    /// tree.
    pub stop: Arc<AtomicBool>,
    /// Set while the operator has the dashboard paused. Changes still
    /// accumulate; nothing is measured.
    pub paused: Arc<AtomicBool>,
    /// Set by `B`: re-measure the baseline at whatever HEAD is now. Cleared
    /// here, since this is the thread that can act on it.
    pub rebase: Arc<AtomicBool>,
    /// The commit to open the session's baseline on, or `None` for a session
    /// that measures from its first reading (MON-007).
    ///
    /// Always `None` in a comparison, where neither end is the tree and both
    /// travel to [`super::compare::run`] as arguments (MON-008).
    pub baseline: Option<super::baseline::Plan>,
    /// Optional JSONL sink, one line per reading.
    pub log: Option<PathBuf>,
}

/// Take the first reading, then measure whenever the tree settles, until
/// `stop`.
///
/// Runs on its own thread; the caller owns the terminal and only ever reads
/// the far end of `tx`.
///
/// The error is *returned* rather than printed, and that is the whole reason
/// this returns anything. While this thread starts, the caller is taking the
/// alternate screen; a `cannot watch …` written here would land on a drawing
/// that is about to be torn down, and the operator would be left with a
/// dashboard that quit without saying why. The caller prints it after the
/// terminal is its own again.
pub fn run(m: Measure) -> Result<()> {
    // The dashboard is already gone: nothing to watch on its behalf.
    let Some(mut seq) = opening(&m) else {
        return Ok(());
    };
    let mut last = Instant::now();

    let (notify_tx, notify_rx) = std::sync::mpsc::channel();
    // No watcher means no ticks, and a dashboard frozen on its first reading
    // while claiming to be live is worse than an exit.
    let _debouncer = start_watching(&m, notify_tx).inspect_err(|_| {
        m.stop.store(true, Ordering::Relaxed);
    })?;

    // Changes seen but not yet measured — because a run was in flight, the
    // floor had not elapsed, or the operator had it paused.
    let mut pending: BTreeSet<String> = BTreeSet::new();

    while !m.stop.load(Ordering::Relaxed) {
        match notify_rx.recv_timeout(Duration::from_millis(POLL_MS)) {
            Ok(events) => pending.extend(admitted(events, &m.config)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // Whatever else is already waiting belongs to this same tick.
        pending.extend(drain(&notify_rx, &m.config));

        // Before the floor, and before the pending check: `B` is the operator
        // asking for something, and a quiet tree is exactly when they are most
        // likely to ask. Whatever is pending stays pending for the next lap.
        if m.rebase.swap(false, Ordering::Relaxed) {
            snap_to_head(&m);
            // It cost an analysis, so it counts as one against the floor.
            last = Instant::now();
            continue;
        }

        if !due(
            !pending.is_empty(),
            m.paused.load(Ordering::Relaxed),
            last.elapsed(),
            m.min_interval,
        ) {
            continue;
        }

        let changed: Vec<String> = std::mem::take(&mut pending).into_iter().collect();
        let taken = measure(&m, seq, &changed);
        // Stamped when the attempt *finished*. A failed one spent the time
        // too, and stamping it is what keeps an unreadable tree from spinning
        // (MON-005, and the fourth criterion of MON-006).
        last = Instant::now();
        let Some(sample) = taken else {
            // Cancellation answers `None` as well, and quitting is not
            // failing: no warning, no `Failed`, just go.
            if m.stop.load(Ordering::Relaxed) {
                break;
            }
            // The paths that woke this attempt go back, or the next good
            // reading will not name them and the movers list will attribute
            // their effect to whatever arrived later. `pending` is a set, so
            // anything that landed meanwhile dedupes against them.
            pending.extend(changed);
            crate::activity::warn(
                crate::activity::ANALYSIS,
                "   ⚠ monitor: the tree could not be read; the figures are stale",
            );
            if m.tx.send(Reading::Failed).is_err() {
                break;
            }
            continue;
        };
        seq += 1;
        if m.tx.send(Reading::Took(Box::new(sample))).is_err() {
            break;
        }
    }
    Ok(())
}

/// Open the session: the zero, then the tree as found. Answers the tick
/// number the loop starts from, or `None` when the dashboard has already gone.
///
/// The baseline comes first because it is the thing the first reading is
/// measured *from* — a commit costs a whole analysis, so the dashboard stays
/// empty a little longer than it used to, and in exchange the very first tile
/// it draws already carries the work sitting uncommitted in the tree
/// (MON-007).
fn opening(m: &Measure) -> Option<u64> {
    if let Some(plan) = &m.baseline {
        rebase(m, plan);
    }
    // Unattributed on purpose: nothing changed to cause it, it is the tree as
    // found. Sent as early as it can be, so the dashboard fills.
    let Some(first) = measure(m, 0, &[]) else {
        return Some(0);
    };
    m.tx.send(Reading::Took(Box::new(first))).ok().map(|()| 1)
}

/// Measure a commit and send it over as the new baseline.
///
/// A failure is reported and otherwise dropped: the session keeps whatever
/// zero it already had, which for the opening call means the first reading of
/// the tree — the behaviour before MON-007 — and for `B` means the baseline
/// the operator was already looking at. Neither is worth ending a session
/// over, and neither may be silent, because the header would go on naming a
/// commit the deltas are not measured from.
fn rebase(m: &Measure, plan: &super::baseline::Plan) {
    let site = super::baseline::Site {
        config: &m.config,
        root: &m.root,
        repo_root: &m.repo_root,
        stop: &m.stop,
    };
    match super::baseline::measure(&site, plan, now_ms(), super::baseline::Role::Zero) {
        Ok(sample) => {
            // Logged like any other reading, and marked as the zero: a log
            // whose deltas are measured from a state it does not hold is a
            // file nobody can check the dashboard against.
            logged(m, &sample);
            let _ = m.tx.send(Reading::Base(Box::new(sample)));
        }
        // Cancellation comes through here too, and quitting is not failing.
        Err(_) if m.stop.load(Ordering::Relaxed) => {}
        Err(e) => crate::activity::warn(
            crate::activity::ANALYSIS,
            format!(
                "   ⚠ monitor: could not measure the baseline at {}: {e}",
                plan.reference
            ),
        ),
    }
}

/// The `B` key: re-measure the baseline at whatever HEAD is now.
fn snap_to_head(m: &Measure) {
    let Some(plan) = super::baseline::at_head(&m.repo_root) else {
        crate::activity::warn(
            crate::activity::ANALYSIS,
            "   ⚠ monitor: there is no HEAD here to baseline against",
        );
        return;
    };
    rebase(m, &plan);
}

/// Whether an attempt is due: something to measure, not paused, and the floor
/// elapsed.
///
/// A function rather than two `continue`s inline because the floor is the rule
/// MON-005 got wrong, and a rule stated in one place can be tested without
/// putting a clock inside the loop.
pub fn due(pending: bool, paused: bool, since_last: Duration, min_interval: Duration) -> bool {
    pending && !paused && since_last >= min_interval
}

/// Build the debouncer and point it at the root.
///
/// The two failures read as one sentence to the operator — the tree cannot be
/// watched — so they are worded once here rather than twice at the call site.
/// The returned debouncer must be held: dropping it stops the watch.
fn start_watching(
    m: &Measure,
    notify_tx: Sender<DebounceEventResult>,
) -> Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> {
    let mut debouncer = new_debouncer(m.debounce, notify_tx)
        .with_context(|| format!("cannot watch {}", m.root.display()))?;
    debouncer
        .watcher()
        .watch(&m.root, notify::RecursiveMode::Recursive)
        .with_context(|| format!("cannot watch {}", m.root.display()))?;
    Ok(debouncer)
}

/// One reading, logged if the operator asked for a log.
///
/// `None` when the analysis failed or was cancelled — a tree mid-`git
/// checkout` can be unreadable for a moment, and skipping that tick is the
/// right answer where showing zeros would not be.
fn measure(m: &Measure, seq: u64, changed: &[String]) -> Option<Sample> {
    let started = Instant::now();
    let result = Analyzer::new(m.config.clone())
        .analyze_with_cancel(&m.stop)
        .ok()?;
    if m.stop.load(Ordering::Relaxed) {
        return None;
    }
    let graph = DependencyGraph::from_analysis(&result);
    let sample = Sample::of(
        &graph,
        &m.root,
        &m.repo_root,
        Stamp {
            seq,
            at_ms: now_ms(),
            analysis_ms: started.elapsed().as_millis() as u64,
            changed,
            // Asked per reading rather than once per session: a swarm commits
            // while the dashboard is up, and the pinned baseline can only be
            // reported as behind HEAD by something that keeps asking where
            // HEAD is.
            head: super::baseline::head_of(&m.repo_root),
            baseline: false,
        },
    );
    logged(m, &sample);
    Some(sample)
}

/// Put a reading in the JSONL log, if the operator asked for one.
///
/// A failed append is reported into the activity feed rather than to stderr:
/// the dashboard owns the screen, and a log nobody asked to be essential is
/// not worth ending the session over.
///
/// Shared with [`super::compare`] rather than written twice: a session's log
/// is one file in one format, whichever thread filled it.
pub(super) fn logged(m: &Measure, sample: &Sample) {
    let Some(path) = &m.log else {
        return;
    };
    if let Err(e) = append(path, sample) {
        crate::activity::warn(
            crate::activity::ANALYSIS,
            format!("   ⚠ monitor log {}: {e}", path.display()),
        );
    }
}

/// Append one reading to the JSONL log.
fn append(path: &std::path::Path, sample: &Sample) -> Result<()> {
    use std::io::Write;
    let mut line = serde_json::to_string(sample)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Everything else the watcher has ready, right now.
fn drain(rx: &Receiver<DebounceEventResult>, config: &Config) -> Vec<String> {
    let mut more = Vec::new();
    while let Ok(events) = rx.try_recv() {
        more.extend(admitted(events, config));
    }
    more
}

/// The paths in one batch that an analysis would actually read.
///
/// Delegated to the watch server's filter rather than reimplemented, which is
/// what stops a swarm's `cargo build` from re-measuring the repo every second:
/// the answer comes from the walker and from git, not from a file extension.
fn admitted(events: DebounceEventResult, config: &Config) -> Vec<String> {
    let events: Vec<DebouncedEvent> = match events {
        Ok(events) => events,
        // A watcher error is not a change. Reported into the feed, where the
        // dashboard can show it without stepping on its own drawing.
        Err(e) => {
            crate::activity::warn(crate::activity::ANALYSIS, format!("   ⚠ watch: {e:?}"));
            return Vec::new();
        }
    };
    crate::server::analyzable_changes(&events, config)
}

/// The wall clock, for stamping a reading. Lives on this side because the
/// measuring threads are the only ones that take readings — [`super::baseline`]
/// is handed the result rather than reaching for it, which is what keeps that
/// module out of a loop with this one.
pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
