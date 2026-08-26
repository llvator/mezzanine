//! The clone → analyze pipeline behind `POST /api/repos` (SRV-004).
//!
//! Submitting a URL returns immediately with a slug; the work runs on a
//! background task and its progress is observable through
//! `GET /api/repos/{slug}` and the SSE stream at `.../events`.
//!
//! Everything here treats the submitted repo as hostile input: the URL is
//! validated against a one-host allowlist before it reaches `git`, the clone
//! is shallow and submodule-free, it's killed on a timeout, its size is
//! checked before anything reads it, and the analysis runs with
//! `allow_unsafe_passes` cleared so no `build.rs` from the repo executes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use tokio::sync::Semaphore;

use super::repo::{analyze_repo, clone_dir, persist, JobStatus, RepoRegistry, RepoSlot};

/// Knobs the ticket asks to be configurable, resolved once at startup.
#[derive(Clone)]
pub(crate) struct JobConfig {
    pub cache_dir: PathBuf,
    pub clone_timeout: Duration,
    pub max_repo_bytes: u64,
    pub include_tests: bool,
    pub languages: Option<Vec<String>>,
}

/// Bounds how many clones/analyses run at once. Default 1 — analysis is
/// already rayon-parallel internally, so a second concurrent job mostly
/// competes with the first for the same cores.
pub(crate) type JobLimiter = Arc<Semaphore>;

/// Outcome of a submission, so the handler can pick its status code. The
/// resulting status is read back off the slot, not carried here, so the
/// response can't disagree with the registry.
pub(crate) enum Submission {
    /// Already analyzed — nothing to do. `200`.
    AlreadyReady,
    /// A job is now running, or was already. `202`.
    Accepted,
}

/// Register `slug` and start its pipeline unless one is already loaded or
/// in flight.
///
/// The registry write lock is held across the check *and* the insert, so two
/// simultaneous POSTs for the same slug can't both spawn a job — the second
/// sees the first's slot and joins it.
pub(crate) async fn submit(
    registry: &RepoRegistry,
    limiter: &JobLimiter,
    config: &JobConfig,
    url: String,
    slug: String,
) -> Submission {
    let slot = {
        let mut repos = registry.write().await;
        if let Some(existing) = repos.get(&slug) {
            match existing.status() {
                JobStatus::Ready => return Submission::AlreadyReady,
                // A retry of a failed slug re-runs the pipeline: the usual
                // cause is a fixed typo or a transient network failure, and
                // refusing would leave the slug permanently poisoned.
                JobStatus::Failed(_) => {}
                _ => return Submission::Accepted,
            }
        }
        let slot = Arc::new(RepoSlot::new(
            slug.clone(),
            Some(url.clone()),
            JobStatus::Queued,
        ));
        repos.insert(slug.clone(), slot.clone());
        slot
    };

    let limiter = limiter.clone();
    let config = config.clone();
    tokio::spawn(async move {
        // Cloning and analysis both block; keep them off the async runtime's
        // worker threads. The permit is held for the whole job so `--jobs`
        // bounds real concurrency, not just task count.
        let permit = limiter.acquire_owned().await;
        let outcome = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            run_job(&slot, &config, &url)
        })
        .await;
        if let Err(e) = outcome {
            eprintln!("   ⚠ job task panicked: {e}");
        }
    });

    Submission::Accepted
}

/// Clone, size-check, analyze, persist. Every failure path lands in
/// `JobStatus::Failed` with a message the user can act on.
fn run_job(slot: &Arc<RepoSlot>, config: &JobConfig, url: &str) {
    let dir = clone_dir(&config.cache_dir, &slot.slug);

    slot.transition(JobStatus::Cloning);
    if let Err(e) = clone(url, &dir, config) {
        // A partial clone would otherwise look like a valid checkout to the
        // next submission of the same slug.
        let _ = std::fs::remove_dir_all(&dir);
        slot.transition(JobStatus::Failed(format!("{e:#}")));
        return;
    }

    slot.transition(JobStatus::Analyzing);
    match analyze_repo(
        &slot.slug,
        Some(url.to_string()),
        &dir,
        config.include_tests,
        &config.languages,
    ) {
        Ok((state, result)) => {
            if let Err(e) = persist(&config.cache_dir, &state, &result) {
                // The repo is analyzed and serveable; only the restart
                // shortcut is lost. Warn, don't fail the submission.
                eprintln!("   ⚠ cache: failed to persist {}: {e:#}", slot.slug);
            }
            eprintln!(
                "   ✓ {}: {} entities, {} relationships",
                slot.slug,
                state.graph.node_count(),
                state.graph.edge_count()
            );
            slot.publish(Arc::new(state));
        }
        Err(e) => slot.transition(JobStatus::Failed(format!("analysis failed: {e:#}"))),
    }
}

