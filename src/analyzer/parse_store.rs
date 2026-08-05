//! AN-003 — persistent on-disk parse store + incremental re-analysis.
//!
//! Serialized per-file parse results ([`super::ParsedFile`]) keyed by
//! (absolute path, parser version), with the content hash carried *inside*
//! the entry and compared on read. The determinism guarantee (AN-002) is the
//! safety argument: identical content + identical parser ⇒ identical entities,
//! so a hash match can never be stale-wrong.
//!
//! One entry per file, not per revision of a file: a re-parse overwrites its
//! predecessor. Folding the content hash into the *filename* instead — the
//! original design — meant every save minted a permanent entry nothing ever
//! removed, so the store grew with edits rather than with files.
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
use std::time::{Duration, SystemTime};
use std::sync::atomic::{AtomicU64, Ordering};

use super::ParsedFile;

/// Manual cache salt — an escape hatch, no longer the primary lever.
///
/// [`PARSE_FINGERPRINT`] now invalidates the store automatically for any
/// change to the parsers, the models, or the grammars. Bump this only for a
/// change those three cannot see: a behavioural dependency bump, or an
/// environment-driven parse. Bumping it is always safe and always cheap —
/// one cold analysis.
const PARSE_CACHE_SALT: &str = "5";

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
const PARSE_FINGERPRINT: &str = env!("NAO_PARSE_FINGERPRINT");

/// How long an abandoned generation must sit untouched before
/// [`prune_stale_generations`] reclaims it. Long enough that a still-installed
/// older binary keeps its own cache warm between runs.
///
/// This is also what keeps the automatic fingerprint affordable: every parser
/// edit mints a generation, so a day of parser work leaves a handful behind.
/// They are reclaimed a week after last use, and a superseded generation is
/// never read in the meantime.
const STALE_GENERATION_DAYS: u64 = 7;

/// Version tag folded into the store path. Three parts, each covering what
/// the others cannot: the crate version (auto-bumps every release), the
/// [`PARSE_FINGERPRINT`] (auto-bumps whenever parse output could change), and
/// the manual [`PARSE_CACHE_SALT`] (the escape hatch). Entries under an old
/// tag are never read, so any of the three changing is a clean invalidation.
fn version_tag() -> String {
    format!(
        "{}-{}-{}",
        env!("CARGO_PKG_VERSION"),
        PARSE_CACHE_SALT,
        PARSE_FINGERPRINT
    )
}

/// Persistent on-disk store of per-file parse results.
///
/// Global (not per-repo): entity IDs embed absolute file paths, so two
/// repos can never collide on a key. Construct once per analysis and
/// share by reference across the parallel parse loop.
pub struct ParseStore {
    /// Version-scoped directory holding the entries. `None` when no cache
    /// location could be resolved — the store then behaves as a no-op
    /// (every `get` misses, every `put` is dropped), i.e. cold analysis.
    dir: Option<PathBuf>,
    hits: AtomicU64,
    misses: AtomicU64,
    /// Monotonic counter making temp filenames unique within a process,
    /// so concurrent writers of different keys never share a temp path.
    seq: AtomicU64,
}

impl ParseStore {
    /// Open the store, resolving the cache directory from the environment:
    ///
    /// 1. `NAO_CACHE_DIR` — explicit override (CI, tests).
    /// 2. `XDG_CACHE_HOME/nao`
    /// 3. `~/.cache/nao`
    ///
    /// If none resolve, the store is disabled (analysis stays cold but
    /// correct). The directory is created lazily on first write.
    pub fn open() -> Self {
        let dir = cache_root().map(|root| root.join("parse").join(version_tag()));
        if let Some(dir) = dir.as_deref() {
            prune_stale_generations(dir);
        }
        Self::from_dir(dir)
    }

    /// Open the store rooted at an explicit cache directory, bypassing env
    /// resolution. Handy for embedders (and tests) that want full control
    /// over cache placement; the same version scoping still applies.
    pub fn open_at(cache_root: PathBuf) -> Self {
        Self::from_dir(Some(cache_root.join("parse").join(version_tag())))
    }

    /// A disabled store: never hits, never writes. Used where a caller
    /// wants to opt out of persistence entirely.
    pub fn disabled() -> Self {
        Self::from_dir(None)
    }

