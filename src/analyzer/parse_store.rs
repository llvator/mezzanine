//! AN-003 — persistent on-disk parse store + incremental re-analysis.
//!
//! Serialized per-file parse results ([`super::ParsedFile`]) keyed by
//! (file identity, parser version), with the content hash carried *inside*
//! the entry and compared on read. The determinism guarantee (AN-002) is the
//! safety argument: identical content + identical parser ⇒ identical entities,
//! so a hash match can never be stale-wrong.
//!
//! One entry per file, not per revision of a file: a re-parse overwrites its
//! predecessor. Folding the content hash into the *filename* instead — the
//! original design — meant every save minted a permanent entry nothing ever
//! removed, so the store grew with edits rather than with files.
//!
//! "One file" is a repository plus a path within it, not an absolute path
//! (AN-037). Every checkout of a repository agrees on the former, which is
//! what lets the two throwaway worktrees of a comparison — and the reader's
//! live tree — share one entry instead of parsing the same bytes three times.
//! Since an entry is written in whichever spelling walked it first, `get`
//! rebases it onto the spelling asked for.
//!
//! This *is* the incremental mechanism. There is no separate dirty
//! tracking: on re-analysis, unchanged files hit the store and the one
//! edited file misses and re-parses, because its content hash changed.
//!
//! Robustness contract: every operation swallows its own errors. A
//! corrupt entry, an unreadable cache dir, or a failed write all degrade
//! to a cold parse — never to an error surfaced to the caller. The store
//! is a speedup, never a source of failure.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use super::ParsedFile;

/// Manual cache salt — an escape hatch, no longer the primary lever.
///
/// [`PARSE_FINGERPRINT`] now invalidates the store automatically for any
/// change to the parsers, the models, or the grammars. Bump this only for a
/// change those three cannot see: a behavioural dependency bump, or an
/// environment-driven parse. Bumping it is always safe and always cheap —
/// one cold analysis.
const PARSE_CACHE_SALT: &str = "6";

/// Fingerprint of everything that decides what a stored parse contains:
/// `src/parser/**`, `src/models/**`, and the resolved `tree-sitter*`
/// versions. Computed by `build.rs`, which owns the rationale.
///
/// This exists because the manual salt was forgotten. `c2bcb85` rewrote
/// receiver resolution across six parser files and left the salt at `4`, so
/// every warm cache kept serving the older parser's output and the change
/// measured as no change at all. Deriving the generation removes the step
/// that has to be remembered: the store now invalidates because the parser
/// differs, not because someone noticed it differs.
const PARSE_FINGERPRINT: &str = env!("MEZZ_PARSE_FINGERPRINT");

/// How long an abandoned generation must sit untouched before
/// [`prune_stale_generations`] reclaims it. Long enough that a still-installed
/// older binary keeps its own cache warm between runs.
///
/// This is also what keeps the automatic fingerprint affordable: every parser
/// edit mints a generation, so a day of parser work leaves a handful behind.
/// They are reclaimed a week after last use, and a superseded generation is
/// never read in the meantime.
const STALE_GENERATION_DAYS: u64 = 7;

/// The file naming a bucket's repository, written inside the bucket
/// directory. A hash cannot be shown to a person, so the bucket carries its
/// own label; [`super::cache_report`] reads it and never guesses.
pub(crate) const BUCKET_LABEL: &str = "_repo.json";

/// The bucket for files that belong to no repository — a spec directory
/// outside the tree, an `analyze` of a bare folder. These take the
/// absolute-path fallback and would otherwise have nowhere to go.
pub(crate) const LOOSE_BUCKET: &str = "loose";

/// Version tag folded into the store path. Three parts, each covering what
/// the others cannot: the crate version (auto-bumps every release), the
/// [`PARSE_FINGERPRINT`] (auto-bumps whenever parse output could change), and
/// the manual [`PARSE_CACHE_SALT`] (the escape hatch). Entries under an old
/// tag are never read, so any of the three changing is a clean invalidation.
pub(crate) fn version_tag() -> String {
    format!(
        "{}-{}-{}",
        env!("CARGO_PKG_VERSION"),
        PARSE_CACHE_SALT,
        PARSE_FINGERPRINT
    )
}

