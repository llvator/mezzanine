//! `mezz monitor`: what the tree's quality is doing, while it is being done
//! to.
//!
//! `mezz quality` and `mezz check` answer *now*, once, into a terminal that
//! scrolls away. The browser UI answers *now* in more depth than a glance
//! affords. Neither answers the question a swarm of agents raises — **is this
//! getting better or worse, and since when** — because neither has a time
//! axis. This does, and holds far fewer numbers in exchange (MON-001).
//!
//! ## Shape
//!
//! Two threads and one direction of travel:
//!
//! ```text
//!   watch.rs  ──── Sample ────▶  mod.rs ──▶ series.rs ──▶ draw.rs
//!   (watch, analyze)   mpsc        (keys)     (ring)        (screen)
//!      │                                        ▲
//!      └── baseline.rs ── the zero ─────────────┘
//!          (checkout, analyze)
//!      ▲
//!   compare.rs — the same two readings, both of them checkouts, when
//!   `--against` pins the head side to a commit as well (MON-008). It replaces
//!   `watch.rs` for the session; everything downstream of the channel is the
//!   same code drawing a screen that cannot change.
//! ```
//!
//! The measuring thread never draws and the drawing thread never analyzes,
//! which is what keeps the dashboard responsive to `q` through a cold
//! analysis of a large tree. `B` travels the other way as a flag, never as
//! work: the key sets it, and the measuring thread is what acts on it.
//!
//! ## The screen is ours
//!
//! While the dashboard is up it owns the alternate screen, so anything the
//! analyzer would have written to stderr — the parse progress bar, every
//! phase line, a warning about an unmatched exclude pattern — would be
//! written straight over the drawing. A [`crate::activity::Sink::quiet`]
//! is what makes that not happen: the same notices are recorded and shown in
//! the header, where the dashboard decided to put them, rather than wherever
//! the cursor happened to be.

mod baseline;
mod compare;
mod draw;
mod movers;
mod series;
mod sample;
#[cfg(test)]
mod tests;
mod watch;

use std::io::Stdout;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ListState;
use ratatui::Terminal;

use crate::activity;
use crate::settings::Settings;
use sample::{Commit, Sample};
use series::{Anchor, Series};
use watch::Reading;

/// How long the loop waits for a keystroke before redrawing. Fast enough
/// that the elapsed clock in the footer does not visibly stall, slow enough
/// to be invisible in a `top` beside the swarm it is watching.
const TICK: Duration = Duration::from_millis(250);

/// Movers shown at once. The list scrolls; this is what is computed.
const MOVERS: usize = 200;

/// What the command was asked for.
pub struct MonitorOptions {
    pub path: PathBuf,
    pub include_tests: bool,
    pub include_docs: bool,
    pub languages: Option<Vec<String>>,
    pub spec_dir: Option<PathBuf>,
    /// How long the tree must be quiet before it is measured.
    pub debounce_ms: u64,
    /// Never measure more often than this, however fast the edits arrive.
    pub min_interval_ms: u64,
    /// Readings kept for the sparklines.
    pub history: usize,
    /// What the deltas are measured from: a git ref, the literal `working`
    /// for the tree as found, or unset for `HEAD` (MON-007).
    pub baseline: Option<String>,
    /// A git ref to pin the *head* side to, which makes the session a static
    /// comparison of two commits rather than a watch (MON-008).
    pub against: Option<String>,
    /// Append one JSON line per reading here.
    pub log: Option<PathBuf>,
    pub settings: Settings,
}

/// The flags the drawing thread sets and the measuring thread obeys.
///
/// Together because they are one conversation between the same two threads,
/// and because passing bare `Arc<AtomicBool>`s around is how one eventually
/// gets handed over in another's place.
#[derive(Clone)]
struct Flags {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    /// `B`: re-measure the baseline at whatever HEAD is now.
    rebase: Arc<AtomicBool>,
}