    fn from_dir(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            seq: AtomicU64::new(0),
        }
    }

    /// blake3 hex digest of file content. Used both as the store key
    /// component and to populate `FileInfo.content_hash`. Cryptographic
    /// strength rules out a collision serving a wrong parse.
    pub fn content_hash(content: &str) -> String {
        blake3::hash(content.as_bytes()).to_hex().to_string()
    }

    /// Look up a stored parse for `abs_path` at `content_hash`. Returns
    /// `None` on a miss or on *any* error (missing file, corrupt JSON,
    /// unreadable dir) — the caller then parses fresh. Records a hit/miss
    /// for the end-of-run summary.
    pub(crate) fn get(&self, abs_path: &Path, content_hash: &str) -> Option<ParsedFile> {
        let path = self.entry_path(abs_path)?;
        match std::fs::read(&path).ok().and_then(|bytes| {
            serde_json::from_slice::<StoredEntry>(&bytes).ok()
        }) {
            // The entry is keyed by path alone, so it may hold a parse of
            // *different* content for that path. The hash recorded beside the
            // parse is what makes that safe: a mismatch is a miss, never a
            // wrong parse served.
            Some(entry) if entry.content_hash == content_hash => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(entry.parsed)
            }
            // Absent, corrupt, *or* holding a different revision of this path.
            _ => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Persist a parse result. Best-effort: serialization or IO failure is
    /// swallowed (the entry simply won't exist next time). Writes to a
    /// process-unique temp file then atomically renames, so a concurrent
    /// reader — even in a second `nao` process — never sees a torn entry.
    pub(crate) fn put(&self, abs_path: &Path, content_hash: &str, parsed: &ParsedFile) {
        let Some(dir) = self.dir.as_ref() else { return };
        let Some(final_path) = self.entry_path(abs_path) else { return };
        let Ok(bytes) = serde_json::to_vec(&EntryRef { content_hash, parsed }) else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!("{}.{}.{}.tmp", key_stem(abs_path), std::process::id(), seq));
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
        (self.hits.load(Ordering::Relaxed), self.misses.load(Ordering::Relaxed))
    }

    /// Absolute path of the entry file for `(abs_path, content_hash)`, or
    /// `None` when the store is disabled.
    fn entry_path(&self, abs_path: &Path) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        Some(dir.join(format!("{}.json", key_stem(abs_path))))
    }
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

