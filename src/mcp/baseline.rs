//! What `reshape` last told an agent about a folder, and where the server
//! keeps it.
//!
//! Its own file because two modules need it for unrelated reasons.
//! `reshape` builds a reading and compares it with the one before;
//! [`McpServer`](super::McpServer) merely has to hold the readings between
//! calls. With the type living in `reshape`, the server depended on the
//! whole tool to declare one field, and the folder's children closed a
//! loop — `mod.rs → reshape.rs → mod.rs` — that left the drawing with no
//! reading order at all.
//!
//! The store travels with the type for the same reason. A `Mutex<HashMap<…>>`
//! spelled out in the server struct is the map's shape leaking into a file
//! that never touches it; `swap` is the only thing anyone does to it.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::models::{FolderPicture, FolderShape, ShapePattern, ShapeTerms};

/// What `reshape` told an agent about a folder last time it asked.
///
/// Structure is stored beside the scores because the two answer different
/// halves of the closing instruction. The scores say whether the verdict
/// moved; the edge and boundary sets say whether anything about
/// who-depends-on-what moved with it. A tier that rose while both sets
/// stayed identical is the cosmetic change this tool spends its whole
/// forbidden list warning against, and it is only detectable by having
/// kept the earlier drawing.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Baseline {
    /// The scope the reading was taken under — see
    /// [`scope_id`](super::scope_id).
    ///
    /// Recorded beside the numbers because a difference here is the one
    /// explanation for an unchanged drawing that the agent cannot check
    /// for itself: the settings file it would have to read is the
    /// server's, and the server may have been started somewhere else
    /// entirely (CFG-014).
    pub(super) scope: String,
    pub(super) pattern: ShapePattern,
    pub(super) compliance: f32,
    pub(super) layering: Option<f32>,
    pub(super) arborescence: Option<f32>,
    pub(super) entry_concentration: Option<f32>,
    /// `default` on read, so a cache file written before the egress gate
    /// existed still loads — it merely reports one fewer moved number
    /// until the folder is measured again.
    #[serde(default)]
    pub(super) egress: Option<f32>,
    pub(super) child_count: u32,
    /// Every drawn edge, as `from → to`.
    pub(super) edges: BTreeSet<(String, String)>,
    /// Every dependency crossing the boundary, so a change in entry
    /// concentration can be attributed to something rather than appearing
    /// out of nowhere.
    pub(super) outside: BTreeSet<(String, String)>,
    /// The counts each ratio above was divided from.
    ///
    /// Kept so a ratio that *fell* can be explained by the same arithmetic
    /// that produced it. The tool has always been able to say a tier rose
    /// on no real change and should be reverted; the converse — a number
    /// falling on a change that was right — had no words anywhere, and an
    /// agent reported it as the case that makes you hesitate. Two counts
    /// settle it, and re-deriving them from the stored edges would be a
    /// second definition of a formula this record can simply carry.
    ///
    /// `default` on read, so a cache file written before this field
    /// existed still loads and merely explains less.
    #[serde(default)]
    pub(super) terms: ShapeTerms,
}

impl Baseline {
    pub(super) fn of(shape: &FolderShape, p: &FolderPicture, scope: String) -> Self {
        Self {
            scope,
            pattern: shape.pattern,
            compliance: shape.compliance,
            layering: shape.layering,
            arborescence: shape.arborescence,
            entry_concentration: shape.entry_concentration,
            egress: shape.egress,
            child_count: shape.child_count,
            terms: shape.terms,
            edges: p
                .edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone()))
                .collect(),
            outside: p
                .outside
                .iter()
                .map(|o| (o.outside.clone(), o.inside.clone()))
                .collect(),
        }
    }

    /// Whether every measured value is identical — the test behind
    /// "Nothing has changed".
    ///
    /// Not `==`, because [`Self::scope`] travels on the same record
    /// without being part of the reading. A configuration change that
    /// moved no number still has to reach that sentence rather than the
    /// field-by-field diff below it, which would otherwise print
    /// `verdict tangled → tangled` and call it progress. Written as a
    /// comparison against a rescoped copy rather than a list of fields,
    /// so a field added later counts as part of the reading by default.
    pub(super) fn same_reading(&self, other: &Self) -> bool {
        let mut rescoped = self.clone();
        rescoped.scope.clone_from(&other.scope);
        rescoped == *other
    }

    /// Whether anything about who-depends-on-what differs. The test the
    /// forbidden list already states in prose, made mechanical.
    pub(super) fn same_structure(&self, other: &Self) -> bool {
        self.edges == other.edges
            && self.outside == other.outside
            && self.child_count == other.child_count
    }
}