/// Persistent on-disk store of per-file parse results.
///
/// Global (not per-repo): a key carries either the repository the file
/// belongs to or its absolute path, so two repos can never collide.
/// Construct once per analysis and share by reference across the parallel
/// parse loop.
pub struct ParseStore {
    /// Version-scoped directory holding the entries. `None` when no cache
    /// location could be resolved — the store then behaves as a no-op
    /// (every `get` misses, every `put` is dropped), i.e. cold analysis.
    dir: Option<PathBuf>,
    /// The repository the walk root sits in, when it sits in one. Resolved
    /// once per analysis; see [`RepoKey`] for what it buys.
    repo: Option<RepoKey>,
    hits: AtomicU64,
    misses: AtomicU64,
    /// Monotonic counter making temp filenames unique within a process,
    /// so concurrent writers of different keys never share a temp path.
    seq: AtomicU64,
    /// Whether this analysis has already written its bucket's
    /// [`BUCKET_LABEL`]. The label is the same for every file of a run, so
    /// writing it costs one atomic swap per file rather than a stat.
    named: AtomicBool,
}

/// What makes two checkouts of one repository name the same entry (AN-037).
///
/// A comparison analyses each side in a throwaway `git worktree`, so the same
/// file is walked at two absolute paths that share no prefix with each other
/// or with the reader's live tree. Keyed on the absolute path, those are three
/// disjoint keyspaces over byte-identical content: the head side of a
/// comparison re-parsed every file the base side had just parsed, and each new
/// pair of refs minted entries that were never hit *and* never overwritten.
struct RepoKey {
    /// `git rev-parse --git-common-dir`, canonicalized — the same directory
    /// in the main checkout and in every linked worktree, which is precisely
    /// the property the absolute path lacks.
    common_dir: PathBuf,
    /// This worktree's own top level, stripped off a file's path to leave one
    /// that is relative to the repository. Differs per worktree; that is the
    /// point.
    toplevel: PathBuf,
}

impl ParseStore {
    /// Open the store for an analysis rooted at `walk_root`, resolving the
    /// cache directory from the environment:
    ///
    /// 1. `MEZZ_CACHE_DIR` — explicit override (CI, tests).
    /// 2. `XDG_CACHE_HOME/mezz`
    /// 3. `~/.cache/mezz`
    ///
    /// If none resolve, the store is disabled (analysis stays cold but
    /// correct). The directory is created lazily on first write.
    ///
    /// `walk_root` decides only how entries are *keyed* — see [`RepoKey`].
    /// A root outside any repository keys on the absolute path, which is
    /// what every root did before AN-037.
    pub fn open_for(walk_root: &Path) -> Self {
        let dir = cache_root().map(|root| root.join("parse").join(version_tag()));
        if let Some(dir) = dir.as_deref() {
            prune_stale_generations(dir);
        }
        Self::from_dir(dir, walk_root)
    }

    /// Open the store rooted at an explicit cache directory, bypassing env
    /// resolution. Handy for embedders (and tests) that want full control
    /// over cache placement; the same version scoping still applies.
    pub fn open_at(cache_root: PathBuf, walk_root: &Path) -> Self {
        Self::from_dir(
            Some(cache_root.join("parse").join(version_tag())),
            walk_root,
        )
    }

    /// A disabled store: never hits, never writes. Used where a caller
    /// wants to opt out of persistence entirely.
    pub fn disabled() -> Self {
        Self {
            dir: None,
            repo: None,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            named: AtomicBool::new(false),
        }
    }

    fn from_dir(dir: Option<PathBuf>, walk_root: &Path) -> Self {
        Self {
            repo: dir.as_ref().and_then(|_| resolve_repo_key(walk_root)),
            dir,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            named: AtomicBool::new(false),
        }
    }

    /// blake3 hex digest of file content. Used both as the store key
    /// component and to populate `FileInfo.content_hash`. Cryptographic
    /// strength rules out a collision serving a wrong parse.
    pub fn content_hash(content: &str) -> String {
        blake3::hash(content.as_bytes()).to_hex().to_string()
    }

