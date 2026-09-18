//! The commit the deltas are measured from (MON-007).
//!
//! The dashboard's zero used to be *the moment the operator started the
//! process*: the first reading of the working tree. That is a state nobody
//! else can resolve, and it hides everything already in the tree — start
//! monitor on two hours of uncommitted swarm output and every tile reads `=`.
//!
//! So the baseline is a commit, `HEAD` unless the operator says otherwise. It
//! is measured the way both sides of a diff are: a detached worktree at the
//! resolved SHA, analyzed under the **working tree's** config rather than the
//! one committed at that ref, so the two readings agree about scope and their
//! per-scope rows key alike (SRV-019, and [`crate::diff::checkout_root`] for
//! the subdirectory half).
//!
//! ## Resolved once, before the screen
//!
//! [`plan`] runs while the terminal is still the operator's, because a ref
//! they typed and git cannot resolve is a mistake to report plainly, and a
//! message written after [`super::enter`] lands on a drawing that is about to
//! be torn down. The default `HEAD` is deliberately *not* strict: a directory
//! that is not a repository is not a mistake, it just has no commit to be
//! measured from, and falls back to the tree as found.
//!
//! ## Both ends, when the head side is a commit too
//!
//! `--against <REF>` ([`pinned`]) pins the other end to a commit, which makes
//! the session a comparison rather than a watch (MON-008). [`Ends`] is the two
//! flags resolved into the one question the session is going to answer, and it
//! is what decides which measuring thread runs.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Result};

use super::sample::{Commit, Sample, Stamp};
use crate::config::Config;
use crate::diff;

/// What the operator asked the deltas to be measured from.
pub enum Wanted {
    /// A git ref. `strict` when they named it: an unresolvable ref that was
    /// typed is an error, while the default `HEAD` simply does not apply to
    /// every tree.
    Commit { reference: String, strict: bool },
    /// The tree as found when the session starts — the behaviour before
    /// MON-007, still what `--baseline working` asks for and what a tree with
    /// no git falls back to.
    Working,
}

/// The spelling that asks for the old behaviour. A ref could in principle be
/// named this; a branch called `working` is worth `--baseline refs/heads/working`.
const WORKING: &str = "working";

impl Wanted {
    /// Read `--baseline`. Unset is `HEAD`, tolerantly.
    pub fn from_flag(flag: Option<String>) -> Self {
        match flag {
            None => Wanted::Commit {
                reference: "HEAD".to_string(),
                strict: false,
            },
            Some(f) if f.eq_ignore_ascii_case(WORKING) => Wanted::Working,
            Some(reference) => Wanted::Commit {
                reference,
                strict: true,
            },
        }
    }
}

/// A commit baseline that resolved, ready to be measured.
#[derive(Debug)]
pub struct Plan {
    /// What the operator called it, for the message when it fails.
    pub reference: String,
    /// What it resolved to. The SHA is what gets checked out, so a branch
    /// moving mid-session cannot silently change what the deltas mean.
    pub commit: Commit,
}

/// Resolve what was asked for, or answer `None` for a session that measures
/// from its first reading.
///
/// `Err` only for a ref the operator typed — see the module note.
pub fn plan(wanted: &Wanted, repo_root: &Path) -> Result<Option<Plan>> {
    let Wanted::Commit { reference, strict } = wanted else {
        return Ok(None);
    };
    match resolve(repo_root, reference) {
        Some(plan) => Ok(Some(plan)),
        None if *strict => bail!(
            "cannot resolve --baseline {reference} in {}",
            repo_root.display()
        ),
        None => Ok(None),
    }
}