/// Run the dashboard until the operator quits.
pub fn run(opts: MonitorOptions) -> Result<()> {
    let root = opts.path.canonicalize().unwrap_or_else(|_| opts.path.clone());
    let zero = zero(&root, opts.baseline.clone(), opts.against.clone())?;
    // Kept for the header, which has to say what it is drawing before either
    // checkout has finished being measured.
    let against = zero.ends.against().cloned();

    // Installed before the first analysis and before the screen is taken, so
    // there is no window in which a phase line could reach the terminal.
    let sink = activity::Sink::quiet();
    activity::install(sink.clone());

    let flags = Flags {
        stop: Arc::new(AtomicBool::new(false)),
        paused: Arc::new(AtomicBool::new(false)),
        rebase: Arc::new(AtomicBool::new(false)),
    };
    let (tx, rx) = std::sync::mpsc::channel::<Reading>();
    let measuring = measuring(&opts, &root, zero, tx, &flags);

    let outcome = {
        let mut terminal = enter()?;
        Dashboard {
            repo: repo_name(&root),
            series: None,
            movers: Vec::new(),
            movers_state: ListState::default(),
            history: opts.history.max(2),
            sink,
            paused: flags.paused.clone(),
            rebase: flags.rebase.clone(),
            pending_base: None,
            failed: 0,
            against,
        }
        .drive(&mut terminal, &rx, &flags.stop)
    };

    flags.stop.store(true, Ordering::Relaxed);
    // The measuring thread's error is reported *here*, with the terminal
    // restored, because that is the only place the operator can read it. A
    // dashboard that exits instantly having written its reason onto a screen
    // it then tore down has not reported anything. The dashboard's own error
    // wins when there is one: it is the nearer cause.
    let watching = measuring.join();
    outcome.and(match watching {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("the measuring thread panicked")),
    })
}

/// What the deltas are measured from, and the repository that answered for
/// it.
///
/// The two travel together because the second is what resolved the first, and
/// because `repo_root` is not the analyzed path — a monitor pointed at a
/// subdirectory checks out the whole repository and compares one subtree of it
/// (SRV-021). Handing those two paths over separately is how they get handed
/// over the wrong way round.
struct Zero {
    repo_root: PathBuf,
    /// What the session measures: the tree against a commit, or one commit
    /// against another.
    ends: baseline::Ends,
}

/// Resolve the zero, while the terminal is still the operator's.
///
/// Before the screen and before the sink, because a ref they typed and git
/// cannot resolve has to be reported as an ordinary error — a message written
/// after [`enter`] lands on a drawing that is about to be torn down, and one
/// written after the quiet sink is installed is not written at all. The two
/// flags that cannot be held together are refused here for the same reason.
fn zero(root: &std::path::Path, flag: Option<String>, against: Option<String>) -> Result<Zero> {
    let repo_root = crate::settings::repo_root(root);
    let ends = baseline::ends(flag, against, &repo_root)?;
    Ok(Zero { repo_root, ends })
}

/// Start the thread that takes the readings: a watcher, or the two checkouts
/// of a comparison.
///
/// The choice is made once, here, out of what the flags resolved to — the
/// dashboard downstream of the channel does not know which thread is filling
/// it, and both feed it the same two kinds of reading.
fn measuring(
    opts: &MonitorOptions,
    root: &std::path::Path,
    zero: Zero,
    tx: std::sync::mpsc::Sender<Reading>,
    flags: &Flags,
) -> std::thread::JoinHandle<Result<()>> {
    let Zero { repo_root, ends } = zero;
    match ends {
        baseline::Ends::Watching(plan) => {
            let m = measurer(opts, root, repo_root, plan, tx, flags);
            std::thread::spawn(move || watch::run(m))
        }
        baseline::Ends::Between { from, to } => {
            let m = measurer(opts, root, repo_root, None, tx, flags);
            std::thread::spawn(move || compare::run(m, from, to))
        }
    }
}