    /// Look up a stored parse of `content_hash` for the file being walked at
    /// `walked` (canonically, `abs_path`). Returns `None` on a miss or on
    /// *any* error (missing file, corrupt JSON, unreadable dir) — the caller
    /// then parses fresh. Records a hit/miss for the end-of-run summary.
    ///
    /// Content decides *staleness*; it does not decide *applicability*. An
    /// entry records the path it was walked at, and one entry serves every
    /// spelling of that file — the two sides of a comparison, a symlinked
    /// spec directory, a `./`-prefixed relative walk. [`rebase`] is what
    /// makes serving it to another spelling correct rather than a leak.
    pub(crate) fn get(
        &self,
        abs_path: &Path,
        content_hash: &str,
        walked: &Path,
    ) -> Option<ParsedFile> {
        // `?`, not a counted miss: a disabled store is not a cache that
        // failed to answer, and counting it would report a miss per file on
        // every run that opted out of persistence.
        let path = self.entry_path(abs_path)?;
        let hit = std::fs::read(&path)
            .ok()
            .and_then(|bytes| decode(&bytes, content_hash, walked));
        let counter = if hit.is_some() {
            &self.hits
        } else {
            &self.misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
        hit
    }

    /// Persist a parse result. Best-effort: serialization or IO failure is
    /// swallowed (the entry simply won't exist next time). Writes to a
    /// process-unique temp file then atomically renames, so a concurrent
    /// reader — even in a second `mezz` process — never sees a torn entry.
    pub(crate) fn put(&self, abs_path: &Path, content_hash: &str, parsed: &ParsedFile) {
        let Ok(bytes) = serde_json::to_vec(&EntryRef {
            content_hash,
            parsed,
        }) else {
            return;
        };
        let Some((tmp, final_path)) = self.staged_paths(abs_path) else {
            return;
        };
        if std::fs::write(&tmp, &bytes).is_err() {
            return;
        }
        // Rename is atomic on the same filesystem; a failure just leaves a
        // stray temp file, which we clean up so it can't accumulate.
        if std::fs::rename(&tmp, &final_path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// `(hits, misses)` since this store was opened.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    /// Where the entry for `abs_path` lives: its bucket directory, and its
    /// basename stem within that bucket.
    ///
    /// The bucket is a directory level rather than another hash component so
    /// that the panel can group by repository from a `stat`-only walk
    /// (UI-153). Recovering the same grouping from a flat directory would mean
    /// opening all 140,987 entries — 8.3 GB of JSON against a 1.46s walk.
    ///
    /// **One entry per file, not per revision of a file** — see [`repo_stem`]
    /// for what "per file" means across checkouts.
    fn slot(&self, abs_path: &Path) -> (String, String) {
        match self
            .repo
            .as_ref()
            .and_then(|repo| repo_stem(repo, abs_path).map(|stem| (repo, stem)))
        {
            Some((repo, stem)) => (bucket_name(&repo.common_dir), stem),
            // Outside any repository: the absolute-path fallback, which is
            // what every root did before AN-037.
            None => (
                LOOSE_BUCKET.to_string(),
                hex(abs_path.to_string_lossy().as_bytes()),
            ),
        }
    }

    /// Prepare the bucket for `abs_path` and hand back the two paths a write
    /// needs: the process-unique temp file to fill, and the entry it renames
    /// to.
    ///
    /// One call rather than six locals in [`put`]. The bucket directory, its
    /// label, the stem and the sequence number are all one concern — *where
    /// this entry goes* — and only the two ends of it are the writer's
    /// business.
    ///
    /// The temp file sits inside the bucket, not beside it: rename is only
    /// atomic within a filesystem, and this also keeps a crashed run's
    /// leftovers out of the generation directory, where
    /// [`super::cache_report`] would count them as a bucket.
    fn staged_paths(&self, abs_path: &Path) -> Option<(PathBuf, PathBuf)> {
        let dir = self.dir.as_ref()?;
        let (bucket_name, stem) = self.slot(abs_path);
        let bucket = dir.join(bucket_name);
        std::fs::create_dir_all(&bucket).ok()?;
        self.name_the_bucket_once(&bucket);
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        Some((
            bucket.join(format!("{}.{}.{}.tmp", stem, std::process::id(), seq)),
            bucket.join(format!("{stem}.json")),
        ))
    }

    /// Absolute path of the entry file for `abs_path`, or `None` when the
    /// store is disabled.
    fn entry_path(&self, abs_path: &Path) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let (bucket, stem) = self.slot(abs_path);
        Some(dir.join(bucket).join(format!("{stem}.json")))
    }

    /// Write the bucket's [`BUCKET_LABEL`] the first time this analysis puts
    /// an entry, so the panel can name the repository instead of showing its
    /// hash.
    ///
    /// The label names the **common dir's parent**, not this worktree's top
    /// level: a comparison analyses throwaway checkouts, and naming from those
    /// would label the whole repository `mezz-diff-base-88fb99f`.
    ///
    /// The `swap` is what keeps this off the per-file path — every file of a
    /// run would otherwise stat or rewrite the same small file.
    fn name_the_bucket_once(&self, bucket: &Path) {
        let Some(repo) = self.repo.as_ref() else { return };
        if self.named.swap(true, Ordering::Relaxed) {
            return;
        }
        let label = repo
            .common_dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| repo.common_dir.to_string_lossy().to_string());
        let body = serde_json::json!({
            "common_dir": repo.common_dir.to_string_lossy(),
            "label": label,
        });
        // Best-effort like every other store write: a missing label costs the
        // panel a readable name, never a parse.
        if let Ok(bytes) = serde_json::to_vec(&body) {
            let _ = std::fs::write(bucket.join(BUCKET_LABEL), bytes);
        }
    }
}

/// A repository's bucket directory name: `r-` plus a prefix of the common
/// dir's digest. Prefixed so it can never collide with [`LOOSE_BUCKET`], and
/// truncated because the directory only has to separate repositories, not
/// carry the key — the entry stem still holds the full identity.
fn bucket_name(common_dir: &Path) -> String {
    let digest = hex(common_dir.as_os_str().as_encoded_bytes());
    format!("r-{}", &digest[..24])
}

/// Decode one stored entry, rebasing it onto the path it is being served to.
///
/// Split out of [`ParseStore::get`] so the hit/miss bookkeeping there reads as
/// one expression: every `None` below is a miss, and there are four ways to
/// reach one.
fn decode(bytes: &[u8], content_hash: &str, walked: &Path) -> Option<ParsedFile> {
    let entry: StoredEntry = serde_json::from_slice(bytes).ok()?;
    // One entry per file means it may hold a parse of *different* content
    // for that file. The hash recorded beside the parse is what makes that
    // safe: a mismatch is a miss, never a wrong parse served.
    if entry.content_hash != content_hash {
        return None;
    }
    if entry.parsed.file_path == walked {
        return Some(entry.parsed);
    }
    rebase(entry.parsed, walked)
}

/// Rewrite a parse so it describes the file at `walked` rather than the path
/// it was stored under.
///
/// Sound because a `ParsedFile` describes exactly one file, and every string
/// in it that names a path names *that* file: `file_path`, `file_info.path`,
/// every `entity.file_path`, and every identifier built from one — an entity
/// id is `{path}:{line}:{name}`, a relationship id is built from its source
/// id, a warning is `{path}: {message}`. Sampling 774 stored entries found
/// 123,741 path-shaped strings and not one that was not prefixed by its own
/// file's path.
///
/// Done over the serialized tree rather than field by field on purpose. An
/// enumerated list is how the walker and the watcher came to hold two copies
/// of one predicate and disagree; a field added later that carries a path
/// would ship a stale root and nothing would say so. Paid only when the
/// spelling actually differs — the alternative on that path is a full
/// tree-sitter parse, which this is not close to.
fn rebase(parsed: ParsedFile, walked: &Path) -> Option<ParsedFile> {
    let from = parsed.file_path.to_str()?.to_string();
    let to = walked.to_str()?;
    let mut value = serde_json::to_value(&parsed).ok()?;
    rebase_value(&mut value, &from, to);
    serde_json::from_value(value).ok()
}

/// Rewrite every string in `value` that names the path `from` so it names
/// `to`, recursively.
fn rebase_value(value: &mut serde_json::Value, from: &str, to: &str) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(rest) = names_path(s, from) {
                *s = format!("{to}{rest}");
            }
        }
        serde_json::Value::Array(items) => {
            items.iter_mut().for_each(|v| rebase_value(v, from, to));
        }
        serde_json::Value::Object(map) => {
            map.values_mut().for_each(|v| rebase_value(v, from, to));
        }
        _ => {}
    }
}