/// Shallow-clone `url` into `dir`, then enforce the size limit.
///
/// The clone is `--depth 1 --no-tags --recurse-submodules=no`: history isn't
/// needed for analysis, and submodules would fetch arbitrary extra hosts the
/// URL allowlist never saw.
fn clone(url: &str, dir: &Path, config: &JobConfig) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut child = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--no-tags",
            "--recurse-submodules=no",
            // Stop git from prompting for credentials on a private or
            // misspelled repo — it would hang until the timeout otherwise.
            "--config",
            "credential.helper=",
            "--",
            url,
        ])
        .arg(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let status = wait_with_timeout(&mut child, config.clone_timeout)?;
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }

    match status {
        None => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("clone timed out after {}s", config.clone_timeout.as_secs());
        }
        Some(s) if !s.success() => {
            bail!("clone failed: {}", sanitize_git_stderr(&stderr, dir));
        }
        Some(_) => {}
    }

    let bytes = dir_size_bytes(dir)?;
    if bytes > config.max_repo_bytes {
        bail!(
            "repository is {} MB, over the {} MB limit",
            bytes / 1_000_000,
            config.max_repo_bytes / 1_000_000
        );
    }
    Ok(())
}

/// Poll `child` until it exits or `timeout` elapses. `None` means it's still
/// running and the caller should kill it.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Apparent size of `dir` via `du -sk`, in bytes.
fn dir_size_bytes(dir: &Path) -> Result<u64> {
    let output = Command::new("du").arg("-sk").arg(dir).output()?;
    if !output.status.success() {
        bail!("could not measure repository size");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let kb: u64 = text
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("could not parse `du` output"))?;
    Ok(kb * 1024)
}

/// Turn git's stderr into something safe to hand an anonymous client.
///
/// The message goes out over the network in a `failed` status, so it must
/// carry the *diagnosis* (bad URL, private repo, network) without carrying
/// the server's filesystem layout. `Cloning into '<abs path>'…` is dropped
/// outright — it's pure noise — and any remaining mention of the clone
/// directory is redacted, since git repeats the path in several messages.
fn sanitize_git_stderr(text: &str, clone_dir: &Path) -> String {
    let dir = clone_dir.display().to_string();
    let lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Cloning into"))
        .map(|l| {
            if dir.is_empty() {
                l.to_string()
            } else {
                l.replace(&dir, "<clone-dir>")
            }
        })
        .collect();
    if lines.is_empty() {
        return "git exited with an error but wrote no diagnostics".to_string();
    }
    first_lines(&lines, 5)
}

/// Cap a message at `n` lines so one runaway error can't become the response.
fn first_lines(lines: &[String], n: usize) -> String {
    lines.iter().take(n).cloned().collect::<Vec<_>>().join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_the_diagnosis_and_drops_the_noise() {
        let dir = Path::new("/srv/cache/a__b/repo");
        let stderr = "Cloning into '/srv/cache/a__b/repo'...\n\nremote: Not Found\nfatal: repository not found\n";
        assert_eq!(
            sanitize_git_stderr(stderr, dir),
            "remote: Not Found; fatal: repository not found"
        );
    }

    /// The failure message is returned to anonymous clients, so it must not
    /// disclose where the server keeps its files.
    #[test]
    fn sanitize_redacts_the_clone_path_wherever_it_appears() {
        let dir = Path::new("/home/op/.cache/mezz/serve/a__b/repo");
        let stderr = "fatal: could not create work tree dir '/home/op/.cache/mezz/serve/a__b/repo': Permission denied";
        let out = sanitize_git_stderr(stderr, dir);
        assert!(!out.contains("/home/op"), "leaked a path: {out}");
        assert!(out.contains("<clone-dir>"), "{out}");
        assert!(out.contains("Permission denied"), "{out}");
    }

    #[test]
    fn sanitize_caps_a_runaway_error() {
        let stderr = (1..=20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            sanitize_git_stderr(&stderr, Path::new("/x")),
            "line 1; line 2; line 3; line 4; line 5"
        );
    }

    #[test]
    fn sanitize_handles_silent_failures() {
        assert_eq!(
            sanitize_git_stderr("Cloning into '/x'...\n", Path::new("/x")),
            "git exited with an error but wrote no diagnostics"
        );
    }

    #[test]
    fn dir_size_measures_a_real_directory() {
        let dir = std::env::temp_dir().join("mezz-jobs-size-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), vec![0u8; 40_000]).unwrap();
        let bytes = dir_size_bytes(&dir).unwrap();
        // `du` reports allocated blocks, so only assert the order of
        // magnitude — enough to catch a unit error (KB read as bytes).
        assert!(bytes >= 40_000, "expected >= 40000 bytes, got {bytes}");
        assert!(bytes < 10_000_000, "implausibly large: {bytes}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A process that never exits must be reported as timed out, not waited
    /// on forever.
    #[test]
    fn wait_with_timeout_gives_up() {
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_millis(400)).unwrap();
        assert!(status.is_none(), "expected a timeout");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn wait_with_timeout_returns_a_fast_exit() {
        let mut child = Command::new("true").spawn().unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_secs(5)).unwrap();
        assert!(status.expect("should have exited").success());
    }
}
