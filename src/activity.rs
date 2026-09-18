//! What the engine is doing right now, as something other than stderr (UI-138).
//!
//! Every phase line the analyzer and the diff pipeline print — `Analyzing
//! base (784646003a) …`, `Computing structural diff…`, `Failed to parse …` —
//! used to exist only on the terminal that launched `mezz watch`. A browser
//! pointed at that same engine saw nothing for the seventy seconds a diff on
//! a large repository takes, because the SSE stream only carried *completion*
//! pings (`reload`, `diff`, `head`) and had no way to say "still working".
//!
//! This module is the second destination. A [`Sink`] holds a bounded ring of
//! recent [`Notice`]s plus one [`Running`] slot — "a run is in flight, and
//! this is its latest line" — and broadcasts each notice as it happens. The
//! server subscribes for its SSE stream and reads the ring for
//! `GET /api/activity`.
//!
//! **stderr is unchanged.** The helpers here print exactly the string they
//! record, indentation and emoji included, so the terminal output of every
//! command is byte-for-byte what it was. Recording is the *addition*; a
//! process with no sink installed (`mezz map`, `mezz quality`, every CLI
//! subcommand) just prints, as before.
//!
//! ## Why the sink is a process global
//!
//! The loudest messages come from inside `Analyzer::parse_files`, several
//! frames below anything that has heard of a server and inside a rayon
//! parallel iterator. Reaching it explicitly means a handle on `Config`,
//! which is `Serialize + Deserialize` and cannot carry a trait object without
//! a `#[serde(skip)]` field that then lies about what a round-tripped config
//! is. The global is not papering over an ambiguity: one process watches one
//! root, so there is never a second sink a message could have meant.
//!
//! Every type here is still usable without the global — [`Sink`] has the same
//! methods the free functions call — which is what the tests at the bottom
//! drive.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

/// Phase name for a code analysis: the walk, the parse, the resolve.
pub const ANALYSIS: &str = "analysis";
/// Phase name for the structural-diff pipeline, worktrees included.
pub const DIFF: &str = "diff";

/// How many notices the ring keeps. This is a status feed, not a log file:
/// enough to fill a panel for someone who just opened the page, and small
/// enough that nobody has to think about it growing.
const RECENT_CAPACITY: usize = 200;

/// Warnings recorded per run before the sink stops broadcasting them.
///
/// The reason this exists: a single diff of a large repository emits the same
/// parse failure once per worktree, and a tree with thousands of unreadable
/// files would push thousands of SSE frames describing one fact. Past the cap
/// the warnings are counted instead — and still *printed*, since the terminal
/// has never had a reason to lose them.
const WARNINGS_PER_RUN: usize = 20;

/// Broadcast backlog. A slow client that falls behind gets `Lagged` and skips
/// what it missed, which for a status feed is the correct loss.
const CHANNEL_CAPACITY: usize = 256;

/// What a notice is doing to the run it belongs to.
///
/// The distinction that matters is `Step` versus `Warn`: a step *replaces*
/// the line a reader is watching, a warning accumulates beside it. Collapsing
/// the two would mean a parse failure overwrote "Resolving dependencies…" and
/// then stayed there as the status of a run that had moved on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A run began. Opens the [`Running`] slot.
    Start,
    /// Progress within the open run. Replaces its message.
    Step,
    /// Something was survived rather than done. Leaves the message alone.
    Warn,
    /// The run is over. Closes the slot once the last open run has ended.
    End,
}

/// One thing the engine said, stamped and numbered.
///
/// `seq` is what makes the ring re-readable: a client says how far it got and
/// gets only what came after, so a reconnecting page neither repeats what it
/// already showed nor silently skips what it missed while disconnected.
#[derive(Clone, Debug, Serialize)]
pub struct Notice {
    pub seq: u64,
    /// Unix epoch milliseconds, so the browser can age it without a format
    /// to agree on.
    pub at_ms: u64,
    pub kind: Kind,
    pub phase: &'static str,
    pub message: String,
}