/// Everything the measuring thread needs, built from the flags the command
/// was given.
fn measurer(
    opts: &MonitorOptions,
    root: &std::path::Path,
    repo_root: PathBuf,
    baseline: Option<baseline::Plan>,
    tx: std::sync::mpsc::Sender<Reading>,
    flags: &Flags,
) -> watch::Measure {
    watch::Measure {
        config: crate::server::state::build_config(
            root,
            opts.include_tests,
            opts.include_docs,
            &opts.languages,
            opts.spec_dir.clone(),
            &opts.settings,
        ),
        root: root.to_path_buf(),
        repo_root,
        debounce: Duration::from_millis(opts.debounce_ms),
        min_interval: Duration::from_millis(opts.min_interval_ms),
        tx,
        stop: flags.stop.clone(),
        paused: flags.paused.clone(),
        rebase: flags.rebase.clone(),
        baseline,
        log: opts.log.clone(),
    }
}

/// Everything the loop mutates.
struct Dashboard {
    repo: String,
    series: Option<Series>,
    movers: Vec<movers::Mover>,
    movers_state: ListState,
    history: usize,
    sink: Arc<activity::Sink>,
    paused: Arc<AtomicBool>,
    /// Raised by `B`, lowered by the measuring thread when it acts on it.
    rebase: Arc<AtomicBool>,
    /// A commit's reading that arrived before the tree's, which is the
    /// opening case: the baseline is measured first. Held rather than shown,
    /// so the tiles never carry numbers about a tree nobody is editing.
    pending_base: Option<Sample>,
    /// Attempts that failed since the last good reading. Non-zero means every
    /// figure on screen is older than the tree, which is the one thing a live
    /// dashboard must not keep to itself (MON-006).
    failed: u32,
    /// The commit the head side is pinned to (`--against`), which is also what
    /// makes this session a comparison: `Some` and the screen is a still of two
    /// states, `None` and it is a watch (MON-008).
    against: Option<Commit>,
}