/// blake3 hex of the absolute path — the entry's basename stem. **One entry
/// per file, not per revision of a file.**
///
/// The content hash used to be folded in here, which meant every save of every
/// file minted a permanent new entry that nothing ever removed: this repo has
/// ~475 source files and had accumulated 10,647 entries / 1.9 GB. Keying on the
/// path alone makes a re-parse *overwrite* its predecessor, so the store grows
/// with the number of files analysed rather than with the number of edits.
///
/// The content hash still guards correctness — it rides inside the entry and
/// [`ParseStore::get`] compares it, so a stale entry misses instead of serving
/// a wrong parse. The path is still the key because entity IDs are
/// path-qualified: two files with identical content must not share an entry.
///
/// Trade-off: a parse of *older* content at the same path (flipping between
/// branches in place) is no longer a hit. `nao diff` is unaffected — it
/// analyses git worktrees, which are distinct paths.
fn key_stem(abs_path: &Path) -> String {
    blake3::hash(abs_path.to_string_lossy().as_bytes()).to_hex().to_string()
}

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
    let Some(parse_root) = current.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parse_root) else { return };
    let grace = Duration::from_secs(STALE_GENERATION_DAYS * 24 * 60 * 60);
    for entry in entries.flatten() {
        let path = entry.path();
        if path == current || !path.is_dir() {
            continue;
        }
        let untouched_for = entry
            .metadata()
            .and_then(|m| m.modified())
            .and_then(|t| SystemTime::now().duration_since(t).map_err(std::io::Error::other));
        if matches!(untouched_for, Ok(age) if age > grace) {
            // Best-effort, like every other store operation: a failure here
            // costs disk, never correctness.
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Resolve the nao cache root from the environment. `None` when neither an
/// override nor a home/XDG directory is available.
fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NAO_CACHE_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("nao"));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache").join("nao"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::file_info::Language;
    use crate::models::{CodeEntity, EntityKind, FileInfo, Position, Span};

    /// A unique-per-test temp cache root, isolated from `~/.cache` and
    /// from sibling tests. Removed if a stale one exists.
    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("nao-parse-store-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// A small but non-trivial `ParsedFile` to round-trip through the store.
    fn sample(path: &Path) -> ParsedFile {
        let span = Span::new(Position::new(0, 0, 0), Position::new(1, 0, 10));
        let entity = CodeEntity::new("foo".to_string(), EntityKind::Function, path.to_path_buf(), span);
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

    #[test]
    fn roundtrip_hit_is_byte_identical() {
        let store = ParseStore::open_at(temp_root("roundtrip"));
        let path = Path::new("/proj/src/foo.rs");
        assert!(store.get(path, "h1").is_none(), "cold lookup must miss");

        let original = sample(path);
        store.put(path, "h1", &original);

        let hit = store.get(path, "h1").expect("second lookup must hit");
        assert_eq!(json(&original), json(&hit), "stored parse diverged from original");
        assert_eq!(store.stats(), (1, 1), "one hit, one miss");
    }

    #[test]
    fn reparsing_a_file_overwrites_rather_than_accumulates() {
        // The growth bug: the content hash used to be part of the *filename*,
        // so every edit left a permanent entry. 475 files had become 10,647
        // entries / 1.9 GB. One entry per path is the fix.
        let root = temp_root("overwrite");
        let store = ParseStore::open_at(root.clone());
        let path = Path::new("/proj/src/foo.rs");
        for revision in ["h1", "h2", "h3", "h4"] {
            store.put(path, revision, &sample(path));
        }
        let dir = root.join("parse").join(version_tag());
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .expect("store dir")
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect();
        assert_eq!(entries.len(), 1, "four revisions of one file must leave one entry");
    }

    #[test]
    fn a_superseded_revision_misses_instead_of_serving_a_wrong_parse() {
        let store = ParseStore::open_at(temp_root("superseded"));
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path));
        store.put(path, "h2", &sample(path));
        assert!(store.get(path, "h2").is_some(), "current revision must hit");
        assert!(
            store.get(path, "h1").is_none(),
            "the overwritten revision must miss, not return h2's parse"
        );
    }

    #[test]
    fn a_fresh_generation_is_kept_and_the_current_one_is_never_touched() {
        let root = temp_root("prune-grace");
        let store = ParseStore::open_at(root.clone());
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
            tag.starts_with(&format!("{}-{}-", env!("CARGO_PKG_VERSION"), PARSE_CACHE_SALT)),
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
        ParseStore::open_at(root.clone()).put(path, "h1", &sample(path));

        // Same cache root, a generation that isn't ours: the entry above must
        // not be reachable from it.
        let other = ParseStore::from_dir(Some(root.join("parse").join("0.0.1-0-000000000000")));
        assert!(
            other.get(path, "h1").is_none(),
            "a different generation must miss, not read the current one's entry"
        );
    }

    #[test]
    fn changed_content_hash_misses() {
        let store = ParseStore::open_at(temp_root("changed-hash"));
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path));
        // Same file, different content hash — an edit — must not hit.
        assert!(store.get(path, "h2").is_none());
    }

    #[test]
    fn identical_content_different_path_does_not_collide() {
        let store = ParseStore::open_at(temp_root("path-iso"));
        let a = Path::new("/proj/a.rs");
        let b = Path::new("/proj/b.rs");
        store.put(a, "same", &sample(a));
        // Entity IDs are path-qualified, so b must not read a's entry.
        assert!(store.get(b, "same").is_none(), "path must be part of the key");
    }

    #[test]
    fn corrupt_entry_degrades_to_miss() {
        let root = temp_root("corrupt");
        let store = ParseStore::open_at(root);
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path));
        // Corrupt the on-disk entry, then confirm a lookup falls back to a
        // miss instead of erroring/panicking.
        let entry = store.entry_path(path).unwrap();
        std::fs::write(&entry, b"{ this is not valid json").unwrap();
        assert!(store.get(path, "h1").is_none(), "corruption must read as a miss");
    }

    #[test]
    fn disabled_store_never_hits_or_writes() {
        let store = ParseStore::disabled();
        let path = Path::new("/proj/src/foo.rs");
        store.put(path, "h1", &sample(path)); // no-op, must not panic
        assert!(store.get(path, "h1").is_none());
    }
}