/// The run currently in flight, if one is.
///
/// This — not the notice list — is what answers "is the engine busy". A feed
/// alone cannot: the newest entry of a finished run looks exactly like the
/// newest entry of a running one.
#[derive(Clone, Debug, Serialize)]
pub struct Running {
    pub phase: &'static str,
    /// The latest step, or the opening line until a step replaces it.
    pub message: String,
    pub since_ms: u64,
}

/// One notice, plus the state it left behind.
///
/// The running slot rides along on every frame rather than being recomputed
/// by each client from the notices it happens to have seen. That rule — which
/// notice opens a run, which closes it, what a `Step` does to an already-open
/// one — is subtle enough (see [`Inner::apply`]) that a second implementation
/// in TypeScript would be a second thing to keep correct, and it would drift
/// the first time a client missed a frame.
#[derive(Clone, Debug, Serialize)]
pub struct Update {
    pub notice: Notice,
    pub running: Option<Running>,
}

/// Everything a page needs to render the current state in one response.
#[derive(Debug, Serialize)]
pub struct Snapshot {
    /// The highest sequence number issued, whether or not it is in `notices`.
    /// A client stores this and passes it back as `since`.
    pub seq: u64,
    pub running: Option<Running>,
    pub notices: Vec<Notice>,
}

/// The mutable half, behind one lock so `running` can never disagree with the
/// ring it was derived from.
#[derive(Default)]
struct Inner {
    recent: VecDeque<Notice>,
    running: Option<Running>,
    /// How many runs are open. See [`Inner::apply`].
    depth: usize,
    /// Warnings seen in the current run, for the [`WARNINGS_PER_RUN`] cap.
    warnings: usize,
    /// Warnings the cap dropped, reported once when the run closes.
    suppressed: usize,
}

impl Inner {
    /// Fold a notice into the running slot.
    ///
    /// The slot is depth-counted rather than phase-matched, because runs
    /// overlap: the watcher re-analyzes while a diff is mid-flight, and the
    /// diff's own sides are analyses in their own right. Matching phases
    /// would make the inner analysis's `End` read as "the diff finished" and
    /// send the UI idle for the minute the diff still had to run.
    ///
    /// A `Step` therefore updates whatever is open, whichever phase it names.
    /// That is the behaviour the status line wants: while a diff analyzes its
    /// base, `Resolving dependencies…` *is* what the diff is doing.
    fn apply(&mut self, notice: &Notice) {
        match notice.kind {
            Kind::Start => self.open(notice),
            Kind::Step => {
                if let Some(run) = self.running.as_mut() {
                    run.message = notice.message.clone();
                }
            }
            // A warning says nothing about whether the run is still going.
            Kind::Warn => {}
            Kind::End => {
                self.depth = self.depth.saturating_sub(1);
                if self.depth == 0 {
                    self.running = None;
                }
            }
        }
    }

    /// Open a run, keeping the *outermost* start time: a reader watching
    /// "42s" wants how long the work has taken, not how long its current
    /// sub-step has.
    fn open(&mut self, notice: &Notice) {
        let since_ms = self.running.as_ref().map_or(notice.at_ms, |r| r.since_ms);
        self.depth += 1;
        self.running = Some(Running {
            phase: notice.phase,
            message: notice.message.clone(),
            since_ms,
        });
        self.warnings = 0;
        self.suppressed = 0;
    }

    fn push(&mut self, notice: Notice) {
        if self.recent.len() >= RECENT_CAPACITY {
            self.recent.pop_front();
        }
        self.recent.push_back(notice);
    }
}

/// The ring, the running slot and the broadcast, as one shared thing.
pub struct Sink {
    seq: AtomicU64,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<Update>,
    /// When set, notices are recorded and broadcast but never printed — see
    /// [`Sink::quiet`].
    quiet: bool,
}

impl Sink {
    /// A sink that records *and* prints, which is what a server wants: the
    /// terminal that launched it is still someone's log.
    pub fn new() -> Arc<Self> {
        Self::with_voice(false)
    }

