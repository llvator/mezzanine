//! The time axis: a bounded ring of readings, a baseline to measure from,
//! and the sparklines drawn out of both.
//!
//! ## Why a baseline and not just the previous tick
//!
//! A swarm's individual edit is noise — a function is half-written for four
//! seconds and its complexity is briefly absurd. The question the dashboard
//! exists to answer is *since some state I can name*, so every delta on screen
//! is against the baseline, and the previous tick appears only as the last
//! notch of a sparkline.
//!
//! ## What the zero is
//!
//! Two things, and the [`Anchor`] is which. By default a **commit** — `HEAD`
//! when the session opens, measured out of a checkout (MON-007) — so the
//! deltas read *since the last commit* and the work already sitting in the
//! tree is inside them from the first frame. Failing that, a **reading** of
//! the working tree: the session's first, or wherever the operator pressed
//! `b`.
//!
//! `b` re-baselines to now; `B` asks the measuring thread for a fresh reading
//! of HEAD, which arrives later as a [`Series::rebase`].
//!
//! ## What is kept, and what is dropped
//!
//! The ring holds [`Metrics`] alone — `Copy`, a few dozen bytes. The
//! per-scope detail ([`Scopes`]) is kept for exactly two readings, the
//! baseline and the latest, because those are the only two the movers list
//! ever compares. Keeping it for all five hundred would put tens of
//! megabytes behind a question nobody asks of the middle of a series.

use std::collections::VecDeque;

use super::sample::{Commit, Metrics, Sample};

/// Where the zero on every delta came from.
///
/// Kept beside the baseline sample rather than derived from it at draw time:
/// a reading of the working tree also carries the commit in force when it was
/// taken, so `head.is_some()` cannot tell "measured out of a checkout of this
/// commit" from "measured while sitting on it".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// A checkout of a commit. The deltas read "since this commit".
    Commit(Commit),
    /// A reading of the working tree — the session's first, or wherever `b`
    /// was pressed.
    Reading,
}

impl Anchor {
    /// What a baseline reading, taken at `head`, anchors the deltas to.
    ///
    /// The commit rather than the [`Sample`] it was stamped on, because one
    /// field is all this reads — and a function that names the whole record to
    /// reach one of them is a dependent that record did not need. `Sample` is
    /// the dashboard's currency and its fan-in is the thing most likely to
    /// creep past the bar; every borrow of it should be one that wants the
    /// reading.
    ///
    /// A commit reading always carries its commit — the checkout it was taken
    /// from is stamped on it — so the `None` arm is unreachable in practice
    /// and is a `Reading` rather than a panic: a dashboard that dies because
    /// git said nothing has chosen the worse of two answers.
    pub fn of(head: Option<&Commit>) -> Anchor {
        match head {
            Some(commit) => Anchor::Commit(commit.clone()),
            None => Anchor::Reading,
        }
    }
}

/// The session's readings.
pub struct Series {
    /// The notches, oldest first. [`Metrics`] alone: a sparkline is drawn
    /// against position, not against the clock, so a stamp per notch would
    /// be a field nothing reads. The stamps that are read live on the two
    /// [`Sample`]s below.
    ring: VecDeque<Metrics>,
    cap: usize,
    baseline: Sample,
    anchor: Anchor,
    latest: Sample,
    /// Readings that stand against the current baseline, so the header can
    /// say how much evidence the deltas rest on.
    since_baseline: u64,
}

impl Series {
    /// Open a series on its first reading, which is also its first baseline.
    pub fn new(first: Sample, cap: usize) -> Self {
        let mut ring = VecDeque::with_capacity(cap.min(1024));
        ring.push_back(first.metrics);
        Series {
            ring,
            cap: cap.max(2),
            baseline: first.clone(),
            anchor: Anchor::Reading,
            latest: first,
            since_baseline: 0,
        }
    }