/// What follows `from` in `s` when `s` names that path *whole*, else `None`.
///
/// The boundary test is the point. A bare `starts_with` would rewrite the
/// middle of a `source_code` field — and, under the `.`-rooted relative walk
/// that `mezz watch .` performs (AN-020), would turn `.env` into
/// `<root>env`. A path ends where a separator or an id's `:` begins, or at
/// the end of the string.
fn names_path<'a>(s: &'a str, from: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(from)?;
    match rest.chars().next() {
        None | Some(':') | Some(std::path::MAIN_SEPARATOR) => Some(rest),
        _ => None,
    }
}

/// The entry stem for a file inside a known repository: the repository's
/// identity plus the file's path within it.
///
/// `None` for a path that is not under this worktree's top level — a spec
/// directory outside the tree, say — which then falls back to the absolute
/// path and behaves exactly as everything did before AN-037.
fn repo_stem(repo: &RepoKey, abs_path: &Path) -> Option<String> {
    let rel = abs_path.strip_prefix(&repo.toplevel).ok()?;
    let mut bytes = repo.common_dir.as_os_str().as_encoded_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(rel.as_os_str().as_encoded_bytes());
    Some(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Identify the repository `walk_root` sits in, or `None` if it sits in none.
///
/// Two `git rev-parse` calls once per analysis, not once per file.
fn resolve_repo_key(walk_root: &Path) -> Option<RepoKey> {
    let common = git_line(walk_root, "--git-common-dir")?;
    let toplevel = git_line(walk_root, "--show-toplevel")?;
    Some(RepoKey {
        // Relative in a main checkout (`.git`), already absolute in a linked
        // worktree. Joining onto the root the command ran in settles both.
        common_dir: walk_root.join(common).canonicalize().ok()?,
        toplevel: PathBuf::from(toplevel).canonicalize().ok()?,
    })
}

/// One line of `git rev-parse <arg>` run in `dir`, or `None` if git is
/// absent, fails, or answers with nothing.
fn git_line(dir: &Path, arg: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", arg])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// What actually lands on disk: the parse plus the content hash it was
/// produced from.
///
/// The hash is recorded here rather than read back out of
/// `parsed.file_info.content_hash` so the store owns its own key invariant.
/// Deriving it from the payload would make correctness depend on every caller
/// remembering to populate that field consistently with the hash it passes to
/// [`ParseStore::put`] — a silent wrong-parse waiting to happen.
#[derive(serde::Serialize)]
struct EntryRef<'a> {
    content_hash: &'a str,
    parsed: &'a ParsedFile,
}

/// Owned mirror of [`EntryRef`] for reads.
#[derive(serde::Deserialize)]
struct StoredEntry {
    content_hash: String,
    parsed: ParsedFile,
}

// Why the key is what it is, in two layers.
//
// **Not the content hash.** Folding it into the filename meant every save of
// every file minted a permanent new entry that nothing ever removed: this repo
// has ~475 source files and had accumulated 10,647 entries / 1.9 GB (AN-008).
// One entry per file makes a re-parse *overwrite* its predecessor, so the
// store grows with the number of files analysed rather than the number of
// edits. The hash still guards correctness — it rides inside the entry and
// `decode` compares it, so a stale entry misses instead of serving a wrong
// parse.
//
// **Not the absolute path either.** That was the original key, and it made
// "one entry per file" mean one entry per *spelling* of a file. The two sides
// of a comparison are throwaway worktrees at two absolute paths, so a
// comparison hit nothing, and — worse, since a new pair of refs mints new
// directory names — its entries were never overwritten either. The reporting
// machine had 122,838 entries / 6.9 GB, an estimated 58% of them parses of
// worktrees that no longer existed (AN-037).
//
// So: the repository plus the path within it, which every checkout of that
// repository agrees on, falling back to the absolute path outside a repo.
// Two repos still cannot collide, because the repository is in the key.

/// Delete parse-store generations that are no longer current and have gone
/// untouched for [`STALE_GENERATION_DAYS`].
///
/// A version-tag bump is a clean invalidation, but nothing ever reclaimed the
/// abandoned directory, so every bump leaked a full generation. The grace
/// period matters: a previously-installed binary compiled against an older
/// salt is still using its own generation, and deleting it out from under that
/// binary would make the two re-parse each other's caches on every alternating
/// run — trading a disk leak for exactly the repeated rebuilding this is meant
/// to avoid.
fn prune_stale_generations(current: &Path) {
    let Some(parse_root) = current.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parse_root) else {
        return;
    };
    let grace = Duration::from_secs(STALE_GENERATION_DAYS * 24 * 60 * 60);
    for entry in entries.flatten() {
        let path = entry.path();
        if path == current || !path.is_dir() {
            continue;
        }
        let untouched_for = entry.metadata().and_then(|m| m.modified()).and_then(|t| {
            SystemTime::now()
                .duration_since(t)
                .map_err(std::io::Error::other)
        });
        if matches!(untouched_for, Ok(age) if age > grace) {
            // Best-effort, like every other store operation: a failure here
            // costs disk, never correctness.
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Resolve the mezz cache root from the environment. `None` when neither an
/// override nor a home/XDG directory is available.
///
/// Public to the crate because the parse store is no longer the only thing
/// that wants a place to survive a process. `reshape`'s baselines want the
/// same directory and the same `MEZZ_CACHE_DIR` override — a second
/// resolution written beside this one would be a second place for a test
/// to leak into `~/.cache`.
pub(crate) fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MEZZ_CACHE_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("mezz"));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache").join("mezz"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::file_info::Language;
    use crate::models::{CodeEntity, EntityKind, FileInfo, Position, Span};

    /// A unique-per-test temp cache root, isolated from `~/.cache` and
    /// from sibling tests. Removed if a stale one exists.
    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mezz-parse-store-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// A small but non-trivial `ParsedFile` to round-trip through the store.
    fn sample(path: &Path) -> ParsedFile {
        let span = Span::new(Position::new(0, 0, 0), Position::new(1, 0, 10));
        let entity = CodeEntity::new(
            "foo".to_string(),
            EntityKind::Function,
            path.to_path_buf(),
            span,
        );
        ParsedFile {
            file_path: path.to_path_buf(),
            file_info: FileInfo {
                path: path.to_path_buf(),
                language: Language::Rust,
                size: 10,
                line_count: 2,
                content_hash: Some("deadbeef".to_string()),
                documentation: Some("What this file is for.".to_string()),
            },
            entities: vec![entity],
            relationships: Vec::new(),
            imports: Vec::new(),
            schema_ops: Vec::new(),
            warnings: Vec::new(),
        }
    }

    // Compare as `Value` (sorted object keys), not as a string: `metadata`
    // HashMaps stringify in nondeterministic order.
    fn json(p: &ParsedFile) -> serde_json::Value {
        serde_json::to_value(p).unwrap()
    }

    /// A walk root outside any repository, so the tests below exercise the
    /// absolute-path key rather than the repo-relative one. It does not
    /// exist, which is all it takes: `git rev-parse` cannot run there.
    fn no_repo() -> &'static Path {
        Path::new("/proj")
    }

    #[test]
    fn roundtrip_hit_is_byte_identical() {
        let store = ParseStore::open_at(temp_root("roundtrip"), no_repo());
        let path = Path::new("/proj/src/foo.rs");
        assert!(store.get(path, "h1", path).is_none(), "cold lookup must miss");

        let original = sample(path);
        store.put(path, "h1", &original);

        let hit = store.get(path, "h1", path).expect("second lookup must hit");
        assert_eq!(
            json(&original),
            json(&hit),
            "stored parse diverged from original"
        );
        assert_eq!(store.stats(), (1, 1), "one hit, one miss");
    }

    #[test]
    fn reparsing_a_file_overwrites_rather_than_accumulates() {
        // The growth bug: the content hash used to be part of the *filename*,
        // so every edit left a permanent entry. 475 files had become 10,647
        // entries / 1.9 GB. One entry per path is the fix.
        let root = temp_root("overwrite");
        let store = ParseStore::open_at(root.clone(), no_repo());
        let path = Path::new("/proj/src/foo.rs");
        for revision in ["h1", "h2", "h3", "h4"] {
            store.put(path, revision, &sample(path));
        }
        let dir = root.join("parse").join(version_tag());
        assert_eq!(
            stored_entries(&dir),
            1,
            "four revisions of one file must leave one entry"
        );
    }

    /// Every stored entry under a generation, counted across its buckets.
    ///
    /// Recursive rather than a `read_dir` of the generation: since UI-153 an
    /// entry lives in its repository's bucket, and a flat count would read
    /// every layout as empty rather than as wrong.
    fn stored_entries(generation: &Path) -> usize {
        let Ok(buckets) = std::fs::read_dir(generation) else {
            return 0;
        };
        buckets
            .flatten()
            .filter_map(|bucket| std::fs::read_dir(bucket.path()).ok())
            .flat_map(|entries| entries.flatten())
            .filter(|e| e.file_name() != BUCKET_LABEL)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .count()
    }

    /// The layout the cache panel reads (UI-153): a file inside a repository
    /// lands in that repository's bucket, which names itself, and a file
    /// outside every repository lands in `loose/`.
    ///
    /// Asserted because the grouping is what makes a per-repository size
    /// affordable — recovering it from a flat directory means opening all
    /// 140,987 entries instead of stat-ing them.
    #[test]
    fn an_entry_lands_in_its_repositorys_bucket() {
        let root = temp_root("buckets");
        let repo = std::env::current_dir().expect("cwd");
        let store = ParseStore::open_at(root.clone(), &repo);
        let inside = repo.join("src/analyzer/parse_store.rs");
        store.put(&inside, "h1", &sample(&inside));

        let generation = root.join("parse").join(version_tag());
        let buckets: Vec<String> = std::fs::read_dir(&generation)
            .expect("generation dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(buckets.len(), 1, "one repository, one bucket: {buckets:?}");
        assert!(
            buckets[0].starts_with("r-"),
            "a repository bucket is prefixed so it cannot collide with {LOOSE_BUCKET}: {buckets:?}"
        );

        // The label is what lets the panel show a name instead of a digest.
        let label = std::fs::read(generation.join(&buckets[0]).join(BUCKET_LABEL))
            .expect("bucket must name itself");
        let label: serde_json::Value = serde_json::from_slice(&label).expect("label json");
        assert_eq!(
            label["label"], "mezzanine-private",
            "the label names the common dir's parent, not a throwaway checkout"
        );

        // A file outside every repository has nowhere else to go.
        let loose_store = ParseStore::open_at(root.clone(), no_repo());
        let outside = Path::new("/proj/src/foo.rs");
        loose_store.put(outside, "h1", &sample(outside));
        assert!(
            generation.join(LOOSE_BUCKET).is_dir(),
            "a file in no repository belongs in {LOOSE_BUCKET}"
        );
    }

    #[test]
    fn a_superseded_revision_misses_instead_of_serving_a_wrong_parse() {
        let store = ParseStore::open_at(temp_root("superseded"), no_repo());
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path));
        store.put(path, "h2", &sample(path));
        assert!(store.get(path, "h2", path).is_some(), "current revision must hit");
        assert!(
            store.get(path, "h1", path).is_none(),
            "the overwritten revision must miss, not return h2's parse"
        );
    }

    #[test]
    fn a_fresh_generation_is_kept_and_the_current_one_is_never_touched() {
        let root = temp_root("prune-grace");
        let store = ParseStore::open_at(root.clone(), no_repo());
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path));

        let current = root.join("parse").join(version_tag());
        let old = root.join("parse").join("0.0.1-1");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("stale.json"), b"{}").unwrap();

        prune_stale_generations(&current);

        // Both survive: the old one was just written, so it is inside the
        // grace window that keeps a still-installed older binary warm.
        assert!(current.is_dir(), "current generation must never be pruned");
        assert!(old.is_dir(), "a recently-used generation is still in use");
    }

    /// The generation must be pinned to the parser, not to anyone's memory.
    ///
    /// `c2bcb85` changed six parser files and left the salt alone, so warm
    /// caches kept serving the previous parser's output — 975 differing call
    /// targets on this repo, and a landed improvement that measured as zero.
    /// A fingerprint over the parser sources is what makes that impossible,
    /// so assert it is really in the key rather than trusting `version_tag`
    /// to keep including it.
    #[test]
    fn the_cache_generation_carries_the_parser_fingerprint() {
        assert_eq!(
            PARSE_FINGERPRINT.len(),
            12,
            "build.rs emits 12 hex chars; got {PARSE_FINGERPRINT:?}"
        );
        assert!(
            PARSE_FINGERPRINT.chars().all(|c| c.is_ascii_hexdigit()),
            "fingerprint must be hex, so it is safe as a directory name: {PARSE_FINGERPRINT:?}"
        );
        let tag = version_tag();
        assert!(
            tag.contains(PARSE_FINGERPRINT),
            "a parser change must move the cache generation, but {tag} does not carry the fingerprint"
        );
        assert!(
            tag.starts_with(&format!(
                "{}-{}-",
                env!("CARGO_PKG_VERSION"),
                PARSE_CACHE_SALT
            )),
            "crate version and salt must still lead the tag: {tag}"
        );
    }

    /// Two generations are two directories: entries written under one are
    /// invisible to the other. This is the whole invalidation mechanism, and
    /// the reason a fingerprint change is enough on its own.
    #[test]
    fn entries_do_not_cross_generations() {
        let root = temp_root("generations");
        let path = Path::new("/proj/src/foo.rs");
        ParseStore::open_at(root.clone(), no_repo()).put(path, "h1", &sample(path));

        // Same cache root, a generation that isn't ours: the entry above must
        // not be reachable from it.
        let other = ParseStore::from_dir(
            Some(root.join("parse").join("0.0.1-0-000000000000")),
            no_repo(),
        );
        assert!(
            other.get(path, "h1", path).is_none(),
            "a different generation must miss, not read the current one's entry"
        );
    }

    /// A path ends at a separator, at an id's `:`, or at the end of the
    /// string — never in the middle of a name. `mezz watch .` walks relative
    /// and emits `./src/…` (AN-020), so a root of `.` is a real spelling and
    /// a bare `starts_with` would rewrite `.env` into `<root>env`.
    #[test]
    fn a_rebase_rewrites_whole_paths_and_nothing_else() {
        assert_eq!(names_path("/a/foo.rs", "/a/foo.rs"), Some(""));
        assert_eq!(names_path("/a/foo.rs:12:bar", "/a/foo.rs"), Some(":12:bar"));
        assert_eq!(
            names_path("/a/foo.rs: warned", "/a/foo.rs"),
            Some(": warned")
        );
        assert_eq!(names_path("./src/x.rs", "."), Some("/src/x.rs"));

        // Neighbours that merely share a prefix.
        assert_eq!(names_path("/a/foo.rs.bak", "/a/foo.rs"), None);
        assert_eq!(names_path("/a/foobar.rs", "/a/foo.rs"), None);
        assert_eq!(names_path(".env", "."), None);
        // A doc comment is not a path, however it starts.
        assert_eq!(names_path("/// One line.", "/a/foo.rs"), None);
    }

    /// The rebase reaches every string in the tree, at any depth, and leaves
    /// the ones that only look path-shaped alone.
    #[test]
    fn a_rebase_reaches_nested_strings() {
        let mut value = serde_json::json!({
            "file_path": "/old/x.rs",
            "entities": [{"id": "/old/x.rs:3:f", "doc": "see /old/x.rs.bak"}],
            "count": 7,
        });
        rebase_value(&mut value, "/old/x.rs", "/new/x.rs");
        assert_eq!(value["file_path"], "/new/x.rs");
        assert_eq!(value["entities"][0]["id"], "/new/x.rs:3:f");
        assert_eq!(
            value["entities"][0]["doc"], "see /old/x.rs.bak",
            "a longer neighbouring path must not be rewritten"
        );
        assert_eq!(value["count"], 7);
    }

    #[test]
    fn changed_content_hash_misses() {
        let store = ParseStore::open_at(temp_root("changed-hash"), no_repo());
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path));
        // Same file, different content hash — an edit — must not hit.
        assert!(store.get(path, "h2", path).is_none());
    }

    #[test]
    fn identical_content_different_path_does_not_collide() {
        let store = ParseStore::open_at(temp_root("path-iso"), no_repo());
        let a = Path::new("/proj/a.rs");
        let b = Path::new("/proj/b.rs");
        store.put(a, "same", &sample(a));
        // Entity IDs are path-qualified, so b must not read a's entry.
        assert!(
            store.get(b, "same", b).is_none(),
            "path must be part of the key"
        );
    }

    #[test]
    fn corrupt_entry_degrades_to_miss() {
        let root = temp_root("corrupt");
        let store = ParseStore::open_at(root, no_repo());
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path));
        // Corrupt the on-disk entry, then confirm a lookup falls back to a
        // miss instead of erroring/panicking.
        let entry = store.entry_path(path).unwrap();
        std::fs::write(&entry, b"{ this is not valid json").unwrap();
        assert!(
            store.get(path, "h1", path).is_none(),
            "corruption must read as a miss"
        );
    }

    #[test]
    fn disabled_store_never_hits_or_writes() {
        let store = ParseStore::disabled();
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path)); // no-op, must not panic
        assert!(store.get(path, "h1", path).is_none());
    }
}