    /// A sink that records without printing.
    ///
    /// For a caller that owns the terminal itself. `mezz monitor` draws a
    /// full-screen dashboard on the alternate screen, and a `Parsing…` line
    /// arriving from inside a rayon iterator would be written straight over
    /// it. The notices are still recorded and broadcast, so the dashboard
    /// shows exactly the phase text the terminal would have shown — it just
    /// draws it where it decided, instead of wherever the cursor happened
    /// to be.
    ///
    /// Silence is opt-in and belongs to the sink rather than to a global
    /// flag, which is what keeps the module header's promise intact: a
    /// process that installs no sink, or installs [`Sink::new`], prints
    /// byte-for-byte what it always did.
    pub fn quiet() -> Arc<Self> {
        Self::with_voice(true)
    }

    fn with_voice(quiet: bool) -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Arc::new(Self {
            seq: AtomicU64::new(0),
            inner: Mutex::new(Inner::default()),
            tx,
            quiet,
        })
    }

    /// Print, unless this sink is the quiet one.
    fn say(&self, message: &str) {
        if !self.quiet {
            say(message);
        }
    }

    /// Live updates, from now on. Subscribers that lag are told so and
    /// resynchronise through [`Sink::snapshot`].
    pub fn subscribe(&self) -> broadcast::Receiver<Update> {
        self.tx.subscribe()
    }

    /// The current state, plus every retained notice newer than `since`.
    ///
    /// Pass `0` for "everything you still have".
    pub fn snapshot(&self, since: u64) -> Snapshot {
        let inner = self.lock();
        Snapshot {
            seq: self.seq.load(Ordering::Relaxed),
            running: inner.running.clone(),
            notices: inner
                .recent
                .iter()
                .filter(|n| n.seq > since)
                .cloned()
                .collect(),
        }
    }

    /// Open a run. The returned guard closes it even on an early return —
    /// see [`Run`].
    pub fn begin(self: &Arc<Self>, phase: &'static str, message: impl Into<String>) -> Run {
        let message = message.into();
        self.say(&message);
        self.record(Kind::Start, phase, message);
        Run {
            sink: Some(self.clone()),
            phase,
            ended: false,
        }
    }

    /// Report progress inside whatever run is open on `phase`.
    pub fn step(&self, phase: &'static str, message: impl Into<String>) {
        let message = message.into();
        self.say(&message);
        self.record(Kind::Step, phase, message);
    }

    /// Record a step that something else has already put on the terminal.
    ///
    /// For lines an `indicatif` bar owns: it renders `Discovered 8583 source
    /// files` in place, over its own spinner, and printing the same text
    /// again would leave the terminal saying it twice.
    pub fn mirror(&self, phase: &'static str, message: impl Into<String>) {
        self.record(Kind::Step, phase, message.into());
    }

    /// Report something survived. Always printed; recorded until the run has
    /// spent its [`WARNINGS_PER_RUN`] budget.
    pub fn warn(&self, phase: &'static str, message: impl Into<String>) {
        let message = message.into();
        self.say(&message);
        if self.take_warning_budget() {
            self.record(Kind::Warn, phase, message);
        }
    }

    /// Number, stamp, store and broadcast. The only path that mints a `seq`.
    fn record(&self, kind: Kind, phase: &'static str, message: String) {
        let notice = Notice {
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            at_ms: now_ms(),
            kind,
            phase,
            message,
        };
        let running = {
            let mut inner = self.lock();
            inner.apply(&notice);
            inner.push(notice.clone());
            inner.running.clone()
        };
        // No subscribers is the normal case — a CLI run, or a server nobody
        // has a page open against.
        let _ = self.tx.send(Update { notice, running });
    }

    /// Charge one warning against the run's budget. `false` once it is spent.
    fn take_warning_budget(&self) -> bool {
        let mut inner = self.lock();
        inner.warnings += 1;
        if inner.warnings <= WARNINGS_PER_RUN {
            return true;
        }
        inner.suppressed += 1;
        false
    }

    /// How many warnings the cap dropped, and reset the count.
    fn take_suppressed(&self) -> usize {
        std::mem::take(&mut self.lock().suppressed)
    }

    /// A poisoned status feed must not take the server down with it. Nothing
    /// here has an invariant a panicking writer could have broken halfway —
    /// the worst case is one notice that never made it into the ring.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A unit of work the UI shows as in flight, closed by ownership.