/// Resolve the head side of a comparison — `--against` (MON-008).
///
/// Always strict, and never `working`: this flag exists only because the
/// operator typed it, and without it the head side already *is* the working
/// tree, measured live. A tree that is not a repository fails here for the
/// same reason a typo does — there is no commit for the flag to have meant.
pub fn pinned(reference: &str, repo_root: &Path) -> Result<Plan> {
    if reference.eq_ignore_ascii_case(WORKING) {
        bail!("--against takes a commit; without it monitor already watches the working tree");
    }
    match resolve(repo_root, reference) {
        Some(plan) => Ok(plan),
        None => bail!(
            "cannot resolve --against {reference} in {}",
            repo_root.display()
        ),
    }
}

/// Resolve both flags into the question the session is going to answer.
///
/// The whole of `--baseline` and `--against` is settled here rather than at the
/// call site, because between them they decide which measuring thread runs, and
/// a caller that resolves each flag and then pairs the results is holding three
/// intermediate states of one decision. Its caller keeps the part that is
/// actually its own: which repository answered.
pub fn ends(flag: Option<String>, against: Option<String>, repo_root: &Path) -> Result<Ends> {
    let baseline = plan(&Wanted::from_flag(flag), repo_root)?;
    let against = match against {
        Some(reference) => Some(pinned(&reference, repo_root)?),
        None => None,
    };
    Ends::pair(baseline, against)
}

/// What the session is going to measure, once both flags have resolved.
///
/// The pair rather than two `Option`s carried side by side, because only three
/// of their four combinations mean anything and the fourth — a head side
/// pinned to a commit while the zero is a moment — has to be refused with a
/// sentence rather than silently taken as one of the others.
#[derive(Debug)]
pub enum Ends {
    /// The working tree, watched, with deltas measured from the commit inside
    /// — or from the session's first reading when there is none.
    Watching(Option<Plan>),
    /// Two commits, measured once each. No watcher, no time axis (MON-008).
    Between { from: Plan, to: Plan },
}

impl Ends {
    /// Pair what `--baseline` and `--against` resolved to.
    ///
    /// `Err` for the one combination that cannot be drawn: a comparison needs
    /// two states a reader can name, and `--baseline working` is a moment that
    /// only the running process can point at.
    pub fn pair(baseline: Option<Plan>, against: Option<Plan>) -> Result<Ends> {
        match (baseline, against) {
            (Some(from), Some(to)) => Ok(Ends::Between { from, to }),
            (None, Some(_)) => bail!(
                "--against compares two commits, so --baseline has to name one: \
                 the working tree is a moment, not a state the other side can be read against"
            ),
            (baseline, None) => Ok(Ends::Watching(baseline)),
        }
    }

    /// The commit the head side is pinned to, for the header to name. `None`
    /// for a watching session, where the head side is the tree itself.
    pub fn against(&self) -> Option<&Commit> {
        match self {
            Ends::Between { to, .. } => Some(&to.commit),
            Ends::Watching(_) => None,
        }
    }
}

/// Whatever HEAD is right now, for the `B` key. `None` when git cannot say,
/// which leaves the baseline where it was rather than dropping it.
pub fn at_head(repo_root: &Path) -> Option<Plan> {
    resolve(repo_root, "HEAD")
}

/// The commit in force in `repo_root`, for stamping a reading with the state
/// it measured. `None` outside a repository.
pub fn head_of(repo_root: &Path) -> Option<Commit> {
    described(repo_root, "HEAD")
}

/// A ref as a plan, or `None` if git will not answer for it.
fn resolve(repo_root: &Path, reference: &str) -> Option<Plan> {
    diff::verify_git_repo(repo_root).ok()?;
    let commit = described(repo_root, reference)?;
    Some(Plan {
        reference: reference.to_string(),
        commit,
    })
}

/// A ref's short SHA and subject in one git call — two would be two chances
/// for a ref that moved between them to be described as a commit it is not.
fn described(repo_root: &Path, reference: &str) -> Option<Commit> {
    let line = diff::git_lines(repo_root, &["log", "-1", "--format=%h %s", reference]).pop()?;
    let (sha, subject) = line.split_once(' ').unwrap_or((line.as_str(), ""));
    match sha.is_empty() {
        true => None,
        false => Some(Commit {
            sha: sha.to_string(),
            subject: subject.to_string(),
        }),
    }
}

