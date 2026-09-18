//! UI-153 — what is in the on-disk cache, and removing part of it.
//!
//! The parse store is the largest thing mezz puts on a developer's disk and
//! was the only one they could not see: 11 GB on the machine that asked for
//! this, including 3.1 GB of a hand-made backup no code path would ever read.
//!
//! Everything here is read-only bookkeeping over the layout
//! [`super::parse_store`] writes, plus [`clear`]. It deliberately never opens
//! an entry: size, count and last-use come from `stat`, which is what makes a
//! report affordable — a stat-only walk of 140,987 entries measured 1.46s,
//! while reading them would be 8.3 GB of JSON.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::parse_store::{cache_root, version_tag, BUCKET_LABEL, LOOSE_BUCKET};

/// Everything under the cache root, grouped the way the panel shows it.
#[derive(serde::Serialize, Default)]
pub struct CacheReport {
    /// The real directory on disk, so what this describes can be checked
    /// from a terminal.
    pub root: String,
    pub bytes: u64,
    pub generations: Vec<Generation>,
    /// `reshape`'s baselines — the other thing under the same root.
    pub reshape: Usage,
}

/// One parse-store generation. Only one is in force; the rest are waiting out
/// [`super::parse_store`]'s grace window, and are what `Reclaim` targets.
#[derive(serde::Serialize)]
pub struct Generation {
    pub tag: String,
    pub current: bool,
    #[serde(flatten)]
    pub usage: Usage,
    pub repos: Vec<RepoUsage>,
}

/// One repository's entries within a generation.
#[derive(serde::Serialize)]
pub struct RepoUsage {
    /// The bucket directory, and what a clear request names. `None` for the
    /// pseudo-row holding a pre-UI-153 generation's flat entries: those are
    /// reclaimed by clearing the generation, not one repository of it.
    pub id: Option<String>,
    pub label: String,
    /// The git common dir this bucket was written for, when its label file
    /// survives. Shown so two checkouts with the same basename are tellable
    /// apart.
    pub common_dir: Option<String>,
    #[serde(flatten)]
    pub usage: Usage,
}

/// Size, count and last use of some set of files.
#[derive(serde::Serialize, Default, Clone, Copy)]
pub struct Usage {
    pub bytes: u64,
    pub entries: u64,
    /// Unix seconds of the most recently modified file, or `None` for an
    /// empty set.
    pub last_used: Option<u64>,
}

impl Usage {
    fn add_file(&mut self, meta: &std::fs::Metadata) {
        self.bytes += meta.len();
        self.entries += 1;
        let stamp = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        self.last_used = self.last_used.max(stamp);
    }

    fn absorb(&mut self, other: Usage) {
        self.bytes += other.bytes;
        self.entries += other.entries;
        self.last_used = self.last_used.max(other.last_used);
    }
}

/// What a clear request may name. Every variant that carries a name has it
/// checked by [`one_component`] before it reaches the filesystem.
///
/// Deserialized straight off the wire rather than mirrored into a second enum
/// in the handler: the two would be the same five variants, and the failure
/// mode of a mirror is one growing a case the other does not.
#[derive(serde::Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ClearTarget {
    /// The whole cache root — parse generations and reshape baselines.
    Everything,
    /// Every generation except the one in force.
    Abandoned,
    /// `reshape`'s baselines.
    Reshape,
    /// One generation, whether or not it is the current one.
    Generation {
        #[serde(rename = "generation")]
        tag: String,
    },
    /// One repository's entries within one generation.
    Repo {
        #[serde(rename = "generation")]
        tag: String,
        repo: String,
    },
}

/// Walk the cache root and report what is there. `None` when no cache
/// location resolves, which is the same condition that disables the store.
pub fn report() -> Option<CacheReport> {
    let root = cache_root()?;
    let current = version_tag();
    let mut out = CacheReport {
        root: root.to_string_lossy().to_string(),
        ..Default::default()
    };
    out.reshape = scan_flat(&root.join("reshape"));

    for entry in entries_of(&root.join("parse")) {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let tag = entry.file_name().to_string_lossy().to_string();
            out.generations
                .push(scan_generation(&entry.path(), tag == current, tag));
        }
    }
    // Current first, then the biggest of what is waiting to be reclaimed.
    out.generations
        .sort_by_key(|g| (!g.current, std::cmp::Reverse(g.usage.bytes)));
    out.bytes = out.reshape.bytes + out.generations.iter().map(|g| g.usage.bytes).sum::<u64>();
    Some(out)
}

/// One generation: a bucket directory per repository, plus — in a generation
/// written before UI-153 — entries lying flat at the top.
fn scan_generation(dir: &Path, current: bool, tag: String) -> Generation {
    let mut repos = Vec::new();
    let mut flat = Usage::default();
    let mut usage = Usage::default();

    for entry in entries_of(dir) {
        match entry.file_type().map(|t| t.is_dir()) {
            Ok(true) => {
                let name = entry.file_name().to_string_lossy().to_string();
                repos.push(scan_bucket(&entry.path(), name));
            }
            // A generation written before UI-153 keeps its entries here.
            Ok(false) => count_file(&mut flat, &entry),
            Err(_) => {}
        }
    }
    if flat.entries > 0 {
        repos.push(RepoUsage {
            id: None,
            label: "entries from an older layout".to_string(),
            common_dir: None,
            usage: flat,
        });
    }
    repos.sort_by_key(|r| std::cmp::Reverse(r.usage.bytes));
    for repo in &repos {
        usage.absorb(repo.usage);
    }
    Generation {
        tag,
        current,
        usage,
        repos,
    }
}