///
/// The pipeline this wraps returns early from a dozen places — a bad ref, a
/// worktree that would not check out, a poisoned lock — and every one of them
/// would otherwise leave the browser reporting a diff that stopped a minute
/// ago as still running. `Drop` is what makes "the engine is busy" a claim
/// that expires on its own.
pub struct Run {
    /// `None` when no sink is installed: the guard still prints, so a CLI run
    /// behaves exactly as it did.
    sink: Option<Arc<Sink>>,
    phase: &'static str,
    ended: bool,
}

impl Run {
    /// Progress within this run.
    pub fn step(&self, message: impl Into<String>) {
        match &self.sink {
            Some(sink) => sink.step(self.phase, message),
            None => say(&message.into()),
        }
    }

    /// Close the run with the line the engine was going to print anyway —
    /// `✅ Diff complete in 71.9s total` and friends.
    ///
    /// Takes `self`, so a run cannot be ended twice and cannot be used after.
    pub fn end(mut self, message: impl Into<String>) {
        let message = message.into();
        match &self.sink {
            Some(sink) => sink.say(&message),
            None => say(&message),
        }
        if let Some(sink) = &self.sink {
            let dropped = sink.take_suppressed();
            if dropped > 0 {
                // Recorded, deliberately not printed: the terminal already
                // has all of them, and this line exists to tell a reader who
                // only has the panel that the list they can see is partial.
                sink.record(
                    Kind::Warn,
                    self.phase,
                    format!("… and {dropped} more warning(s), see the engine's terminal"),
                );
            }
            sink.record(Kind::End, self.phase, message);
        }
        self.ended = true;
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        // Recorded without printing: whatever cut the run short has already
        // reported itself on stderr, and echoing a second line would make
        // failures read differently on the terminal than they did before.
        if let Some(sink) = &self.sink {
            sink.record(Kind::End, self.phase, "interrupted".to_string());
        }
    }
}

static SINK: OnceLock<Arc<Sink>> = OnceLock::new();

/// Make `sink` the sink the free functions below write to.
///
/// First call wins and later ones are ignored rather than refused: the only
/// way to reach a second call is a second server in one process, which the
/// tests do, and neither of them has a reason to fail because of it.
pub fn install(sink: Arc<Sink>) {
    let _ = SINK.set(sink);
}

/// The installed sink, or `None` in any process that never started a server.
pub fn sink() -> Option<&'static Arc<Sink>> {
    SINK.get()
}

/// Whether something that draws on the terminal itself may do so.
///
/// The phase lines route through [`Sink`] and a quiet one silences them, but
/// an `indicatif` bar writes to stderr on its own schedule and has never heard
/// of this module. It asks here instead, and gets `false` exactly when a
/// [`Sink::quiet`] is installed — the one case where another part of the
/// process is holding the screen.
pub fn drawing_allowed() -> bool {
    sink().is_none_or(|s| !s.quiet)
}

/// Where an `indicatif` bar should draw itself: stderr as always, or nowhere
/// when [`drawing_allowed`] says the screen is taken.
///
/// Here rather than at each bar, so the two of them cannot end up disagreeing
/// about who owns the terminal.
pub fn progress_target() -> indicatif::ProgressDrawTarget {
    match drawing_allowed() {
        true => indicatif::ProgressDrawTarget::stderr(),
        false => indicatif::ProgressDrawTarget::hidden(),
    }
}