impl Dashboard {
    /// Read keys, take readings, draw — until told to stop, or until the
    /// measuring thread goes away.
    fn drive(
        mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        rx: &Receiver<Reading>,
        stop: &Arc<AtomicBool>,
    ) -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            match self.take(rx) {
                Ok(()) => {}
                // The measuring thread is gone and no further reading will
                // ever arrive; staying up would be a frozen dashboard
                // claiming to be live.
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            self.paint(terminal)?;

            if self.keys()? {
                break;
            }
        }
        Ok(())
    }

    /// Gather what the screen shows this frame, and hand it to [`draw`].
    ///
    /// Its own method because assembling the view is six field names plus the
    /// two it reads off other threads, and holding those in view *inside* the
    /// loop put `drive` over the working-set bar — the Overfull Head this
    /// dashboard reports on other people's code. `drive` is a loop; this is
    /// one frame of it.
    fn paint(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        let running = self.sink.snapshot(0).running.map(|r| r.message);
        let view = draw::View {
            repo: &self.repo,
            series: self.series.as_ref(),
            movers: &self.movers,
            running: running.as_deref(),
            paused: self.paused.load(Ordering::Relaxed),
            failed: self.failed,
            against: self.against.as_ref(),
        };
        terminal.draw(|frame| draw::dashboard(frame, &view, &mut self.movers_state))?;
        Ok(())
    }

    /// Fold in every reading that has arrived, then recompute the movers
    /// once — a burst of three readings is three sets of numbers but one
    /// answer to "what moved since the baseline".
    fn take(&mut self, rx: &Receiver<Reading>) -> std::result::Result<(), TryRecvError> {
        let mut arrived = false;
        loop {
            match rx.try_recv() {
                Ok(reading) => arrived |= self.record(reading),
                Err(TryRecvError::Empty) => break,
                Err(e) => return Err(e),
            }
        }
        if arrived {
            self.recompute();
        }
        Ok(())
    }

    /// Fold one reading in, opening the series if this is the first. `true`
    /// when the movers need recomputing — a failure moved nothing, it only
    /// means what is already on screen is older than the tree.
    fn record(&mut self, reading: Reading) -> bool {
        let sample = match reading {
            Reading::Failed => {
                self.failed = self.failed.saturating_add(1);
                return false;
            }
            Reading::Base(sample) => return self.adopt(*sample),
            Reading::Took(sample) => *sample,
        };
        self.failed = 0;
        match &mut self.series {
            Some(series) => series.push(sample),
            None => self.series = Some(self.opened(sample)),
        }
        true
    }

    /// Open the series, on the commit baseline if one is waiting.
    fn opened(&mut self, first: Sample) -> Series {
        let Some(base) = self.pending_base.take() else {
            return Series::new(first, self.history);
        };
        let anchor = Anchor::of(base.head.as_ref());
        Series::anchored(base, anchor, first, self.history)
    }

    /// Take a commit's reading as the new zero. Held aside when it arrives
    /// before the tree has been read at all, which is what the opening
    /// baseline does by construction.
    fn adopt(&mut self, base: Sample) -> bool {
        let Some(series) = &mut self.series else {
            self.pending_base = Some(base);
            return false;
        };
        let anchor = Anchor::of(base.head.as_ref());
        series.rebase(base, anchor);
        true
    }

    fn recompute(&mut self) {
        let Some(series) = &self.series else {
            return;
        };
        self.movers = movers::between(
            &series.baseline().scopes,
            &series.latest().scopes,
            MOVERS,
        );
        // A list that shrank under the cursor would otherwise leave the
        // selection pointing past its end, which ratatui renders as nothing
        // selected at all.
        if let Some(i) = self.movers_state.selected() {
            if i >= self.movers.len() {
                self.movers_state.select(self.movers.len().checked_sub(1));
            }
        }
    }

    /// Handle whatever the operator pressed within one tick. `true` to quit.
    fn keys(&mut self) -> Result<bool> {
        if !event::poll(TICK)? {
            return Ok(false);
        }
        let Event::Key(key) = event::read()? else {
            return Ok(false);
        };
        // Windows reports press *and* release; without this every key acts
        // twice there.
        if key.kind != KeyEventKind::Press {
            return Ok(false);
        }
        Ok(self.press(key))
    }

    /// Whether the session is watching a tree. The three keys that move or
    /// suspend the measuring are guarded on it: in a comparison there is
    /// nothing to re-baseline to, no HEAD the screen is about, and no next
    /// reading to pause — and a key that silently does nothing is worse than
    /// one the footer never offered (MON-008).
    fn live(&self) -> bool {
        self.against.is_none()
    }

    fn press(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('b') if self.live() => {
                if let Some(series) = &mut self.series {
                    series.rebaseline();
                }
                self.recompute();
            }
            // Answered by the measuring thread, an analysis later — there is
            // a commit to check out and walk, and this thread must not be the
            // one doing it.
            KeyCode::Char('B') if self.live() => self.rebase.store(true, Ordering::Relaxed),
            KeyCode::Char('p') if self.live() => {
                let was = self.paused.load(Ordering::Relaxed);
                self.paused.store(!was, Ordering::Relaxed);
            }
            KeyCode::Up => self.scroll(-1),
            KeyCode::Down => self.scroll(1),
            _ => {}
        }
        false
    }

    fn scroll(&mut self, by: isize) {
        if self.movers.is_empty() {
            return;
        }
        let last = self.movers.len() - 1;
        let next = match self.movers_state.selected() {
            None => 0,
            Some(i) => i.saturating_add_signed(by).min(last),
        };
        self.movers_state.select(Some(next));
    }
}

/// Take the screen, and arrange to give it back.
///
/// The panic hook is chained rather than replaced: a panic inside the loop
/// has to restore the terminal *before* the default hook prints the message,
/// or the backtrace lands on the alternate screen and disappears with it.
fn enter() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    enable_raw_mode().context("monitor needs a terminal (raw mode)")?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;
    terminal.clear()?;
    Ok(terminal)
}

/// Give the screen back. Every step is best-effort: this runs on the way out
/// of a panic as well as a clean exit, and a failure here must not mask
/// whatever is already going wrong.
fn restore() {
    let _ = disable_raw_mode();
    let _ = std::io::stdout().execute(LeaveAlternateScreen);
}

impl Drop for Dashboard {
    fn drop(&mut self) {
        restore();
    }
}

/// What to call the tree on the title bar: its directory name, or the path
/// itself when that is empty (a root, or a relative `.` that canonicalised
/// to nothing useful).
fn repo_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string())
}