/// One repository bucket. Its `_repo.json` names it; without one — a
/// half-written bucket, or `loose` — the directory name stands in.
fn scan_bucket(dir: &Path, name: String) -> RepoUsage {
    let label = std::fs::read(dir.join(BUCKET_LABEL))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let text = |key: &str| {
        label
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    RepoUsage {
        label: text("label").unwrap_or_else(|| match name.as_str() {
            LOOSE_BUCKET => "files outside any repository".to_string(),
            other => other.to_string(),
        }),
        common_dir: text("common_dir"),
        id: Some(name),
        usage: scan_flat(dir),
    }
}

/// Size and count of the files directly inside `dir`. The label file is not
/// an entry and is not counted as one.
fn scan_flat(dir: &Path) -> Usage {
    let mut usage = Usage::default();
    for entry in entries_of(dir) {
        if entry.file_name() != BUCKET_LABEL {
            count_file(&mut usage, &entry);
        }
    }
    usage
}

/// Add one directory entry to `usage`, if it is a file we can stat. Anything
/// unreadable is skipped rather than reported: this is a size estimate, and a
/// permission error on one file is not worth failing a panel over.
fn count_file(usage: &mut Usage, entry: &std::fs::DirEntry) {
    match entry.metadata() {
        Ok(meta) if meta.is_file() => usage.add_file(&meta),
        _ => {}
    }
}

/// The readable entries of `dir`, or none. Flattens the two layers of
/// `Result` that `read_dir` hands back, which is what keeps the walkers
/// above at a legible nesting depth.
fn entries_of(dir: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

/// Remove part of the cache. Returns the paths removed, for the log and for
/// the caller to report.
///
/// Safe to run at any time by the store's own contract: a missing entry
/// degrades to a cold parse, never to an error. The cost of clearing under a
/// running analysis is that analysis re-parsing, nothing worse.
pub fn clear(target: &ClearTarget) -> Result<Vec<String>, String> {
    let root = cache_root().ok_or_else(|| "no cache directory resolves".to_string())?;
    let parse = root.join("parse");
    let doomed: Vec<PathBuf> = match target {
        ClearTarget::Everything => vec![parse, root.join("reshape")],
        ClearTarget::Reshape => vec![root.join("reshape")],
        ClearTarget::Generation { tag } => vec![parse.join(one_component(tag)?)],
        ClearTarget::Repo { tag, repo } => {
            vec![parse.join(one_component(tag)?).join(one_component(repo)?)]
        }
        ClearTarget::Abandoned => abandoned_generations(&parse),
    };
    let mut removed = Vec::new();
    for path in doomed {
        // Belt as well as braces: `one_component` already refuses anything
        // that could climb, and this refuses anything that somehow did.
        if !path.starts_with(&root) {
            return Err(format!("refusing to remove {} — outside the cache", path.display()));
        }
        if !path.exists() {
            continue;
        }
        std::fs::remove_dir_all(&path)
            .map_err(|e| format!("could not remove {}: {e}", path.display()))?;
        removed.push(path.to_string_lossy().to_string());
    }
    Ok(removed)
}

/// Every generation directory that is not the one in force.
fn abandoned_generations(parse: &Path) -> Vec<PathBuf> {
    let current = version_tag();
    entries_of(parse)
        .into_iter()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.file_name().to_string_lossy() != current)
        .map(|e| e.path())
        .collect()
}

/// A name that may be joined onto the cache root: exactly one directory, and
/// not one that climbs out of it.
///
/// These names arrive from a browser. Refused rather than sanitized — a
/// stripped `..` is a name the caller did not ask for, and a delete endpoint
/// guessing at intent is how one ends up removing the wrong tree.
fn one_component(name: &str) -> Result<&str, String> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0');
    if bad {
        return Err(format!(
            "refusing {name:?}: a cache name must be a single directory"
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names in a clear request come from a browser. Every way of
    /// climbing out of the cache root must be refused, not cleaned up.
    #[test]
    fn a_name_that_escapes_the_cache_is_refused() {
        for bad in ["..", ".", "", "../..", "a/b", "..\\..", "a\0b", "/etc"] {
            assert!(
                one_component(bad).is_err(),
                "{bad:?} should have been refused"
            );
        }
        for good in ["1.4.0-6-abc", "r-0123456789ab", "loose"] {
            assert_eq!(one_component(good).unwrap(), good);
        }
    }

    /// A traversal that gets past the name check must still not delete
    /// anything: `clear` re-checks containment against the cache root.
    #[test]
    fn clearing_refuses_a_target_outside_the_cache() {
        let err = clear(&ClearTarget::Generation {
            tag: "../../etc".to_string(),
        })
        .unwrap_err();
        assert!(err.contains("single directory"), "{err}");
    }

    #[test]
    fn usage_takes_the_latest_stamp_and_sums_the_rest() {
        let mut a = Usage {
            bytes: 10,
            entries: 1,
            last_used: Some(100),
        };
        a.absorb(Usage {
            bytes: 5,
            entries: 2,
            last_used: Some(300),
        });
        assert_eq!((a.bytes, a.entries, a.last_used), (15, 3, Some(300)));

        a.absorb(Usage::default());
        assert_eq!(
            a.last_used,
            Some(300),
            "an empty set must not erase the stamp"
        );
    }
}