/// Open a run against the installed sink. See [`Sink::begin`].
pub fn begin(phase: &'static str, message: impl Into<String>) -> Run {
    match sink() {
        Some(sink) => sink.begin(phase, message),
        None => {
            say(&message.into());
            Run {
                sink: None,
                phase,
                ended: false,
            }
        }
    }
}

/// Progress on the installed sink. See [`Sink::step`].
pub fn step(phase: &'static str, message: impl Into<String>) {
    match sink() {
        Some(sink) => sink.step(phase, message),
        None => say(&message.into()),
    }
}

/// Record on the installed sink without printing. See [`Sink::mirror`].
pub fn mirror(phase: &'static str, message: impl Into<String>) {
    if let Some(sink) = sink() {
        sink.mirror(phase, message);
    }
}

/// A survived failure on the installed sink. See [`Sink::warn`].
pub fn warn(phase: &'static str, message: impl Into<String>) {
    match sink() {
        Some(sink) => sink.warn(phase, message),
        None => say(&message.into()),
    }
}

/// The one place anything is printed, so "recorded" and "printed" cannot
/// drift into two different strings.
fn say(message: &str) {
    eprintln!("{message}");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(snap: &Snapshot) -> Vec<&str> {
        snap.notices.iter().map(|n| n.message.as_str()).collect()
    }

    #[test]
    fn a_run_opens_and_closes_the_running_slot() {
        let sink = Sink::new();
        assert!(sink.snapshot(0).running.is_none(), "idle to start with");

        let run = sink.begin(DIFF, "starting");
        assert_eq!(sink.snapshot(0).running.unwrap().message, "starting");

        run.step("computing");
        let running = sink.snapshot(0).running.expect("still running");
        assert_eq!(running.message, "computing", "a step replaces the line");
        assert_eq!(running.phase, DIFF);

        run.end("done");
        assert!(sink.snapshot(0).running.is_none(), "idle again");
    }

    /// The failure this guard exists for: a pipeline that returns early must
    /// not leave the UI claiming the engine is busy forever.
    #[test]
    fn dropping_a_run_closes_it() {
        let sink = Sink::new();
        {
            let _run = sink.begin(DIFF, "starting");
            assert!(sink.snapshot(0).running.is_some());
        }
        assert!(sink.snapshot(0).running.is_none(), "dropped, so closed");
        let last = sink.snapshot(0).notices.pop().expect("an end notice");
        assert_eq!(last.kind, Kind::End);
    }

    /// The watcher re-analyzes while a diff is in flight. The analysis
    /// finishing must not report the diff as finished with it.
    #[test]
    fn an_overlapping_run_does_not_close_the_one_it_overlaps() {
        let sink = Sink::new();
        let diff = sink.begin(DIFF, "diffing");
        let analysis = sink.begin(ANALYSIS, "re-analyzing");
        analysis.end("re-analyzed");
        assert!(
            sink.snapshot(0).running.is_some(),
            "the diff is still running"
        );
        diff.end("diff done");
        assert!(sink.snapshot(0).running.is_none(), "and now nothing is");
    }

    /// A diff spends most of its time inside analyses of its two sides, and
    /// those steps are what the reader should see — the whole point of the
    /// ticket is that a diff looks frozen without them.
    #[test]
    fn a_step_updates_the_open_run_whatever_phase_it_names() {
        let sink = Sink::new();
        let diff = sink.begin(DIFF, "🔄 Starting diff");
        sink.step(ANALYSIS, "  Resolving dependencies...");
        let running = sink.snapshot(0).running.expect("still running");
        assert_eq!(running.message, "  Resolving dependencies...");
        assert_eq!(running.phase, DIFF, "it is still the diff that is running");
        diff.end("✅ Diff complete");
    }

    /// The elapsed time a reader watches belongs to the work, not to whatever
    /// sub-run happens to be open at the moment they look.
    #[test]
    fn a_nested_run_keeps_the_outer_start_time() {
        let sink = Sink::new();
        let outer = sink.begin(DIFF, "diffing");
        let started = sink.snapshot(0).running.unwrap().since_ms;
        let inner = sink.begin(ANALYSIS, "analyzing");
        assert_eq!(sink.snapshot(0).running.unwrap().since_ms, started);
        inner.end("analyzed");
        outer.end("diffed");
    }

    /// A warning is beside the run, not the run's status: it must not
    /// overwrite the step a reader is watching.
    #[test]
    fn a_warning_leaves_the_status_line_alone() {
        let sink = Sink::new();
        let run = sink.begin(ANALYSIS, "parsing");
        sink.warn(ANALYSIS, "Warning: Failed to parse a.impex");
        assert_eq!(sink.snapshot(0).running.unwrap().message, "parsing");
        run.end("parsed");
    }

    #[test]
    fn the_ring_is_bounded() {
        let sink = Sink::new();
        for i in 0..RECENT_CAPACITY + 50 {
            sink.step(ANALYSIS, format!("step {i}"));
        }
        let snap = sink.snapshot(0);
        assert_eq!(snap.notices.len(), RECENT_CAPACITY);
        assert_eq!(snap.notices[0].message, "step 50", "oldest dropped first");
        assert_eq!(snap.seq, (RECENT_CAPACITY + 50) as u64);
    }

    /// A reconnecting page asks for what it has not seen, and gets exactly
    /// that — no repeats, no gap.
    #[test]
    fn since_returns_only_what_came_after() {
        let sink = Sink::new();
        sink.step(ANALYSIS, "one");
        let seen = sink.snapshot(0).seq;
        sink.step(ANALYSIS, "two");
        sink.step(ANALYSIS, "three");
        assert_eq!(messages(&sink.snapshot(seen)), vec!["two", "three"]);
    }

    /// One diff of a big repo repeats the same parse failure once per
    /// worktree. The cap is what keeps that off the wire.
    #[test]
    fn warnings_are_capped_per_run_and_summarised() {
        let sink = Sink::new();
        let run = sink.begin(ANALYSIS, "parsing");
        for i in 0..WARNINGS_PER_RUN + 7 {
            sink.warn(ANALYSIS, format!("bad file {i}"));
        }
        let recorded = sink
            .snapshot(0)
            .notices
            .iter()
            .filter(|n| n.kind == Kind::Warn)
            .count();
        assert_eq!(recorded, WARNINGS_PER_RUN, "the rest were counted, not sent");

        run.end("parsed");
        let summarised = sink
            .snapshot(0)
            .notices
            .iter()
            .any(|n| n.message.contains("7 more warning"));
        assert!(summarised, "the dropped ones are accounted for at the end");
    }

    /// Each run gets its own budget, or a long-lived server would stop
    /// reporting warnings altogether after the first noisy analysis.
    #[test]
    fn a_new_run_refills_the_warning_budget() {
        let sink = Sink::new();
        let first = sink.begin(ANALYSIS, "one");
        for i in 0..WARNINGS_PER_RUN + 3 {
            sink.warn(ANALYSIS, format!("bad {i}"));
        }
        first.end("done");

        let second = sink.begin(ANALYSIS, "two");
        let before = sink.snapshot(0).seq;
        sink.warn(ANALYSIS, "fresh warning");
        second.end("done");
        assert!(
            sink.snapshot(before)
                .notices
                .iter()
                .any(|n| n.message == "fresh warning"),
            "the second run starts with a full budget"
        );
    }

    /// Every frame carries the state it produced, so a client never has to
    /// re-derive `running` from the notices it happens to have received.
    #[test]
    fn a_subscriber_hears_notices_and_the_state_they_left() {
        let sink = Sink::new();
        let mut rx = sink.subscribe();

        let run = sink.begin(DIFF, "starting");
        let opened = rx.try_recv().expect("the start");
        assert_eq!(opened.notice.kind, Kind::Start);
        assert_eq!(opened.running.expect("running").message, "starting");

        run.end("done");
        let closed = rx.try_recv().expect("the end");
        assert_eq!(closed.notice.message, "done");
        assert!(closed.running.is_none(), "and the run is over");
    }
}