/// Which end of the session a checkout is being measured for.
///
/// The only difference between the two calls a comparison makes, and it is not
/// cosmetic: `"baseline": true` is what a `--log` reader uses to find the state
/// the other lines' deltas are measured from (MON-007), so a head side wearing
/// that mark would give the file two zeros and no reading.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The state the deltas are measured *from*.
    Zero,
    /// The state they are measured *to* — only ever a checkout in a comparison
    /// (MON-008); a watching session measures its head side off the tree.
    Head,
}

/// What measuring a checkout takes, borrowed from the measuring thread.
///
/// The pieces rather than the [`Measure`](super::watch::Measure) they come
/// from, so this module does not depend on the one that calls it: `watch`
/// asks `baseline` for a reading, and nothing here asks `watch` for anything.
/// Reaching back for the struct — or for its `now_ms` — put the two files in
/// a loop and dropped `src/monitor` from tangled to cyclic, which the
/// dashboard reported on itself within a minute of the code being written.
pub struct Site<'a> {
    /// The live tree's config. Rooted into the checkout here, never rebuilt
    /// from it — see the note on [`measure`].
    pub config: &'a Config,
    /// The analyzed path, which the checkout's own root has to correspond to.
    pub root: &'a Path,
    /// The repository the checkout is made from, and whose rules grade it.
    pub repo_root: &'a Path,
    /// The dashboard is quitting: stop partway rather than finish a walk
    /// nobody will see.
    pub stop: &'a Arc<AtomicBool>,
}

/// Measure a checkout of the planned commit.
///
/// Three things are the working tree's and not the checkout's, and each of
/// them would otherwise make the baseline incomparable with the readings it
/// is subtracted from:
///
/// - **the config**, so both sides look at the same set of files. Analyzing
///   the checkout on its own terms would read the `.mezz/settings.json`
///   committed at that ref, and a scope the two sides disagree about reads as
///   every excluded file arriving or leaving at once.
/// - **the rules file**, so the `rules` tile's delta is one bar applied twice
///   rather than two bars compared. "What today's rules say about last week's
///   tree" is the question a delta answers; "what last week's rules said" is
///   a different measurement wearing the same number.
/// - **the root**, via [`diff::checkout_root`]: `git worktree add`
///   materialises the whole repository, and a monitor pointed at a
///   subdirectory has to compare that subdirectory against itself (SRV-021).
///
/// The worktree is removed before the result is unwrapped, so a failed or
/// cancelled analysis does not leave one behind.
///
/// `at_ms` is passed in rather than read here for the reason [`Site`] gives:
/// the clock lives on the measuring thread, and one import of it back into
/// this module is a cycle.
pub fn measure(at: &Site, plan: &Plan, at_ms: u64, role: Role) -> Result<Sample> {
    let dir = std::env::temp_dir().join(format!(
        "mezz-monitor-base-{}-{}",
        std::process::id(),
        plan.commit.sha
    ));
    diff::create_worktree(at.repo_root, &dir, &plan.commit.sha)?;
    let root = diff::checkout_root(at.root, &dir);

    let started = Instant::now();
    let analyzed = diff::analyze_with_cancel(
        diff::rooted_at(at.config, &root),
        &format!("baseline ({})", plan.commit.sha),
        at.stop,
    );
    diff::remove_worktree(at.repo_root, &dir);
    let (graph, _) = analyzed?;

    Ok(Sample::of(
        &graph,
        &root,
        at.repo_root,
        Stamp {
            // Not a tick of the session: nothing changed to cause it, and it
            // is not in the ring the sparklines are drawn from.
            seq: 0,
            at_ms,
            analysis_ms: started.elapsed().as_millis() as u64,
            changed: &[],
            head: Some(plan.commit.clone()),
            baseline: role == Role::Zero,
        },
    ))
}