    /// Open a series whose zero was measured before it — a commit checkout.
    ///
    /// The baseline is not pushed into the ring. It is not a tick of the
    /// session: no sparkline should carry a notch for a state the tree was
    /// never in while anybody was watching, and the footer's reading count
    /// would claim one measurement more than the operator saw.
    pub fn anchored(baseline: Sample, anchor: Anchor, first: Sample, cap: usize) -> Self {
        let mut series = Series::new(first, cap);
        series.baseline = baseline;
        series.anchor = anchor;
        // The opening reading already stands against this baseline — unlike
        // `new`, where the two are the same sample and every delta is zero.
        series.since_baseline = 1;
        series
    }

    /// Record a reading. The oldest notch falls off the ring at capacity;
    /// the baseline never does, because it is not in the ring.
    pub fn push(&mut self, sample: Sample) {
        if self.ring.len() >= self.cap {
            self.ring.pop_front();
        }
        self.ring.push_back(sample.metrics);
        self.latest = sample;
        self.since_baseline += 1;
    }

    /// Make the current reading the one deltas are measured from (`b`).
    pub fn rebaseline(&mut self) {
        self.baseline = self.latest.clone();
        self.anchor = Anchor::Reading;
        self.since_baseline = 0;
    }

    /// Adopt a separately measured baseline — the answer to `B`.
    ///
    /// One reading stands against it immediately: `latest` is a measurement of
    /// the tree that the new zero does not describe, so the deltas on screen
    /// are real from the moment this lands.
    pub fn rebase(&mut self, baseline: Sample, anchor: Anchor) {
        self.baseline = baseline;
        self.anchor = anchor;
        self.since_baseline = 1;
    }

    pub fn latest(&self) -> &Sample {
        &self.latest
    }

    pub fn baseline(&self) -> &Sample {
        &self.baseline
    }

    pub fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    /// The commit the baseline names, when it names one and the tree has since
    /// moved off it. `None` while the pin is still the commit in force, which
    /// is the case that needs no words on the header.
    pub fn behind_head(&self) -> Option<&Commit> {
        let Anchor::Commit(pinned) = &self.anchor else {
            return None;
        };
        let head = self.latest.head.as_ref()?;
        (head.sha != pinned.sha).then_some(pinned)
    }

    pub fn since_baseline(&self) -> u64 {
        self.since_baseline
    }

    /// How many readings the ring is holding.
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// The last `width` values of one metric, oldest first, scaled for a
    /// block sparkline.
    ///
    /// Scaled against the window's own min and max rather than against zero:
    /// a smell count that sits between 33 and 37 all session is a flat line
    /// against zero and a legible slope against itself, and the number beside
    /// it already says the magnitude.
    pub fn spark(&self, pick: impl Fn(&Metrics) -> f64, width: usize) -> String {
        const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        if width == 0 {
            return String::new();
        }
        let values: Vec<f64> = self
            .ring
            .iter()
            .rev()
            .take(width)
            .rev()
            .map(pick)
            .collect();
        let (lo, hi) = span(&values);
        values
            .iter()
            .map(|v| {
                let share = match hi > lo {
                    true => (v - lo) / (hi - lo),
                    // Every value identical: a flat line at the bottom of the
                    // band, not a full block, so "nothing is moving" does not
                    // look like "everything is maxed".
                    false => 0.0,
                };
                let step = (share * (BLOCKS.len() - 1) as f64).round() as usize;
                BLOCKS[step.min(BLOCKS.len() - 1)]
            })
            .collect()
    }

    /// Latest minus baseline, for one metric.
    pub fn delta(&self, pick: impl Fn(&Metrics) -> f64) -> f64 {
        pick(&self.latest.metrics) - pick(&self.baseline.metrics)
    }
}

/// The smallest and largest of a slice, or `(0, 0)` when it is empty.
fn span(values: &[f64]) -> (f64, f64) {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    match values.is_empty() {
        true => (0.0, 0.0),
        false => (lo, hi),
    }
}