/// The last drawing reported per folder, keyed by its relative path.
///
/// Held by the server rather than asked of the caller on purpose. The
/// tool's closing instruction is to re-run and check the tier moved, and
/// an agent that reports its own before-and-after is grading its own work
/// from memory. Keeping the baseline here means the second call answers
/// the question instead of the agent.
///
/// ## Why it also lives on disk
///
/// It used to live only in the process, and `reshape`'s own closing
/// paragraph had to warn that "rebuilding or reloading between the two
/// calls loses it". That warning fires exactly when the comparison is
/// worth most: a rebuilt binary is what an agent restarts the server for
/// after a session's work, and the run it most wants graded is the one
/// that just ended. An agent reported losing the record across a session
/// boundary as one of two gaps in the feature.
///
/// So the map is mirrored to the cache directory the parse store already
/// uses — outside the project, because `reshape` is annotated
/// `readOnlyHint: true` and that promise is about the caller's tree. A
/// cache entry is not project state, and a missing or unreadable one
/// degrades to exactly the old behaviour rather than to an error.
#[derive(Default)]
pub(crate) struct ShapeBaselines {
    seen: Mutex<HashMap<String, Baseline>>,
    /// Where the mirror lives, or `None` for a memory-only store — which
    /// is what tests and an environment with no cache directory get.
    file: Option<PathBuf>,
}

impl ShapeBaselines {
    /// A store that survives this process, mirrored under the mezz cache
    /// directory and scoped to one analysis root.
    ///
    /// Scoped by a digest of the root rather than by its spelling: two
    /// checkouts of one repository are two trees with two shapes, and a
    /// path is not a filename.
    pub(crate) fn rooted_at(root: &Path) -> Self {
        let file = crate::analyzer::cache_root().map(|dir| {
            dir.join("reshape").join(format!(
                "{}.json",
                blake3::hash(root.display().to_string().as_bytes()).to_hex()
            ))
        });
        Self {
            seen: Mutex::new(HashMap::new()),
            file,
        }
    }

    /// Store this call's drawing and hand back the one it replaced.
    ///
    /// Swapped rather than read-then-written so the record is always the
    /// last thing the agent was actually shown: a call that reports
    /// against the previous drawing is also the call the *next* one will
    /// be graded on. A poisoned lock loses the baseline and reports
    /// nothing, which is the right failure — a comparison against an
    /// unknown drawing would be worse than none.
    pub(super) fn swap(&self, folder: &str, current: &Baseline) -> Option<Baseline> {
        let mut seen = self.seen.lock().ok()?;
        self.fill_from_file(&mut seen, folder);
        let previous = seen.insert(folder.to_string(), current.clone());
        self.write_file(&seen);
        previous
    }

    /// Bring one folder's mirrored record into the live map, if the map
    /// does not already hold a fresher one.
    ///
    /// Only on the first ask for a folder, and only for that folder: an
    /// in-memory entry is always the newer of the two, being this
    /// process's own last report, and loading the whole file over it would
    /// grade an agent against a reading from another session.
    fn fill_from_file(&self, seen: &mut HashMap<String, Baseline>, folder: &str) {
        if seen.contains_key(folder) {
            return;
        }
        if let Some(stored) = self.read_file().and_then(|mut m| m.remove(folder)) {
            seen.insert(folder.to_string(), stored);
        }
    }

    /// The mirror, or an empty map for anything that went wrong reading
    /// it. Every failure here is a lost comparison, which is the state the
    /// tool already reports and already explains.
    fn read_file(&self) -> Option<HashMap<String, Baseline>> {
        let raw = std::fs::read_to_string(self.file.as_ref()?).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Mirror the map, writing through a temp file so a concurrent reader
    /// never sees half a record.
    ///
    /// Silent on failure on purpose: a cache that cannot be written is a
    /// comparison the next call will not have, and failing the *current*
    /// call over it would trade a working report for a missing one.
    fn write_file(&self, seen: &HashMap<String, Baseline>) {
        let Some(path) = self.file.as_ref() else {
            return;
        };
        let (Some(dir), Ok(body)) = (path.parent(), serde_json::to_string(seen)) else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&temp, body).is_ok() && std::fs::rename(&temp, path).is_err() {
            let _ = std::fs::remove_file(&temp);
        }
    }
}
